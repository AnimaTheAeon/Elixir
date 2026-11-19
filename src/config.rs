//! Settings, stored as JSON in the package's var folder.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::naming::MovieNaming;

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "m4v", "mov", "wmv", "mpg", "mpeg", "ts", "m2ts", "mts", "webm", "flv", "vob", "ogm", "divx", "xvid", "3gp", "asf", "rmvb",
];

/// Files that travel with a video (same name start).
pub const SIDECAR_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "sub", "idx", "sup", "vtt", "smi", "nfo", "jpg", "jpeg", "png", "tbn"];

/// Folders DSM and file-sharing tools create that never hold real media.
pub const SYSTEM_FOLDER_NAMES: &[&str] =
    &["@eaDir", "#recycle", "#snapshot", "@tmp", "@sharesnap", "@Recycle", ".@__thumb", "@SynoResource", "@SynoEAStream", ".AppleDouble", ".Trashes"];

/// Extras and disc folders: left alone.
pub const SKIP_FOLDER_NAMES: &[&str] = &[
    "sample", "samples", "extras", "featurettes", "behind the scenes", "deleted scenes", "interviews", "trailers", "shorts", "bdmv", "video_ts", "certificate",
];

/// Settings that are secrets: never sent back to the window.
pub const SECRETS: &[&str] = &["tmdb_key", "tvdb_key", "tvdb_pin", "lifestream_pass", "midgar_key", "turks_key"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Window look: "ff", "green" (Xbox), "blades" or "glass" (Dark Glass).
    pub theme: String,
    pub auto_update: bool,

    /// "Automatic renaming". Off until the user turns it on.
    pub automatic: bool,
    /// Practice mode: check everything, write down what would happen, change nothing.
    pub practice: bool,
    pub watched: Vec<String>,

    pub tmdb_key: String,
    pub tvdb_key: String,
    pub tvdb_pin: String,
    pub anidb_client: String,
    pub anidb_version: u32,
    /// Two-letter language for titles (TheMovieDB/TheTVDB).
    pub language: String,

    pub lifestream_url: String,
    pub lifestream_user: String,
    pub lifestream_pass: String,
    pub midgar_url: String,
    pub midgar_key: String,
    pub turks_url: String,
    pub turks_key: String,

    /// Where shows/anime/movies Midgar and Turks don't track are filed. Empty = Midgar's/Turks' first root folder.
    pub tv_folder: String,
    pub anime_folder: String,
    pub movies_folder: String,
    pub movie_naming: MovieNaming,

    /// Settings page: show the Advanced section.
    pub advanced: bool,
    /// Random ID of this copy, written into watched folders' owner files.
    pub owner_id: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "ff".into(),
            auto_update: true,
            automatic: false,
            practice: true,
            watched: Vec::new(),
            tmdb_key: String::new(),
            tvdb_key: String::new(),
            tvdb_pin: String::new(),
            anidb_client: String::new(),
            anidb_version: 1,
            language: "en".into(),
            lifestream_url: "http://localhost:9091".into(),
            lifestream_user: String::new(),
            lifestream_pass: String::new(),
            midgar_url: "http://localhost:8990".into(),
            midgar_key: String::new(),
            turks_url: "http://localhost:7878".into(),
            turks_key: String::new(),
            tv_folder: String::new(),
            anime_folder: String::new(),
            movies_folder: String::new(),
            movie_naming: MovieNaming::default(),
            advanced: false,
            owner_id: String::new(),
        }
    }
}

