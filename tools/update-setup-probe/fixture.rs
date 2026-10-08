//! Tiny disposable client; no application data, networking or updater implementation.
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::{
    fs,
    time::{Duration, Instant},
};

fn main() {
    if let Err(code) = run() {
        std::process::exit(code);
    }
}
fn run() -> Result<(), i32> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg.starts_with("--veloapp-")) {
        // The installer owns exclusion during this trusted, bounded hook.
        if let Ok(folder) = std::env::var("NOH_SETUP_PROBE_HOOK") {
            let folder = std::path::Path::new(&folder);
            fs::write(folder.join("ready.tmp"), std::process::id().to_string()).map_err(|_| 2)?;
            fs::rename(folder.join("ready.tmp"), folder.join("ready")).map_err(|_| 2)?;
            let start = Instant::now();
            while !folder.join("release").is_file() {
                if start.elapsed() > Duration::from_secs(15) {
                    return Err(3);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        return Ok(());
    }
    let anchor = std::path::Path::new(env!("NOH_SETUP_PROBE_ANCHOR"));
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    options.share_mode(3);
    let _lease = options
        .open(anchor.join(".noh-update-runtime.lock"))
        .map_err(|e| if e.raw_os_error() == Some(32) { 32 } else { 4 })?;
    if args.first().map(String::as_str) == Some("--report") && args.len() == 2 {
        use std::io::Write;
        let version = option_env!("NOH_SETUP_PROBE_VERSION").unwrap_or("1.0.0");
        let value = format!(
            "{{\"version\":\"{version}\",\"pid\":{}}}",
            std::process::id()
        );
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[1])
            .and_then(|mut file| file.write_all(value.as_bytes()))
            .map_err(|_| 7)?;
    }
    if args.first().map(String::as_str) == Some("--hold") && args.len() == 3 {
        fs::write(&args[1], b"ready").map_err(|_| 5)?;
        let start = Instant::now();
        while !std::path::Path::new(&args[2]).exists() {
            if start.elapsed() > Duration::from_secs(30) {
                return Err(6);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    Ok(())
}
