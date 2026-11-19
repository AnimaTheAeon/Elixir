//! Talking to Lifestream (Transmission RPC), Midgar (Sonarr API) and Turks
//! (Radarr-style API). Only reads, plus Midgar's own import and rescans.

use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::config::Config;
use crate::http::{self, enc, Request};
use crate::naming::EpisodeNaming;

// ---------- Lifestream ----------

#[derive(Clone, Debug, Default)]
pub struct Torrent {
    pub hash: String,
    pub dir: String,
    pub total: u64,
    pub percent: f64,
    pub status: i64,
    pub files: Vec<TorrentFile>,
}

#[derive(Clone, Debug, Default)]
pub struct TorrentFile {
    pub path: PathBuf,
    pub length: u64,
    pub done: u64,
}

impl Torrent {
    /// Has its file list, and everything wanted is here.
    pub fn finished(&self) -> bool {
        self.total > 0 && self.percent >= 1.0
    }
}

pub fn lifestream_configured(cfg: &Config) -> bool {
    !cfg.lifestream_url.is_empty()
}

fn session_id() -> &'static Mutex<String> {
    static S: OnceLock<Mutex<String>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn lifestream_rpc(cfg: &Config, body: &Value) -> Result<Value, String> {
    let url = format!("{}/transmission/rpc", cfg.lifestream_url);
    for _ in 0..2 {
        let sid = session_id().lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut req = Request::post_json(url.clone(), body).header("X-Transmission-Session-Id", &sid);
        if !cfg.lifestream_user.is_empty() {
            req.basic_auth = Some((cfg.lifestream_user.clone(), cfg.lifestream_pass.clone()));
        }
        let r = http::send("Lifestream", &req)?;
        if r.status == 409 {
            if let Some(new) = r.header("X-Transmission-Session-Id") {
                *session_id().lock().unwrap_or_else(|e| e.into_inner()) = new;
                continue;
            }
        }
        return match r.status {
            200..=299 => r.json().map_err(|e| format!("Lifestream {e}")),
            401 | 403 => Err("Lifestream refused the username or password. Check them in Settings".into()),
            s => Err(format!("Lifestream answered with an error (HTTP {s})")),
        };
    }
    Err("Lifestream didn't accept the session".into())
}

pub fn lifestream_torrents(cfg: &Config) -> Result<Vec<Torrent>, String> {
    let v = lifestream_rpc(
        cfg,
        &json!({"method": "torrent-get", "arguments": {"fields": ["hashString", "downloadDir", "totalSize", "percentDone", "status", "files"]}}),
    )?;
    if v["result"].as_str().is_some_and(|r| r != "success") {
        return Err(format!("Lifestream said: {}", v["result"].as_str().unwrap_or("")));
    }
    Ok(v["arguments"]["torrents"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            let dir = t["downloadDir"].as_str().unwrap_or("").trim_end_matches('/').to_string();
            Torrent {
                hash: t["hashString"].as_str().unwrap_or("").to_ascii_lowercase(),
                total: t["totalSize"].as_u64().unwrap_or(0),
                percent: t["percentDone"].as_f64().unwrap_or(0.0),
                status: t["status"].as_i64().unwrap_or(0),
                files: t["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| TorrentFile {
                        path: Path::new(&dir).join(f["name"].as_str().unwrap_or("")),
                        length: f["length"].as_u64().unwrap_or(0),
                        done: f["bytesCompleted"].as_u64().unwrap_or(0),
                    })
                    .collect(),
                dir,
            }
        })
        .collect())
}

// ---------- Midgar and Turks (shared *arr-style API) ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arr {
    Midgar,
    Turks,
}

impl Arr {
    pub fn label(self) -> &'static str {
        match self {
            Arr::Midgar => "Midgar",
            Arr::Turks => "Turks",
        }
    }
    fn conf(self, cfg: &Config) -> (&str, &str) {
        match self {
            Arr::Midgar => (&cfg.midgar_url, &cfg.midgar_key),
            Arr::Turks => (&cfg.turks_url, &cfg.turks_key),
        }
    }
    pub fn configured(self, cfg: &Config) -> bool {
        let (u, k) = self.conf(cfg);
        !u.is_empty() && !k.is_empty()
    }
}

fn arr_req(cfg: &Config, arr: Arr, path: &str, body: Option<&Value>, timeout: u64) -> Result<Value, String> {
    let (url, key) = arr.conf(cfg);
    let full = format!("{url}/api/v3/{path}");
    let mut req = match body {
        Some(b) => Request::post_json(full, b),
        None => Request::get(full),
    };
    req.timeout = timeout;
    let req = req.header("X-Api-Key", key);
    http::ok(arr.label(), &req)?.json().map_err(|e| format!("{} {e}", arr.label()))
}

