//! HTTP through DSM's curl. Addresses, keys and passwords go to curl on its
//! standard input, so they never show up in the process list or in messages.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: String,
}

impl Response {
    pub fn json(&self) -> Result<serde_json::Value, String> {
        serde_json::from_slice(&self.body).map_err(|_| "sent an answer Elixir couldn't read".to_string())
    }
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.lines().rev().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    }
}

#[derive(Default)]
pub struct Request<'a> {
    pub method: &'a str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub basic_auth: Option<(String, String)>,
    pub timeout: u64,
    /// Ask for gzip and unpack it (AniDB).
    pub compressed: bool,
}

impl<'a> Request<'a> {
    pub fn get(url: impl Into<String>) -> Self {
        Request { method: "GET", url: url.into(), timeout: 30, ..Default::default() }
    }
    pub fn post_json(url: impl Into<String>, body: &serde_json::Value) -> Self {
        Request {
            method: "POST",
            url: url.into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: Some(body.to_string().into_bytes()),
            timeout: 30,
            ..Default::default()
        }
    }
    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
}

fn quote(v: &str) -> String {
    let mut out = String::from("\"");
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Send a request. Errors are plain sentences that name `service`, never an address.
pub fn send(service: &str, req: &Request) -> Result<Response, String> {
    let dir = std::env::temp_dir().join(format!("elixir-http-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let result = (|| {
        let out = dir.join("body");
        let hdr = dir.join("headers");
        let mut cfg = String::new();
        cfg.push_str(&format!("url = {}\n", quote(&req.url)));
        cfg.push_str(&format!("request = {}\n", quote(req.method)));
        for (k, v) in &req.headers {
            cfg.push_str(&format!("header = {}\n", quote(&format!("{k}: {v}"))));
        }
        if let Some((u, p)) = &req.basic_auth {
            cfg.push_str(&format!("user = {}\n", quote(&format!("{u}:{p}"))));
        }
        if let Some(body) = &req.body {
            let path = dir.join("req");
            std::fs::write(&path, body).map_err(|e| e.to_string())?;
            cfg.push_str(&format!("data-binary = {}\n", quote(&format!("@{}", path.display()))));
        }
        if req.compressed {
            cfg.push_str("compressed\n");
        }
        cfg.push_str(&format!("output = {}\n", quote(&out.display().to_string())));
        cfg.push_str(&format!("dump-header = {}\n", quote(&hdr.display().to_string())));
        cfg.push_str(&format!("max-time = {}\n", req.timeout.max(5)));
        cfg.push_str("connect-timeout = 10\nsilent\nshow-error\nuser-agent = \"Elixir\"\nwrite-out = \"%{http_code}\"\n");
        let mut child = Command::new("curl")
            .args(["--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Elixir couldn't run curl on this NAS".to_string())?;
        child.stdin.take().ok_or("no stdin")?.write_all(cfg.as_bytes()).map_err(|e| e.to_string())?;
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        match output.status.code() {
            Some(0) => {}
            Some(6) => return Err(format!("couldn't find {service}. Check its address in Settings")),
            Some(7) => return Err(format!("couldn't connect to {service}. Is it running?")),
            Some(28) => return Err(format!("{service} took too long to answer")),
            Some(35) | Some(51) | Some(58) | Some(60) => return Err(format!("couldn't make a secure connection to {service}")),
            Some(52) | Some(56) => return Err(format!("{service} closed the connection")),
            _ => return Err(format!("couldn't reach {service}")),
        }
        let status: u16 = String::from_utf8_lossy(&output.stdout).trim().parse().unwrap_or(0);
        Ok(Response {
            status,
            body: std::fs::read(&out).unwrap_or_default(),
            headers: std::fs::read_to_string(&hdr).unwrap_or_default(),
        })
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Send and require a 2xx answer.
pub fn ok(service: &str, req: &Request) -> Result<Response, String> {
    let r = send(service, req)?;
    match r.status {
        200..=299 => Ok(r),
        401 | 403 => Err(format!("{service} refused the key or password. Check it in Settings")),
        404 => Err(format!("{service} doesn't know that address (404)")),
        429 => Err(format!("{service} asked Elixir to slow down. It will try again later")),
        s => Err(format!("{service} answered with an error (HTTP {s})")),
    }
}

/// Percent-encode a query value.
pub fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
