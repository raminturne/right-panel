//! Linux + macOS implementation (beta). Uses standard system tools where they exist;
//! anything missing simply does nothing. Windows-only widgets are hidden by the UI.

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

pub const PLATFORM: &str = if cfg!(target_os = "macos") {
    "macos"
} else {
    "linux"
};

static SCALE: AtomicU64 = AtomicU64::new(0x3FF0_0000_0000_0000); // 1.0
static AWAKE: Mutex<Option<Child>> = Mutex::new(None);

fn run(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

fn output(cmd: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(cmd)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (o.status.success() && !s.is_empty()).then_some(s)
}

pub fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

pub fn data_dir() -> PathBuf {
    let dir = if cfg!(target_os = "macos") {
        PathBuf::from(home()).join("Library/Application Support/RightPanel")
    } else {
        std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(home()).join(".config"))
            .join("right-panel")
    };
    let _ = fs::create_dir_all(&dir);
    dir
}

/// macOS reports the cursor in points; the window is placed in physical pixels.
pub fn set_scale(s: f64) {
    SCALE.store(s.to_bits(), Ordering::Relaxed);
}

/* ---------------- startup at login ---------------- */
fn autostart_file() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from(home()).join("Library/LaunchAgents/app.rightpanel.plist")
    } else {
        PathBuf::from(home()).join(".config/autostart/right-panel.desktop")
    }
}

pub fn startup_enabled() -> bool {
    autostart_file().exists()
}

pub fn set_startup(on: bool) {
    let f = autostart_file();
    if !on {
        let _ = fs::remove_file(f);
        return;
    }
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let body = if cfg!(target_os = "macos") {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict><key>Label</key><string>app.rightpanel</string><key>ProgramArguments</key><array><string>{exe}</string></array>\
             <key>RunAtLoad</key><true/></dict></plist>\n"
        )
    } else {
        format!(
            "[Desktop Entry]\nType=Application\nName=Right Panel\nExec=\"{exe}\"\nX-GNOME-Autostart-enabled=true\n"
        )
    };
    if let Some(dir) = f.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(f, body);
}

/* ---------------- pointer ---------------- */
pub fn cursor_pos() -> Option<(i32, i32)> {
    use mouse_position::mouse_position::Mouse;
    match Mouse::get_mouse_position() {
        Mouse::Position { x, y } => {
            let s = if cfg!(target_os = "macos") {
                f64::from_bits(SCALE.load(Ordering::Relaxed))
            } else {
                1.0
            };
            Some(((x as f64 * s) as i32, (y as f64 * s) as i32))
        }
        Mouse::Error => None,
    }
}

pub fn left_down() -> bool {
    false
}

pub fn remember_foreground() {}

pub fn clip_seq() -> u64 {
    let mut h = DefaultHasher::new();
    crate::get_clip().hash(&mut h);
    h.finish()
}

/// Auto-paste needs OS permissions here, so items are copied instead (UI switches to copy mode).
pub fn paste_into_previous() {}

/* ---------------- media ---------------- */
pub fn press(name: &str) {
    if cfg!(target_os = "macos") {
        let player = |cmd: &str| {
            format!(
                "if application \"Spotify\" is running then tell application \"Spotify\" to {cmd}\n\
                 if application \"Music\" is running then tell application \"Music\" to {cmd}"
            )
        };
        let script = match name {
            "play" => player("playpause"),
            "next" => player("next track"),
            "prev" => player("previous track"),
            "volup" => {
                "set volume output volume ((output volume of (get volume settings)) + 6)".into()
            }
            "voldown" => {
                "set volume output volume ((output volume of (get volume settings)) - 6)".into()
            }
            "mute" => "set volume output muted not (output muted of (get volume settings))".into(),
            _ => return,
        };
        run("osascript", &["-e", &script]);
    } else {
        match name {
            "play" => run("playerctl", &["play-pause"]),
            "next" => run("playerctl", &["next"]),
            "prev" => run("playerctl", &["previous"]),
            "mute" => run("pactl", &["set-sink-mute", "@DEFAULT_SINK@", "toggle"]),
            "volup" => run("pactl", &["set-sink-volume", "@DEFAULT_SINK@", "+5%"]),
            "voldown" => run("pactl", &["set-sink-volume", "@DEFAULT_SINK@", "-5%"]),
            _ => false,
        };
    }
}

