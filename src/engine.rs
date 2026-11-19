//! Elixir's engine: items to rename, matching, planning names, applying
//! with a journal, undo, and the optional automation over watched folders.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::{self, var_dir, Config};
use crate::fsops::{self, Ident, Made};
use crate::meta::{self, Episode, Movie, Provider, Show};
use crate::naming::{self, EpisodeNaming, Values};
use crate::parse::{self, normalize, Kind, Parsed};
use crate::suite::{self, Arr, Torrent};
use crate::{info, warn};

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---------- items ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Auto,
    Tmdb,
    Tvdb,
    Anidb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    #[default]
    Manual,
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Matching,
    /// Elixir can name it; waiting for "Rename" (or done automatically).
    Ready,
    /// Needs the user's pick.
    Pick,
    /// A show Midgar tracks: hand it to Midgar.
    Midgar,
    /// A movie Turks tracks: Turks' Manual Import.
    Turks,
    /// Midgar or Turks downloaded it: theirs.
    Owned,
    /// Practice mode: what would have happened.
    Practice,
    Done,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Cand {
    /// "show" or "movie".
    pub kind: String,
    pub provider: Option<Provider>,
    pub id: u64,
    pub title: String,
    pub year: Option<u16>,
    pub tvdb_id: Option<u64>,
    pub tmdb_id: Option<u64>,
    pub genres: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    pub batch: u64,
    pub origin: Origin,
    pub path: String,
    pub size: u64,
    pub ident: Ident,
    pub parsed: Parsed,
    pub source: Source,
    pub state: State,
    pub message: String,
    pub checks: Vec<Check>,
    pub candidates: Vec<Cand>,
    pub chosen: Option<Cand>,
    /// The user picked the match (or confirmed it once before).
    pub pinned: bool,
    pub episode_label: String,
    pub target: String,
    pub sidecars: Vec<(String, String)>,
    /// Lifestream torrent hash when the file belongs to a torrent.
    pub torrent: String,
    /// Whether Lifestream could be asked about this file at all.
    pub torrent_known: bool,
    /// Midgar series id or Turks movie id when tracked.
    pub tracked_id: i64,
    pub tracked_title: String,
    pub seasons_eps: Vec<(u32, u32)>,
    /// What a practice entry would have done (Ready or Midgar).
    #[serde(default)]
    pub plan: Option<State>,
    pub created: u64,
    pub updated: u64,
}

impl Item {
    fn all_checks_pass(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.ok)
    }
    pub fn remember_key(&self) -> String {
        format!("{}|{}|{}", normalize(&self.parsed.title), self.parsed.year.map(|y| y.to_string()).unwrap_or_default(), if is_movie(&self.parsed) { "m" } else { "s" })
    }
}

/// Same title and year (for "use this for the whole batch").
pub fn normalize_key(i: &Item) -> String {
    i.remember_key()
}

fn is_movie(p: &Parsed) -> bool {
    matches!(p.kind, Kind::Movie | Kind::Unknown)
}

