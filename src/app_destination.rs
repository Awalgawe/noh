//! Immediate filename validation and one coalesced filesystem worker.
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Issue {
    Empty,
    Invalid,
    NotMp4,
    Folder,
    Unavailable,
    Exists,
}
impl Issue {
    pub fn key(&self) -> &'static str {
        match self {
            Self::Empty => "ui.name_empty",
            Self::Invalid => "ui.name_invalid",
            Self::NotMp4 => "error.mp4",
            Self::Folder => "error.output_folder",
            Self::Unavailable => "ui.destination_unavailable",
            Self::Exists => "error.output_exists",
        }
    }
}
pub fn syntax(name: &str) -> Option<Issue> {
    if name.trim().is_empty() {
        return Some(Issue::Empty);
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if name.ends_with([' ', '.'])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || reserved
    {
        return Some(Issue::Invalid);
    }
    if !Path::new(name)
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("mp4"))
    {
        return Some(Issue::NotMp4);
    }
    None
}
/// One-click export: while a name is automatic, a taken name gives
/// way to the first free one, found from the base name, so `song_noh-2.mp4`
/// is followed by `song_noh-3.mp4`, not `song_noh-2-2.mp4`. Returns the name
/// to use next, if it changes; a name the person chose never changes here.
pub fn adopt(auto: bool, current: &str, base: &str, result: &ResultView) -> Option<String> {
    if !auto
        || result.issue != Some(Issue::Exists)
        || result.path.file_name().is_none_or(|name| name != current)
    {
        return None;
    }
    if current != base {
        return Some(base.to_owned());
    }
    result.suggestion.clone()
}
#[derive(Clone, Debug)]
pub struct ResultView {
    pub path: PathBuf,
    pub issue: Option<Issue>,
    pub suggestion: Option<String>,
    pub bytes: Option<u64>,
}
#[derive(Default)]
struct State {
    request: Option<PathBuf>,
    generation: u64,
    pending: bool,
    result: Option<(u64, ResultView)>,
    closed: bool,
}
pub struct Destination {
    shared: Arc<(Mutex<State>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}
impl Destination {
    pub fn new(ctx: eframe::egui::Context) -> Self {
        let shared: Arc<(Mutex<State>, Condvar)> = Arc::default();
        let state = shared.clone();
        let thread = std::thread::spawn(move || {
            loop {
                let mut s = state.0.lock().unwrap();
                while !s.pending && !s.closed {
                    let (next, timed) = state.1.wait_timeout(s, Duration::from_secs(2)).unwrap();
                    s = next;
                    if timed.timed_out() && s.request.is_some() {
                        s.pending = true;
                    }
                }
                if s.closed {
                    break;
                }
                s.pending = false;
                let Some(path) = s.request.clone() else {
                    continue;
                };
                let generation = s.generation;
                drop(s);
                let result = inspect_with(path, || {
                    let s = state.0.lock().unwrap();
                    s.closed || s.generation != generation
                });
                let mut s = state.0.lock().unwrap();
                if s.generation == generation {
                    s.result = Some((generation, result));
                    drop(s);
                    ctx.request_repaint();
                }
            }
        });
        Self {
            shared,
            thread: Some(thread),
        }
    }
    pub fn update(&self, path: Option<PathBuf>, force: bool) -> bool {
        let mut s = self.shared.0.lock().unwrap();
        if !force && s.request == path {
            return false;
        }
        s.generation += 1;
        s.request = path;
        s.pending = s.request.is_some();
        s.result = None;
        drop(s);
        self.shared.1.notify_one();
        true
    }
    pub fn receive(&self) -> Option<ResultView> {
        let mut s = self.shared.0.lock().unwrap();
        s.result
            .take()
            .and_then(|(g, result)| (g == s.generation).then_some(result))
    }
}
impl Drop for Destination {
    fn drop(&mut self) {
        self.shared.0.lock().unwrap().closed = true;
        self.shared.1.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[cfg(test)]
fn inspect(path: PathBuf) -> ResultView {
    inspect_with(path, || false)
}
fn inspect_with(path: PathBuf, obsolete: impl Fn() -> bool) -> ResultView {
    let mut result = ResultView {
        path: path.clone(),
        issue: None,
        suggestion: None,
        bytes: None,
    };
    match path.parent().map(Path::metadata) {
        Some(Ok(metadata)) if metadata.is_dir() => {}
        Some(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
            result.issue = Some(Issue::Unavailable);
            return result;
        }
        _ => {
            result.issue = Some(Issue::Folder);
            return result;
        }
    }
    match path.metadata() {
        Ok(metadata) => {
            result.issue = Some(Issue::Exists);
            result.bytes = Some(metadata.len());
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            for number in 2..=4096 {
                if obsolete() {
                    break;
                }
                let name = format!("{stem}-{number}.mp4");
                match path.with_file_name(&name).metadata() {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        result.suggestion = Some(name);
                        break;
                    }
                    Err(_) => {
                        result.issue = Some(Issue::Unavailable);
                        break;
                    }
                    Ok(_) => {}
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => result.issue = Some(Issue::Unavailable),
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn immediate_validation_preserves_invalid_text() {
        for name in ["", " "] {
            assert_eq!(syntax(name), Some(Issue::Empty));
        }
        for name in [
            "CON.mp4",
            "clip/part.mp4",
            "a?.mp4",
            "LPT9.mp4",
            "clip.mp4 ",
        ] {
            assert_eq!(syntax(name), Some(Issue::Invalid));
        }
        assert_eq!(syntax("clip.mov"), Some(Issue::NotMp4));
        assert_eq!(syntax("動画.MP4"), None);
    }
    #[test]
    fn first_free_name_is_advisory_and_never_replaces_a_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("montage.mp4");
        std::fs::write(&path, b"original").unwrap();
        std::fs::write(folder.path().join("montage-2.mp4"), []).unwrap();
        let result = inspect(path.clone());
        assert_eq!(result.suggestion.as_deref(), Some("montage-3.mp4"));
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }
    /// An automatic name moves to the first free name counted from its base;
    /// a chosen one, or a result for another name, changes nothing.
    #[test]
    fn automatic_names_count_from_their_base() {
        let folder = tempfile::tempdir().unwrap();
        let base = "song_noh.mp4";
        std::fs::write(folder.path().join(base), []).unwrap();
        let taken = inspect(folder.path().join(base));
        assert_eq!(
            adopt(true, base, base, &taken).as_deref(),
            Some("song_noh-2.mp4")
        );
        assert_eq!(adopt(false, base, base, &taken), None);
        assert_eq!(adopt(true, "other.mp4", base, &taken), None);
        // After exporting to -2, that name is taken: back to the base, then -3.
        std::fs::write(folder.path().join("song_noh-2.mp4"), []).unwrap();
        let second = inspect(folder.path().join("song_noh-2.mp4"));
        assert_eq!(
            adopt(true, "song_noh-2.mp4", base, &second).as_deref(),
            Some(base)
        );
        let again = inspect(folder.path().join(base));
        assert_eq!(
            adopt(true, base, base, &again).as_deref(),
            Some("song_noh-3.mp4")
        );
        let free = inspect(folder.path().join("song_noh-3.mp4"));
        assert_eq!(adopt(true, "song_noh-3.mp4", base, &free), None);
    }
}
