//! Loopback-only JSON API used by the DSM window (through api.cgi).
//! Every request must carry a valid DSM session for an administrator.
//!
//! On Windows the same server also serves the window's pages, and requests
//! must carry the key the window was given when it opened.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
#[cfg(unix)]
use std::time::Duration;

use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::config::{self, Config, SECRETS};
use crate::engine::{self, Item, Origin, Shared, Source, State};
use crate::suite::{self, Arr};
use crate::{info, warn};

pub const DEFAULT_PORT: u16 = 5471;
#[cfg(unix)]
const AUTH_TTL: Duration = Duration::from_secs(300);

pub fn port() -> u16 {
    std::env::var("ELIXIR_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT)
}

type AuthCache = Mutex<HashMap<(String, String), (String, Instant)>>;

#[cfg(unix)]
pub fn serve(shared: Arc<Shared>) {
    let addr = format!("127.0.0.1:{}", port());
    let server = match Server::http(&addr) {
        Ok(server) => Arc::new(server),
        Err(error) => {
            warn!("Can't listen on {addr}: {error}");
            std::process::exit(1);
        }
    };
    info!("API listening on {addr}");
    serve_on(server, shared);
}

/// Listen on loopback: the usual port, or any free one if it's taken (Windows).
#[cfg(windows)]
pub fn bind_any() -> Result<(Arc<Server>, u16), String> {
    let server = Server::http(format!("127.0.0.1:{}", port())).or_else(|_| Server::http("127.0.0.1:0")).map_err(|e| e.to_string())?;
    let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
    info!("Listening on 127.0.0.1:{port}");
    Ok((Arc::new(server), port))
}

pub fn serve_on(server: Arc<Server>, shared: Arc<Shared>) {
    let cache: Arc<AuthCache> = Arc::new(Mutex::new(HashMap::new()));
    // A few workers, so a slow request (a connection test) doesn't hold up the window.
    let mut workers = Vec::new();
    for _ in 0..4 {
        let (server, shared, cache) = (server.clone(), shared.clone(), cache.clone());
        workers.push(std::thread::spawn(move || {
            for request in server.incoming_requests() {
                handle(request, &shared, &cache);
            }
        }));
    }
    for w in workers {
        let _ = w.join();
    }
}

/// The key the Windows window sends with every request (made fresh each start).
#[cfg(windows)]
pub fn window_key() -> &'static str {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    KEY.get_or_init(|| config::random_hex(48))
}

/// The window's own files, built into the program (Windows).
#[cfg(windows)]
fn static_file(path: &str) -> Option<(&'static [u8], &'static str)> {
    let f: (&'static [u8], &'static str) = match path {
        "/" | "/index.html" => (include_bytes!("../ui/index.html"), "text/html; charset=utf-8"),
        "/app.js" => (include_bytes!("../ui/app.js"), "text/javascript; charset=utf-8"),
        "/elixir.css" => (include_bytes!("../ui/elixir.css"), "text/css; charset=utf-8"),
        "/fonts/nunito-700.woff2" => (include_bytes!("../ui/fonts/nunito-700.woff2"), "font/woff2"),
        "/fonts/nunito-800.woff2" => (include_bytes!("../ui/fonts/nunito-800.woff2"), "font/woff2"),
        "/fonts/orbitron-800.ttf" => (include_bytes!("../ui/fonts/orbitron-800.ttf"), "font/ttf"),
        "/images/elixir_16.png" => (include_bytes!("../ui/images/elixir_16.png"), "image/png"),
        "/images/elixir_24.png" => (include_bytes!("../ui/images/elixir_24.png"), "image/png"),
        "/images/elixir_32.png" => (include_bytes!("../ui/images/elixir_32.png"), "image/png"),
        "/images/elixir_48.png" => (include_bytes!("../ui/images/elixir_48.png"), "image/png"),
        "/images/elixir_64.png" => (include_bytes!("../ui/images/elixir_64.png"), "image/png"),
        "/images/elixir_72.png" => (include_bytes!("../ui/images/elixir_72.png"), "image/png"),
        "/images/elixir_256.png" => (include_bytes!("../ui/images/elixir_256.png"), "image/png"),
        _ => return None,
    };
    Some(f)
}

/// Windows: serve a page file, or say whether an API request may go on.
#[cfg(windows)]
fn windows_gate(req: Request) -> Option<Request> {
    // Only pages that came from this server (blocks DNS-rebinding tricks).
    let host = header(&req, "Host");
    let host_ok = host.starts_with("127.0.0.1:") || host.starts_with("localhost:");
    let path = req.url().split('?').next().unwrap_or("/").to_string();
    if !host_ok {
        respond(req, 403, json!({ "error": "Not allowed" }));
        return None;
    }
    if path == "/api.cgi" || path == "/api" {
        if header(&req, "X-Elixir-Key") != window_key() {
            respond(req, 401, json!({ "error": "Open Elixir from its own window." }));
            return None;
        }
        return Some(req);
    }
    match static_file(&path) {
        Some((bytes, kind)) => {
            let response = Response::from_data(bytes)
                .with_header(Header::from_bytes("Content-Type", kind).unwrap())
                .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap())
                .with_header(Header::from_bytes("X-Content-Type-Options", "nosniff").unwrap());
            let _ = req.respond(response);
        }
        None => {
            let _ = req.respond(Response::from_string("Not found").with_status_code(404));
        }
    }
    None
}