// ---------- journal ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpKind {
    Mkdir,
    Link,
    Clone,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpState {
    Planned,
    Done,
    Failed,
    Undone,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Op {
    pub kind: OpKind,
    pub src: String,
    pub dst: String,
    pub state: OpState,
    /// The new name's identity right after the step, so undo can tell if it changed since.
    #[serde(default)]
    pub after: Option<Ident>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct RunItem {
    pub label: String,
    pub from: String,
    pub to: String,
    pub ops: Vec<Op>,
    pub ok: bool,
    pub note: String,
    /// "midgar"/"turks" when the app was told to rescan or imported it.
    pub app: String,
    pub app_id: i64,
    pub handoff: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Run {
    pub id: u64,
    pub time: u64,
    pub automatic: bool,
    pub items: Vec<RunItem>,
    pub undone: bool,
    pub undo_note: String,
    pub interrupted: bool,
}

fn runs_dir() -> PathBuf {
    var_dir().join("runs")
}

fn save_run(run: &Run) {
    let dir = runs_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(bytes) = serde_json::to_vec(run) {
        if let Err(e) = config::write_atomic(&dir.join(format!("{}.json", run.id)), &bytes) {
            warn!("Couldn't write the journal: {e}");
        }
    }
}

pub fn load_runs() -> Vec<Run> {
    let mut runs: Vec<Run> = std::fs::read_dir(runs_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice(&b).ok())
        .collect();
    runs.sort_by(|a, b| b.time.cmp(&a.time).then(b.id.cmp(&a.id)));
    runs
}

/// Finish or roll back steps a crash or restart left half done.
pub fn recover_journal() {
    for mut run in load_runs() {
        let mut touched = false;
        for item in &mut run.items {
            for op in &mut item.ops {
                if op.state != OpState::Planned {
                    continue;
                }
                touched = true;
                let (src, dst) = (Path::new(&op.src), Path::new(&op.dst));
                op.state = match op.kind {
                    OpKind::Mkdir => {
                        if dst.is_dir() { OpState::Done } else { OpState::Failed }
                    }
                    OpKind::Link | OpKind::Clone if dst.exists() && fsops::ident(src) == fsops::ident(dst) => {
                        op.kind = OpKind::Link;
                        OpState::Done
                    }
                    OpKind::Link | OpKind::Clone => {
                        let (s, d) = (std::fs::metadata(src).map(|m| m.len()).ok(), std::fs::metadata(dst).map(|m| m.len()).ok());
                        if d.is_some() && d == s {
                            op.kind = OpKind::Clone;
                            OpState::Done
                        } else {
                            if d == Some(0) {
                                let _ = std::fs::remove_file(dst);
                            }
                            OpState::Failed
                        }
                    }
                    // Windows moves in one step, so there's no half-done state; a name
                    // that only changed letter case is one file and must never be deleted.
                    OpKind::Move if cfg!(windows) => {
                        if dst.exists() { OpState::Done } else { OpState::Failed }
                    }
                    OpKind::Move => {
                        if dst.exists() && src.exists() && fsops::ident(src) == fsops::ident(dst) {
                            // Linked but the old name wasn't dropped yet: finish it.
                            if std::fs::remove_file(src).is_ok() { OpState::Done } else { OpState::Failed }
                        } else if dst.exists() && !src.exists() {
                            OpState::Done
                        } else {
                            OpState::Failed
                        }
                    }
                };
                if op.state == OpState::Done {
                    op.after = fsops::ident(dst);
                }
            }
            if item.ops.iter().any(|o| o.state == OpState::Failed) {
                item.ok = false;
                item.note = "Interrupted (NAS restart or update); the half-done step was undone".into();
            }
        }
        if touched {
            run.interrupted = true;
            info!("Journal: finished run {} after an interruption", run.id);
            save_run(&run);
        }
    }
}

/// Paths Elixir created (so automation never treats its own work as new).
fn our_outputs(runs: &[Run]) -> HashSet<String> {
    runs.iter().filter(|r| !r.undone).flat_map(|r| r.items.iter()).flat_map(|i| i.ops.iter()).filter(|o| o.state == OpState::Done && o.kind != OpKind::Mkdir).map(|o| o.dst.clone()).collect()
}

// ---------- inventory of watched folders ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Seen {
    /// Was there when the folder was added: left alone unless asked.
    Old,
    /// Arrived later: waiting until it's finished.
    New,
    /// Handed to the matcher.
    Taken,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub seen: Seen,
    pub first: u64,
    pub changed: u64,
    #[serde(default)]
    pub torrent_dir: String,
    #[serde(default)]
    pub stable_polls: u32,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Inventory {
    /// folder -> "dev:ino" -> entry
    pub folders: HashMap<String, HashMap<String, Entry>>,
}

fn key_of(id: Ident) -> String {
    format!("{}:{}", id.dev, id.ino)
}

// ---------- shared state ----------

#[derive(Default, Clone, Serialize)]
pub struct FolderInfo {
    pub path: String,
    pub state: String,
    pub message: String,
    pub old: usize,
    pub waiting: usize,
}

#[derive(Default, Clone, Serialize)]
pub struct AutoInfo {
    pub state: String,
    pub message: String,
    pub last_pass: u64,
    pub folders: Vec<FolderInfo>,
}

pub struct Shared {
    pub config: RwLock<Config>,
    pub items: Mutex<Vec<Item>>,
    pub remembered: Mutex<HashMap<String, Cand>>,
    pub inventory: Mutex<Inventory>,
    pub auto: Mutex<AutoInfo>,
    next_id: AtomicU64,
    tx: Mutex<Sender<u64>>,
    /// Midgar's naming, read when Midgar is connected.
    pub naming: Mutex<EpisodeNaming>,
    pub kick: Mutex<Option<Sender<()>>>,
}

impl Shared {
    pub fn cfg(&self) -> Config {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    pub fn update_config(&self, mut c: Config) -> Result<(), String> {
        c.normalize();
        c.save(&var_dir().join("config.json")).map_err(|e| e.to_string())?;
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = c;
        self.wake();
        Ok(())
    }

    pub fn wake(&self) {
        if let Some(tx) = self.kick.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = tx.send(());
        }
    }

    pub fn save_items(&self) {
        let items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(b) = serde_json::to_vec(&*items) {
            let _ = config::write_atomic(&var_dir().join("items.json"), &b);
        }
    }

    fn save_remembered(&self) {
        let r = self.remembered.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(b) = serde_json::to_vec(&*r) {
            let _ = config::write_atomic(&var_dir().join("remembered.json"), &b);
        }
    }

    pub fn save_inventory(&self) {
        let inv = self.inventory.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(b) = serde_json::to_vec(&*inv) {
            let _ = config::write_atomic(&var_dir().join("inventory.json"), &b);
        }
    }

    pub fn queue_match(&self, id: u64) {
        let _ = self.tx.lock().unwrap_or_else(|e| e.into_inner()).send(id);
    }

    fn with_item<R>(&self, id: u64, f: impl FnOnce(&mut Item) -> R) -> Option<R> {
        let mut items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        items.iter_mut().find(|i| i.id == id).map(f)
    }

    pub fn item(&self, id: u64) -> Option<Item> {
        self.items.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|i| i.id == id).cloned()
    }
}

pub fn start() -> Arc<Shared> {
    let var = var_dir();
    let _ = std::fs::create_dir_all(&var);
    let mut cfg = Config::load(&var.join("config.json"));
    cfg.normalize();
    let _ = cfg.save(&var.join("config.json"));
    recover_journal();
    let mut items: Vec<Item> = std::fs::read(var.join("items.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    // Anything mid-match when Elixir stopped is matched again.
    let rematch: Vec<u64> = items.iter().filter(|i| i.state == State::Matching).map(|i| i.id).collect();
    items.retain(|i| !(matches!(i.state, State::Done | State::Skipped | State::Owned) && now().saturating_sub(i.updated) > 7 * 86_400));
    let max_id = items.iter().map(|i| i.id.max(i.batch)).max().unwrap_or(0);
    let remembered = std::fs::read(var.join("remembered.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let inventory = std::fs::read(var.join("inventory.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared {
        config: RwLock::new(cfg),
        items: Mutex::new(items),
        remembered: Mutex::new(remembered),
        inventory: Mutex::new(inventory),
        auto: Mutex::new(AutoInfo { state: "off".into(), ..Default::default() }),
        next_id: AtomicU64::new(max_id.max(now()) + 1),
        tx: Mutex::new(tx),
        naming: Mutex::new(EpisodeNaming::default()),
        kick: Mutex::new(None),
    });
    let s = shared.clone();
    std::thread::Builder::new().name("matcher".into()).spawn(move || matcher(s, rx)).ok();
    for id in rematch {
        shared.queue_match(id);
    }
    let (ktx, krx) = mpsc::channel();
    *shared.kick.lock().unwrap_or_else(|e| e.into_inner()) = Some(ktx);
    let s = shared.clone();
    std::thread::Builder::new().name("automation".into()).spawn(move || automation(s, krx)).ok();
    shared
}

// ---------- scanning by hand ----------

/// Videos in a folder (or a single file), skipping system, extras and sample folders.
pub fn videos_in(path: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if path.is_file() {
        if config::is_video(path) {
            out.push(path.to_path_buf());
        }
        return out;
    }
    fn walk(dir: &Path, depth: usize, max: usize, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                if depth < max && !config::is_skip_folder(&name) {
                    walk(&p, depth + 1, max, out);
                }
            } else if t.is_file() && config::is_video(&p) && !config::is_partial(&name) && !parse::parse_name(&name).sample {
                out.push(p);
            }
        }
    }
    walk(path, 0, max_depth, &mut out);
    out
}

pub fn new_item(shared: &Shared, path: &Path, batch: u64, origin: Origin, source: Source) -> Item {
    let meta = std::fs::metadata(path).ok();
    Item {
        id: shared.next_id(),
        batch,
        origin,
        path: path.to_string_lossy().into_owned(),
        size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        ident: fsops::ident(path).unwrap_or_default(),
        parsed: parse::parse_path(path),
        source,
        state: State::Matching,
        created: now(),
        updated: now(),
        ..Default::default()
    }
}

/// "Rename" page: scan a folder or file into a new batch.
pub fn scan(shared: &Arc<Shared>, path: &Path, source: Source, origin: Origin) -> Result<u64, String> {
    let files = videos_in(path, 4);
    if files.is_empty() {
        return Err("No videos found there".into());
    }
    if files.len() > 500 {
        return Err(format!("That's {} videos. Pick a smaller folder (500 at most)", files.len()));
    }
    let batch = shared.next_id();
    let new: Vec<Item> = files.iter().map(|f| new_item(shared, f, batch, origin, source)).collect();
    let ids: Vec<u64> = new.iter().map(|i| i.id).collect();
    {
        let mut items = shared.items.lock().unwrap_or_else(|e| e.into_inner());
        // Earlier scans of the same files that haven't been done are replaced.
        let paths: HashSet<&str> = new.iter().map(|i| i.path.as_str()).collect();
        items.retain(|i| !(i.origin == Origin::Manual && paths.contains(i.path.as_str()) && !matches!(i.state, State::Done)));
        items.extend(new);
    }
    for id in ids {
        shared.queue_match(id);
    }
    shared.save_items();
    Ok(batch)
}

// ---------- matching ----------

fn matcher(shared: Arc<Shared>, rx: Receiver<u64>) {
    // What Lifestream/Midgar/Turks said is reused for 20 seconds (a batch of
    // 50 files would otherwise ask them 50 times).
    let mut ctx_cache: Option<(Instant, Context)> = None;
    while let Ok(id) = rx.recv() {
        let Some(item) = shared.item(id) else { continue };
        if item.state != State::Matching {
            continue;
        }
        let cfg = shared.cfg();
        if ctx_cache.as_ref().is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(20)) {
            ctx_cache = Some((Instant::now(), Context::load(&shared, &cfg)));
        }
        let ctx = &ctx_cache.as_ref().expect("set above").1;
        let mut done = item.clone();
        evaluate(&shared, &cfg, ctx, &mut done);
        done.updated = now();
        let auto = done.origin == Origin::Auto;
        shared.with_item(id, |i| {
            // The user may have changed the source or pick meanwhile.
            if i.state == State::Matching && i.source == item.source && i.chosen == item.chosen {
                *i = done.clone();
            }
        });
        shared.save_items();
        if auto {
            after_auto_match(&shared, id);
        }
    }
}

/// What Elixir knows about the other apps for one round of matching.
pub struct Context {
    pub torrents: Option<Vec<Torrent>>,
    pub owned: HashSet<String>,
    pub owned_error: String,
    pub midgar: Option<Vec<suite::Tracked>>,
    pub turks: Option<Vec<suite::Tracked>>,
    pub tv_root: String,
    pub anime_root: String,
    pub movie_root: String,
}

impl Context {
    pub fn load(shared: &Shared, cfg: &Config) -> Context {
        let torrents = if suite::lifestream_configured(cfg) { suite::lifestream_torrents(cfg).ok() } else { None };
        let mut owned = HashSet::new();
        let mut owned_error = String::new();
        for arr in [Arr::Midgar, Arr::Turks] {
            if arr.configured(cfg) {
                match suite::arr_hashes(cfg, arr) {
                    Ok(h) => owned.extend(h),
                    Err(e) => owned_error = e,
                }
            }
        }
        let midgar = Arr::Midgar.configured(cfg).then(|| suite::arr_library(cfg, Arr::Midgar).ok()).flatten();
        let turks = Arr::Turks.configured(cfg).then(|| suite::arr_library(cfg, Arr::Turks).ok()).flatten();
        // Show naming: Midgar's own (when it's the choice and connected), else the chosen formats.
        if cfg.naming_preset == "midgar" {
            if Arr::Midgar.configured(cfg) {
                if let Ok(n) = suite::midgar_naming(cfg) {
                    *shared.naming.lock().unwrap_or_else(|e| e.into_inner()) = n;
                }
            } else {
                *shared.naming.lock().unwrap_or_else(|e| e.into_inner()) = EpisodeNaming::default();
            }
        } else {
            *shared.naming.lock().unwrap_or_else(|e| e.into_inner()) = cfg.episode_naming.clone();
        }
        let first_root = |arr: Arr| if arr.configured(cfg) { suite::arr_roots(cfg, arr).ok().and_then(|r| r.into_iter().next()).unwrap_or_default() } else { String::new() };
        let tv_root = if cfg.tv_folder.is_empty() { first_root(Arr::Midgar) } else { cfg.tv_folder.clone() };
        let anime_root = if cfg.anime_folder.is_empty() { tv_root.clone() } else { cfg.anime_folder.clone() };
        let movie_root = if cfg.movies_folder.is_empty() { first_root(Arr::Turks) } else { cfg.movies_folder.clone() };
        Context { torrents, owned, owned_error, midgar, turks, tv_root, anime_root, movie_root }
    }

    fn torrent_of(&self, path: &Path) -> Option<&Torrent> {
        self.torrents.as_ref()?.iter().find(|t| t.files.iter().any(|f| fsops::same_path(&f.path, path)))
    }
}

fn check(name: &str, ok: bool, text: impl Into<String>) -> Check {
    Check { name: name.into(), ok, text: text.into() }
}

fn title_matches(parsed: &str, title: &str, aliases: &[String]) -> bool {
    let want = normalize(parsed);
    if want.is_empty() {
        return false;
    }
    let strip_year = |t: &str| {
        let n = normalize(t);
        let parts: Vec<&str> = n.split(' ').collect();
        if parts.len() > 1 && parts.last().is_some_and(|w| w.len() == 4 && w.chars().all(|c| c.is_ascii_digit())) {
            parts[..parts.len() - 1].join(" ")
        } else {
            n
        }
    };
    std::iter::once(title).chain(aliases.iter().map(String::as_str)).any(|t| normalize(t) == want || strip_year(t) == want)
}

fn show_cand(s: &Show) -> Cand {
    Cand { kind: "show".into(), provider: Some(s.provider), id: s.id, title: s.title.clone(), year: s.year, tvdb_id: s.tvdb_id, tmdb_id: s.tmdb_id, genres: vec![] }
}

fn movie_cand(m: &Movie) -> Cand {
    Cand { kind: "movie".into(), provider: Some(Provider::Tmdb), id: m.tmdb_id, title: m.title.clone(), year: m.year, tvdb_id: None, tmdb_id: Some(m.tmdb_id), genres: m.genres.clone() }
}

/// Match one item: candidates, checks, target name, and who it belongs to.
pub fn evaluate(shared: &Shared, cfg: &Config, ctx: &Context, item: &mut Item) {
    item.plan = None;
    item.checks.clear();
    item.message.clear();
    item.target.clear();
    item.sidecars.clear();
    item.tracked_id = 0;
    item.tracked_title.clear();
    item.seasons_eps.clear();
    let path = PathBuf::from(&item.path);
    if !path.is_file() {
        item.state = State::Failed;
        item.message = "The file isn't there any more".into();
        return;
    }
    // Lifestream first: a torrent Midgar or Turks grabbed is theirs.
    item.torrent_known = ctx.torrents.is_some();
    if let Some(t) = ctx.torrent_of(&path) {
        item.torrent = t.hash.clone();
        if ctx.owned.contains(&t.hash) {
            item.state = State::Owned;
            item.message = "Midgar or Turks downloaded this, so they handle it".into();
            return;
        }
    } else {
        item.torrent.clear();
    }
    if !ctx.owned_error.is_empty() && item.origin == Origin::Auto {
        item.state = State::Pick;
        item.message = format!("Couldn't ask who owns it: {}", ctx.owned_error);
        return;
    }
    let p = item.parsed.clone();
    if p.sample {
        item.state = State::Skipped;
        item.message = "Sample clip, left out".into();
        return;
    }
    if p.title.is_empty() {
        item.state = State::Pick;
        item.message = "Couldn't read a title from the name. Search for it".into();
        return;
    }
    if p.kind == Kind::SeasonPack {
        item.state = State::Pick;
        item.message = "The name has a season but no episode number".into();
        return;
    }
    let remembered = shared.remembered.lock().unwrap_or_else(|e| e.into_inner()).get(&item.remember_key()).cloned();
    let pinned = item.chosen.clone().filter(|_| item.pinned).or_else(|| {
        let r = remembered.clone();
        if r.is_some() {
            item.pinned = true;
        }
        r
    });
    let result = if is_movie(&p) && item.source != Source::Anidb { match_movie(cfg, item, &p, pinned) } else { match_show(cfg, item, &p, pinned) };
    if let Err(e) = result {
        item.state = State::Pick;
        item.message = e;
        return;
    }
    let ok = item.all_checks_pass();
    plan_target(shared, cfg, ctx, item);
    if item.state == State::Failed || item.state == State::Pick && !item.message.is_empty() {
        return;
    }
    if !ok {
        item.state = State::Pick;
        if item.message.is_empty() {
            item.message = "Not sure enough to rename by itself. Pick the right match".into();
        }
    }
}

fn match_movie(cfg: &Config, item: &mut Item, p: &Parsed, pinned: Option<Cand>) -> Result<(), String> {
    let movies = meta::tmdb_search_movie(cfg, &p.title, p.year)?;
    item.candidates = movies.iter().map(movie_cand).collect();
    let chosen: Movie = match pinned.filter(|c| c.kind == "movie") {
        Some(c) => meta::tmdb_movie(cfg, c.id)?,
        None => {
            let best = movies.iter().find(|m| title_matches(&p.title, &m.title, &m.aliases) && (p.year.is_none() || m.year == p.year)).or(movies.iter().find(|m| title_matches(&p.title, &m.title, &m.aliases))).or(movies.first());
            best.cloned().ok_or_else(|| format!("TheMovieDB found nothing for \"{}\"", p.title))?
        }
    };
    let same: Vec<&Movie> = movies.iter().filter(|m| title_matches(&p.title, &m.title, &m.aliases) && (p.year.is_none() || m.year == p.year)).collect();
    item.checks = vec![
        check("title", item.pinned || title_matches(&p.title, &chosen.title, &chosen.aliases), if item.pinned { "You picked it".to_string() } else { format!("\"{}\"", chosen.title) }),
        match p.year {
            None => check("year", true, "No year in the name"),
            Some(y) => check("year", item.pinned || chosen.year == Some(y), format!("{} in the name, {} on TheMovieDB", y, chosen.year.map(|y| y.to_string()).unwrap_or("?".into()))),
        },
        check("exists", true, "Found on TheMovieDB"),
        check("single", item.pinned || same.len() <= 1, if same.len() > 1 { format!("{} movies share this name", same.len()) } else { "Only one match".into() }),
    ];
    item.chosen = Some(movie_cand(&chosen));
    item.episode_label = chosen.year.map(|y| format!("{} ({y})", chosen.title)).unwrap_or(chosen.title.clone());
    Ok(())
}

fn show_provider(cfg: &Config, source: Source) -> Provider {
    match source {
        Source::Tmdb => Provider::Tmdb,
        Source::Tvdb => Provider::Tvdb,
        Source::Anidb => Provider::Anidb,
        Source::Auto => {
            if !cfg.tvdb_key.is_empty() {
                Provider::Tvdb
            } else {
                Provider::Tmdb
            }
        }
    }
}

fn search_shows(cfg: &Config, prov: Provider, title: &str, year: Option<u16>) -> Result<Vec<Show>, String> {
    match prov {
        Provider::Tmdb => meta::tmdb_search_tv(cfg, title, year),
        Provider::Tvdb => meta::tvdb_search(cfg, title, year),
        Provider::Anidb => meta::anidb_search(title),
    }
}

fn match_show(cfg: &Config, item: &mut Item, p: &Parsed, pinned: Option<Cand>) -> Result<(), String> {
    let mut prov = show_provider(cfg, item.source);
    // Anime numbered straight through needs TheTVDB's absolute numbers.
    if p.kind == Kind::Absolute && prov == Provider::Tmdb {
        if cfg.tvdb_key.is_empty() && cfg.anidb_client.is_empty() {
            return Err("Anime numbered like this needs TheTVDB (or AniDB) set up in Settings".into());
        }
        if !cfg.tvdb_key.is_empty() {
            prov = Provider::Tvdb;
        }
    }
    let shows = search_shows(cfg, prov, &p.title, p.year)?;
    item.candidates = shows.iter().map(show_cand).collect();
    let pinned = pinned.filter(|c| c.kind == "show" && c.provider == Some(prov));
    let chosen: Show = match &pinned {
        Some(c) => match prov {
            Provider::Tvdb => meta::tvdb_show(cfg, c.id)?,
            Provider::Tmdb => meta::tmdb_show(cfg, c.id)?,
            Provider::Anidb => shows.iter().find(|s| s.id == c.id).cloned().unwrap_or(Show { provider: Provider::Anidb, id: c.id, title: c.title.clone(), year: c.year, aliases: vec![], tvdb_id: None, tmdb_id: None }),
        },
        None => {
            let best = shows.iter().find(|s| title_matches(&p.title, &s.title, &s.aliases) && (p.year.is_none() || s.year == p.year)).or(shows.iter().find(|s| title_matches(&p.title, &s.title, &s.aliases))).or(shows.first());
            best.cloned().ok_or_else(|| format!("{} found nothing for \"{}\"", prov.label(), p.title))?
        }
    };
    item.pinned = pinned.is_some();
    let same: Vec<&Show> = shows.iter().filter(|s| title_matches(&p.title, &s.title, &s.aliases) && (p.year.is_none() || s.year == p.year)).collect();

    // Episodes, from the provider; AniDB anime is numbered like TheTVDB when it can be.
    let (mut show, mut eps): (Show, Vec<Episode>) = match prov {
        Provider::Tmdb => {
            let e = meta::tmdb_episodes(cfg, chosen.id)?;
            let mut s = chosen.clone();
            s.tvdb_id = s.tvdb_id.or_else(|| meta::tmdb_tvdb_id(cfg, chosen.id));
            (s, e)
        }
        Provider::Tvdb => (chosen.clone(), meta::tvdb_episodes(cfg, chosen.id)?),
        Provider::Anidb => {
            let (year, e) = meta::anidb_episodes(cfg, chosen.id)?;
            let mut s = chosen.clone();
            s.year = s.year.or(year);
            (s, e)
        }
    };
    if prov == Provider::Anidb {
        // Find the same show on TheTVDB so the numbers match Midgar's.
        if cfg.tvdb_key.is_empty() {
            return Err("Anime from AniDB is named with TheTVDB's season numbers, so Midgar can find it. Add a TheTVDB key in Settings".into());
        }
        let wanted_abs: Vec<u32> = p.absolute.clone();
        let names: Vec<String> = std::iter::once(show.title.clone()).chain(show.aliases.iter().take(4).cloned()).collect();
        let mut mapped = None;
        for n in &names {
            if let Ok(found) = meta::tvdb_search(cfg, n, None) {
                if let Some(t) = found.into_iter().find(|t| names.iter().any(|n2| title_matches(n2, &t.title, &t.aliases))) {
                    mapped = Some(t);
                    break;
                }
            }
        }
        let t = mapped.ok_or("AniDB found it, but Elixir couldn't find the same show on TheTVDB to number it like Midgar")?;
        eps = meta::tvdb_episodes(cfg, t.id)?;
        show = Show { title: t.title.clone(), tvdb_id: t.tvdb_id, year: t.year.or(show.year), ..show };
        let _ = wanted_abs;
    }

    // Which episodes the file is.
    let found: Vec<Episode> = match p.kind {
        Kind::Daily => eps.iter().filter(|e| e.air_date.as_deref() == p.air_date.as_deref()).cloned().collect(),
        Kind::Absolute => p.absolute.iter().filter_map(|n| eps.iter().find(|e| e.absolute == Some(*n)).cloned()).collect(),
        _ => p.episodes.iter().filter_map(|n| eps.iter().find(|e| Some(e.season) == p.season && e.number == *n).cloned()).collect(),
    };
    let wanted = match p.kind {
        Kind::Daily => 1,
        Kind::Absolute => p.absolute.len(),
        _ => p.episodes.len(),
    };
    let what = match p.kind {
        Kind::Daily => format!("the episode from {}", p.air_date.clone().unwrap_or_default()),
        Kind::Absolute => format!("episode {}", p.absolute.iter().map(|n| n.to_string()).collect::<Vec<_>>().join("-")),
        _ => format!("S{:02}E{}", p.season.unwrap_or(0), p.episodes.iter().map(|n| format!("{n:02}")).collect::<Vec<_>>().join("-E")),
    };
    item.checks = vec![
        check("title", item.pinned || title_matches(&p.title, &chosen.title, &chosen.aliases), if item.pinned { "You picked it".to_string() } else { format!("\"{}\"", chosen.title) }),
        match p.year {
            None => check("year", true, "No year in the name"),
            Some(y) => check("year", item.pinned || chosen.year == Some(y), format!("{} in the name, {} on {}", y, chosen.year.map(|y| y.to_string()).unwrap_or("?".into()), prov.label())),
        },
        check("exists", !found.is_empty() && found.len() >= wanted, if found.is_empty() { format!("{} isn't listed on {}", what, prov.label()) } else { format!("{what} is listed") }),
        check("single", item.pinned || same.len() <= 1, if same.len() > 1 { format!("{} shows share this name", same.len()) } else { "Only one match".into() }),
    ];
    item.chosen = Some(Cand { title: show.title.clone(), year: show.year, tvdb_id: show.tvdb_id, ..show_cand(&chosen) });
    item.seasons_eps = found.iter().map(|e| (e.season, e.number)).collect();
    item.episode_label = match found.first() {
        Some(e) if found.len() == 1 => format!("S{:02}E{:02} · {}", e.season, e.number, if e.title.is_empty() { "no title yet" } else { &e.title }),
        Some(e) => format!("S{:02}E{:02}-E{:02}", e.season, e.number, found.last().map(|l| l.number).unwrap_or(e.number)),
        None => String::new(),
    };
    // Keep the episode details for naming.
    item.message.clear();
    keep_episodes(item.id, (show, found));
    Ok(())
}

/// Episode details from the last match, per item (naming needs titles and dates).
type EpMap = HashMap<u64, (Show, Vec<Episode>)>;
static EPISODES: Mutex<Option<EpMap>> = Mutex::new(None);

fn keep_episodes(id: u64, v: (Show, Vec<Episode>)) {
    EPISODES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(id, v);
}

fn take_episodes(id: u64) -> Option<(Show, Vec<Episode>)> {
    EPISODES.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.remove(&id))
}

/// Folders inside `root` that look like genre folders (not "Title (Year)").
fn genre_folders(root: &Path) -> Vec<String> {
    let re = regex::Regex::new(r"\(\d{4}\)\s*$").expect("regex");
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !config::is_system_folder(n) && !re.is_match(n))
        .collect()
}

