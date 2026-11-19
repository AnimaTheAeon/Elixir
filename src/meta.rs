//! Lookups in TheMovieDB, TheTVDB and AniDB, with small caches so the same
//! show isn't fetched again for every episode.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::config::{var_dir, Config};
use crate::http::{self, enc, Request};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Tmdb,
    Tvdb,
    Anidb,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::Tmdb => "TheMovieDB",
            Provider::Tvdb => "TheTVDB",
            Provider::Anidb => "AniDB",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Show {
    pub provider: Provider,
    pub id: u64,
    pub title: String,
    pub year: Option<u16>,
    pub aliases: Vec<String>,
    pub tvdb_id: Option<u64>,
    pub tmdb_id: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub season: u32,
    pub number: u32,
    pub absolute: Option<u32>,
    pub title: String,
    pub air_date: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Movie {
    pub tmdb_id: u64,
    pub title: String,
    pub year: Option<u16>,
    pub aliases: Vec<String>,
    pub genres: Vec<String>,
}

fn base(env: &str, default: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| default.to_string())
}

// ---------- caches ----------

type Cache = Mutex<HashMap<String, (Instant, Value)>>;
fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn cached(key: &str, ttl: Duration, fetch: impl FnOnce() -> Result<Value, String>) -> Result<Value, String> {
    if let Some((at, v)) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(key) {
        if at.elapsed() < ttl {
            return Ok(v.clone());
        }
    }
    let v = fetch()?;
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    if c.len() > 2000 {
        c.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(600));
    }
    c.insert(key.to_string(), (Instant::now(), v.clone()));
    Ok(v)
}

fn year_of(date: &str) -> Option<u16> {
    date.get(..4).and_then(|y| y.parse().ok()).filter(|y| *y > 1800)
}

// ---------- TheMovieDB ----------

fn tmdb(cfg: &Config, path: &str, query: &[(&str, String)]) -> Result<Value, String> {
    if cfg.tmdb_key.is_empty() {
        return Err("TheMovieDB needs an API key (Settings › Databases)".into());
    }
    let mut url = format!("{}/3/{path}?language={}", base("ELIXIR_TMDB_URL", "https://api.themoviedb.org"), enc(&lang_tag(cfg)));
    for (k, v) in query {
        url.push_str(&format!("&{k}={}", enc(v)));
    }
    let mut req = Request::get(url);
    // A long "read access token" goes in a header; a short v3 key in the address.
    if cfg.tmdb_key.starts_with("eyJ") {
        req = req.header("Authorization", &format!("Bearer {}", cfg.tmdb_key));
    } else {
        req.url.push_str(&format!("&api_key={}", enc(&cfg.tmdb_key)));
    }
    http::ok("TheMovieDB", &req)?.json().map_err(|e| format!("TheMovieDB {e}"))
}

fn lang_tag(cfg: &Config) -> String {
    match cfg.language.as_str() {
        "en" => "en-US".into(),
        l => l.to_string(),
    }
}

pub fn tmdb_search_tv(cfg: &Config, title: &str, year: Option<u16>) -> Result<Vec<Show>, String> {
    let key = format!("tmdb-tv-{title}-{year:?}-{}", cfg.language);
    let v = cached(&key, Duration::from_secs(3600), || {
        let mut q = vec![("query", title.to_string())];
        if let Some(y) = year {
            q.push(("first_air_date_year", y.to_string()));
        }
        let first = tmdb(cfg, "search/tv", &q)?;
        // A year that's off by a season finds nothing; ask again without it.
        if year.is_some() && first["results"].as_array().is_none_or(|a| a.is_empty()) {
            return tmdb(cfg, "search/tv", &[("query", title.to_string())]);
        }
        Ok(first)
    })?;
    Ok(v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .take(8)
        .filter_map(|r| {
            let id = r["id"].as_u64()?;
            let title = r["name"].as_str()?.to_string();
            let original = r["original_name"].as_str().unwrap_or("").to_string();
            Some(Show {
                provider: Provider::Tmdb,
                id,
                title,
                year: r["first_air_date"].as_str().and_then(year_of),
                aliases: if original.is_empty() { vec![] } else { vec![original] },
                tvdb_id: None,
                tmdb_id: Some(id),
            })
        })
        .collect())
}

