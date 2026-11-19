//! Elixir for Synology DSM.
//!
//! `elixir daemon`  background service (started by DSM)
//! `elixir cgi`     the DSM web bridge (run by DSM as api.cgi)
//! `elixir update`  install the latest release (root, Task Scheduler)

mod api;
mod cgi;
mod config;
mod engine;
mod fsops;
mod http;
mod log;
mod meta;
mod naming;
mod parse;
mod suite;
mod update;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "daemon".into());
    match mode.as_str() {
        "cgi" => cgi::run(),
        "update" => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(update::run_cli(&args));
        }
        "version" | "--version" => println!("{}", env!("CARGO_PKG_VERSION")),
        _ => daemon(),
    }
}

fn daemon() {
    // Files and folders Elixir makes stay writable for Midgar and Turks (group).
    // SAFETY: umask has no preconditions.
    unsafe { libc::umask(0o002) };
    let var = config::var_dir();
    let _ = std::fs::create_dir_all(&var);
    log::init(var.join("elixir.log"));
    let _ = std::fs::write(var.join("elixir.pid"), std::process::id().to_string());
    info!("Elixir {} starting", env!("CARGO_PKG_VERSION"));
    let shared = engine::start();
    update::start_checker();
    api::serve(shared);
}