fn header(req: &Request, name: &str) -> String {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default()
}

fn query(url: &str) -> HashMap<String, String> {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (!k.is_empty()).then(|| (percent_decode(k), percent_decode(v)))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Check the DSM session with DSM's own authenticate.cgi; admins only.
#[cfg(unix)]
fn authenticate(cookie: &str, token: &str, remote: &str, cache: &AuthCache) -> Result<String, String> {
    if std::env::var("ELIXIR_NO_AUTH").is_ok_and(|v| v == "1") {
        return Ok("dev".into());
    }
    if cookie.is_empty() {
        return Err("Not signed in to DSM".into());
    }
    let key = (cookie.to_string(), token.to_string());
    if let Some((user, at)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        if at.elapsed() < AUTH_TTL {
            return Ok(user.clone());
        }
    }
    let script = [
        "/usr/syno/synoman/webman/authenticate.cgi",
        "/usr/syno/synoman/webman/modules/authenticate.cgi",
    ]
    .into_iter()
    .find(|p| Path::new(p).exists())
    .ok_or("DSM sign-in check isn't available")?;
    let output = std::process::Command::new(script)
        .env_clear()
        .env("HTTP_COOKIE", cookie)
        .env("HTTP_X_SYNO_TOKEN", token)
        .env("REMOTE_ADDR", if remote.is_empty() { "127.0.0.1" } else { remote })
        .env("REQUEST_METHOD", "GET")
        .env("QUERY_STRING", format!("SynoToken={token}"))
        .output()
        .map_err(|e| e.to_string())?;
    let user = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if user.is_empty() {
        return Err("Your DSM session has expired. Sign in to DSM again.".into());
    }
    if !is_admin(&user) {
        return Err("Only DSM administrators can use Elixir".into());
    }
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, (_, at)| at.elapsed() < AUTH_TTL);
    cache.insert(key, (user.clone(), Instant::now()));
    Ok(user)
}

#[cfg(unix)]
fn is_admin(user: &str) -> bool {
    if user == "root" {
        return true;
    }
    std::process::Command::new("id")
        .args(["-Gn", user])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().any(|g| g == "administrators"))
        .unwrap_or(false)
}

fn respond(req: Request, code: u16, body: Value) {
    let data = body.to_string();
    let response = Response::from_string(data)
        .with_status_code(code)
        .with_header(Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap())
        .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap());
    let _ = req.respond(response);
}

/// Whether `path` is inside /volumeN (no `..` tricks).
#[cfg(unix)]
fn inside_volume(path: &Path) -> bool {
    if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return false;
    }
    path.strip_prefix(browse_root())
        .ok()
        .and_then(|rel| rel.components().next())
        .and_then(|c| c.as_os_str().to_str())
        .is_some_and(|first| first.starts_with("volume") && first[6..].chars().all(|c| c.is_ascii_digit()) && first.len() > 6)
}

/// Windows: a full path on a drive (C:\\…) or a network share (\\\\server\\share\\…).
#[cfg(windows)]
fn inside_volume(path: &Path) -> bool {
    !path.components().any(|c| matches!(c, std::path::Component::ParentDir)) && path.is_absolute()
}

#[cfg(unix)]
fn browse_root() -> String {
    std::env::var("ELIXIR_BROWSE_ROOT").unwrap_or_else(|_| "/".into())
}

/// Where-to-pick wording for errors.
const PICK_HINT: &str = if cfg!(windows) { "Pick a folder on a drive or a network share" } else { "Pick a folder on one of the NAS volumes" };

/// Drop a trailing slash but keep "C:\\" and "/" whole.
fn trim_folder(path: &str) -> String {
    let t = path.trim().trim_end_matches(['/', '\\']);
    if t.is_empty() {
        return path.trim().chars().take(1).collect();
    }
    if cfg!(windows) && t.len() == 2 && t.ends_with(':') {
        return format!("{t}\\");
    }
    t.to_string()
}