pub fn arr_get(cfg: &Config, arr: Arr, path: &str) -> Result<Value, String> {
    arr_req(cfg, arr, path, None, 30)
}

pub fn arr_post(cfg: &Config, arr: Arr, path: &str, body: &Value) -> Result<Value, String> {
    arr_req(cfg, arr, path, Some(body), 60)
}

pub fn arr_version(cfg: &Config, arr: Arr) -> Result<String, String> {
    Ok(arr_get(cfg, arr, "system/status")?["version"].as_str().unwrap_or("").to_string())
}

/// Torrent hashes this app grabbed or is tracking (queue + recent history), lowercase.
pub fn arr_hashes(cfg: &Config, arr: Arr) -> Result<HashSet<String>, String> {
    let mut out = HashSet::new();
    let q = arr_get(cfg, arr, "queue?page=1&pageSize=1000&includeUnknownSeriesItems=true&includeUnknownMovieItems=true")?;
    let records = q["records"].as_array().or(q.as_array()).cloned().unwrap_or_default();
    for r in records {
        if let Some(id) = r["downloadId"].as_str() {
            out.insert(id.to_ascii_lowercase());
        }
    }
    for page in 1..=3 {
        let h = arr_get(cfg, arr, &format!("history?page={page}&pageSize=1000&sortKey=date&sortDirection=descending"))?;
        let recs = h["records"].as_array().cloned().unwrap_or_default();
        for r in &recs {
            if let Some(id) = r["downloadId"].as_str().filter(|s| !s.is_empty()) {
                out.insert(id.to_ascii_lowercase());
            }
        }
        if recs.len() < 1000 {
            break;
        }
    }
    Ok(out)
}

#[derive(Clone, Debug, Default)]
pub struct Tracked {
    pub id: i64,
    pub title: String,
    pub year: Option<u16>,
    pub tvdb_id: Option<u64>,
    pub tmdb_id: Option<u64>,
    pub path: String,
}

/// Shows (Midgar) or movies (Turks) the app tracks, cached for a minute.
pub fn arr_library(cfg: &Config, arr: Arr) -> Result<Vec<Tracked>, String> {
    static CACHE: OnceLock<Mutex<Vec<(Arr, std::time::Instant, Vec<Tracked>)>>> = OnceLock::new();
    let c = CACHE.get_or_init(Default::default);
    if let Some((_, _, v)) = c.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(a, at, _)| *a == arr && at.elapsed() < Duration::from_secs(60)) {
        return Ok(v.clone());
    }
    let v = arr_get(cfg, arr, if arr == Arr::Midgar { "series" } else { "movie" })?;
    let list: Vec<Tracked> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| Tracked {
            id: s["id"].as_i64().unwrap_or(0),
            title: s["title"].as_str().unwrap_or("").to_string(),
            year: s["year"].as_u64().map(|y| y as u16).filter(|y| *y > 0),
            tvdb_id: s["tvdbId"].as_u64().filter(|v| *v > 0),
            tmdb_id: s["tmdbId"].as_u64().filter(|v| *v > 0),
            path: s["path"].as_str().unwrap_or("").to_string(),
        })
        .collect();
    let mut g = c.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|(a, _, _)| *a != arr);
    g.push((arr, std::time::Instant::now(), list.clone()));
    Ok(list)
}

pub fn arr_roots(cfg: &Config, arr: Arr) -> Result<Vec<String>, String> {
    let v = arr_get(cfg, arr, "rootfolder")?;
    Ok(v.as_array().into_iter().flatten().filter_map(|r| r["path"].as_str().map(|p| p.trim_end_matches('/').to_string())).collect())
}

pub fn midgar_naming(cfg: &Config) -> Result<EpisodeNaming, String> {
    let v = arr_get(cfg, Arr::Midgar, "config/naming")?;
    let d = EpisodeNaming::default();
    let s = |k: &str, def: &str| v[k].as_str().filter(|s| !s.trim().is_empty()).unwrap_or(def).to_string();
    Ok(EpisodeNaming {
        standard: s("standardEpisodeFormat", &d.standard),
        daily: s("dailyEpisodeFormat", &d.daily),
        anime: s("animeEpisodeFormat", &d.anime),
        series_folder: s("seriesFolderFormat", &d.series_folder),
        season_folder: s("seasonFolderFormat", &d.season_folder),
        specials_folder: s("specialsFolderFormat", &d.specials_folder),
        multi_style: v["multiEpisodeStyle"].as_u64().map(|n| n as u8).unwrap_or(d.multi_style),
        colon: v["colonReplacementFormat"].as_u64().map(|n| n as u8).unwrap_or(d.colon),
    })
}

