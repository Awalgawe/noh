//! An opt-in timeline of what the preview does, to find out on a user's machine
//! why something is slow. `NOH_PROFILE=<file>` appends one line per event:
//! milliseconds since the first event, the thread, the event. Without that
//! variable nothing is opened and a call costs the read of one flag.
use std::{
    fs::File,
    io::Write,
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

struct Sink {
    file: Mutex<File>,
    began: Instant,
}
fn sink() -> Option<&'static Sink> {
    static SINK: OnceLock<Option<Sink>> = OnceLock::new();
    SINK.get_or_init(|| {
        let path = std::env::var_os("NOH_PROFILE").filter(|p| !p.is_empty())?;
        let mut file = File::options().create(true).append(true).open(path).ok()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let _ = writeln!(
            file,
            "# NOH {} profile, process {}, unix time {now}",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        );
        Some(Sink {
            file: Mutex::new(file),
            began: Instant::now(),
        })
    })
    .as_ref()
}
/// Whether a profile is being written. Callers test it before building a line.
pub fn enabled() -> bool {
    sink().is_some()
}
/// Appends one event. Use the `profile!` macro, which skips the formatting
/// when no profile is written.
pub fn note(event: std::fmt::Arguments<'_>) {
    let Some(sink) = sink() else {
        return;
    };
    let thread = std::thread::current();
    let line = format!(
        "{:>10.1}\t{}\t{event}\n",
        sink.began.elapsed().as_secs_f64() * 1000.0,
        thread.name().unwrap_or("-")
    );
    if let Ok(mut file) = sink.file.lock() {
        let _ = file.write_all(line.as_bytes());
    }
}
/// One event of the profile, formatted like `format!`; nothing when it is off.
#[macro_export]
macro_rules! profile {
    ($($arg:tt)*) => {
        if $crate::profile::enabled() {
            $crate::profile::note(format_args!($($arg)*));
        }
    };
}
