#![windows_subsystem = "windows"]

//! Right Panel — a liquid side panel that lives on the right edge of the screen.

use std::{
    collections::BTreeMap,
    fs,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

mod plugins;
mod sys;
mod util;

use serde_json::{Value, json};
use tao::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::WindowBuilder,
};
use wry::{WebContext, WebViewBuilder};

const UI: &str = include_str!("ui.html");
const WIN_W: f64 = 480.0;
const WIN_H: f64 = 680.0;

static OPEN: AtomicBool = AtomicBool::new(false);
static PICKING: AtomicBool = AtomicBool::new(false);

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
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
        "sentence" => {
            let lower = text.to_lowercase();
            let mut c = lower.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        }
        "trim" => text
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        "oneline" | "plain" => text.split_whitespace().collect::<Vec<_>>().join(" "),
        "reverse" => text.chars().rev().collect(),
        "slug" => {
            let s: String = text
                .to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '-' })
                .collect();
            s.split('-')
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("-")
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
                if let Some(t) = get_clip()
                    && t.len() < 100_000
                    && proxy
                        .send_event(Ev::Script(format!("app.clip({})", json!(t))))
                        .is_err()
                {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(if cfg!(windows) { 400 } else { 800 }));
        }
    });
}

/* ---------------- edge hover detection ---------------- */
fn spawn_edge_watch(
    proxy: EventLoopProxy<Ev>,
    win_x: i32,
    win_y: i32,
    scale: f64,
    edge: i32,
    h: i32,
) {
    thread::spawn(move || {
        let mut near = false;
        let mut last = (i32::MIN, i32::MIN);
        loop {
            thread::sleep(Duration::from_millis(12));
            if OPEN.load(Ordering::Relaxed) || PICKING.load(Ordering::Relaxed) {
                near = false;
                continue;
            }
            let Some((px, py)) = sys::cursor_pos() else {
                continue;
            };
            let moved = (px, py) != last;
            last = (px, py);
            let in_band = py >= win_y && py < win_y + h;
            let dist = edge - 1 - px;
            if moved && in_band && (0..=1).contains(&dist) && !sys::left_down() {
                sys::remember_foreground();
                OPEN.store(true, Ordering::Relaxed);
                let _ = proxy.send_event(Ev::Open);
                near = false;
            } else if in_band && dist >= 0 && dist < (160.0 * scale) as i32 {
                near = true;
                let lx = (px - win_x) as f64 / scale;
                let ly = (py - win_y) as f64 / scale;
                if proxy
                    .send_event(Ev::Script(format!("app.cursor({lx:.1},{ly:.1})")))
                    .is_err()
                {
                    return;
                }
            } else if near {
                near = false;
                let _ = proxy.send_event(Ev::Script("app.cursor(-1,0)".into()));
            }
        }
    });
}

fn log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(sys::data_dir().join("log.txt"))
    {
        let _ = writeln!(f, "{msg}");
    }
}

/// File picker runs on its own thread so the panel keeps animating.
fn add_app(proxy: EventLoopProxy<Ev>) {
    thread::spawn(move || {
        if let Some(path) = sys::pick_file() {
            let item = json!({ "path": path, "name": util::display_name(&path), "icon": sys::icon_data_uri(&path) });
            let _ = proxy.send_event(Ev::Script(format!("app.appAdded({item})")));
            let _ = proxy.send_event(Ev::Show("settings"));
        }
    });
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
        let rect = if on {
            gtk::cairo::RectangleInt::new(w - 3, 0, 3, h)
        } else {
            gtk::cairo::RectangleInt::new(0, 0, w, h)
        };
        gdk.input_shape_combine_region(&gtk::cairo::Region::create_rectangle(&rect), 0, 0);
    }
}