/* ---------------- system actions ---------------- */
pub fn toggle_topmost() -> String {
    "Pin window is only available on Windows".into()
}

pub fn screen_off() {
    if cfg!(target_os = "macos") {
        run("pmset", &["displaysleepnow"]);
    } else {
        run("sh", &["-c", "sleep 0.6; xset dpms force off"]);
    }
}

pub fn lock() {
    if cfg!(target_os = "macos") {
        run("pmset", &["displaysleepnow"]);
    } else if !run("loginctl", &["lock-session"]) {
        run("xdg-screensaver", &["lock"]);
    }
}

pub fn beep() {}

pub fn keep_awake(on: bool) {
    let mut g = AWAKE.lock().unwrap();
    if let Some(mut c) = g.take() {
        let _ = c.kill();
    }
    if on {
        *g = if cfg!(target_os = "macos") {
            Command::new("caffeinate").arg("-di").spawn().ok()
        } else {
            Command::new("systemd-inhibit")
                .args([
                    "--what=idle:sleep",
                    "--why=Right Panel keep awake",
                    "sleep",
                    "infinity",
                ])
                .spawn()
                .ok()
        };
    }
}

pub fn screenshot() {
    if cfg!(target_os = "macos") {
        run("screencapture", &["-ic"]);
    } else {
        // whichever screenshot tool the desktop has
        run(
            "sh",
            &[
                "-c",
                "sleep 0.35; spectacle -r || gnome-screenshot -a || flameshot gui || xfce4-screenshooter -r",
            ],
        );
    }
}

pub fn pick_color(done: impl FnOnce(Option<String>) + Send + 'static) {
    done(None);
}

/* ---------------- apps ---------------- */
pub fn pick_file() -> Option<String> {
    if cfg!(target_os = "macos") {
        output(
            "osascript",
            &[
                "-e",
                "POSIX path of (path to (choose application with prompt \"Add app to Right Panel\"))",
            ],
        )
    } else {
        output(
            "zenity",
            &[
                "--file-selection",
                "--title=Add app to Right Panel",
                "--filename=/usr/share/applications/",
            ],
        )
        .or_else(|| output("kdialog", &["--getopenfilename", "/usr/share/applications"]))
    }
}

pub fn pick_plugin_package() -> Option<String> {
    output(
        "zenity",
        &[
            "--file-selection",
            "--title=Install Right Panel plugin",
            "--file-filter=Right Panel plugin | *.rpp *.zip",
        ],
    )
    .or_else(|| {
        output(
            "kdialog",
            &[
                "--getopenfilename",
                ".",
                "Right Panel plugins (*.rpp *.zip)",
            ],
        )
    })
}

pub fn launch(path: &str) {
    if cfg!(target_os = "macos") {
        let target = match path {
            "taskmgr" => "/System/Applications/Utilities/Activity Monitor.app",
            "ms-settings:" => "x-apple.systempreferences:",
            p => p,
        };
        run("open", &[target]);
    } else if path.ends_with(".desktop") {
        let stem = Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !run("gtk-launch", &[&stem]) {
            run("dex", &[path]);
        }
    } else {
        match path {
            "taskmgr" => run(
                "sh",
                &[
                    "-c",
                    "gnome-system-monitor || plasma-systemmonitor || xfce4-taskmanager",
                ],
            ),
            "ms-settings:" => run(
                "sh",
                &[
                    "-c",
                    "gnome-control-center || systemsettings || xfce4-settings-manager",
                ],
            ),
            p => run("xdg-open", &[p]),
        };
    }
}

pub fn icon_data_uri(_: &str) -> Option<String> {
    None
}
