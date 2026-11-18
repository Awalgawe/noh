//! What happened while the interface started, kept where a user can find it.
//!
//! On Windows the interface has no console: an interface that cannot open its window used to
//! close without a word. The start is now written to `startup.log` beside the preferences, step
//! by step and from its first instruction, so a start that dies leaves its last step behind, and
//! a start that fails says why in a dialog. A capture run (`NOH_CAPTURE_UI`) writes the log too
//! but shows no dialog: nothing may wait for a click there.

use noh::i18n::Language;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::ThreadId,
};

/// Set once the window exists: from then on a failure is the running application's, not the start's.
static OPENED: AtomicBool = AtomicBool::new(false);
/// The interface language, once known; a message shown earlier is in English.
static LANGUAGE: OnceLock<Language> = OnceLock::new();
/// The thread `begin` ran on: the only one a dialog may be shown from.
static MAIN: OnceLock<ThreadId> = OnceLock::new();

/// `startup.log` in the folder that already holds the language preference.
pub fn log_path() -> Option<PathBuf> {
    Some(noh::i18n::preferences_path()?.parent()?.join("startup.log"))
}

fn append(path: &Path, line: &str) {
    // Opened, written and closed each time: a process that dies next keeps every earlier line.
    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

/// Record one step of the start. Does nothing when the log cannot be written.
pub fn note(line: &str) {
    if let Some(path) = log_path() {
        append(&path, line);
    }
}

fn header(now: &str) -> String {
    format!(
        "NOH {} on {} {}, started {now}",
        noh::build_info::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Start a new log, replacing the previous start's, and record panics from now on.
/// Call it on the main thread, before anything that can fail.
pub fn begin() {
    let _ = MAIN.set(std::thread::current().id());
    if let Some(path) = log_path() {
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let now = chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S %:z")
            .to_string();
        let _ = std::fs::write(&path, header(&now) + "\n");
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        note(&format!("panic: {info}"));
        // A dialog only for a start that dies on the main thread. Raised from another thread it
        // could hang a process that was about to end; once the window exists it is not a start.
        if !OPENED.load(Ordering::Relaxed) && MAIN.get() == Some(&std::thread::current().id()) {
            tell(&info.to_string());
        }
        previous(info);
    }));
}

/// The language the user reads, for a message shown from now on.
pub fn speak(language: Language) {
    let _ = LANGUAGE.set(language);
}

/// The window exists. `graphics` names the OpenGL implementation that drew it, when known.
pub fn opened(graphics: Option<String>) {
    OPENED.store(true, Ordering::Relaxed);
    note(&match graphics {
        Some(graphics) => format!("window opened; OpenGL: {graphics}"),
        None => "window opened; OpenGL: not reported".into(),
    });
}

/// The vendor, renderer and version strings of an OpenGL context.
pub fn graphics(gl: &eframe::glow::Context) -> String {
    use eframe::glow::HasContext;
    // SAFETY: called on the thread that owns `gl`, with the context eframe just made current.
    let read = |name| unsafe { gl.get_parameter_string(name) };
    format!(
        "{} / {} / {}",
        read(eframe::glow::VENDOR),
        read(eframe::glow::RENDERER),
        read(eframe::glow::VERSION)
    )
}

/// The interface ended on an error: record why and tell the user.
pub fn failed(error: &str) {
    note(&format!("failed: {error}"));
    tell(error);
}

fn message(language: Language, error: &str, opened: bool, log: Option<&Path>) -> String {
    // The error is the system's or the toolkit's own text, in whatever language it comes.
    let mut parts = vec![
        language
            .text(if opened {
                "startup.failed.running"
            } else {
                "startup.failed.intro"
            })
            .to_string(),
        error.to_string(),
    ];
    if !opened {
        // Before the window exists the usual cause is graphics; afterwards the hint would mislead.
        parts.push(language.text("startup.failed.hint").to_string());
    }
    if let Some(log) = log {
        parts.push(language.format("startup.failed.log", &[log.display().to_string()]));
    }
    parts.join("\n\n")
}

fn tell(error: &str) {
    if std::env::var_os("NOH_CAPTURE_UI").is_some() {
        return;
    }
    let language = *LANGUAGE.get().unwrap_or(&Language::En);
    let opened = OPENED.load(Ordering::Relaxed);
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(language.text("startup.failed.title"))
        .set_description(message(language, error, opened, log_path().as_deref()))
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_step_is_kept_even_if_the_process_stops_next() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("startup.log");
        std::fs::write(&path, header("2026-01-01 00:00:00 +00:00") + "\n").unwrap();
        append(&path, "creating the window");
        append(&path, "failed: no OpenGL 3.3");
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert!(lines[0].starts_with(&format!("NOH {} on ", noh::build_info::VERSION)));
        assert!(lines[0].ends_with("started 2026-01-01 00:00:00 +00:00"));
        assert_eq!(lines[1..], ["creating the window", "failed: no OpenGL 3.3"]);
        // A log that cannot be opened is not an error of its own.
        append(&folder.path().join("missing/startup.log"), "ignored");
    }

    #[test]
    fn the_message_names_the_error_and_the_log_in_every_language() {
        let log = Path::new("C:/Users/someone/AppData/Roaming/noh/startup.log");
        for language in Language::ALL {
            let hint = language.text("startup.failed.hint");
            let text = message(language, "no OpenGL {0}", false, Some(log));
            assert!(text.contains("no OpenGL {0}"), "{}", language.code());
            assert!(text.contains("startup.log"), "{}", language.code());
            assert!(text.contains(hint), "{}", language.code());
            assert!(
                hint.contains("2.0") && hint.contains("3.3"),
                "{}",
                language.code()
            );
            // Once the window exists the graphics hint is left out.
            let later = message(language, "event loop error", true, Some(log));
            assert!(!later.contains(hint), "{}", language.code());
            assert!(later.contains(language.text("startup.failed.running")));
            assert!(!language.text("startup.failed.title").is_empty());
        }
    }

    #[test]
    fn the_log_sits_beside_the_preferences() {
        if let (Some(log), Some(preferences)) = (log_path(), noh::i18n::preferences_path()) {
            assert_eq!(log.parent(), preferences.parent());
            assert_eq!(log.file_name().unwrap(), "startup.log");
        }
    }
}