/* ---------------- tray ---------------- */
mod tray {
    use super::{Ev, util};
    use tao::event_loop::EventLoopProxy;
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };

    pub struct Tray {
        _icon: TrayIcon,
        startup: CheckMenuItem,
    }

    impl Tray {
        pub fn new(proxy: &EventLoopProxy<Ev>, startup_on: bool) -> Option<Tray> {
            let login = if cfg!(windows) {
                "Start with Windows"
            } else {
                "Open at login"
            };
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
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = e
                {
                    let _ = tp.send_event(Ev::Show(""));
                }
            }));
            Some(Tray {
                _icon: icon,
                startup,
            })
        }

        pub fn set_startup(&self, on: bool) {
            self.startup.set_checked(on);
        }
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
        && std::env::var_os("GDK_BACKEND").is_none()
    {
        // Wayland won't let an app position itself or read the pointer; XWayland does.
        unsafe { std::env::set_var("GDK_BACKEND", "x11") };
    }
    let dir = sys::data_dir();
    let settings_path = dir.join("settings.json");
    let notes_path = dir.join("notes.txt");

    let mut settings: Value = fs::read_to_string(&settings_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    settings["startup"] = json!(sys::startup_enabled());
    let notes = fs::read_to_string(&notes_path).unwrap_or_default();
    let enabled: BTreeMap<String, bool> = settings["plugins"]["enabled"]
        .as_object()
        .map(|v| {
            v.iter()
                .filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), b)))
                .collect()
        })
        .unwrap_or_default();
    let mut plugins = plugins::Manager::load(dir.clone(), &enabled);

    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory); // no Dock icon
    }
    let proxy = event_loop.create_proxy();

    let monitor = event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next())
        .expect("no monitor");
    let scale = monitor.scale_factor();
    sys::set_scale(scale);
    let (mpos, msize) = (monitor.position(), monitor.size());
    let (w, h) = (
        (WIN_W * scale) as i32,
        (WIN_H * scale).min(msize.height as f64) as i32,
    );
    let x = mpos.x + msize.width as i32 - w;
    let y = mpos.y + (msize.height as i32 - h) / 2;

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
        builder
            .with_skip_taskbar(true)
            .with_undecorated_shadow(false)
    };
    #[cfg(target_os = "linux")]
    let builder = {
        use tao::platform::unix::WindowBuilderExtUnix;
        builder.with_skip_taskbar(true)
    };
    let window = builder.build(&event_loop).expect("window");
    set_passthrough(&window, true);
    #[cfg(windows)]
    {
        use tao::platform::windows::WindowExtWindows;
        sys::set_self_window(window.hwnd());
    }

    let mut ctx = WebContext::new(Some(dir.join("webview")));
    let init = format!(
        "window.onerror=(m,s,l)=>window.ipc.postMessage(JSON.stringify({{t:\"log\",text:m+\" @\"+l}}));window.__init = {};",
        json!({ "settings": settings, "notes": notes, "platform": sys::PLATFORM, "home": sys::home(), "plugins": plugins.infos() })
    );
    let ipc_proxy = proxy.clone();
    let wv = WebViewBuilder::new_with_web_context(&mut ctx)
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
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        wv.build_gtk(window.default_vbox().expect("gtk box"))
            .expect("webview")
    };

    spawn_clip_watch(proxy.clone());
    spawn_edge_watch(proxy.clone(), x, y, scale, mpos.x + msize.width as i32, h);
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
                "addapp" => add_app(proxy.clone()),
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
                        let _ = fs::write(&notes_path, s("text"));
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
                    "pluginInstall" => {
                        let p = proxy.clone();
                        let data = dir.clone();
                        let current_enabled: BTreeMap<String, bool> = settings["plugins"]["enabled"].as_object().map(|v| v.iter().filter_map(|(k,v)| v.as_bool().map(|b|(k.clone(),b))).collect()).unwrap_or_default();
                        thread::spawn(move || if let Some(path) = sys::pick_plugin_package() {
                            // The manager is reloaded in the UI event path; this thread only reports the selected path.
                            let _ = p.send_event(Ev::Ipc(json!({"t":"pluginInstallPath","path":path,"data":data,"enabled":current_enabled}).to_string()));
                        });
                    }
                    "pluginInstallPath" => {
                        match plugins.install(std::path::Path::new(&s("path")), &enabled) {
                            Ok(id) => { let _ = webview.evaluate_script(&format!("app.pluginsUpdated({}, {})", json!(plugins.infos()), json!(format!("Installed {id}")))); }
                            Err(e) => { let _ = webview.evaluate_script(&format!("app.toast({})", json!(format!("Plugin install failed: {e}")))); }
                        }
                    }
                    "pluginUninstall" => {
                        let id = s("id"); match plugins.uninstall(&id) {
                            Ok(()) => { settings["plugins"]["enabled"][&id] = Value::Null; let _ = fs::write(&settings_path, settings.to_string()); let _ = webview.evaluate_script(&format!("app.pluginsUpdated({}, {})", json!(plugins.infos()), json!("Plugin uninstalled"))); }
                            Err(e) => { let _ = webview.evaluate_script(&format!("app.toast({})", json!(e))); }
                        }
                    }
                    "pluginReload" => {
                        plugins = plugins::Manager::load(dir.clone(), &enabled);
                        let _ = webview.evaluate_script(&format!("app.pluginsUpdated({}, {})", json!(plugins.infos()), json!("Plugins reloaded")));
                    }
                    "pluginBridge" => {
                        let id = s("id"); let request = s("request"); let op = s("op"); let args = &m["args"];
                        let allowed = |p: plugins::Permission| plugins.permissions(&id).is_some_and(|ps| ps.contains(&p));
                        let answer: Result<Value, String> = match op.as_str() {
                            "storage.get" => if allowed(plugins::Permission::Storage) { plugins.storage(&id, "get", args["key"].as_str(), None) } else { Err("storage permission denied".into()) },
                            "storage.set" => if allowed(plugins::Permission::Storage) { plugins.storage(&id, "set", args["key"].as_str(), Some(args["value"].clone())) } else { Err("storage permission denied".into()) },
                            "storage.remove" => if allowed(plugins::Permission::Storage) { plugins.storage(&id, "remove", args["key"].as_str(), None) } else { Err("storage permission denied".into()) },
                            "storage.clear" => if allowed(plugins::Permission::Storage) { plugins.storage(&id, "clear", Some("_"), None) } else { Err("storage permission denied".into()) },
                            "clipboard.read" => if allowed(plugins::Permission::ClipboardRead) { Ok(json!(get_clip().unwrap_or_default())) } else { Err("clipboard.read permission denied".into()) },
                            "clipboard.write" => if allowed(plugins::Permission::ClipboardWrite) { set_clip(args["text"].as_str().unwrap_or_default()); Ok(Value::Null) } else { Err("clipboard.write permission denied".into()) },
                            "system.openUrl" => { let u=args["url"].as_str().unwrap_or_default(); if !allowed(plugins::Permission::SystemOpenUrl) { Err("system.openUrl permission denied".into()) } else if u.starts_with("https://") || u.starts_with("http://") { sys::launch(u); Ok(Value::Null) } else { Err("only http(s) URLs are allowed".into()) } },
                            "ui.close" => { let _=proxy.send_event(Ev::Ipc(json!({"t":"pass"}).to_string())); Ok(Value::Null) },
                            "ui.toast" => { let _=webview.evaluate_script(&format!("app.toast({})", json!(args["message"].as_str().unwrap_or("Plugin")))); Ok(Value::Null) },
                            _ => Err("unknown plugin API operation".into()),
                        };
                        let msg = match answer { Ok(value) => json!({"id":id,"request":request,"ok":true,"value":value}), Err(error)=>json!({"id":id,"request":request,"ok":false,"error":error}) };
                        let _ = webview.evaluate_script(&format!("app.pluginReply({})", msg));
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
                    "addApp" => add_app(proxy.clone()),
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