fn handle(req: Request, shared: &Arc<Shared>, cache: &AuthCache) {
    #[cfg(windows)]
    let Some(mut req) = windows_gate(req) else { return };
    #[cfg(windows)]
    let _ = cache;
    #[cfg(unix)]
    let mut req = req;
    let q = query(req.url());
    let route = q.get("r").cloned().unwrap_or_default();
    #[cfg(unix)]
    {
        let cookie = header(&req, "X-Elixir-Cookie");
        let token = header(&req, "X-Elixir-Token");
        let remote = header(&req, "X-Elixir-Remote");
        if let Err(message) = authenticate(&cookie, &token, &remote, cache) {
            return respond(req, 401, json!({ "error": message }));
        }
    }
    let mut body = String::new();
    if *req.method() == Method::Post {
        let _ = req.as_reader().take(64 * 1024).read_to_string(&mut body);
    }
    let input: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let post = *req.method() == Method::Post;
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let n = |k: &str| input.get(k).and_then(Value::as_u64).unwrap_or(0);

    let result: Result<Value, String> = match (post, route.as_str()) {
        (false, "status") => Ok(status(shared)),
        (false, "config") => Ok(config_json(shared)),
        (true, "config") => save_config(shared, input.clone()),
        (false, "items") => Ok(items(shared, q.get("view").map(String::as_str).unwrap_or("rename"), q.get("batch").and_then(|b| b.parse().ok()))),
        (true, "scan") => {
            let path = s("path");
            if !inside_volume(Path::new(&path)) || !Path::new(&path).exists() {
                Err(format!("{PICK_HINT} (or a video on one)"))
            } else {
                engine::scan(shared, Path::new(&path), parse_source(&s("source")), Origin::Manual).map(|b| json!({ "batch": b }))
            }
        }
        (true, "existing") => {
            // "Rename these too": files that were already in a watched folder.
            let folder = s("folder");
            let paths: Vec<String> = {
                let inv = shared.inventory.lock().unwrap_or_else(|e| e.into_inner());
                inv.folders.get(&folder).map(|m| m.values().filter(|e| e.seen == engine::Seen::Old).map(|e| e.path.clone()).collect()).unwrap_or_default()
            };
            if paths.is_empty() {
                Err("Nothing left from before in that folder".into())
            } else {
                let batch = shared.next_id();
                let items: Vec<Item> = paths.iter().filter(|p| Path::new(p).is_file()).take(500).map(|p| engine::new_item(shared, Path::new(p), batch, Origin::Manual, Source::Auto)).collect();
                let ids: Vec<u64> = items.iter().map(|i| i.id).collect();
                shared.items.lock().unwrap_or_else(|e| e.into_inner()).extend(items);
                for id in &ids {
                    shared.queue_match(*id);
                }
                // They're the user's to rename now; automation won't offer them again.
                let mut inv = shared.inventory.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(m) = inv.folders.get_mut(&folder) {
                    for e in m.values_mut().filter(|e| e.seen == engine::Seen::Old) {
                        e.seen = engine::Seen::Taken;
                    }
                }
                drop(inv);
                shared.save_inventory();
                shared.save_items();
                Ok(json!({ "batch": batch }))
            }
        }
        (true, "source") => {
            // Match a whole batch (or one item) again with another source.
            let src = parse_source(&s("source"));
            let batch = n("batch");
            let id = n("id");
            let ids: Vec<u64> = {
                let mut items = shared.items.lock().unwrap_or_else(|e| e.into_inner());
                items
                    .iter_mut()
                    .filter(|i| (batch > 0 && i.batch == batch || i.id == id) && !matches!(i.state, State::Done))
                    .map(|i| {
                        i.source = src;
                        i.pinned = false;
                        i.chosen = None;
                        i.state = State::Matching;
                        i.id
                    })
                    .collect()
            };
            for id in &ids {
                shared.queue_match(*id);
            }
            shared.save_items();
            Ok(json!({ "ok": true, "count": ids.len() }))
        }
        (true, "search") => search(shared, &input),
        (true, "pick") => pick(shared, &input),
        (true, "apply") => {
            let ids: Vec<u64> = input.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
            if ids.is_empty() {
                Err("Nothing picked".into())
            } else {
                {
                    let mut items = shared.items.lock().unwrap_or_else(|e| e.into_inner());
                    for i in items.iter_mut().filter(|i| ids.contains(&i.id)) {
                        if matches!(i.state, State::Ready | State::Midgar | State::Practice) {
                            i.message = "Working…".into();
                        }
                    }
                }
                // Midgar imports can take a little while: answer now, the window polls.
                let sh = shared.clone();
                std::thread::spawn(move || {
                    if let Err(e) = engine::apply(&sh, &ids, false) {
                        warn!("Rename: {e}");
                    }
                });
                Ok(json!({ "ok": true }))
            }
        }
        (true, "skip") => {
            let id = n("id");
            shared.items.lock().unwrap_or_else(|e| e.into_inner()).iter_mut().filter(|i| i.id == id).for_each(|i| {
                i.state = State::Skipped;
                i.message = "Skipped".into();
                i.updated = engine::now();
            });
            shared.save_items();
            Ok(json!({ "ok": true }))
        }
        (true, "clear") => {
            let view = s("view");
            shared.items.lock().unwrap_or_else(|e| e.into_inner()).retain(|i| {
                let in_view = if view == "queue" { i.origin == Origin::Auto } else { i.origin == Origin::Manual };
                !(in_view && matches!(i.state, State::Done | State::Skipped | State::Owned | State::Failed | State::Practice) || in_view && view == "rename" && s("all") == "1")
            });
            shared.save_items();
            Ok(json!({ "ok": true }))
        }
        (false, "history") => Ok(history(shared)),
        (true, "undo") => {
            let id = n("run");
            engine::undo(id).map(|note| json!({ "ok": true, "note": note }))
        }
        (true, "watched/add") => add_watched(shared, &s("path")),
        (true, "watched/remove") => {
            let path = s("path");
            let mut cfg = shared.cfg();
            cfg.watched.retain(|f| *f != path);
            let _ = std::fs::remove_file(Path::new(&path).join(".elixir-owner"));
            shared.inventory.lock().unwrap_or_else(|e| e.into_inner()).folders.remove(&path);
            shared.save_inventory();
            info!("Stopped watching {path}");
            shared.update_config(cfg).map(|_| json!({ "ok": true }))
        }
        (true, "watched/noseed") => {
            // "Nothing seeds from this folder": automation may run without Lifestream and move files.
            let path = s("path");
            let on = input.get("on").and_then(Value::as_bool).unwrap_or(false);
            let mut cfg = shared.cfg();
            if !cfg.watched.contains(&path) {
                Err("That folder isn't watched".into())
            } else {
                cfg.no_seed.retain(|f| *f != path);
                if on {
                    cfg.no_seed.push(path.clone());
                }
                info!("{path}: nothing seeds from it = {on}");
                shared.update_config(cfg).map(|_| {
                    shared.wake();
                    json!({ "ok": true })
                })
            }
        }
        (true, "naming/example") => Ok(naming_example(shared, &input)),
        (true, "watched/takeover") => {
            let path = s("path");
            let cfg = shared.cfg();
            if !cfg.watched.contains(&path) {
                Err("That folder isn't watched".into())
            } else {
                engine::take_over(Path::new(&path), &cfg.owner_id).map(|_| {
                    shared.wake();
                    json!({ "ok": true })
                })
            }
        }
        (false, "preflight") => Ok(preflight(shared)),
        (true, "test") => test(shared, &s("service")),
        (false, "log") => Ok(json!({ "lines": crate::log::recent() })),
        (false, "browse") => browse(q.get("path").map(String::as_str).unwrap_or(""), q.get("files").is_some_and(|f| f == "1")),
        (false, "update") => Ok(crate::update::status_json()),
        (true, "update/install") => crate::update::start_install().map(|()| crate::update::status_json()),
        (true, "update/check") => {
            crate::update::request_check();
            Ok(crate::update::status_json())
        }
        _ => Err("Unknown request".into()),
    };
    match result {
        Ok(value) => respond(req, 200, value),
        Err(message) => respond(req, 400, json!({ "error": message })),
    }
}