/// Same mapping Turks uses to suggest a genre folder.
pub fn suggest_genre_folder(genres: &[String], folders: &[String]) -> Option<String> {
    let alias = |g: &str| -> Vec<String> {
        let g = g.to_lowercase();
        let mut v = vec![g.clone()];
        match g.as_str() {
            "animation" => v.extend(["animated", "cartoon", "anime"].map(String::from)),
            "science fiction" => v.extend(["sci-fi", "scifi", "sci fi"].map(String::from)),
            "thriller" | "mystery" => v.extend(["suspense", "thriller", "mystery"].map(String::from)),
            "family" => v.extend(["kids", "children"].map(String::from)),
            "documentary" => v.push("documentaries".into()),
            "romance" => v.push("romantic".into()),
            "music" => v.push("musical".into()),
            _ => {}
        }
        v
    };
    for g in genres {
        for word in alias(g) {
            if let Some(f) = folders.iter().find(|f| {
                let f = f.to_lowercase();
                f == word || f.split(|c: char| !c.is_alphanumeric() && c != '-').any(|part| part == word) || (word.len() >= 5 && f.starts_with(&word[..5]))
            }) {
                return Some(f.clone());
            }
        }
    }
    None
}

/// Work out the new name, and who should do the renaming.
fn plan_target(shared: &Shared, cfg: &Config, ctx: &Context, item: &mut Item) {
    let path = PathBuf::from(&item.path);
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
    let Some(chosen) = item.chosen.clone() else { return };
    let naming = shared.naming.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let (dir, stem, ep_title): (PathBuf, String, String) = if chosen.kind == "movie" {
        let v = Values { movie_title: chosen.title.clone(), movie_year: chosen.year, ..Default::default() };
        let mn = &cfg.movie_naming;
        let stem = naming::clean_component(&naming::fill(&mn.file, &v, 5), mn.colon);
        let tracked = ctx.turks.as_ref().and_then(|l| l.iter().find(|m| m.tmdb_id.is_some() && m.tmdb_id == chosen.tmdb_id).cloned());
        if let Some(t) = tracked {
            item.tracked_id = t.id;
            item.tracked_title = t.title.clone();
            item.state = State::Turks;
            item.message = "Turks tracks this movie. Use Turks' Manual Import so Turks keeps track of it".into();
            item.target = Path::new(&t.path).join(format!("{stem}{ext}")).to_string_lossy().into_owned();
            return;
        }
        if ctx.movie_root.is_empty() {
            item.state = State::Pick;
            item.message = "No movies folder yet: set one in Settings (or connect Turks)".into();
            return;
        }
        let root = PathBuf::from(&ctx.movie_root);
        let genres = genre_folders(&root);
        let dir = if genres.len() >= 2 {
            match suggest_genre_folder(&chosen.genres, &genres) {
                Some(g) => root.join(g),
                None => {
                    item.state = State::Pick;
                    item.message = format!("No genre folder fits ({}). Rename it by hand", if chosen.genres.is_empty() { "no genres listed".into() } else { chosen.genres.join(", ") });
                    return;
                }
            }
        } else {
            root.join(naming::clean_component(&naming::fill(&mn.folder, &v, 5), mn.colon))
        };
        (dir, stem, String::new())
    } else {
        let Some((show, eps)) = take_episodes(item.id) else {
            // Re-planned without a fresh match (shouldn't happen): match again.
            item.state = State::Matching;
            return;
        };
        if eps.is_empty() {
            // The show matched but not the episode: no name to offer.
            item.target.clear();
            item.sidecars.clear();
            item.state = State::Pick;
            item.message = format!("{} has no such episode in the database. Pick the right match, or the database may not list it yet", show.title);
            return;
        }
        let p = &item.parsed;
        let v = Values {
            series_title: show.title.clone(),
            series_year: show.year,
            season: eps.first().map(|e| e.season),
            episodes: eps.iter().map(|e| e.number).collect(),
            absolute: eps.iter().filter_map(|e| e.absolute).collect(),
            episode_titles: eps.iter().map(|e| e.title.clone()).filter(|t| !t.is_empty()).collect(),
            air_date: eps.first().and_then(|e| e.air_date.clone()).or(p.air_date.clone()),
            ..Default::default()
        };
        let fmt = match p.kind {
            Kind::Daily => &naming.daily,
            Kind::Absolute => &naming.anime,
            _ => &naming.standard,
        };
        let stem = naming::clean_component(&naming::fill(fmt, &v, naming.multi_style), naming.colon);
        let ep_title = naming::clean_component(&v.episode_titles.join(" + "), naming.colon);
        let season = v.season.unwrap_or(1);
        let season_dir = if season == 0 { naming.specials_folder.clone() } else { naming.season_folder.clone() };
        let season_dir = naming::clean_component(&naming::fill(&season_dir, &v, 5), naming.colon);
        let tracked = ctx.midgar.as_ref().and_then(|l| {
            l.iter()
                .find(|s| (s.tvdb_id.is_some() && s.tvdb_id == show.tvdb_id) || (s.tmdb_id.is_some() && s.tmdb_id == show.tmdb_id))
                .or_else(|| l.iter().find(|s| normalize(&s.title) == normalize(&show.title) && (s.year.is_none() || show.year.is_none() || s.year == show.year)))
                .cloned()
        });
        if let Some(t) = tracked {
            item.tracked_id = t.id;
            item.tracked_title = t.title.clone();
            item.state = State::Midgar;
            item.message = "Midgar tracks this show, so Midgar imports it and names it its way".into();
            item.target = Path::new(&t.path).join(&season_dir).join(format!("{stem}{ext}")).to_string_lossy().into_owned();
            return;
        }
        let root = if p.kind == Kind::Absolute || p.anime { &ctx.anime_root } else { &ctx.tv_root };
        if root.is_empty() {
            item.state = State::Pick;
            item.message = "No TV folder yet: set one in Settings (or connect Midgar)".into();
            return;
        }
        let series_dir = naming::clean_component(&naming::fill(&naming.series_folder, &v, 5), naming.colon);
        (PathBuf::from(root).join(series_dir).join(season_dir), stem, ep_title)
    };
    let limit = fsops::name_limit(dir.ancestors().find(|a| a.exists()).unwrap_or(Path::new("/"))).min(255);
    // Leave room for the longest sidecar ending (".en.forced.srt").
    let stem = naming::fit(&stem, &ep_title, limit.saturating_sub(24));
    let target = dir.join(format!("{stem}{ext}"));
    item.target = target.to_string_lossy().into_owned();
    item.sidecars = sidecars_of(&path, &dir, &stem);
    if fsops::same_path(Path::new(&item.target), &path) && Path::new(&item.target).file_name() == path.file_name() {
        item.state = State::Done;
        item.message = "Already named like this".into();
        return;
    }
    item.state = State::Ready;
}

