#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
mod accounts;
#[cfg(windows)]
mod anim;
#[cfg(windows)]
mod app;
#[cfg(windows)]
mod backup;
#[cfg(windows)]
mod codex;
#[cfg(windows)]
mod gfx;
#[cfg(windows)]
mod i18n;
#[cfg(windows)]
mod polling;
#[cfg(windows)]
mod profiles;
#[cfg(windows)]
mod sessions;
#[cfg(windows)]
mod system;
#[cfg(windows)]
mod theme;
#[cfg(windows)]
mod ui;
#[cfg(windows)]
mod usage;
#[cfg(target_os = "linux")]
fn main() {
    use std::process::{Command, Stdio};
    let bootstrap = r#"import sys,json,types
p=json.load(sys.stdin)
backend=types.ModuleType('backend')
sys.modules['backend']=backend
MUISLAND_HTML=p['html']
MUISLAND_VERSION=p['version']
try:
    exec(p['backend'],backend.__dict__)
    exec(p['app'])
except ImportError:
    print('Muisland needs system Python 3, PyGObject/Cairo, GTK 3, WebKitGTK and cryptography. Install the Linux runtime dependencies in docs/LINUX.md.', file=sys.stderr)
    sys.exit(1)
"#;
    let mut child = match Command::new("/usr/bin/python3")
        .arg("-c")
        .arg(bootstrap)
        .args(std::env::args().skip(1))
        .stdin(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            eprintln!("Install Python 3 and the Linux dependencies listed in README.");
            std::process::exit(1)
        }
    };
    let payload = serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"backend":include_str!("../linux/backend.py"),"html":include_str!("../linux/island.html"),"app":include_str!("../linux/app.py")});
    let result = serde_json::to_writer(child.stdin.take().unwrap(), &payload);
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        eprintln!("Could not initialize the Linux UI runtime.");
        std::process::exit(1)
    }
    std::process::exit(child.wait().ok().and_then(|s| s.code()).unwrap_or(1));
}
#[cfg(not(any(windows, target_os = "linux")))]
fn main() {
    eprintln!("Muisland supports Windows and Linux.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if std::env::args().any(|a| a == "--check-codex-usage") {
        match codex::fetch_current() {
            Ok(u) => println!("{}", u.to_cache()),
            Err(e) => {
                eprintln!("Codex usage: {e}");
                std::process::exit(1)
            }
        }
        return;
    }
    if std::env::args().any(|a| a == "--check-usage") {
        match usage::fetch_current() {
            Ok(u) => println!("{}", u.to_cache()),
            Err(e) => {
                eprintln!("Usage check: {e}");
                std::process::exit(1)
            }
        }
        return;
    }
    if std::env::args().any(|a| a == "--render-motion") {
        let folder = std::env::args().nth(2).unwrap_or_else(|| "motion".into());
        if let Err(e) = app::export_motion(std::path::Path::new(&folder)) {
            eprintln!("Motion render: {e}");
            std::process::exit(1)
        }
        return;
    }
    if std::env::args().any(|a| a == "--render-previews") {
        let folder = std::env::args().nth(2).unwrap_or_else(|| "preview".into());
        if let Err(e) = app::export_previews(std::path::Path::new(&folder)) {
            eprintln!("Preview render: {e}");
            std::process::exit(1)
        }
        return;
    }
    app::run();
}