/// Every episode of a TheMovieDB show (season by season).
pub fn tmdb_episodes(cfg: &Config, id: u64) -> Result<Vec<Episode>, String> {
    let v = cached(&format!("tmdb-eps-{id}-{}", cfg.language), Duration::from_secs(6 * 3600), || {
        let show = tmdb(cfg, &format!("tv/{id}"), &[("append_to_response", "external_ids".into())])?;
        let mut all = Vec::new();
        for s in show["seasons"].as_array().into_iter().flatten() {
            let n = s["season_number"].as_u64().unwrap_or(0);
            let season = tmdb(cfg, &format!("tv/{id}/season/{n}"), &[])?;
            for e in season["episodes"].as_array().into_iter().flatten() {
                all.push(json!({
                    "season": n, "number": e["episode_number"], "title": e["name"], "air_date": e["air_date"],
                }));
            }
        }
        Ok(json!({ "episodes": all, "tvdb_id": show["external_ids"]["tvdb_id"] }))
    })?;
    Ok(parse_episodes(&v["episodes"]))
}

pub fn tmdb_tvdb_id(cfg: &Config, id: u64) -> Option<u64> {
    tmdb_episodes(cfg, id).ok()?;
    cache().lock().ok()?.get(&format!("tmdb-eps-{id}-{}", cfg.language)).and_then(|(_, v)| v["tvdb_id"].as_u64())
}

fn parse_episodes(v: &Value) -> Vec<Episode> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some(Episode {
                season: e["season"].as_u64()? as u32,
                number: e["number"].as_u64()? as u32,
                absolute: e["absolute"].as_u64().map(|n| n as u32).filter(|n| *n > 0),
                title: e["title"].as_str().unwrap_or("").trim().to_string(),
                air_date: e["air_date"].as_str().filter(|s| s.len() >= 10).map(|s| s[..10].to_string()),
            })
        })
        .collect()
}

fn tmdb_genres(cfg: &Config) -> HashMap<u64, String> {
    let v = cached(&format!("tmdb-genres-{}", cfg.language), Duration::from_secs(24 * 3600), || tmdb(cfg, "genre/movie/list", &[])).unwrap_or(Value::Null);
    v["genres"].as_array().into_iter().flatten().filter_map(|g| Some((g["id"].as_u64()?, g["name"].as_str()?.to_string()))).collect()
}

pub fn tmdb_search_movie(cfg: &Config, title: &str, year: Option<u16>) -> Result<Vec<Movie>, String> {
    let key = format!("tmdb-movie-{title}-{year:?}-{}", cfg.language);
    let v = cached(&key, Duration::from_secs(3600), || {
        let mut q = vec![("query", title.to_string())];
        if let Some(y) = year {
            q.push(("year", y.to_string()));
        }
        let first = tmdb(cfg, "search/movie", &q)?;
        if year.is_some() && first["results"].as_array().is_none_or(|a| a.is_empty()) {
            return tmdb(cfg, "search/movie", &[("query", title.to_string())]);
        }
        Ok(first)
    })?;
    let genres = tmdb_genres(cfg);
    Ok(v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .take(8)
        .filter_map(|r| {
            let original = r["original_title"].as_str().unwrap_or("").to_string();
            Some(Movie {
                tmdb_id: r["id"].as_u64()?,
                title: r["title"].as_str()?.to_string(),
                year: r["release_date"].as_str().and_then(year_of),
                aliases: if original.is_empty() { vec![] } else { vec![original] },
                genres: r["genre_ids"].as_array().into_iter().flatten().filter_map(|g| genres.get(&g.as_u64()?).cloned()).collect(),
            })
        })
        .collect())
}

