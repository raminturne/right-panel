#![windows_subsystem = "windows"]

//! Right Panel — a liquid side panel that lives on the right edge of the screen.

use std::{
    fs,
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

mod sys;
mod util;

use serde_json::{json, Value};
use tao::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::WindowBuilder,
};
use wry::{WebContext, WebViewBuilder};

const UI: &str = include_str!("ui.html");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = "raminturne/right-panel";
const WIN_W: f64 = 480.0;
const WIN_H: f64 = 680.0;

static OPEN: AtomicBool = AtomicBool::new(false);
static PICKING: AtomicBool = AtomicBool::new(false);
/// Where the panel currently lives, so the hover watcher follows moves between edges / monitors.
static PANEL_X: AtomicI32 = AtomicI32::new(0);
static PANEL_Y: AtomicI32 = AtomicI32::new(0);
static PANEL_H: AtomicI32 = AtomicI32::new(0);
static EDGE_X: AtomicI32 = AtomicI32::new(0);
static ON_LEFT: AtomicBool = AtomicBool::new(false);
/// Recent clipboard images, newest last. Thumbnails go to the UI; the pixels stay here.
static IMAGES: Mutex<Vec<(u64, u32, u32, Vec<u8>)>> = Mutex::new(Vec::new());
/// Pinned images are also kept on disk, so they come back after a restart.
static PINNED: Mutex<Vec<(u64, u32, u32, Vec<u8>)>> = Mutex::new(Vec::new());
static LAST_IMAGE: AtomicU64 = AtomicU64::new(0);

