//! The Windows app: one window showing the same pages as the NAS version,
//! a tray icon, and one running copy at a time. Closing the window keeps
//! Elixir working in the tray; Quit (tray menu) stops it.

use std::path::PathBuf;
use std::time::Duration;

use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Icon, WindowBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Threading::CreateMutexW;
use wry::{DragDropEvent, WebContext, WebViewBuilder};

use crate::{api, config, engine, info, log, update, warn};

/// The installer checks for this to know Elixir is running.
const MUTEX_NAME: &str = "ElixirSingleInstance";

#[derive(Debug)]
enum UserEvent {
    Show,
    Quit,
    Dropped(Vec<PathBuf>),
    TrayClick,
    Menu(tray_icon::menu::MenuId),
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// True if this is the only copy. Otherwise asks the running copy to show itself.
fn claim_single_instance(var: &std::path::Path) -> bool {
    let name = wide(MUTEX_NAME);
    // SAFETY: valid NUL-terminated name. The handle is kept for the life of the process.
    let h = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if h.is_null() {
        return true;
    }
    if exists {
        let _ = std::fs::write(var.join("show.request"), b"1");
        return false;
    }
    true
}

fn icon_rgba() -> (Vec<u8>, u32, u32) {
    let png = include_bytes!("../ui/images/elixir_64.png");
    decode_png(png).unwrap_or_else(|| (vec![200, 60, 140, 255].repeat(16 * 16), 16, 16))
}

/// Tiny PNG decoder for our own 8-bit RGBA icons (no extra crates).
fn decode_png(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if data.len() < 8 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut pos = 8;
    let (mut w, mut h, mut idat) = (0u32, 0u32, Vec::new());
    let (mut depth, mut color) = (0u8, 0u8);
    while pos + 8 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().ok()?) as usize;
        let kind = &data[pos + 4..pos + 8];
        let body = data.get(pos + 8..pos + 8 + len)?;
        match kind {
            b"IHDR" => {
                w = u32::from_be_bytes(body[0..4].try_into().ok()?);
                h = u32::from_be_bytes(body[4..8].try_into().ok()?);
                depth = body[8];
                color = body[9];
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    if depth != 8 || color != 6 {
        return None;
    }
    let mut raw = Vec::new();
    use std::io::Read;
    flate2::read::ZlibDecoder::new(&idat[..]).read_to_end(&mut raw).ok()?;
    let stride = w as usize * 4;
    let mut out = vec![0u8; stride * h as usize];
    let mut prev = vec![0u8; stride];
    for y in 0..h as usize {
        let line = raw.get(y * (stride + 1)..(y + 1) * (stride + 1))?;
        let (filter, src) = (line[0], &line[1..]);
        let mut cur = vec![0u8; stride];
        for i in 0..stride {
            let a = if i >= 4 { cur[i - 4] as i16 } else { 0 };
            let b = prev[i] as i16;
            let c = if i >= 4 { prev[i - 4] as i16 } else { 0 };
            let pred = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => (a + b) / 2,
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
                }
                _ => return None,
            };
            cur[i] = (src[i] as i16 + pred) as u8;
        }
        out[y * stride..(y + 1) * stride].copy_from_slice(&cur);
        prev = cur;
    }
    Some((out, w, h))
}

/// Requests from the installer ("quit") or a second copy ("show"), as files in the data folder.
fn watch_requests(var: PathBuf, proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(400));
        if std::fs::remove_file(var.join("show.request")).is_ok() {
            let _ = proxy.send_event(UserEvent::Show);
        }
        if var.join("quit.request").exists() {
            let _ = std::fs::remove_file(var.join("quit.request"));
            let _ = proxy.send_event(UserEvent::Quit);
        }
    });
}

fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