fn parse_source(s: &str) -> Source {
    match s {
        "tmdb" => Source::Tmdb,
        "tvdb" => Source::Tvdb,
        "anidb" => Source::Anidb,
        _ => Source::Auto,
    }
}

fn status(shared: &Arc<Shared>) -> Value {
    let cfg = shared.cfg();
    let auto = shared.auto.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let items = shared.items.lock().unwrap_or_else(|e| e.into_inner());
    // Midnight on the NAS's clock.
    let off = crate::log::local_offset_seconds();
    let today = ((engine::now() as i64 + off).div_euclid(86_400) * 86_400 - off).max(0) as u64;
    let queue = items.iter().filter(|i| i.origin == Origin::Auto && matches!(i.state, State::Pick | State::Turks | State::Failed | State::Practice)).count();
    let matching = items.iter().filter(|i| i.state == State::Matching).count();
    drop(items);
    let renamed_today: usize = engine::load_runs().iter().filter(|r| r.time >= today && !r.undone).map(|r| r.items.iter().filter(|i| i.ok).count()).sum();
    let old: Vec<Value> = {
        let inv = shared.inventory.lock().unwrap_or_else(|e| e.into_inner());
        cfg.watched.iter().map(|f| json!({ "folder": f, "count": inv.folders.get(f).map(|m| m.values().filter(|e| e.seen == engine::Seen::Old).count()).unwrap_or(0) })).collect()
    };
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "automatic": cfg.automatic,
        "practice": cfg.practice,
        "auto": auto,
        "queue": queue,
        "matching": matching,
        "renamed_today": renamed_today,
        "old": old,
        "connected": {
            "lifestream": suite::lifestream_configured(&cfg),
            "midgar": Arr::Midgar.configured(&cfg),
            "turks": Arr::Turks.configured(&cfg),
            "tmdb": !cfg.tmdb_key.is_empty(),
            "tvdb": !cfg.tvdb_key.is_empty(),
            "anidb": !cfg.anidb_client.is_empty(),
        },
        "user": std::env::var("USER").unwrap_or_default(),
        "update": crate::update::status_json(),
    })
}

fn config_json(shared: &Arc<Shared>) -> Value {
    let config = shared.cfg();
    let mut value = serde_json::to_value(&config).unwrap_or(Value::Null);
    if let Some(obj) = value.as_object_mut() {
        for k in SECRETS {
            let has = obj.get(*k).and_then(Value::as_str).is_some_and(|v| !v.is_empty());
            obj.insert(k.to_string(), Value::String(String::new()));
            obj.insert(format!("has_{k}"), Value::Bool(has));
        }
        obj.remove("owner_id");
    }
    value
}

fn save_config(shared: &Arc<Shared>, input: Value) -> Result<Value, String> {
    let Value::Object(changes) = input else {
        return Err("Expected an object".into());
    };
    let current = shared.cfg();
    let mut merged = serde_json::to_value(&current).map_err(|e| e.to_string())?;
    let target = merged.as_object_mut().ok_or("bad config")?;
    let was_auto = current.automatic;
    for (key, value) in changes {
        // An empty secret field means "keep the saved one"; clear_<key> empties it.
        if SECRETS.contains(&key.as_str()) && value.as_str() == Some("") {
            continue;
        }
        if let Some(k) = key.strip_prefix("clear_") {
            if SECRETS.contains(&k) && value.as_bool() == Some(true) {
                target.insert(k.to_string(), Value::String(String::new()));
            }
            continue;
        }
        if key == "watched" || key == "owner_id" || key == "no_seed" {
            continue;
        }
        if target.contains_key(&key) {
            target.insert(key, value);
        }
    }
    let config: Config = serde_json::from_value(merged).map_err(|e| format!("Invalid setting: {e}"))?;
    if config.automatic && !was_auto {
        info!("Automatic renaming turned on{}", if config.practice { " (practice mode)" } else { "" });
    } else if !config.automatic && was_auto {
        info!("Automatic renaming turned off");
    }
    shared.update_config(config).map_err(|e| format!("Couldn't save settings: {e}"))?;
    Ok(config_json(shared))
}