/// Subtitles, .nfo and artwork that go with a video: same name start, same folder
/// (or a Subs folder next to it).
fn sidecars_of(video: &Path, new_dir: &Path, new_stem: &str) -> Vec<(String, String)> {
    let Some(dir) = video.parent() else { return vec![] };
    let stem = video.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out = Vec::new();
    let mut look = |folder: &Path, any_name: bool| {
        for e in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let p = e.path();
            if !p.is_file() || p == video {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !config::SIDECAR_EXTENSIONS.contains(&ext.as_str()) {
                continue;
            }
            let suffix = if let Some(rest) = name.strip_prefix(&stem) {
                if !rest.starts_with('.') && !rest.starts_with('-') {
                    continue;
                }
                rest.to_string()
            } else if any_name && ["srt", "ass", "ssa", "sub", "idx", "sup", "vtt"].contains(&ext.as_str()) {
                // Subs/2_English.srt -> .2_English.srt
                format!(".{}", name)
            } else {
                continue;
            };
            out.push((p.to_string_lossy().into_owned(), new_dir.join(format!("{new_stem}{suffix}")).to_string_lossy().into_owned()));
        }
    };
    look(dir, false);
    // A Subs folder only when it clearly belongs to this one video.
    let videos_here = std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| config::is_video(&e.path())).count();
    if videos_here == 1 {
        for sub in ["Subs", "subs", "Subtitles", "subtitles"] {
            let d = dir.join(sub);
            if d.is_dir() {
                look(&d, true);
            }
        }
    }
    out
}