pub fn run() {
    let start_hidden = std::env::args().any(|a| a == "--tray");
    let var = config::var_dir();
    let _ = std::fs::create_dir_all(&var);
    let _ = std::fs::remove_file(var.join("quit.request"));
    if !claim_single_instance(&var) {
        return;
    }
    log::init(var.join("elixir.log"));
    info!("Elixir {} starting (Windows)", env!("CARGO_PKG_VERSION"));

    let shared = engine::start();
    update::start_checker();
    let (server, port) = match api::bind_any() {
        Ok(x) => x,
        Err(e) => {
            warn!("Can't start: {e}");
            return;
        }
    };
    {
        let shared = shared.clone();
        std::thread::spawn(move || api::serve_on(server, shared));
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    watch_requests(var.clone(), proxy.clone());

    let (rgba, w, h) = icon_rgba();
    let window = WindowBuilder::new()
        .with_title("Elixir")
        .with_inner_size(LogicalSize::new(1180.0, 780.0))
        .with_min_inner_size(LogicalSize::new(900.0, 620.0))
        .with_window_icon(Icon::from_rgba(rgba.clone(), w, h).ok())
        .with_visible(!start_hidden)
        .build(&event_loop)
        .expect("window");

    let init = format!("window.ELIXIR_KEY = {}; window.ELIXIR_WINDOWS = true;", js_string(api::window_key()));
    let drop_proxy = proxy.clone();
    // The page's browser data stays in Elixir's own data folder.
    let mut web_context = WebContext::new(Some(var.join("webview")));
    let webview = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_url(format!("http://127.0.0.1:{port}/"))
        .with_initialization_script(init.as_str())
        .with_devtools(cfg!(debug_assertions))
        .with_drag_drop_handler(move |ev| {
            if let DragDropEvent::Drop { paths, .. } = ev {
                let _ = drop_proxy.send_event(UserEvent::Dropped(paths));
            }
            true
        })
        .build(&window)
        .expect("webview");

    // Tray: click opens the window; the menu has Open and Quit.
    let menu = Menu::new();
    let open_item = MenuItem::new("Open Elixir", true, None);
    let quit_item = MenuItem::new("Quit Elixir", true, None);
    let _ = menu.append(&open_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit_item);
    let (open_id, quit_id) = (open_item.id().clone(), quit_item.id().clone());
    {
        let p = proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                let _ = p.send_event(UserEvent::TrayClick);
            }
        }));
        let p = proxy.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = p.send_event(UserEvent::Menu(e.id));
        }));
    }
    let tray_icon = tray_icon::Icon::from_rgba(rgba, w, h).ok();
    let mut tray = None;
    let mut told_about_tray = false;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                let mut b = TrayIconBuilder::new().with_tooltip("Elixir").with_menu(Box::new(menu.clone()));
                if let Some(i) = tray_icon.clone() {
                    b = b.with_icon(i);
                }
                tray = b.build().ok();
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                // Keep working in the tray.
                window.set_visible(false);
                if !told_about_tray {
                    told_about_tray = true;
                    info!("Window closed; Elixir keeps running in the tray");
                }
            }
            Event::UserEvent(UserEvent::Show) | Event::UserEvent(UserEvent::TrayClick) => {
                window.set_visible(true);
                window.set_minimized(false);
                window.set_focus();
            }
            Event::UserEvent(UserEvent::Menu(id)) => {
                if id == open_id {
                    window.set_visible(true);
                    window.set_minimized(false);
                    window.set_focus();
                } else if id == quit_id {
                    let _ = proxy.send_event(UserEvent::Quit);
                }
            }
            Event::UserEvent(UserEvent::Dropped(paths)) => {
                let list: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                let json = serde_json::to_string(&list).unwrap_or_else(|_| "[]".into());
                let _ = webview.evaluate_script(&format!("window.elixirDrop && window.elixirDrop({json})"));
            }
            Event::UserEvent(UserEvent::Quit) => {
                info!("Quitting");
                // Let a rename in progress finish first.
                let _busy = engine::APPLY_LOCK.lock();
                tray.take();
                *control_flow = ControlFlow::Exit;
                std::process::exit(0);
            }
            _ => {}
        }
    });
}