fn item_json(i: &Item) -> Value {
    let name = |p: &str| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    json!({
        "id": i.id, "batch": i.batch, "origin": i.origin, "state": i.state, "message": i.message,
        "path": i.path, "name": name(&i.path), "folder": Path::new(&i.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        "size": i.size, "source": i.source, "parsed": i.parsed, "checks": i.checks,
        "candidates": i.candidates, "chosen": i.chosen, "pinned": i.pinned,
        "label": i.episode_label, "target": i.target, "target_name": name(&i.target),
        "sidecars": i.sidecars.len(), "torrent": !i.torrent.is_empty(), "torrent_known": i.torrent_known,
        "tracked_id": i.tracked_id, "tracked_title": i.tracked_title, "plan": i.plan,
        "created": i.created, "updated": i.updated,
    })
}

fn items(shared: &Arc<Shared>, view: &str, batch: Option<u64>) -> Value {
    let all = shared.items.lock().unwrap_or_else(|e| e.into_inner());
    let list: Vec<Value> = match view {
        "queue" => all.iter().filter(|i| i.origin == Origin::Auto && matches!(i.state, State::Pick | State::Turks | State::Failed | State::Practice | State::Matching)).rev().take(500).map(item_json).collect(),
        _ => {
            let b = batch.unwrap_or_else(|| all.iter().filter(|i| i.origin == Origin::Manual).map(|i| i.batch).max().unwrap_or(0));
            all.iter().filter(|i| i.origin == Origin::Manual && i.batch == b).map(item_json).collect()
        }
    };
    let batches: Vec<Value> = {
        let mut seen: Vec<(u64, String, usize)> = Vec::new();
        for i in all.iter().filter(|i| i.origin == Origin::Manual) {
            match seen.iter_mut().find(|(b, _, _)| *b == i.batch) {
                Some(s) => s.2 += 1,
                None => seen.push((i.batch, Path::new(&i.path).parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), 1)),
            }
        }
        seen.iter().rev().take(10).map(|(b, f, c)| json!({ "batch": b, "folder": f, "count": c })).collect()
    };
    json!({ "items": list, "batches": batches })
}

fn search(shared: &Arc<Shared>, input: &Value) -> Result<Value, String> {
    let cfg = shared.cfg();
    let query = input.get("query").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if query.is_empty() {
        return Err("Type a title to search for".into());
    }
    let year: Option<u16> = input.get("year").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).map(|y| y as u16).filter(|y| *y > 1800);
    let kind = input.get("kind").and_then(Value::as_str).unwrap_or("show");
    let source = parse_source(input.get("source").and_then(Value::as_str).unwrap_or("auto"));
    let list: Vec<Value> = if kind == "movie" {
        crate::meta::tmdb_search_movie(&cfg, &query, year)?.into_iter().map(|m| json!({ "kind": "movie", "provider": "tmdb", "id": m.tmdb_id, "title": m.title, "year": m.year, "tmdb_id": m.tmdb_id, "genres": m.genres })).collect()
    } else {
        let prov = match source {
            Source::Tmdb => crate::meta::Provider::Tmdb,
            Source::Anidb => crate::meta::Provider::Anidb,
            Source::Tvdb => crate::meta::Provider::Tvdb,
            Source::Auto => if cfg.tvdb_key.is_empty() { crate::meta::Provider::Tmdb } else { crate::meta::Provider::Tvdb },
        };
        let shows = match prov {
            crate::meta::Provider::Tmdb => crate::meta::tmdb_search_tv(&cfg, &query, year)?,
            crate::meta::Provider::Tvdb => crate::meta::tvdb_search(&cfg, &query, year)?,
            crate::meta::Provider::Anidb => crate::meta::anidb_search(&query)?,
        };
        shows.into_iter().map(|s| json!({ "kind": "show", "provider": s.provider, "id": s.id, "title": s.title, "year": s.year, "tvdb_id": s.tvdb_id, "tmdb_id": s.tmdb_id })).collect()
    };
    Ok(json!({ "results": list }))
}