pub fn tmdb_movie(cfg: &Config, id: u64) -> Result<Movie, String> {
    let r = cached(&format!("tmdb-movieid-{id}-{}", cfg.language), Duration::from_secs(6 * 3600), || tmdb(cfg, &format!("movie/{id}"), &[]))?;
    Ok(Movie {
        tmdb_id: id,
        title: r["title"].as_str().unwrap_or("").to_string(),
        year: r["release_date"].as_str().and_then(year_of),
        aliases: vec![],
        genres: r["genres"].as_array().into_iter().flatten().filter_map(|g| g["name"].as_str().map(String::from)).collect(),
    })
}

pub fn tmdb_show(cfg: &Config, id: u64) -> Result<Show, String> {
    let r = cached(&format!("tmdb-showid-{id}-{}", cfg.language), Duration::from_secs(6 * 3600), || tmdb(cfg, &format!("tv/{id}"), &[("append_to_response", "external_ids".into())]))?;
    Ok(Show {
        provider: Provider::Tmdb,
        id,
        title: r["name"].as_str().unwrap_or("").to_string(),
        year: r["first_air_date"].as_str().and_then(year_of),
        aliases: vec![],
        tvdb_id: r["external_ids"]["tvdb_id"].as_u64(),
        tmdb_id: Some(id),
    })
}

// ---------- TheTVDB (v4) ----------

fn tvdb_token(cfg: &Config, refresh: bool) -> Result<String, String> {
    static TOKEN: OnceLock<Mutex<Option<(String, Instant, String)>>> = OnceLock::new();
    let slot = TOKEN.get_or_init(Default::default);
    let mut t = slot.lock().unwrap_or_else(|e| e.into_inner());
    let who = format!("{}|{}", cfg.tvdb_key, cfg.tvdb_pin);
    if !refresh {
        if let Some((tok, at, k)) = t.as_ref() {
            if *k == who && at.elapsed() < Duration::from_secs(20 * 24 * 3600) {
                return Ok(tok.clone());
            }
        }
    }
    let mut body = json!({ "apikey": cfg.tvdb_key });
    if !cfg.tvdb_pin.is_empty() {
        body["pin"] = json!(cfg.tvdb_pin);
    }
    let r = http::ok("TheTVDB", &Request::post_json(format!("{}/v4/login", base("ELIXIR_TVDB_URL", "https://api4.thetvdb.com")), &body))?;
    let tok = r.json()?["data"]["token"].as_str().ok_or("TheTVDB didn't sign Elixir in. Check the key")?.to_string();
    *t = Some((tok.clone(), Instant::now(), who));
    Ok(tok)
}

fn tvdb(cfg: &Config, path: &str) -> Result<Value, String> {
    if cfg.tvdb_key.is_empty() {
        return Err("TheTVDB needs an API key (Settings › Databases)".into());
    }
    let url = format!("{}/v4/{path}", base("ELIXIR_TVDB_URL", "https://api4.thetvdb.com"));
    for attempt in 0..2 {
        let tok = tvdb_token(cfg, attempt > 0)?;
        let r = http::send("TheTVDB", &Request::get(url.clone()).header("Authorization", &format!("Bearer {tok}")))?;
        if r.status == 401 && attempt == 0 {
            continue;
        }
        return match r.status {
            200..=299 => r.json().map_err(|e| format!("TheTVDB {e}")),
            404 => Ok(json!({ "data": null })),
            s => Err(format!("TheTVDB answered with an error (HTTP {s})")),
        };
    }
    Err("TheTVDB refused the key. Check it in Settings".into())
}

fn lang3(cfg: &Config) -> &'static str {
    match cfg.language.as_str() {
        "de" => "deu",
        "fr" => "fra",
        "es" => "spa",
        "it" => "ita",
        "nl" => "nld",
        "pt" => "por",
        "ja" => "jpn",
        _ => "eng",
    }
}