enum Ev {
    Script(String),
    Ipc(String),
    Open,
    /// open the panel from the tray, optionally straight into a widget
    Show(&'static str),
    Menu(String),
}

/* ---------------- clipboard ---------------- */
pub fn set_clip(text: &str) {
    if let Ok(mut c) = arboard::Clipboard::new() {
        let _ = c.set_text(text.to_owned());
    }
}

pub fn get_clip() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

fn transform(text: &str, mode: &str) -> String {
    match mode {
        "upper" => text.to_uppercase(),
        "lower" => text.to_lowercase(),
        "title" => text
            .split(' ')
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
        "sentence" => {
            let lower = text.to_lowercase();
            let mut c = lower.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
        "trim" => text.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n").trim().to_string(),
        "oneline" | "plain" => text.split_whitespace().collect::<Vec<_>>().join(" "),
        "reverse" => text.chars().rev().collect(),
        "slug" => {
            let s: String = text.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
            s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
        }
        _ => text.to_string(),
    }
}

fn spawn_clip_watch(proxy: EventLoopProxy<Ev>) {
    thread::spawn(move || {
        let mut seq = 0;
        loop {
            let s = sys::clip_seq();
            if s != seq {
                seq = s;
                match get_clip() {
                    Some(t) if !t.trim().is_empty() => {
                        if t.len() < 100_000 && proxy.send_event(Ev::Script(format!("app.clip({})", json!(t)))).is_err() {
                            return;
                        }
                    }
                    _ => {
                        if let Some(script) = take_clip_image() {
                            if proxy.send_event(Ev::Script(script)).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(if cfg!(windows) { 400 } else { 800 }));
        }
    });
}

/// Copies a new clipboard image into the history and returns the call that shows its thumbnail.
fn take_clip_image() -> Option<String> {
    let (w, h, rgba) = sys::clip_image()?;
    let id = util::hash(&rgba) ^ ((w as u64) << 32) ^ h as u64;
    if LAST_IMAGE.swap(id, Ordering::Relaxed) == id {
        return None;
    }
    let call = image_call(id, w, h, &rgba, false);
    let mut images = IMAGES.lock().ok()?;
    images.retain(|(other, ..)| *other != id);
    images.push((id, w, h, rgba));
    if images.len() > 8 {
        images.remove(0);
    }
    Some(call)
}

/* ---------------- pinned clipboard images ---------------- */
fn pins_dir() -> std::path::PathBuf {
    let dir = sys::data_dir().join("pinned");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn image_call(id: u64, w: u32, h: u32, rgba: &[u8], pinned: bool) -> String {
    let (tw, th, thumb) = util::thumbnail(w, h, rgba, 240);
    let uri = format!("data:image/png;base64,{}", util::base64(&util::png(tw, th, &thumb)));
    format!("app.clipImage({})", json!({ "id": id.to_string(), "w": w, "h": h, "thumb": uri, "pinned": pinned }))
}

fn pin_image(id: u64, on: bool) -> bool {
    let file = pins_dir().join(format!("{id}.bin"));
    if !on {
        let _ = fs::remove_file(file);
        return PINNED.lock().map(|mut v| v.retain(|(i, ..)| *i != id)).is_ok();
    }
    let Some((_, w, h, px)) = IMAGES.lock().ok().and_then(|v| v.iter().find(|(i, ..)| *i == id).cloned()) else { return false };
    let (w, h, px) = util::thumbnail(w, h, &px, 1600); // keep the saved copy reasonable
    let mut body = Vec::with_capacity(px.len() + 8);
    body.extend_from_slice(&w.to_le_bytes());
    body.extend_from_slice(&h.to_le_bytes());
    body.extend_from_slice(&px);
    if fs::write(file, body).is_err() {
        return false;
    }
    PINNED.lock().map(|mut v| v.push((id, w, h, px))).is_ok()
}

/// Shows pinned images again when the app starts.
fn restore_pins(proxy: EventLoopProxy<Ev>) {
    thread::spawn(move || {
        let Ok(entries) = fs::read_dir(pins_dir()) else { return };
        for entry in entries.flatten().take(12) {
            let Some(id) = entry.path().file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u64>().ok()) else { continue };
            let Ok(body) = fs::read(entry.path()) else { continue };
            if body.len() < 8 {
                continue;
            }
            let w = u32::from_le_bytes(body[0..4].try_into().unwrap());
            let h = u32::from_le_bytes(body[4..8].try_into().unwrap());
            let px = body[8..].to_vec();
            if px.len() as u32 != w * h * 4 {
                continue;
            }
            let _ = proxy.send_event(Ev::Script(image_call(id, w, h, &px, true)));
            if let Ok(mut v) = PINNED.lock() {
                v.push((id, w, h, px));
            }
        }
    });
}

/* ---------------- edge hover detection ---------------- */
fn spawn_edge_watch(proxy: EventLoopProxy<Ev>, scale: f64) {
    thread::spawn(move || {
        let mut near = false;
        let mut last = (i32::MIN, i32::MIN);
        let mut held = 0u32;
        loop {
            thread::sleep(Duration::from_millis(12));
            if OPEN.load(Ordering::Relaxed) || PICKING.load(Ordering::Relaxed) {
                near = false;
                continue;
            }
            let Some((px, py)) = sys::cursor_pos() else { continue };
            let (win_y, h, edge) = (PANEL_Y.load(Ordering::Relaxed), PANEL_H.load(Ordering::Relaxed), EDGE_X.load(Ordering::Relaxed));
            let left = ON_LEFT.load(Ordering::Relaxed);
            let moved = (px, py) != last;
            last = (px, py);
            let in_band = py >= win_y && py < win_y + h;
            let dist = if left { px - edge } else { edge - 1 - px };
            // a held button usually means dragging something: wait until it rests on the edge
            let dragging = sys::left_down();
            held = if dragging && in_band && (0..=1).contains(&dist) { held + 1 } else { 0 };
            if moved && in_band && (0..=1).contains(&dist) && (!dragging || held > 30) {
                sys::remember_foreground();
                OPEN.store(true, Ordering::Relaxed);
                let _ = proxy.send_event(Ev::Open);
                near = false;
            } else if in_band && dist >= 0 && dist < (160.0 * scale) as i32 {
                near = true;
                if proxy.send_event(Ev::Script(format!("app.cursor({:.1})", dist as f64 / scale))).is_err() {
                    return;
                }
            } else if near {
                near = false;
                let _ = proxy.send_event(Ev::Script("app.cursor(-1)".into()));
            }
        }
    });
}

fn log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(sys::data_dir().join("log.txt")) {
        let _ = writeln!(f, "{msg}");
    }
}

/// File picker runs on its own thread so the panel keeps animating.
fn add_app(proxy: EventLoopProxy<Ev>, folder: bool) {
    thread::spawn(move || {
        if let Some(path) = if folder { sys::pick_folder() } else { sys::pick_file() } {
            let item = json!({ "path": path, "name": util::display_name(&path), "icon": sys::icon_data_uri(&path) });
            let _ = proxy.send_event(Ev::Script(format!("app.appAdded({item})")));
            let _ = proxy.send_event(Ev::Show("settings"));
        }
    });
}

/* ---------------- "Add to Right Panel" from the file manager ---------------- */
/// A second launch (right-click → Add to Right Panel) drops the path here and exits;
/// the running copy picks it up.
fn inbox() -> std::path::PathBuf {
    sys::data_dir().join("inbox.txt")
}

fn queue_path(path: &str) {
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(inbox()) {
        let _ = writeln!(f, "{path}");
    }
}

fn spawn_inbox_watch(proxy: EventLoopProxy<Ev>) {
    thread::spawn(move || loop {
        let file = inbox();
        if let Ok(text) = fs::read_to_string(&file) {
            let _ = fs::remove_file(&file);
            for path in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let item = json!({ "path": path, "name": util::display_name(path), "icon": sys::icon_data_uri(path) });
                if proxy.send_event(Ev::Script(format!("app.appAdded({item})"))).is_err() {
                    return;
                }
            }
        }
        thread::sleep(Duration::from_millis(700));
    });
}

/* ---------------- updates ---------------- */
fn newer(latest: &str, current: &str) -> bool {
    let parts = |v: &str| -> Vec<u32> { v.trim_start_matches('v').split('.').map(|x| x.parse().unwrap_or(0)).collect() };
    parts(latest) > parts(current)
}

/// Asks GitHub for the newest release (through curl, which ships with every supported OS).
fn spawn_update_check(proxy: EventLoopProxy<Ev>, manual: bool) {
    thread::spawn(move || {
        let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
        let out = std::process::Command::new("curl")
            .args(["-sL", "--max-time", "15", "-H", "Accept: application/vnd.github+json", "-H", "User-Agent: RightPanel", &url])
            .output();
        let latest = out.ok().and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok()).and_then(|v| v["tag_name"].as_str().map(str::to_owned));
        let info = match latest {
            Some(tag) => json!({ "latest": tag, "newer": newer(&tag, VERSION), "manual": manual }),
            None => json!({ "latest": Value::Null, "newer": false, "manual": manual }),
        };
        let _ = proxy.send_event(Ev::Script(format!("app.updateInfo({info})")));
    });
}

/// Downloads the installer for this platform and hands over to it (the user confirms in the wizard).
fn install_update(proxy: EventLoopProxy<Ev>) {
    thread::spawn(move || {
        let page = format!("https://github.com/{REPO}/releases/latest");
        if !cfg!(windows) {
            sys::launch(&page); // macOS / Linux: packages are installed by the system tools
            return;
        }
        let file = std::env::temp_dir().join("RightPanel-Setup.exe");
        let url = format!("https://github.com/{REPO}/releases/latest/download/RightPanel-Setup.exe");
        let ok = std::process::Command::new("curl")
            .args(["-sL", "--max-time", "300", "-o", &file.display().to_string(), &url])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok && fs::metadata(&file).map(|m| m.len() > 1_000_000).unwrap_or(false) {
            let _ = std::process::Command::new(&file).spawn();
            let _ = proxy.send_event(Ev::Script("app.quitForUpdate()".into()));
        } else {
            sys::launch(&page);
            let _ = proxy.send_event(Ev::Script("app.toast('Could not download the update, opening the page')".into()));
        }
    });
}

/* ---------------- where the panel sits ---------------- */
struct Screen {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    name: String,
}

fn screens(event_loop: &tao::event_loop::EventLoop<Ev>) -> Vec<Screen> {
    event_loop
        .available_monitors()
        .enumerate()
        .map(|(i, m)| {
            let (p, s) = (m.position(), m.size());
            Screen { x: p.x, y: p.y, w: s.width as i32, h: s.height as i32, name: m.name().unwrap_or_else(|| format!("Monitor {}", i + 1)) }
        })
        .collect()
}

/// Moves the panel to the chosen edge of the chosen screen and tells the hover watcher about it.
fn place(window: &tao::window::Window, screens: &[Screen], idx: usize, left: bool, scale: f64) -> (i32, i32) {
    let s = screens.get(idx).or_else(|| screens.first()).expect("no monitor");
    let w = (WIN_W * scale) as i32;
    let h = ((WIN_H * scale) as i32).min(s.h);
    let x = if left { s.x } else { s.x + s.w - w };
    let y = s.y + (s.h - h) / 2;
    window.set_outer_position(PhysicalPosition::new(x, y));
    PANEL_X.store(x, Ordering::Relaxed);
    PANEL_Y.store(y, Ordering::Relaxed);
    PANEL_H.store(h, Ordering::Relaxed);
    EDGE_X.store(if left { s.x } else { s.x + s.w }, Ordering::Relaxed);
    ON_LEFT.store(left, Ordering::Relaxed);
    (w, h)
}

/* ---------------- click-through while closed ---------------- */
/// Closed panel lets clicks through to the apps below. On Linux we instead shrink the input
/// area to a thin strip at the screen edge: its mouse events open the panel even where the
/// compositor won't report the global pointer position (Wayland / XWayland).
#[cfg(not(target_os = "linux"))]
fn set_passthrough(window: &tao::window::Window, on: bool) {
    window.set_ignore_cursor_events(on).ok();
}

#[cfg(target_os = "linux")]
fn set_passthrough(window: &tao::window::Window, on: bool) {
    use gtk::prelude::*;
    use tao::platform::unix::WindowExtUnix;
    let gw = window.gtk_window();
    if let Some(gdk) = gw.window() {
        let (w, h) = (gw.allocated_width().max(1), gw.allocated_height().max(1));
        let strip = 4;
        let rect = if on {
            gtk::cairo::RectangleInt::new((w - strip).max(0), 0, strip, h)
        } else {
            gtk::cairo::RectangleInt::new(0, 0, w, h)
        };
        gdk.input_shape_combine_region(&gtk::cairo::Region::create_rectangle(&rect), 0, 0);
    }
}

/* ---------------- tray ---------------- */
mod tray {
    use super::{util, Ev};
    use tao::event_loop::EventLoopProxy;
    use tray_icon::{
        menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    pub struct Tray {
        _icon: TrayIcon,
        startup: CheckMenuItem,
    }

    impl Tray {
        pub fn new(proxy: &EventLoopProxy<Ev>, startup_on: bool) -> Option<Tray> {
            let login = if cfg!(windows) { "Start with Windows" } else { "Open at login" };
            let startup = CheckMenuItem::with_id("startup", login, true, startup_on, None);
            let menu = Menu::new();
            let _ = menu.append_items(&[
                &MenuItem::with_id("open", "Open Right Panel", true, None),
                &MenuItem::with_id("settings", "Widgets && settings…", true, None),
                &MenuItem::with_id("addapp", "Add app…", true, None),
                &PredefinedMenuItem::separator(),
                &startup,
                &PredefinedMenuItem::separator(),
                &MenuItem::with_id("quit", "Quit Right Panel", true, None),
            ]);
            let icon = TrayIconBuilder::new()
                .with_tooltip("Right Panel")
                .with_icon(Icon::from_rgba(util::tray_rgba(), 32, 32).ok()?)
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .build()
                .ok()?;
            let mp = proxy.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                let _ = mp.send_event(Ev::Menu(e.id.0));
            }));
            let tp = proxy.clone();
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    let _ = tp.send_event(Ev::Show(""));
                }
            }));
            Some(Tray { _icon: icon, startup })
        }

        pub fn set_startup(&self, on: bool) {
            self.startup.set_checked(on);
        }
    }
}