/// The user picked a match: pin it and work the name out again.
fn pick(shared: &Arc<Shared>, input: &Value) -> Result<Value, String> {
    let id = input.get("id").and_then(Value::as_u64).ok_or("Missing item")?;
    let c = input.get("candidate").cloned().ok_or("Missing match")?;
    let cand: engine::Cand = serde_json::from_value(c).map_err(|e| format!("Bad match: {e}"))?;
    let also_batch = input.get("batch_too").and_then(Value::as_bool).unwrap_or(false);
    let ids: Vec<u64> = {
        let mut items = shared.items.lock().unwrap_or_else(|e| e.into_inner());
        let Some(item) = items.iter().find(|i| i.id == id).cloned() else { return Err("That item is gone".into()) };
        let key = engine::normalize_key(&item);
        items
            .iter_mut()
            .filter(|i| i.id == id || (also_batch && i.batch == item.batch && i.origin == item.origin && engine::normalize_key(i) == key && !matches!(i.state, State::Done)))
            .map(|i| {
                i.chosen = Some(cand.clone());
                i.pinned = true;
                if let Some(p) = cand.provider {
                    i.source = match p {
                        crate::meta::Provider::Tmdb => Source::Tmdb,
                        crate::meta::Provider::Tvdb => Source::Tvdb,
                        crate::meta::Provider::Anidb => Source::Anidb,
                    };
                }
                if cand.kind == "movie" && !matches!(i.parsed.kind, crate::parse::Kind::Movie | crate::parse::Kind::Unknown) {
                    i.parsed.kind = crate::parse::Kind::Movie;
                }
                i.state = State::Matching;
                i.message.clear();
                i.id
            })
            .collect()
    };
    for i in &ids {
        shared.queue_match(*i);
    }
    shared.save_items();
    Ok(json!({ "ok": true, "count": ids.len() }))
}