pub fn tvdb_search(cfg: &Config, title: &str, year: Option<u16>) -> Result<Vec<Show>, String> {
    let l3 = lang3(cfg);
    let v = cached(&format!("tvdb-s-{title}-{year:?}-{l3}"), Duration::from_secs(3600), || {
        let mut path = format!("search?type=series&query={}", enc(title));
        if let Some(y) = year {
            path.push_str(&format!("&year={y}"));
        }
        let first = tvdb(cfg, &path)?;
        if year.is_some() && first["data"].as_array().is_none_or(|a| a.is_empty()) {
            return tvdb(cfg, &format!("search?type=series&query={}", enc(title)));
        }
        Ok(first)
    })?;
    Ok(v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .take(8)
        .filter_map(|r| {
            let id: u64 = r["tvdb_id"].as_str().and_then(|s| s.parse().ok()).or(r["tvdb_id"].as_u64())?;
            let translated = r["translations"][l3].as_str().map(String::from);
            let name = r["name"].as_str().unwrap_or("").to_string();
            let title = translated.clone().unwrap_or_else(|| name.clone());
            let mut aliases: Vec<String> = r["aliases"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(String::from)).collect();
            if translated.is_some() && !name.is_empty() {
                aliases.push(name);
            }
            let tmdb_id = r["remote_ids"].as_array().into_iter().flatten().find(|x| x["sourceName"].as_str() == Some("TheMovieDB.com")).and_then(|x| x["id"].as_str()).and_then(|s| s.parse().ok());
            Some(Show {
                provider: Provider::Tvdb,
                id,
                title,
                year: r["year"].as_str().and_then(|y| y.parse().ok()).or(r["first_air_time"].as_str().and_then(year_of)),
                aliases,
                tvdb_id: Some(id),
                tmdb_id,
            })
        })
        .collect())
}

pub fn tvdb_show(cfg: &Config, id: u64) -> Result<Show, String> {
    let l3 = lang3(cfg);
    let v = cached(&format!("tvdb-show-{id}-{l3}"), Duration::from_secs(6 * 3600), || tvdb(cfg, &format!("series/{id}/translations/{l3}")))?;
    let base_v = cached(&format!("tvdb-showb-{id}"), Duration::from_secs(6 * 3600), || tvdb(cfg, &format!("series/{id}")))?;
    let name = v["data"]["name"].as_str().or(base_v["data"]["name"].as_str()).unwrap_or("").to_string();
    Ok(Show {
        provider: Provider::Tvdb,
        id,
        title: name,
        year: base_v["data"]["year"].as_str().and_then(|y| y.parse().ok()).or(base_v["data"]["firstAired"].as_str().and_then(year_of)),
        aliases: vec![],
        tvdb_id: Some(id),
        tmdb_id: None,
    })
}

/// Every episode of a TheTVDB show, in the default (aired) order, with absolute numbers.
pub fn tvdb_episodes(cfg: &Config, id: u64) -> Result<Vec<Episode>, String> {
    let l3 = lang3(cfg);
    let v = cached(&format!("tvdb-eps-{id}-{l3}"), Duration::from_secs(6 * 3600), || {
        let mut all = Vec::new();
        for page in 0..20 {
            let r = tvdb(cfg, &format!("series/{id}/episodes/default/{l3}?page={page}"))?;
            let eps = r["data"]["episodes"].as_array().cloned().unwrap_or_default();
            for e in &eps {
                all.push(json!({
                    "season": e["seasonNumber"], "number": e["number"], "absolute": e["absoluteNumber"],
                    "title": e["name"], "air_date": e["aired"],
                }));
            }
            if eps.is_empty() || r["links"]["next"].is_null() {
                break;
            }
        }
        Ok(json!(all))
    })?;
    Ok(parse_episodes(&v))
}

// ---------- AniDB ----------

/// AniDB bans clients that ask more than once every 2 seconds.
fn anidb_wait() {
    static LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let mut last = LAST.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(at) = *last {
        let gap = Duration::from_millis(std::env::var("ELIXIR_ANIDB_GAP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(2500));
        if at.elapsed() < gap {
            std::thread::sleep(gap - at.elapsed());
        }
    }
    *last = Some(Instant::now());
}

static ANIDB_BANNED: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Clone, Debug)]
struct AniTitle {
    aid: u64,
    title: String,
    main: bool,
    english: bool,
}