/// Whether the app makes hardlinks when it imports (None = couldn't tell).
pub fn arr_uses_hardlinks(cfg: &Config, arr: Arr) -> Option<bool> {
    arr_get(cfg, arr, "config/mediamanagement").ok().and_then(|v| v["copyUsingHardlinks"].as_bool())
}

/// Ask Midgar to import one file it tracks. `copy` keeps the original (hardlink when Midgar can).
/// Returns Midgar's own words when it won't.
pub fn midgar_import(cfg: &Config, file: &Path, series_id: i64, episode_ids: &[i64], copy: bool) -> Result<String, String> {
    let folder = file.parent().unwrap_or(Path::new("/"));
    let list = arr_req(cfg, Arr::Midgar, &format!("manualimport?folder={}&filterExistingFiles=false", enc(&folder.to_string_lossy())), None, 120)?;
    let entry = list
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| e["path"].as_str().is_some_and(|p| Path::new(p) == file))
        .cloned()
        .ok_or("Midgar doesn't see this file as a video it can import")?;
    let ids: Vec<i64> = if episode_ids.is_empty() {
        entry["episodes"].as_array().into_iter().flatten().filter_map(|e| e["id"].as_i64()).collect()
    } else {
        episode_ids.to_vec()
    };
    let sid = if series_id > 0 { series_id } else { entry["series"]["id"].as_i64().unwrap_or(0) };
    if sid == 0 || ids.is_empty() {
        return Err("Midgar couldn't tell which episode this is".into());
    }
    // Midgar's own reasons it wouldn't import (not an upgrade, sample, ...).
    let hard: Vec<String> = entry["rejections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["type"].as_str() == Some("permanent"))
        .filter_map(|r| r["reason"].as_str().map(String::from))
        .filter(|r| !r.to_lowercase().contains("unable to parse") && !r.to_lowercase().contains("unknown series"))
        .collect();
    if !hard.is_empty() {
        return Err(format!("Midgar won't import it: {}", hard.join("; ")));
    }
    let body = json!({
        "name": "ManualImport",
        "importMode": if copy { "copy" } else { "move" },
        "files": [{
            "path": file.to_string_lossy(),
            "folderName": folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            "seriesId": sid,
            "episodeIds": ids,
            "quality": entry["quality"],
            "languages": entry["languages"],
            "releaseGroup": entry["releaseGroup"],
            "indexerFlags": entry["indexerFlags"].as_i64().unwrap_or(0),
            "releaseType": entry["releaseType"].as_str().unwrap_or("singleEpisode"),
        }]
    });
    let cmd = arr_post(cfg, Arr::Midgar, "command", &body)?;
    let id = cmd["id"].as_i64().ok_or("Midgar didn't start the import")?;
    // Wait for Midgar to finish (it's quick: a hardlink and a database write).
    for _ in 0..90 {
        std::thread::sleep(Duration::from_secs(1));
        let st = arr_get(cfg, Arr::Midgar, &format!("command/{id}"))?;
        match st["status"].as_str().unwrap_or("") {
            "completed" => {
                // Make sure the file really arrived.
                let files = arr_get(cfg, Arr::Midgar, &format!("episode?seriesId={sid}"))?;
                let have = files.as_array().into_iter().flatten().filter(|e| ids.contains(&e["id"].as_i64().unwrap_or(-1))).all(|e| e["hasFile"].as_bool() == Some(true));
                return if have { Ok(format!("Midgar imported it ({} episode{})", ids.len(), if ids.len() == 1 { "" } else { "s" })) } else { Err("Midgar finished but the episode still has no file. See Midgar's log".into()) };
            }
            "failed" | "aborted" | "cancelled" | "orphaned" => {
                let msg = st["message"].as_str().or(st["exception"].as_str()).unwrap_or("no reason given");
                return Err(format!("Midgar's import failed: {msg}"));
            }
            _ => {}
        }
    }
    Err("Midgar is still busy with the import. Check Midgar's Activity".into())
}

pub fn arr_rescan(cfg: &Config, arr: Arr, id: i64) {
    let body = match arr {
        Arr::Midgar => json!({"name": "RescanSeries", "seriesId": id}),
        Arr::Turks => json!({"name": "RescanMovie", "movieIds": [id]}),
    };
    let _ = arr_post(cfg, arr, "command", &body);
}