fn history(shared: &Arc<Shared>) -> Value {
    let runs = engine::load_runs();
    let cutoff = engine::now().saturating_sub(30 * 86_400);
    let list: Vec<Value> = runs
        .iter()
        .take(200)
        .map(|r| {
            let undoable = !r.undone && r.time >= cutoff && r.items.iter().any(|i| i.ok && !i.handoff);
            json!({
                "id": r.id, "time": r.time, "automatic": r.automatic, "undone": r.undone, "undo_note": r.undo_note, "interrupted": r.interrupted,
                "undoable": undoable,
                "items": r.items.iter().map(|i| json!({ "label": i.label, "from": i.from, "to": i.to, "ok": i.ok, "note": i.note, "handoff": i.handoff, "app": i.app })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let practice: Vec<Value> = shared.items.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|i| i.state == State::Practice).rev().take(100).map(item_json).collect();
    json!({ "runs": list, "practice": practice })
}

fn add_watched(shared: &Arc<Shared>, path: &str) -> Result<Value, String> {
    let path = trim_folder(path);
    let p = Path::new(&path);
    if !inside_volume(p) || !p.is_dir() {
        return Err(PICK_HINT.into());
    }
    let mut cfg = shared.cfg();
    if cfg.watched.iter().any(|f| crate::fsops::same_path(Path::new(f), p)) {
        return Err("That folder is already watched".into());
    }
    if let Some(f) = cfg.watched.iter().find(|f| crate::fsops::within(p, Path::new(f)) || crate::fsops::within(Path::new(f), p)) {
        return Err(format!("It overlaps {f}, which is already watched"));
    }
    // Midgar's and Turks' library folders are theirs.
    let roots = engine::library_roots(&cfg);
    if let Some((r, app)) = roots.iter().find(|(r, _)| crate::fsops::within(p, Path::new(r)) || crate::fsops::within(Path::new(r), p)) {
        return Err(format!("{app} manages {r}, so Elixir won't watch it. Watch your downloads instead; Rename by hand still works there"));
    }
    if !Arr::Midgar.configured(&cfg) || !Arr::Turks.configured(&cfg) {
        // Without Midgar/Turks connected their folders aren't known: refuse the obvious ones.
        let lower = path.to_lowercase();
        for (lib, what) in [(&cfg.tv_folder, "your TV folder"), (&cfg.movies_folder, "your movies folder"), (&cfg.anime_folder, "your anime folder")] {
            if !lib.is_empty() && (crate::fsops::within(p, Path::new(lib)) || crate::fsops::within(Path::new(lib), p)) {
                return Err(format!("That's {what}. Watch your downloads instead"));
            }
        }
        let _ = lower;
    }
    if std::fs::write(p.join(".elixir-write-test"), b"").is_err() {
        return Err(if cfg!(windows) {
            "Elixir can't write in that folder. Check that your Windows account (or the network share) allows changes there".into()
        } else {
            format!("Elixir can't write in that folder. Give the Elixir user Read/Write on the shared folder {}", path.split('/').nth(2).unwrap_or(""))
        });
    }
    let _ = std::fs::remove_file(p.join(".elixir-write-test"));
    let found = engine::snapshot_folder(shared, &path);
    cfg.watched.push(path.clone());
    info!("Watching {path} ({found} video(s) already there are left alone)");
    shared.update_config(cfg)?;
    Ok(json!({ "ok": true, "existing": found }))
}

/// Example names for the formats being edited (Settings).
fn naming_example(shared: &Arc<Shared>, input: &Value) -> Value {
    use crate::naming::{self, EpisodeNaming, MovieNaming, Values};
    let midgar_style = input.get("preset").and_then(Value::as_str) == Some("midgar");
    let cfg = shared.cfg();
    // "Same as Midgar": the formats read from Midgar (or Midgar's defaults).
    let e: EpisodeNaming = if midgar_style {
        if Arr::Midgar.configured(&cfg) {
            suite::midgar_naming(&cfg).unwrap_or_default()
        } else {
            EpisodeNaming::default()
        }
    } else {
        input.get("episode_naming").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
    };
    let m: MovieNaming = input.get("movie_naming").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let sep = std::path::MAIN_SEPARATOR.to_string();
    let show = |fmt: &str, season: u32, episodes: Vec<u32>, titles: Vec<&str>, date: Option<&str>| {
        let v = Values {
            series_title: "The Office (US)".into(),
            series_year: Some(2005),
            season: Some(season),
            episodes,
            absolute: vec![],
            episode_titles: titles.into_iter().map(String::from).collect(),
            air_date: date.map(String::from),
            ..Default::default()
        };
        let series = naming::clean_component(&naming::fill(&e.series_folder, &v, 5), e.colon);
        let season_dir = naming::clean_component(&naming::fill(if season == 0 { &e.specials_folder } else { &e.season_folder }, &v, 5), e.colon);
        let file = naming::clean_component(&naming::fill(fmt, &v, e.multi_style), e.colon);
        [series, season_dir, format!("{file}.mkv")].join(&sep)
    };
    let mv = Values { movie_title: "Dune: Part Two".into(), movie_year: Some(2024), ..Default::default() };
    let movie = [naming::clean_component(&naming::fill(&m.folder, &mv, 5), m.colon), format!("{}.mkv", naming::clean_component(&naming::fill(&m.file, &mv, 5), m.colon))].join(&sep);
    json!({
        "midgar": midgar_style && Arr::Midgar.configured(&cfg),
        "naming": e,
        "episode": show(&e.standard, 2, vec![1], vec!["The Dundies"], None),
        "multi": show(&e.standard, 3, vec![1, 2], vec!["Gay Witch Hunt", "The Convention"], None),
        "special": show(&e.standard, 0, vec![1], vec!["The Accountants"], None),
        "daily": show(&e.daily, 2024, vec![], vec!["Guest Night"], Some("2024-03-14")).replace("The Office (US)", "The Daily Show"),
        "movie": movie,
    })
}

/// The checks shown before automation is turned on.
fn preflight(shared: &Arc<Shared>) -> Value {
    let cfg = shared.cfg();
    let mut checks: Vec<Value> = Vec::new();
    let mut add = |name: &str, ok: Option<bool>, text: String| checks.push(json!({ "name": name, "ok": ok, "text": text }));
    match suite::lifestream_configured(&cfg).then(|| suite::lifestream_torrents(&cfg)) {
        None => add("Lifestream", Some(false), "Not set up. Automation needs it to know which files are seeding".into()),
        Some(Ok(t)) => add("Lifestream", Some(true), format!("Answers ({} torrent{})", t.len(), if t.len() == 1 { "" } else { "s" })),
        Some(Err(e)) => add("Lifestream", Some(false), e),
    }
    for arr in [Arr::Midgar, Arr::Turks] {
        if !arr.configured(&cfg) {
            add(arr.label(), None, "Not connected (optional; without it Elixir can't hand files to it)".into());
            continue;
        }
        match suite::arr_version(&cfg, arr) {
            Ok(v) => {
                let links = suite::arr_uses_hardlinks(&cfg, arr);
                let note = match links {
                    Some(true) => " · uses hardlinks".to_string(),
                    Some(false) => " · \"Use hardlinks\" is OFF, so its imports copy files (double space)".to_string(),
                    None => String::new(),
                };
                add(arr.label(), Some(links != Some(false)), format!("Answers (version {v}){note}"));
            }
            Err(e) => add(arr.label(), Some(false), e),
        }
    }
    let ctx = engine::Context::load(shared, &cfg);
    for folder in &cfg.watched {
        let f = Path::new(folder);
        for (lib, what) in [(&ctx.tv_root, "TV"), (&ctx.movie_root, "Movies"), (&ctx.anime_root, "Anime")] {
            if lib.is_empty() || (what == "Anime" && ctx.anime_root == ctx.tv_root) {
                continue;
            }
            let res = link_test(f, Path::new(lib));
            add(&format!("{} → {what}", short(folder)), Some(res.is_ok()), res.unwrap_or_else(|e| e));
        }
    }
    json!({ "checks": checks })
}

fn short(p: &str) -> String {
    if cfg!(windows) {
        p.to_string()
    } else {
        p.trim_start_matches("/volume1").to_string()
    }
}

/// Can files in `from` get a second name in `to` without copying?
fn link_test(from: &Path, to: &Path) -> Result<String, String> {
    if !to.is_dir() {
        return Err(format!("{} can't be reached", to.display()));
    }
    let src = from.join(".elixir-linktest");
    let dst = to.join(".elixir-linktest");
    std::fs::write(&src, b"elixir").map_err(|e| format!("Can't write in {}: {}", from.display(), crate::fsops::plain(&e)))?;
    let _ = std::fs::remove_file(&dst);
    let r = crate::fsops::link_or_clone(&src, &dst);
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
    match r {
        Ok(crate::fsops::Made::Hardlink) => Ok("Hardlinks work (no extra space)".into()),
        Ok(crate::fsops::Made::Clone) => Ok("Instant clones work (no extra space)".into()),
        Err(e) if e.contains("already exists") => Err(e),
        Err(_) => Err("No hardlink or clone possible: seeding files there would go to the Queue".into()),
    }
}

fn test(shared: &Arc<Shared>, service: &str) -> Result<Value, String> {
    let cfg = shared.cfg();
    let text = match service {
        "lifestream" => suite::lifestream_torrents(&cfg).map(|t| format!("Connected. {} torrent{}", t.len(), if t.len() == 1 { "" } else { "s" }))?,
        "midgar" => suite::arr_version(&cfg, Arr::Midgar).map(|v| format!("Connected to Midgar {v}"))?,
        "turks" => suite::arr_version(&cfg, Arr::Turks).map(|v| format!("Connected to Turks {v}"))?,
        "tmdb" => crate::meta::tmdb_search_movie(&cfg, "Inception", Some(2010)).map(|r| if r.is_empty() { "Connected, but the test search found nothing".to_string() } else { "Connected".to_string() })?,
        "tvdb" => crate::meta::tvdb_search(&cfg, "Breaking Bad", None).map(|_| "Connected".to_string())?,
        "anidb" => crate::meta::anidb_search("Cowboy Bebop").map(|r| format!("Title list ready ({} matches for a test search). Episode lookups use the client name", r.len()))?,
        _ => return Err("Unknown service".into()),
    };
    Ok(json!({ "ok": true, "text": text }))
}

/// Folders (and with `files`, videos) for the pickers. NAS volumes only.
#[cfg(unix)]
fn browse(path: &str, files: bool) -> Result<Value, String> {
    let root = browse_root();
    let path = path.trim_end_matches('/');
    let mut entries: Vec<Value> = Vec::new();
    let list = |dir: &Path, out: &mut Vec<Value>, volume: Option<String>| {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut all: Vec<_> = rd.flatten().collect();
        all.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
        for e in all {
            let name = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() && !config::is_system_folder(&name) {
                let readable = std::fs::read_dir(&p).is_ok();
                out.push(json!({ "name": name, "path": p.to_string_lossy(), "dir": true, "readable": readable, "volume": volume }));
            } else if files && t.is_file() && config::is_video(&p) {
                out.push(json!({ "name": name, "path": p.to_string_lossy(), "dir": false, "size": e.metadata().map(|m| m.len()).unwrap_or(0) }));
            }
        }
    };
    let (current, parent): (String, Option<String>) = if path.is_empty() {
        let mut volumes: Vec<PathBuf> = std::fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("volume") && n.len() > 6 && n[6..].chars().all(|c| c.is_ascii_digit())))
            .collect();
        volumes.sort();
        for v in volumes {
            let vol = v.file_name().map(|n| n.to_string_lossy().into_owned());
            let mut dirs = Vec::new();
            list(&v, &mut dirs, vol);
            entries.extend(dirs.into_iter().filter(|d| d["dir"].as_bool() == Some(true)));
        }
        (String::new(), None)
    } else {
        let p = Path::new(path);
        if !inside_volume(p) {
            return Err("Pick a folder on one of the NAS volumes".into());
        }
        list(p, &mut entries, None);
        let parent = p.parent().filter(|pp| pp.strip_prefix(&root).ok().is_some_and(|rel| rel.components().count() >= 2)).map(|pp| pp.to_string_lossy().into_owned()).or(Some(String::new()));
        (path.to_string(), parent)
    };
    Ok(json!({ "path": current, "parent": parent, "entries": entries }))
}


/// Windows pickers: drives first, then folders. Network shares can be typed in.
#[cfg(windows)]
fn browse(path: &str, files: bool) -> Result<Value, String> {
    let path = if path.trim().is_empty() { String::new() } else { trim_folder(path) };
    let mut entries: Vec<Value> = Vec::new();
    if path.is_empty() {
        for letter in b'A'..=b'Z' {
            let d = format!("{}:\\", letter as char);
            if Path::new(&d).exists() {
                entries.push(json!({ "name": d, "path": d, "dir": true, "readable": std::fs::read_dir(&d).is_ok(), "volume": "Drive" }));
            }
        }
        return Ok(json!({ "path": "", "parent": null, "entries": entries }));
    }
    let p = Path::new(&path);
    if !inside_volume(p) {
        return Err(PICK_HINT.into());
    }
    let rd = std::fs::read_dir(p).map_err(|_| format!("Elixir can't open {path}"))?;
    let mut all: Vec<_> = rd.flatten().collect();
    all.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
    for e in all {
        let name = e.file_name().to_string_lossy().into_owned();
        let ep = e.path();
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() && !config::is_system_folder(&name) && !["$recycle.bin", "system volume information"].contains(&name.to_lowercase().as_str()) {
            let readable = std::fs::read_dir(&ep).is_ok();
            entries.push(json!({ "name": name, "path": ep.to_string_lossy(), "dir": true, "readable": readable }));
        } else if files && t.is_file() && config::is_video(&ep) {
            entries.push(json!({ "name": name, "path": ep.to_string_lossy(), "dir": false, "size": e.metadata().map(|m| m.len()).unwrap_or(0) }));
        }
    }
    let parent = p.parent().map(|pp| pp.to_string_lossy().into_owned()).filter(|pp| !pp.is_empty() && pp != &path).unwrap_or_default();
    Ok(json!({ "path": path, "parent": parent, "entries": entries }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_strings_decode() {
        let q = query("/x?r=browse&path=%2Fvolume1%2Fmy%20video&empty=");
        assert_eq!(q["r"], "browse");
        assert_eq!(q["path"], "/volume1/my video");
        assert_eq!(q["empty"], "");
    }
}