fn main() {
    // right-click → "Add to Right Panel" launches us with --add <path>
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(path) = args.iter().position(|a| a == "--add").and_then(|i| args.get(i + 1)) {
        queue_path(path);
    }
    // debug helper: write whatever image is on the clipboard to a PNG
    if let Some(out) = args.iter().position(|a| a == "--dump-clip-image").and_then(|i| args.get(i + 1)) {
        match sys::clip_image() {
            Some((w, h, px)) => {
                let _ = fs::write(out, util::png(w, h, &px));
                println!("{w}x{h} written to {out}");
            }
            None => println!("no image on the clipboard"),
        }
        return;
    }

    // lets the installer / a script turn the Explorer menu entry on or off
    if let Some(v) = args.iter().position(|a| a == "--context-menu").and_then(|i| args.get(i + 1)) {
        sys::set_context_menu(v == "on");
        return;
    }
    if !sys::single_instance() {
        return; // the copy already running picks the path up from the inbox
    }
    let _ = fs::remove_file(inbox()); // stale entries from a previous session

    #[cfg(target_os = "linux")]
    {
        // The panel needs edge placement + input shaping, which Wayland (xdg-shell) forbids, so it
        // runs through XWayland. RIGHT_PANEL_BACKEND=wayland opts out (for testing on compositors
        // where that works, or systems without XWayland).
        let want = std::env::var("RIGHT_PANEL_BACKEND").unwrap_or_else(|_| "x11".into());
        unsafe { std::env::set_var("GDK_BACKEND", &want) };
    }
    let dir = sys::data_dir();
    let settings_path = dir.join("settings.json");
    let notes_path = dir.join("notes.txt");

    let mut settings: Value = fs::read_to_string(&settings_path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}));
    settings["startup"] = json!(sys::startup_enabled());
    let notes_json = dir.join("notes.json");
    let notes: Value = fs::read_to_string(&notes_json)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| match fs::read_to_string(&notes_path) {
            // carry the single note from older versions into the new list
            Ok(old) if !old.trim().is_empty() => json!([{ "id": "note-1", "title": "Note", "text": old }]),
            _ => json!([]),
        });

    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory); // no Dock icon
    }
    let proxy = event_loop.create_proxy();

    let monitor = event_loop.primary_monitor().or_else(|| event_loop.available_monitors().next()).expect("no monitor");
    let scale = monitor.scale_factor();
    sys::set_scale(scale);
    let all_screens = screens(&event_loop);
    let on_left = settings["edge"] == "left";
    let mut screen_idx = settings["monitor"].as_u64().unwrap_or(0) as usize;
    if screen_idx >= all_screens.len() {
        screen_idx = 0;
    }
    let home = all_screens.get(screen_idx).or_else(|| all_screens.first()).expect("no monitor");
    let (w, h) = ((WIN_W * scale) as i32, ((WIN_H * scale) as i32).min(home.h));
    let x = if on_left { home.x } else { home.x + home.w - w };
    let y = home.y + (home.h - h) / 2;

    let builder = WindowBuilder::new()
        .with_title("Right Panel")
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top(true)
        .with_resizable(false)
        .with_inner_size(PhysicalSize::new(w, h))
        .with_position(PhysicalPosition::new(x, y));
    #[cfg(windows)]
    let builder = {
        use tao::platform::windows::WindowBuilderExtWindows;
        builder.with_skip_taskbar(true).with_undecorated_shadow(false)
    };
    #[cfg(target_os = "linux")]
    let builder = {
        use tao::platform::unix::WindowBuilderExtUnix;
        builder.with_skip_taskbar(true).with_visible(false)
    };
    let window = builder.build(&event_loop).expect("window");
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::*;
        use tao::platform::unix::WindowExtUnix;
        let gw = window.gtk_window();
        gw.set_type_hint(gtk::gdk::WindowTypeHint::Dock);
        gw.set_keep_above(true);
        gw.set_accept_focus(true); // dock-type windows still need the keyboard for search / notes
        gw.move_(x, y);
    }
    #[cfg(not(target_os = "linux"))]
    set_passthrough(&window, true);
    #[cfg(windows)]
    {
        use tao::platform::windows::WindowExtWindows;
        sys::set_self_window(window.hwnd() as isize);
    }

    let mut ctx = WebContext::new(Some(dir.join("webview")));
    let init = format!(
        "window.onerror=(m,s,l)=>window.ipc.postMessage(JSON.stringify({{t:\"log\",text:m+\" @\"+l}}));window.__init = {};",
        json!({ "settings": settings, "notes": notes, "platform": sys::PLATFORM, "home": sys::home(),
                "version": VERSION, "contextMenu": sys::context_menu_enabled(),
                "screens": all_screens.iter().map(|s| json!({ "name": s.name, "w": s.w, "h": s.h })).collect::<Vec<_>>() })
    );
    let ipc_proxy = proxy.clone();
    let drop_proxy = proxy.clone();
    let wv = WebViewBuilder::new_with_web_context(&mut ctx)
        // dropping a file or folder on the panel pins it (this replaces WebView2's own
        // drag handling on Windows, so the UI reorders with pointer events instead)
        .with_drag_drop_handler(move |e| {
            match e {
                wry::DragDropEvent::Enter { .. } => {
                    let _ = drop_proxy.send_event(Ev::Script("app.dropHint(true)".into()));
                }
                wry::DragDropEvent::Leave => {
                    let _ = drop_proxy.send_event(Ev::Script("app.dropHint(false)".into()));
                }
                wry::DragDropEvent::Drop { paths, .. } => {
                    let _ = drop_proxy.send_event(Ev::Script("app.dropHint(false)".into()));
                    for path in paths {
                        let p = path.display().to_string();
                        let item = json!({ "path": p, "name": util::display_name(&p), "icon": sys::icon_data_uri(&p) });
                        let _ = drop_proxy.send_event(Ev::Script(format!("app.appAdded({item})")));
                    }
                }
                _ => {}
            }
            true
        })
        .with_transparent(true)
        .with_background_color((0, 0, 0, 0))
        .with_initialization_script(&init)
        .with_html(UI)
        .with_ipc_handler(move |req| {
            let _ = ipc_proxy.send_event(Ev::Ipc(req.body().clone()));
        });
    #[cfg(not(target_os = "linux"))]
    let webview = wv.build(&window).expect("webview");
    #[cfg(target_os = "linux")]
    let webview = {
        use gtk::prelude::*;
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let wv = wv.build_gtk(window.default_vbox().expect("gtk box")).expect("webview");
        let gw = window.gtk_window();
        gw.show_all();
        gw.move_(x, y);
        set_passthrough(&window, true);
        wv
    };

    spawn_clip_watch(proxy.clone());
    spawn_inbox_watch(proxy.clone());
    restore_pins(proxy.clone());
    place(&window, &all_screens, screen_idx, on_left, scale);
    spawn_edge_watch(proxy.clone(), scale);
    let mut tray: Option<tray::Tray> = None;

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            // macOS needs the tray created once the app is running
            Event::NewEvents(StartCause::Init) => {
                tray = tray::Tray::new(&proxy, sys::startup_enabled());
                if !OPEN.load(Ordering::Relaxed) {
                    set_passthrough(&window, true);
                }
            }
            Event::UserEvent(Ev::Script(js)) => {
                let _ = webview.evaluate_script(&js);
            }
            Event::UserEvent(Ev::Open) => {
                set_passthrough(&window, false);
                let _ = webview.evaluate_script("app.open()");
            }
            Event::UserEvent(Ev::Show(panel)) => {
                sys::remember_foreground();
                OPEN.store(true, Ordering::Relaxed);
                set_passthrough(&window, false);
                let _ = webview.evaluate_script(&format!("app.show('{panel}')"));
            }
            Event::UserEvent(Ev::Menu(id)) => match id.as_str() {
                "open" => {
                    let _ = proxy.send_event(Ev::Show(""));
                }
                "settings" => {
                    let _ = proxy.send_event(Ev::Show("settings"));
                }
                "addapp" => add_app(proxy.clone(), false),
                "startup" => {
                    let on = !sys::startup_enabled();
                    sys::set_startup(on);
                    if let Some(t) = &tray {
                        t.set_startup(on);
                    }
                    let _ = webview.evaluate_script(&format!("app.syncStartup({on})"));
                }
                "quit" => *flow = ControlFlow::Exit,
                _ => {}
            },
            Event::UserEvent(Ev::Ipc(body)) => {
                let Ok(m) = serde_json::from_str::<Value>(&body) else { return };
                let s = |k: &str| m[k].as_str().unwrap_or_default().to_string();
                match m["t"].as_str().unwrap_or_default() {
                    "edge" => {
                        if !OPEN.swap(true, Ordering::Relaxed) {
                            sys::remember_foreground();
                            set_passthrough(&window, false);
                            let _ = webview.evaluate_script("app.open()");
                        }
                    }
                    "pass" => {
                        set_passthrough(&window, true);
                        OPEN.store(false, Ordering::Relaxed);
                    }
                    "copy" => set_clip(&s("text")),
                    "paste" => {
                        set_clip(&s("text"));
                        sys::paste_into_previous();
                    }
                    "transform" => {
                        if let Some(t) = get_clip() {
                            set_clip(&transform(&t, &s("mode")));
                            if m["paste"].as_bool().unwrap_or(false) {
                                sys::paste_into_previous();
                            }
                        }
                    }
                    "note" => {
                        let _ = fs::write(&notes_json, m["notes"].to_string());
                    }
                    "settings" => {
                        let st = &m["settings"];
                        let want = st["startup"].as_bool().unwrap_or(false);
                        if want != sys::startup_enabled() {
                            sys::set_startup(want);
                        }
                        if let Some(t) = &tray {
                            t.set_startup(want);
                        }
                        let _ = fs::write(&settings_path, st.to_string());
                    }
                    "screenshot" => sys::screenshot(),
                    "pickColor" => {
                        if !PICKING.swap(true, Ordering::Relaxed) {
                            let p = proxy.clone();
                            sys::pick_color(move |hex| {
                                if let Some(hex) = hex {
                                    set_clip(&hex);
                                    OPEN.store(true, Ordering::Relaxed);
                                    let _ = p.send_event(Ev::Open);
                                    let _ = p.send_event(Ev::Script(format!("app.picked('{hex}')")));
                                }
                                PICKING.store(false, Ordering::Relaxed);
                            });
                        }
                    }
                    "lock" => sys::lock(),
                    "alarm" => {
                        sys::beep();
                        OPEN.store(true, Ordering::Relaxed);
                        set_passthrough(&window, false);
                    }
                    "log" => log(&s("text")),
                    "addApp" => add_app(proxy.clone(), m["folder"].as_bool().unwrap_or(false)),
                    "launch" | "open" => sys::launch(&s(if m["t"] == "open" { "target" } else { "path" })),
                    "key" => sys::press(&s("name")),
                    "screenoff" => sys::screen_off(),
                    "pinwin" => {
                        let msg = sys::toggle_topmost();
                        let _ = webview.evaluate_script(&format!("app.toast({})", json!(msg)));
                    }
                    "awake" => sys::keep_awake(m["on"].as_bool().unwrap_or(false)),
                    "refreshIcon" => {
                        let (id, path, p2) = (s("id"), s("path"), proxy.clone());
                        thread::spawn(move || {
                            if let Some(icon) = sys::icon_data_uri(&path) {
                                let _ = p2.send_event(Ev::Script(format!("app.iconFor({},{})", json!(id), json!(icon))));
                            }
                        });
                    }
                    "contextMenu" => sys::set_context_menu(m["on"].as_bool().unwrap_or(false)),
                    "placement" => {
                        let left = m["edge"] == "left";
                        let idx = m["monitor"].as_u64().unwrap_or(0) as usize;
                        place(&window, &all_screens, idx, left, scale);
                        let _ = webview.evaluate_script(&format!("app.placed({})", json!({ "edge": if left { "left" } else { "right" } })));
                    }
                    "pinImage" => {
                        let id: u64 = s("id").parse().unwrap_or(0);
                        let on = m["on"].as_bool().unwrap_or(false);
                        let ok = pin_image(id, on);
                        let msg = if !ok { "Could not pin that image" } else if on { "Image pinned" } else { "Image unpinned" };
                        let _ = webview.evaluate_script(&format!("app.toast({})", json!(msg)));
                    }
                    "clipImage" => {
                        let id: u64 = s("id").parse().unwrap_or(0);
                        let found = IMAGES
                            .lock()
                            .ok()
                            .and_then(|v| v.iter().find(|(i, ..)| *i == id).cloned())
                            .or_else(|| PINNED.lock().ok().and_then(|v| v.iter().find(|(i, ..)| *i == id).cloned()));
                        let ok = found.map(|(_, w, h, px)| sys::set_clip_image(w, h, &px)).unwrap_or(false);
                        LAST_IMAGE.store(id, Ordering::Relaxed); // don't re-announce our own copy
                        let _ = webview.evaluate_script(if ok { "app.toast('Image copied')" } else { "app.toast('Could not copy that image')" });
                    }
                    "checkUpdate" => spawn_update_check(proxy.clone(), m["manual"].as_bool().unwrap_or(false)),
                    "installUpdate" => install_update(proxy.clone()),
                    "quit" => *flow = ControlFlow::Exit,
                    _ => {}
                }
            }
            Event::WindowEvent { event: WindowEvent::Focused(false), .. } => {
                let _ = webview.evaluate_script("app.blur()");
            }
            _ => {}
        }
    });
}