// ---------- applying ----------

/// Held while files are being renamed or put back; an update waits for it.
pub static APPLY_LOCK: Mutex<()> = Mutex::new(());

fn op(kind: OpKind, src: &str, dst: &str) -> Op {
    Op { kind, src: src.into(), dst: dst.into(), state: OpState::Planned, after: None }
}

/// Run one step, saving the journal before and after.
fn do_op(run: &mut Run, ri: usize, oi: usize) -> Result<(), String> {
    save_run(run);
    let o = run.items[ri].ops[oi].clone();
    let (src, dst) = (Path::new(&o.src), Path::new(&o.dst));
    let r: Result<(), String> = match o.kind {
        OpKind::Mkdir => std::fs::create_dir(dst).or_else(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { Ok(()) } else { Err(fsops::plain(&e)) }).map(|_| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o775));
            }
        }),
        OpKind::Link | OpKind::Clone => fsops::link_or_clone(src, dst).map(|made| {
            run.items[ri].ops[oi].kind = if made == Made::Clone { OpKind::Clone } else { OpKind::Link };
        }),
        OpKind::Move => fsops::move_file(src, dst),
    };
    let op = &mut run.items[ri].ops[oi];
    match &r {
        Ok(()) => {
            op.state = OpState::Done;
            op.after = fsops::ident(dst);
        }
        Err(_) => op.state = OpState::Failed,
    }
    save_run(run);
    r
}