impl Config {
    pub fn normalize(&mut self) {
        if !["green", "blades", "glass"].contains(&self.theme.as_str()) {
            self.theme = "ff".into();
        }
        let mut seen = std::collections::HashSet::new();
        self.watched.retain(|f| f.starts_with('/') && seen.insert(f.trim_end_matches('/').to_string()));
        for f in self.watched.iter_mut().chain([&mut self.tv_folder, &mut self.anime_folder, &mut self.movies_folder]) {
            while f.len() > 1 && f.ends_with('/') {
                f.pop();
            }
        }
        for url in [&mut self.lifestream_url, &mut self.midgar_url, &mut self.turks_url] {
            *url = url.trim().trim_end_matches('/').to_string();
            if !url.is_empty() && !url.starts_with("http://") && !url.starts_with("https://") {
                *url = format!("http://{url}");
            }
        }
        self.language = self.language.trim().to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphabetic()).take(2).collect();
        if self.language.len() != 2 {
            self.language = "en".into();
        }
        self.anidb_client = self.anidb_client.trim().to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        self.anidb_version = self.anidb_version.max(1);
        if self.movie_naming.file.trim().is_empty() {
            self.movie_naming.file = MovieNaming::default().file;
        }
        if self.movie_naming.folder.trim().is_empty() {
            self.movie_naming.folder = MovieNaming::default().folder;
        }
        for k in [&mut self.tmdb_key, &mut self.tvdb_key, &mut self.tvdb_pin, &mut self.midgar_key, &mut self.turks_key] {
            *k = k.trim().to_string();
        }
        if self.owner_id.is_empty() {
            self.owner_id = random_id();
        }
    }

    pub fn load(path: &Path) -> Self {
        let mut config: Config = std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        config.normalize();
        config
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        write_atomic(path, &json)?;
        use std::os::unix::fs::PermissionsExt;
        // It holds keys and passwords.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
}

pub fn random_id() -> String {
    let bytes = std::fs::read("/proc/sys/kernel/random/uuid").ok().map(|b| String::from_utf8_lossy(&b).trim().replace('-', ""));
    bytes.filter(|s| s.len() >= 16).map(|s| s[..16].to_string()).unwrap_or_else(|| {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        format!("{:016x}", t ^ (std::process::id() as u128) << 32)
    })
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

pub fn is_video(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|ext| VIDEO_EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(ext)))
}

pub fn is_system_folder(name: &str) -> bool {
    SYSTEM_FOLDER_NAMES.iter().any(|f| f.eq_ignore_ascii_case(name)) || (name.starts_with('.') && name.len() > 1) || name.starts_with('@') || name.starts_with('#')
}

pub fn is_skip_folder(name: &str) -> bool {
    is_system_folder(name) || SKIP_FOLDER_NAMES.iter().any(|f| f.eq_ignore_ascii_case(name))
}

/// Partial downloads and temp files.
pub fn is_partial(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    [".part", ".!qb", ".!ut", ".tmp", ".crdownload", ".partial", ".lftp-pget-status"].iter().any(|s| l.ends_with(s)) || l.starts_with('~') || l.starts_with(".")
}

/// Package data folder (/var/packages/Elixir/var on DSM).
pub fn var_dir() -> PathBuf {
    std::env::var_os("SYNOPKG_PKGVAR")
        .or_else(|| std::env::var_os("ELIXIR_VAR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/packages/Elixir/var"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_cleans_input() {
        let mut c = Config {
            watched: vec!["/volume1/data/torrents/".into(), "/volume1/data/torrents".into(), "relative".into()],
            midgar_url: "192.168.1.127:8990/".into(),
            theme: "nope".into(),
            language: "EN-us".into(),
            ..Config::default()
        };
        c.normalize();
        assert_eq!(c.watched, vec!["/volume1/data/torrents"]);
        assert_eq!(c.midgar_url, "http://192.168.1.127:8990");
        assert_eq!(c.theme, "ff");
        assert_eq!(c.language, "en");
        assert!(!c.automatic);
        assert!(c.practice);
        assert_eq!(c.owner_id.len(), 16);
    }

    #[test]
    fn file_kinds() {
        assert!(is_video(Path::new("a/b.MKV")));
        assert!(!is_video(Path::new("a/b.srt")));
        assert!(is_partial("movie.mkv.part"));
        assert!(is_skip_folder("Sample"));
        assert!(is_skip_folder("@eaDir"));
        assert!(!is_skip_folder("Season 1"));
    }
}