/// AniDB's title list, downloaded at most once a day (their rule) and kept on disk.
fn anidb_titles() -> Result<std::sync::Arc<Vec<AniTitle>>, String> {
    static TITLES: OnceLock<Mutex<Option<(Instant, std::sync::Arc<Vec<AniTitle>>)>>> = OnceLock::new();
    let slot = TITLES.get_or_init(Default::default);
    let mut t = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = t.as_ref() {
        if at.elapsed() < Duration::from_secs(24 * 3600) {
            return Ok(v.clone());
        }
    }
    let path = var_dir().join("anidb-titles.xml.gz");
    let fresh = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok()).and_then(|m| m.elapsed().ok()).is_some_and(|age| age < Duration::from_secs(24 * 3600));
    if !fresh {
        let r = http::ok("AniDB", &Request { timeout: 120, ..Request::get(base("ELIXIR_ANIDB_TITLES_URL", "http://anidb.net/api/anime-titles.xml.gz")) });
        match r {
            Ok(r) if r.body.len() > 100 => {
                let _ = crate::config::write_atomic(&path, &r.body);
            }
            Ok(_) | Err(_) if path.exists() => {}
            Ok(_) => return Err("AniDB sent an empty title list".into()),
            Err(e) => return Err(e),
        }
    }
    let gz = std::fs::read(&path).map_err(|e| e.to_string())?;
    let mut xml = String::new();
    use std::io::Read;
    if flate2::read::GzDecoder::new(&gz[..]).read_to_string(&mut xml).is_err() {
        xml = String::from_utf8_lossy(&gz).into_owned();
    }
    let list = std::sync::Arc::new(parse_anidb_titles(&xml));
    *t = Some((Instant::now(), list.clone()));
    Ok(list)
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&#039;", "'").replace("&amp;", "&")
}