/// Rename (or hand to Midgar) the given items. Returns the run id.
pub fn apply(shared: &Arc<Shared>, ids: &[u64], automatic: bool) -> Result<u64, String> {
    let _guard = APPLY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = shared.cfg();
    let items: Vec<Item> = {
        let all = shared.items.lock().unwrap_or_else(|e| e.into_inner());
        ids.iter()
            .filter_map(|id| all.iter().find(|i| i.id == *id).cloned())
            // A Queue item that only waited for a yes (new show, or automation was off) can go by hand.
            .filter(|i| matches!(i.state, State::Ready | State::Midgar | State::Practice) || (!automatic && i.state == State::Pick && i.plan.is_some()))
            .map(|mut i| {
                if matches!(i.state, State::Practice | State::Pick) {
                    i.state = i.plan.unwrap_or(State::Ready);
                }
                i
            })
            .collect()
    };
    if items.is_empty() {
        return Err("Nothing ready to rename".into());
    }
    // Check again right before acting: torrents and their owners can change.
    let torrents = if suite::lifestream_configured(&cfg) { suite::lifestream_torrents(&cfg).ok() } else { None };
    let mut owned = HashSet::new();
    for arr in [Arr::Midgar, Arr::Turks] {
        if arr.configured(&cfg) {
            match suite::arr_hashes(&cfg, arr) {
                Ok(h) => owned.extend(h),
                Err(e) if automatic => return Err(format!("Couldn't ask {}: {e}", arr.label())),
                Err(_) => {}
            }
        }
    }
    let mut run = Run { id: shared.next_id(), time: now(), automatic, ..Default::default() };
    let mut made_dirs: HashSet<PathBuf> = HashSet::new();
    let mut targets: HashSet<String> = HashSet::new();
    for item in &items {
        let path = PathBuf::from(&item.path);
        let label = if item.episode_label.is_empty() { fsops::show(&path) } else { format!("{} · {}", item.chosen.as_ref().map(|c| c.title.clone()).unwrap_or_default(), item.episode_label) };
        let mut ri = RunItem { label, from: item.path.clone(), to: item.target.clone(), ..Default::default() };
        let fail = |run: &mut Run, mut ri: RunItem, msg: String| {
            ri.ok = false;
            ri.note = msg;
            run.items.push(ri);
        };
        // The file must be the one that was matched.
        if fsops::ident(&path) != Some(item.ident) || std::fs::metadata(&path).map(|m| m.len()).ok() != Some(item.size) {
            fail(&mut run, ri, "The file changed since it was matched. Scan it again".into());
            continue;
        }
        let torrent = torrents.as_ref().and_then(|ts| ts.iter().find(|t| t.files.iter().any(|f| fsops::same_path(&f.path, &path))));
        if let Some(t) = torrent {
            if owned.contains(&t.hash) {
                fail(&mut run, ri, "Midgar or Turks downloaded this, so they handle it".into());
                continue;
            }
            let f = t.files.iter().find(|f| fsops::same_path(&f.path, &path));
            if !t.finished() || f.is_some_and(|f| f.done < f.length) {
                fail(&mut run, ri, "Lifestream hasn't finished this download yet".into());
                continue;
            }
        }
        // Never move a file that is, or might be, seeding: one Lifestream lists, any
        // file in a folder Lifestream downloads into, or anything at all when
        // Lifestream can't be asked.
        let in_download_dir = torrents.as_ref().is_some_and(|ts| ts.iter().any(|t| !t.dir.is_empty() && fsops::within(&path, Path::new(&t.dir))));
        // Lifestream set up but not answering: play safe, every file might be seeding.
        // Not set up at all: nothing here seeds through Lifestream, so files move
        // (and automation only runs in folders marked "Nothing seeds from this folder").
        let lifestream_down = suite::lifestream_configured(&cfg) && torrents.is_none();
        let keep_original = torrent.is_some() || in_download_dir || (lifestream_down && !cfg.no_seed_for(&path));
        if item.state == State::Midgar {
            ri.handoff = true;
            ri.app = "midgar".into();
            ri.app_id = item.tracked_id;
            let ep_ids: Vec<i64> = match suite::arr_get(&cfg, Arr::Midgar, &format!("episode?seriesId={}", item.tracked_id)) {
                Ok(v) => item.seasons_eps.iter().filter_map(|(s, n)| v.as_array().into_iter().flatten().find(|e| e["seasonNumber"].as_u64() == Some(*s as u64) && e["episodeNumber"].as_u64() == Some(*n as u64)).and_then(|e| e["id"].as_i64())).collect(),
                Err(_) => vec![],
            };
            match suite::midgar_import(&cfg, &path, item.tracked_id, &ep_ids, keep_original) {
                Ok(msg) => {
                    ri.ok = true;
                    ri.note = msg;
                }
                Err(e) => {
                    ri.ok = false;
                    ri.note = e;
                }
            }
            run.items.push(ri);
            save_run(&run);
            continue;
        }
        if !targets.insert(item.target.clone()) {
            fail(&mut run, ri, "Another file in this batch wants the same name".into());
            continue;
        }
        let target = PathBuf::from(&item.target);
        // On Windows a name that only differs in letter case is the same file.
        let case_only = cfg!(windows) && fsops::same_path(&target, &path) && fsops::ident(&target) == fsops::ident(&path);
        if target.exists() && !case_only {
            fail(&mut run, ri, format!("{} already exists. Elixir never replaces files", fsops::show(&target)));
            continue;
        }
        if case_only && keep_original {
            fail(&mut run, ri, "Only the letter case differs, and a seeding file can't get a second name like that. Rename it after it stops seeding".into());
            continue;
        }
        let dir = target.parent().unwrap_or(Path::new("/")).to_path_buf();
        // Plan the steps: folders, the video, then its sidecars.
        let mut missing = Vec::new();
        for d in dir.ancestors() {
            if d.exists() || made_dirs.contains(d) {
                break;
            }
            missing.push(d.to_path_buf());
        }
        for d in missing.iter().rev() {
            ri.ops.push(op(OpKind::Mkdir, "", &d.to_string_lossy()));
            made_dirs.insert(d.clone());
        }
        let kind = if keep_original { OpKind::Link } else { OpKind::Move };
        if kind == OpKind::Move && !fsops::same_volume(&path, dir.ancestors().find(|a| a.exists()).unwrap_or(Path::new("/"))) {
            fail(&mut run, ri, "It would have to move to another volume, which Elixir doesn't do".into());
            continue;
        }
        ri.ops.push(op(kind, &item.path, &item.target));
        for (s, d) in &item.sidecars {
            if Path::new(d).exists() || !targets.insert(d.clone()) {
                continue;
            }
            ri.ops.push(op(kind, s, d));
        }
        run.items.push(ri);
        let idx = run.items.len() - 1;
        let mut ok = true;
        let mut note = String::new();
        for oi in 0..run.items[idx].ops.len() {
            let is_side = oi > 0 && run.items[idx].ops[oi].src != item.path && run.items[idx].ops[oi].kind != OpKind::Mkdir;
            if let Err(e) = do_op(&mut run, idx, oi) {
                if is_side {
                    note = format!("A subtitle or extra file couldn't come along: {e}");
                    continue;
                }
                ok = false;
                note = e;
                break;
            }
        }
        let ri = &mut run.items[idx];
        ri.ok = ok;
        ri.note = if ok {
            let how = match ri.ops.iter().find(|o| o.src == item.path).map(|o| o.kind) {
                Some(OpKind::Link) => "Renamed (hardlink; the download keeps seeding)",
                Some(OpKind::Clone) => "Renamed (instant copy that shares the data; the download keeps seeding)",
                _ => "Renamed",
            };
            if note.is_empty() { how.to_string() } else { format!("{how}. {note}") }
        } else {
            note
        };
        // A show "renamed myself" into Midgar's folder: let Midgar see it.
        if ok && item.tracked_id > 0 {
            let arr = if item.chosen.as_ref().is_some_and(|c| c.kind == "movie") { Arr::Turks } else { Arr::Midgar };
            if arr.configured(&cfg) {
                suite::arr_rescan(&cfg, arr, item.tracked_id);
                ri.app = arr.label().to_lowercase();
                ri.app_id = item.tracked_id;
            }
        }
    }
    save_run(&run);
    // Remember confirmed matches, update item states.
    {
        let mut all = shared.items.lock().unwrap_or_else(|e| e.into_inner());
        let mut rem = shared.remembered.lock().unwrap_or_else(|e| e.into_inner());
        for (item, ri) in items.iter().zip(run.items.iter()) {
            if let Some(i) = all.iter_mut().find(|i| i.id == item.id) {
                i.state = if ri.ok { State::Done } else { State::Failed };
                i.message = ri.note.clone();
                i.updated = now();
                if ri.ok {
                    if let Some(c) = &i.chosen {
                        rem.insert(i.remember_key(), c.clone());
                    }
                }
            }
        }
    }
    shared.save_remembered();
    shared.save_items();
    let good = run.items.iter().filter(|i| i.ok).count();
    info!("{} {good} of {} file(s)", if automatic { "Automatic renaming:" } else { "Renamed" }, run.items.len());
    for ri in run.items.iter().filter(|i| !i.ok) {
        warn!("Not renamed: {} ({})", ri.from, ri.note);
    }
    Ok(run.id)
}

// ---------- undo ----------

pub fn undo(run_id: u64) -> Result<String, String> {
    let _guard = APPLY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut run = load_runs().into_iter().find(|r| r.id == run_id).ok_or("That entry is gone")?;
    if run.undone {
        return Err("Already undone".into());
    }
    if now().saturating_sub(run.time) > 30 * 86_400 {
        return Err("Undo is kept for 30 days".into());
    }
    let cfg = Config::load(&var_dir().join("config.json"));
    let mut undone = 0;
    let mut refused: Vec<String> = Vec::new();
    for item in &mut run.items {
        if item.handoff || !item.ok {
            continue;
        }
        // Check every step can be reversed before touching anything.
        let mut problem = None;
        for o in item.ops.iter().filter(|o| o.state == OpState::Done && o.kind != OpKind::Mkdir) {
            let dst = Path::new(&o.dst);
            if !dst.exists() {
                problem = Some(format!("{} isn't there any more", fsops::show(dst)));
            } else if o.after.is_some() && fsops::ident(dst) != o.after {
                problem = Some(format!("{} was replaced since", fsops::show(dst)));
            } else if o.kind == OpKind::Move && Path::new(&o.src).exists() && !(cfg!(windows) && fsops::same_path(Path::new(&o.src), dst)) {
                problem = Some(format!("{} exists again", fsops::show(Path::new(&o.src))));
            }
        }
        if let Some(p) = problem {
            refused.push(format!("{}: {p}", item.label));
            continue;
        }
        // Subtitles added later (Jenova) under the new name go back too.
        let video_op = item.ops.iter().find(|o| o.src == item.from && o.state == OpState::Done).cloned();
        let mut extra: Vec<Op> = Vec::new();
        if let Some(v) = &video_op {
            let (new, old) = (Path::new(&v.dst), Path::new(&v.src));
            let new_stem = new.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let old_stem = old.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let known: HashSet<&str> = item.ops.iter().map(|o| o.dst.as_str()).collect();
            if let (Some(nd), Some(od)) = (new.parent(), old.parent()) {
                for e in std::fs::read_dir(nd).into_iter().flatten().flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let p = e.path();
                    if known.contains(p.to_string_lossy().as_ref()) || p == new {
                        continue;
                    }
                    if let Some(rest) = name.strip_prefix(&new_stem) {
                        let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
                        if rest.starts_with('.') && ["srt", "ass", "ssa", "sub", "idx", "vtt", "sup"].contains(&ext.as_str()) {
                            let back = od.join(format!("{old_stem}{rest}"));
                            if !back.exists() {
                                extra.push(Op { kind: OpKind::Move, src: back.to_string_lossy().into_owned(), dst: p.to_string_lossy().into_owned(), state: OpState::Done, after: None });
                            }
                        }
                    }
                }
            }
        }
        for o in extra.iter().chain(item.ops.iter().rev()) {
            if o.state != OpState::Done {
                continue;
            }
            let (src, dst) = (Path::new(&o.src), Path::new(&o.dst));
            let r = match o.kind {
                OpKind::Mkdir => {
                    let _ = std::fs::remove_dir(dst);
                    Ok(())
                }
                OpKind::Link | OpKind::Clone => {
                    if src.exists() {
                        // Another name for the same data: the original is still there.
                        std::fs::remove_file(dst).map_err(|e| fsops::plain(&e))
                    } else {
                        fsops::move_file(dst, src)
                    }
                }
                OpKind::Move => fsops::move_file(dst, src),
            };
            if let Err(e) = r {
                refused.push(format!("{}: {e}", item.label));
            }
        }
        for o in item.ops.iter_mut() {
            if o.state == OpState::Done {
                o.state = OpState::Undone;
            }
        }
        undone += 1;
        if !item.app.is_empty() && item.app_id > 0 {
            let arr = if item.app == "turks" { Arr::Turks } else { Arr::Midgar };
            if arr.configured(&cfg) {
                suite::arr_rescan(&cfg, arr, item.app_id);
            }
        }
    }
    run.undone = refused.is_empty();
    run.undo_note = if refused.is_empty() { format!("Undone ({undone} file{})", if undone == 1 { "" } else { "s" }) } else { format!("Undid {undone}; not undone: {}", refused.join("; ")) };
    save_run(&run);
    info!("Undo of run {run_id}: {}", run.undo_note);
    if undone == 0 && !refused.is_empty() {
        Err(run.undo_note)
    } else {
        Ok(run.undo_note)
    }
}

// ---------- automation ----------

const OWNER_FILE: &str = ".elixir-owner";

/// Whether another copy of Elixir watches this folder (Windows app, another NAS).
fn owner_check(folder: &Path, my_id: &str, last_seen: &mut HashMap<String, (u64, Instant)>) -> Result<(), String> {
    let file = folder.join(OWNER_FILE);
    let theirs: Option<Value> = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let mut counter = 0;
    if let Some(v) = &theirs {
        let id = v["id"].as_str().unwrap_or("");
        let c = v["counter"].as_u64().unwrap_or(0);
        if id != my_id && !id.is_empty() {
            let key = folder.to_string_lossy().into_owned();
            let entry = last_seen.entry(key).or_insert((c, Instant::now()));
            if entry.0 != c {
                *entry = (c, Instant::now());
            }
            // Still alive (its counter moved in the last 10 minutes on our clock).
            if entry.1.elapsed() < Duration::from_secs(600) {
                return Err(format!("Another copy of Elixir ({}) watches this folder", v["label"].as_str().unwrap_or("unknown")));
            }
            return Err(format!("Another copy of Elixir ({}) owned this folder; it hasn't been seen for a while. Use Take over to watch it here", v["label"].as_str().unwrap_or("unknown")));
        }
        counter = c;
    }
    let mine = json!({"id": my_id, "label": config::machine_label(), "counter": counter + 1, "app": "Elixir"});
    let r = config::write_atomic(&file, mine.to_string().as_bytes()).map_err(|e| format!("Can't write in this folder: {}", fsops::plain(&e)));
    fsops::hide(&file);
    r
}

pub fn take_over(folder: &Path, my_id: &str) -> Result<(), String> {
    let mine = json!({"id": my_id, "label": config::machine_label(), "counter": 1, "app": "Elixir"});
    let r = config::write_atomic(&folder.join(OWNER_FILE), mine.to_string().as_bytes()).map_err(|e| fsops::plain(&e));
    fsops::hide(&folder.join(OWNER_FILE));
    r
}

/// Folders Midgar and Turks manage, which can't be watched.
pub fn library_roots(cfg: &Config) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for arr in [Arr::Midgar, Arr::Turks] {
        if arr.configured(cfg) {
            for r in suite::arr_roots(cfg, arr).unwrap_or_default() {
                out.push((r, arr.label()));
            }
        }
    }
    out
}

/// Record what's in a newly watched folder, so only later arrivals are automatic.
pub fn snapshot_folder(shared: &Shared, folder: &str) -> usize {
    let files = videos_in(Path::new(folder), 6);
    let mut inv = shared.inventory.lock().unwrap_or_else(|e| e.into_inner());
    let map = inv.folders.entry(folder.to_string()).or_default();
    for f in &files {
        if let Some(id) = fsops::ident(f) {
            map.entry(key_of(id)).or_insert(Entry {
                path: f.to_string_lossy().into_owned(),
                size: std::fs::metadata(f).map(|m| m.len()).unwrap_or(0),
                seen: Seen::Old,
                first: now(),
                changed: now(),
                torrent_dir: String::new(),
                stable_polls: 0,
            });
        }
    }
    drop(inv);
    shared.save_inventory();
    files.len()
}

/// Quiet times (seconds). Shorter values are only for testing.
fn quiet(env: &str, default: u64) -> u64 {
    std::env::var(env).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn pass_every() -> Duration {
    Duration::from_secs(quiet("ELIXIR_PASS_SECS", 30))
}

fn automation(shared: Arc<Shared>, kick: Receiver<()>) {
    let mut owners: HashMap<String, (u64, Instant)> = HashMap::new();
    std::thread::sleep(Duration::from_secs(5));
    loop {
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| auto_pass(&shared, &mut owners))) {
            warn!("Automation pass failed: {e:?}");
        }
        let _ = kick.recv_timeout(pass_every());
        while kick.try_recv().is_ok() {}
    }
}

fn set_auto(shared: &Shared, state: &str, message: &str, folders: Vec<FolderInfo>) {
    let mut a = shared.auto.lock().unwrap_or_else(|e| e.into_inner());
    if a.state != state || a.message != message {
        if state == "paused" {
            warn!("Automatic renaming: {message}");
        } else if a.state == "paused" {
            info!("Automatic renaming running again");
        }
    }
    a.state = state.into();
    a.message = message.into();
    a.last_pass = now();
    a.folders = folders;
}