fn parse_anidb_titles(xml: &str) -> Vec<AniTitle> {
    let anime = regex::Regex::new(r#"(?s)<anime aid="(\d+)">(.*?)</anime>"#).expect("regex");
    let title = regex::Regex::new(r#"<title ([^>]*)>([^<]*)</title>"#).expect("regex");
    let mut out = Vec::new();
    for a in anime.captures_iter(xml) {
        let aid: u64 = a[1].parse().unwrap_or(0);
        for t in title.captures_iter(&a[2]) {
            let attrs = &t[1];
            out.push(AniTitle {
                aid,
                title: xml_unescape(&t[2]),
                main: attrs.contains("type=\"main\""),
                english: attrs.contains("xml:lang=\"en\"") && (attrs.contains("official") || attrs.contains("main")),
            });
        }
    }
    out
}

fn anidb_anime_xml(cfg: &Config, aid: u64) -> Result<String, String> {
    if cfg.anidb_client.is_empty() {
        return Err("AniDB needs a registered client name (Settings › Databases)".into());
    }
    let dir = var_dir().join("anidb");
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join(format!("{aid}.xml"));
    let fresh = std::fs::metadata(&file).ok().and_then(|m| m.modified().ok()).and_then(|m| m.elapsed().ok()).is_some_and(|age| age < Duration::from_secs(3 * 24 * 3600));
    if fresh {
        return std::fs::read_to_string(&file).map_err(|e| e.to_string());
    }
    if let Some(at) = *ANIDB_BANNED.lock().unwrap_or_else(|e| e.into_inner()) {
        if at.elapsed() < Duration::from_secs(3600) {
            return Err("AniDB asked Elixir to slow down. It will try again in an hour".into());
        }
    }
    anidb_wait();
    let url = format!(
        "{}/httpapi?request=anime&client={}&clientver={}&protover=1&aid={aid}",
        base("ELIXIR_ANIDB_URL", "http://api.anidb.net:9001"),
        enc(&cfg.anidb_client),
        cfg.anidb_version
    );
    let r = http::ok("AniDB", &Request { compressed: true, ..Request::get(url) })?;
    let text = String::from_utf8_lossy(&r.body).into_owned();
    if let Some(err) = regex::Regex::new(r"<error[^>]*>([^<]*)</error>").expect("regex").captures(&text) {
        let msg = err[1].to_string();
        if msg.to_ascii_lowercase().contains("banned") {
            *ANIDB_BANNED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
            return Err("AniDB asked Elixir to slow down. It will try again in an hour".into());
        }
        if msg.to_ascii_lowercase().contains("client") {
            return Err("AniDB doesn't recognise the client name. Check it in Settings".into());
        }
        return Err(format!("AniDB said: {msg}"));
    }
    let _ = crate::config::write_atomic(&file, text.as_bytes());
    Ok(text)
}

pub fn anidb_search(title: &str) -> Result<Vec<Show>, String> {
    let titles = anidb_titles()?;
    let want = crate::parse::normalize(title);
    let mut scored: Vec<(u64, u32)> = Vec::new();
    for t in titles.iter() {
        let n = crate::parse::normalize(&t.title);
        let score = if n == want {
            100
        } else if n.starts_with(&want) || want.starts_with(&n) {
            60
        } else if n.contains(&want) {
            40
        } else {
            0
        };
        if score > 0 {
            match scored.iter_mut().find(|(a, _)| *a == t.aid) {
                Some(s) => s.1 = s.1.max(score),
                None => scored.push((t.aid, score)),
            }
        }
    }
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(scored
        .into_iter()
        .take(8)
        .map(|(aid, _)| {
            let all: Vec<&AniTitle> = titles.iter().filter(|t| t.aid == aid).collect();
            let main = all.iter().find(|t| t.english).or(all.iter().find(|t| t.main)).map(|t| t.title.clone()).unwrap_or_default();
            Show {
                provider: Provider::Anidb,
                id: aid,
                title: main.clone(),
                year: None,
                aliases: all.iter().map(|t| t.title.clone()).filter(|t| *t != main).collect(),
                tvdb_id: None,
                tmdb_id: None,
            }
        })
        .collect())
}

/// Episodes of an AniDB anime (regular episodes only), and its start year.
pub fn anidb_episodes(cfg: &Config, aid: u64) -> Result<(Option<u16>, Vec<Episode>), String> {
    let xml = anidb_anime_xml(cfg, aid)?;
    let year = regex::Regex::new(r"<startdate>(\d{4})").expect("regex").captures(&xml).and_then(|c| c[1].parse().ok());
    let ep_re = regex::Regex::new(r"(?s)<episode [^>]*>(.*?)</episode>").expect("regex");
    let epno = regex::Regex::new(r#"<epno type="1">(\d+)</epno>"#).expect("regex");
    let air = regex::Regex::new(r"<airdate>([^<]*)</airdate>").expect("regex");
    let t_en = regex::Regex::new(r#"<title xml:lang="en">([^<]*)</title>"#).expect("regex");
    let mut eps = Vec::new();
    for e in ep_re.captures_iter(&xml) {
        let body = &e[1];
        let Some(n) = epno.captures(body).and_then(|c| c[1].parse::<u32>().ok()) else { continue };
        eps.push(Episode {
            season: 1,
            number: n,
            absolute: Some(n),
            title: t_en.captures(body).map(|c| xml_unescape(&c[1])).unwrap_or_default(),
            air_date: air.captures(body).map(|c| c[1].to_string()),
        });
    }
    eps.sort_by_key(|e| e.number);
    Ok((year, eps))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anidb_title_list() {
        let xml = r#"<?xml version="1.0"?><animetitles>
<anime aid="17617"><title xml:lang="x-jat" type="main">Sousou no Frieren</title><title xml:lang="en" type="official">Frieren: Beyond Journey&apos;s End</title></anime>
<anime aid="1"><title xml:lang="x-jat" type="main">Seikai no Monshou</title></anime></animetitles>"#;
        let t = parse_anidb_titles(xml);
        assert_eq!(t.len(), 3);
        assert!(t[0].main && !t[0].english);
        assert!(t[1].english);
        assert_eq!(t[1].title, "Frieren: Beyond Journey's End");
    }
}