fn auto_pass(shared: &Arc<Shared>, owners: &mut HashMap<String, (u64, Instant)>) {
    let cfg = shared.cfg();
    let infos = |state: &str, msg: &str| -> Vec<FolderInfo> {
        cfg.watched.iter().map(|f| FolderInfo { path: f.clone(), state: state.into(), message: msg.into(), ..Default::default() }).collect()
    };
    if !cfg.automatic {
        set_auto(shared, "off", "Automatic renaming is off", infos("off", ""));
        return;
    }
    if cfg.watched.is_empty() {
        set_auto(shared, "idle", "Add a folder to watch", vec![]);
        return;
    }
    // Without Lifestream only folders marked "Nothing seeds from this folder" can run.
    let lifestream = suite::lifestream_configured(&cfg);
    if !lifestream && !cfg.watched.iter().any(|f| cfg.no_seed_for(Path::new(f))) {
        set_auto(shared, "paused", "Connect Lifestream in Settings, or mark a watched folder \"Nothing seeds from this folder\"", infos("paused", ""));
        return;
    }
    let torrents = if lifestream {
        match suite::lifestream_torrents(&cfg) {
            Ok(t) => t,
            Err(e) => {
                set_auto(shared, "paused", &format!("Paused: {e}"), infos("paused", ""));
                return;
            }
        }
    } else {
        Vec::new()
    };
    for arr in [Arr::Midgar, Arr::Turks] {
        if arr.configured(&cfg) {
            if let Err(e) = suite::arr_version(&cfg, arr) {
                set_auto(shared, "paused", &format!("Paused: {e}"), infos("paused", ""));
                return;
            }
        }
    }
    let ours = our_outputs(&load_runs());
    let roots = library_roots(&cfg);
    let mut fis = Vec::new();
    let mut new_items: Vec<Item> = Vec::new();
    for folder in &cfg.watched {
        let fp = Path::new(folder);
        let mut fi = FolderInfo { path: folder.clone(), state: "watching".into(), ..Default::default() };
        if let Some((r, app)) = roots.iter().find(|(r, _)| fsops::within(fp, Path::new(r)) || fsops::within(Path::new(r), fp)) {
            fi.state = "refused".into();
            fi.message = format!("{app} manages {r}. Watch your downloads instead");
            fis.push(fi);
            continue;
        }
        if !lifestream && !cfg.no_seed_for(fp) {
            fi.state = "paused".into();
            fi.message = "Needs Lifestream to know what's seeding (or mark it \"Nothing seeds from this folder\")".into();
            fis.push(fi);
            continue;
        }
        if !fp.is_dir() {
            fi.state = "paused".into();
            fi.message = "The folder can't be reached (volume offline or removed). Waiting for it".into();
            fis.push(fi);
            continue;
        }
        if let Err(e) = owner_check(fp, &cfg.owner_id, owners) {
            fi.state = "paused".into();
            fi.message = e;
            fis.push(fi);
            continue;
        }
        let files = videos_in(fp, 6);
        let mut inv = shared.inventory.lock().unwrap_or_else(|e| e.into_inner());
        let fresh_folder = !inv.folders.contains_key(folder);
        let map = inv.folders.entry(folder.clone()).or_default();
        let present: HashSet<String> = files.iter().filter_map(|f| fsops::ident(f)).map(key_of).collect();
        // Files that went away (moved, deleted) are forgotten.
        map.retain(|k, _| present.contains(k));
        for f in &files {
            let Some(id) = fsops::ident(f) else { continue };
            let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
            let path = f.to_string_lossy().into_owned();
            let e = map.entry(key_of(id)).or_insert(Entry {
                path: path.clone(),
                size,
                seen: if fresh_folder { Seen::Old } else { Seen::New },
                first: now(),
                changed: now(),
                torrent_dir: String::new(),
                stable_polls: 0,
            });
            e.path = path.clone();
            if e.seen != Seen::New {
                continue;
            }
            if ours.contains(&path) {
                e.seen = Seen::Taken;
                continue;
            }
            if e.size != size {
                e.size = size;
                e.changed = now();
                e.stable_polls = 0;
            }
            // Finished?
            let torrent = torrents.iter().find(|t| t.files.iter().any(|tf| fsops::same_path(&tf.path, f)));
            let ready = match torrent {
                Some(t) => {
                    let tf = t.files.iter().find(|tf| fsops::same_path(&tf.path, f));
                    let complete = t.finished() && tf.is_some_and(|tf| tf.done >= tf.length && tf.length > 0) && matches!(t.status, 0 | 5 | 6);
                    // Settled: same folder over two passes, and quiet for 2 minutes.
                    if e.torrent_dir == t.dir && complete {
                        e.stable_polls += 1;
                    } else {
                        e.torrent_dir = t.dir.clone();
                        e.stable_polls = 0;
                        e.changed = now();
                    }
                    complete && e.stable_polls >= 2 && now().saturating_sub(e.changed) >= quiet("ELIXIR_TORRENT_QUIET", 120)
                }
                None => now().saturating_sub(e.changed) >= quiet("ELIXIR_FILE_QUIET", 300) && !fsops::open_for_writing(f),
            };
            if !ready {
                fi.waiting += 1;
                continue;
            }
            e.seen = Seen::Taken;
            new_items.push(new_item(shared, f, 0, Origin::Auto, Source::Auto));
        }
        fi.old = map.values().filter(|e| e.seen == Seen::Old).count();
        drop(inv);
        fis.push(fi);
    }
    shared.save_inventory();
    let state = if fis.iter().any(|f| f.state == "watching") { if cfg.practice { "practice" } else { "on" } } else { "paused" };
    let msg = match state {
        "practice" => "Practicing: checks new files and writes down what it would do",
        "on" => "Watching for new files",
        _ => "No folder can be watched right now",
    };
    set_auto(shared, state, msg, fis);
    if !new_items.is_empty() {
        let ids: Vec<u64> = new_items.iter().map(|i| i.id).collect();
        info!("Automation: {} new file(s) ready", ids.len());
        shared.items.lock().unwrap_or_else(|e| e.into_inner()).extend(new_items);
        shared.save_items();
        for id in ids {
            shared.queue_match(id);
        }
    }
}

/// After an automatic file is matched: practice, rename, or leave it in the Queue.
fn after_auto_match(shared: &Arc<Shared>, id: u64) {
    let cfg = shared.cfg();
    let Some(item) = shared.item(id) else { return };
    let first_time = !shared.remembered.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&item.remember_key());
    match item.state {
        State::Ready | State::Midgar => {
            if !cfg.automatic {
                shared.with_item(id, |i| {
                    i.plan = Some(i.state);
                    i.state = State::Pick;
                    i.message = "Automatic renaming was turned off. Rename it here if you like".into();
                });
            } else if cfg.practice {
                shared.with_item(id, |i| {
                    i.message = if i.state == State::Midgar { "Would hand this to Midgar".into() } else { format!("Would rename to {}", fsops::show(Path::new(&i.target))) };
                    i.plan = Some(i.state);
                    i.state = State::Practice;
                });
            } else if first_time {
                shared.with_item(id, |i| {
                    i.plan = Some(i.state);
                    i.state = State::Pick;
                    i.message = "A show or movie Elixir hasn't seen before: confirm it once".into();
                });
            } else {
                match apply(shared, &[id], true) {
                    Ok(_) => {}
                    Err(e) => {
                        shared.with_item(id, |i| {
                            i.state = State::Pick;
                            i.message = format!("Didn't rename: {e}");
                        });
                    }
                }
            }
        }
        State::Owned | State::Skipped | State::Done => {}
        _ => {}
    }
    shared.save_items();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genre_folders_and_suggestions() {
        let folders = vec!["Action Adventure".to_string(), "Animated".into(), "Suspense".into(), "Comedy".into()];
        assert_eq!(suggest_genre_folder(&["Thriller".into()], &folders).as_deref(), Some("Suspense"));
        assert_eq!(suggest_genre_folder(&["Animation".into()], &folders).as_deref(), Some("Animated"));
        assert_eq!(suggest_genre_folder(&["Western".into()], &folders), None);
    }

    #[test]
    fn titles_match_loosely() {
        assert!(title_matches("The Lord of the Rings The Rings of Power", "The Lord of the Rings: The Rings of Power", &[]));
        assert!(title_matches("Scrubs", "Scrubs (2001)", &[]));
        assert!(!title_matches("The Office", "The Office (UK)", &[]));
        assert!(title_matches("Sousou no Frieren", "Frieren: Beyond Journey's End", &["Sousou no Frieren".into()]));
    }

    #[test]
    fn sidecars_follow_the_video() {
        let d = tempfile::tempdir().unwrap();
        let v = d.path().join("Show.S01E01.720p.mkv");
        std::fs::write(&v, b"x").unwrap();
        std::fs::write(d.path().join("Show.S01E01.720p.en.srt"), b"s").unwrap();
        std::fs::write(d.path().join("Show.S01E01.720p.nfo"), b"n").unwrap();
        std::fs::write(d.path().join("RARBG.txt"), b"t").unwrap();
        std::fs::create_dir(d.path().join("Subs")).unwrap();
        std::fs::write(d.path().join("Subs/2_English.srt"), b"s").unwrap();
        let s = sidecars_of(&v, Path::new("/new"), "Show - S01E01 - Pilot");
        let mut dsts: Vec<&str> = s.iter().map(|(_, d)| d.as_str()).collect();
        dsts.sort();
        let want: Vec<String> = ["Show - S01E01 - Pilot.2_English.srt", "Show - S01E01 - Pilot.en.srt", "Show - S01E01 - Pilot.nfo"].iter().map(|n| Path::new("/new").join(n).to_string_lossy().into_owned()).collect();
        assert_eq!(dsts, want);
    }

    // Linux moves are link + unlink, so a crash can leave both names; Windows moves in one step.
    #[cfg(unix)]
    #[test]
    fn journal_recovery_finishes_half_moves() {
        let d = tempfile::tempdir().unwrap();
        std::env::set_var("ELIXIR_VAR", d.path());
        std::env::remove_var("SYNOPKG_PKGVAR");
        let a = d.path().join("a.mkv");
        let b = d.path().join("b.mkv");
        std::fs::write(&a, b"v").unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        let run = Run { id: 7, time: now(), items: vec![RunItem { from: a.to_string_lossy().into(), ops: vec![op(OpKind::Move, &a.to_string_lossy(), &b.to_string_lossy())], ok: true, ..Default::default() }], ..Default::default() };
        save_run(&run);
        recover_journal();
        assert!(!a.exists() && b.exists());
        let r = load_runs().into_iter().find(|r| r.id == 7).unwrap();
        assert!(r.interrupted);
        assert_eq!(r.items[0].ops[0].state, OpState::Done);
        // And undo puts it back.
        undo(7).unwrap();
        assert!(a.exists() && !b.exists());
    }
}
