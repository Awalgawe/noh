//! Bounded metadata scheduler. One active worker, coalesced pending work and LRU cache.
use noh::{
    engine::Event,
    i18n::Message,
    input::MediaItem,
    inspection::{InspectionRequest, InspectionResult, Snapshot},
    media::MediaInfo,
};
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

const CAPACITY: usize = 4097; // Maximum clips plus one soundtrack.
const CACHE_CAPACITY: usize = 128;
pub struct Clip {
    pub id: u64,
    pub request_id: u64,
    pub item: MediaItem,
    pub info: Option<MediaInfo>,
    pub error: Option<Message>,
}
pub fn item_for_path(path: PathBuf) -> MediaItem {
    let image = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["png", "jpg", "jpeg"]
            .iter()
            .any(|ext| e.eq_ignore_ascii_case(ext))
    });
    if image {
        MediaItem::Image {
            path,
            duration: 3.0,
        }
    } else {
        MediaItem::Video { path }
    }
}
pub struct Failure {
    pub key: &'static str,
    pub detail: String,
}
pub struct Analysis {
    pub id: u64,
    pub wav: bool,
    pub result: Result<MediaInfo, Failure>,
}
struct Request {
    id: u64,
    path: PathBuf,
    wav: bool,
    image: bool,
    ffmpeg: Option<PathBuf>,
    ctx: eframe::egui::Context,
}
#[derive(Default)]
struct State {
    pending: VecDeque<Request>,
    wanted: HashSet<u64>,
    results: VecDeque<Analysis>,
    active: Option<(u64, Arc<AtomicBool>)>,
    closed: bool,
}
impl State {
    fn retain(&mut self, ids: HashSet<u64>) {
        self.pending.retain(|r| ids.contains(&r.id));
        self.results.retain(|r| ids.contains(&r.id));
        if let Some((id, cancel)) = &self.active
            && !ids.contains(id)
        {
            cancel.store(true, Ordering::Release);
        }
        self.wanted = ids;
    }
}
type Shared = Arc<(Mutex<State>, Condvar)>;
pub struct Results(Shared);
impl Results {
    pub fn try_recv(&self) -> Result<Analysis, ()> {
        self.0.0.lock().unwrap().results.pop_front().ok_or(())
    }
    #[cfg(test)]
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Result<Analysis, ()> {
        let state = self.0.0.lock().unwrap();
        let (mut state, _) = self
            .0
            .1
            .wait_timeout_while(state, timeout, |s| s.results.is_empty() && !s.closed)
            .unwrap();
        state.results.pop_front().ok_or(())
    }
}
pub struct Metadata {
    state: Shared,
    pub events: Results,
    thread: Option<JoinHandle<()>>,
}
struct Cached {
    path: PathBuf,
    ffmpeg: Option<PathBuf>,
    wav: bool,
    image: bool,
    snapshot: Snapshot,
    info: MediaInfo,
}
#[derive(Default)]
struct Cache(VecDeque<Cached>);
impl Cache {
    fn insert(&mut self, entry: Cached) {
        self.0.retain(|c| {
            !(c.path == entry.path
                && c.ffmpeg == entry.ffmpeg
                && c.wav == entry.wav
                && c.image == entry.image)
        });
        self.0.push_back(entry);
        while self.0.len() > CACHE_CAPACITY {
            self.0.pop_front();
        }
    }
    fn get(&mut self, r: &Request) -> Option<MediaInfo> {
        let index = self.0.iter().position(|c| {
            c.path == r.path && c.ffmpeg == r.ffmpeg && c.wav == r.wav && c.image == r.image
        })?;
        let entry = self.0.remove(index)?;
        if entry.snapshot.verify().is_err() {
            return None;
        }
        let info = entry.info.clone();
        self.0.push_back(entry);
        Some(info)
    }
}
impl Default for Metadata {
    fn default() -> Self {
        Self::with_worker(std::env::current_exe().unwrap_or_default())
    }
}
impl Metadata {
    pub fn with_worker(worker: PathBuf) -> Self {
        let state: Shared = Arc::default();
        let shared = state.clone();
        let thread = std::thread::spawn(move || {
            let mut cache = Cache::default();
            loop {
                let (mut request, cancel) = {
                    let state = shared.0.lock().unwrap();
                    let mut state = shared
                        .1
                        .wait_while(state, |s| !s.closed && s.pending.is_empty())
                        .unwrap();
                    if state.closed {
                        break;
                    }
                    let request = state.pending.pop_front().unwrap();
                    let cancel = Arc::new(AtomicBool::new(false));
                    state.active = Some((request.id, cancel.clone()));
                    (request, cancel)
                };
                if !request.wav {
                    // A changed PATH/default engine cannot reuse a cache entry for
                    // another executable, even when that old executable still exists.
                    if let Ok(ffmpeg) = noh::inspection::resolve_ffmpeg(
                        request
                            .ffmpeg
                            .as_deref()
                            .unwrap_or(std::path::Path::new("")),
                    ) {
                        request.ffmpeg = Some(ffmpeg);
                    }
                }
                let result = if let Some(info) = cache.get(&request) {
                    Ok(info)
                } else {
                    let job = noh::jobs::Job::inspect_with_worker(
                        if request.image {
                            InspectionRequest::Image {
                                path: request.path.clone(),
                                ffmpeg: request.ffmpeg.clone(),
                            }
                        } else {
                            InspectionRequest::Metadata {
                                path: request.path.clone(),
                                wav: request.wav,
                                ffmpeg: request.ffmpeg.clone(),
                            }
                        },
                        worker.clone(),
                        cancel.clone(),
                        || {},
                    );
                    loop {
                        match job.events.recv_timeout(std::time::Duration::from_secs(1)) {
                            Ok(Event::Inspected(Ok(InspectionResult::Metadata {
                                info,
                                snapshot,
                            }))) => {
                                cache.insert(Cached {
                                    path: request.path.clone(),
                                    ffmpeg: request.ffmpeg.clone(),
                                    wav: request.wav,
                                    image: request.image,
                                    snapshot,
                                    info: info.clone(),
                                });
                                break Ok(info);
                            }
                            Ok(Event::Inspected(Err(error))) => {
                                break Err(Failure {
                                    key: if request.wav {
                                        "error.wav"
                                    } else if request.image {
                                        "error.image"
                                    } else {
                                        "error.video"
                                    },
                                    detail: error.to_string(),
                                });
                            }
                            Ok(Event::Cancelled) => {
                                break Err(Failure {
                                    key: "status.export_cancelled",
                                    detail: "Inspection cancelled".into(),
                                });
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                break Err(Failure {
                                    key: "error.engine",
                                    detail: "Inspection worker disconnected".into(),
                                });
                            }
                            _ => {}
                        }
                    }
                };
                let mut state = shared.0.lock().unwrap();
                state.active = None;
                // Job destruction also sets its cancellation flag after success.
                // Request identity, not that flag, decides whether a reply is current.
                if !state.closed && state.wanted.contains(&request.id) {
                    // Each accepted request ID has at most one reply.
                    state.results.retain(|r| r.id != request.id);
                    if state.results.len() == CAPACITY {
                        state.results.pop_front();
                    }
                    state.results.push_back(Analysis {
                        id: request.id,
                        wav: request.wav,
                        result,
                    });
                    drop(state);
                    shared.1.notify_all();
                    request.ctx.request_repaint();
                }
            }
        });
        Self {
            events: Results(state.clone()),
            state,
            thread: Some(thread),
        }
    }
}
impl Metadata {
    pub fn retain(&self, ids: impl Iterator<Item = u64>) {
        self.state
            .0
            .lock()
            .unwrap()
            .retain(ids.take(CAPACITY).collect());
    }
    pub fn request(
        &self,
        id: u64,
        path: PathBuf,
        wav: bool,
        ffmpeg: Option<PathBuf>,
        ctx: &eframe::egui::Context,
    ) {
        self.request_inner(id, path, wav, false, ffmpeg, ctx);
    }
    pub fn request_media(
        &self,
        id: u64,
        item: MediaItem,
        ffmpeg: Option<PathBuf>,
        ctx: &eframe::egui::Context,
    ) {
        self.request_inner(id, item.path().clone(), false, item.is_image(), ffmpeg, ctx);
    }
    fn request_inner(
        &self,
        id: u64,
        path: PathBuf,
        wav: bool,
        image: bool,
        ffmpeg: Option<PathBuf>,
        ctx: &eframe::egui::Context,
    ) {
        let mut state = self.state.0.lock().unwrap();
        state.pending.retain(|r| r.id != id && !(wav && r.wav));
        if state.pending.len() == CAPACITY
            || (state.wanted.len() == CAPACITY && !state.wanted.contains(&id))
        {
            return;
        }
        state.wanted.insert(id);
        state.pending.push_back(Request {
            id,
            path,
            wav,
            image,
            ffmpeg,
            ctx: ctx.clone(),
        });
        drop(state);
        self.state.1.notify_one();
    }
}
impl Drop for Metadata {
    fn drop(&mut self) {
        let mut state = self.state.0.lock().unwrap();
        state.closed = true;
        state.retain(HashSet::new());
        drop(state);
        self.state.1.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What a dropped or picked file is, from its extension only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Picture,
    Video,
    Song,
    Lyrics,
}
pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" | "jpg" | "jpeg" => Kind::Picture,
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => Kind::Video,
        "wav" => Kind::Song,
        "srt" => Kind::Lyrics,
        _ => return None,
    })
}
/// A drop or a picker selection sorted by type.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropPlan {
    /// Pictures and videos, in drop order.
    pub visuals: Vec<PathBuf>,
    pub pictures: usize,
    pub videos: usize,
    pub song: Option<PathBuf>,
    pub lyrics: Option<PathBuf>,
}
fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
/// Sorts files by type, or refuses the whole batch with one reason: nothing
/// changes before a valid drop. `check_files` is false while hovering, which
/// reads names only (no filesystem access, no decoding).
pub fn classify(
    paths: &[Option<PathBuf>],
    has_song: bool,
    check_files: bool,
) -> Result<DropPlan, Message> {
    if paths.is_empty() {
        return Err("drop.empty".into());
    }
    let mut plan = DropPlan::default();
    let mut songs = 0;
    let mut lyrics = 0;
    for path in paths {
        let refused = |path: &Path| Message::new("drop.rejected_unsupported", &[file_name(path)]);
        let Some(path) = path else {
            return Err(Message::new("drop.rejected_unsupported", &["?".into()]));
        };
        let kind = kind_of(path).ok_or_else(|| refused(path))?;
        if check_files && !path.is_file() {
            return Err(refused(path));
        }
        match kind {
            Kind::Picture | Kind::Video => {
                if kind == Kind::Picture {
                    plan.pictures += 1;
                } else {
                    plan.videos += 1;
                }
                plan.visuals.push(path.clone());
            }
            Kind::Song => {
                songs += 1;
                plan.song = Some(path.clone());
            }
            Kind::Lyrics => {
                lyrics += 1;
                plan.lyrics = Some(path.clone());
            }
        }
    }
    if songs > 1 {
        return Err("drop.rejected_songs".into());
    }
    if lyrics > 1 {
        return Err("drop.rejected_lyrics".into());
    }
    if plan.lyrics.is_some() && !has_song && plan.song.is_none() {
        return Err("drop.lyrics_need_song".into());
    }
    Ok(plan)
}
/// Hovered names as they are known before the drop (no filesystem access).
pub fn hovered_paths(files: &[eframe::egui::HoveredFile]) -> Vec<Option<PathBuf>> {
    files.iter().map(|file| file.path.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_requests_replies_and_cache_are_bounded_and_obsolete_work_is_cancelled() {
        let shared: Shared = Arc::default();
        let metadata = Metadata {
            state: shared.clone(),
            events: Results(shared.clone()),
            thread: None,
        };
        let ctx = eframe::egui::Context::default();
        for id in 0..(CAPACITY * 2) as u64 {
            metadata.request(id, "clip.mp4".into(), false, None, &ctx);
        }
        assert_eq!(shared.0.lock().unwrap().pending.len(), CAPACITY);
        assert_eq!(shared.0.lock().unwrap().wanted.len(), CAPACITY);
        let cancel = Arc::new(AtomicBool::new(false));
        shared.0.lock().unwrap().active = Some((0, cancel.clone()));
        shared.0.lock().unwrap().results.push_back(Analysis {
            id: 0,
            wav: false,
            result: Ok(MediaInfo::default()),
        });
        metadata.retain([3].into_iter());
        assert!(cancel.load(Ordering::Acquire));
        assert!(
            metadata
                .events
                .recv_timeout(std::time::Duration::ZERO)
                .is_err()
        );
        assert_eq!(shared.0.lock().unwrap().pending.len(), 1);
        metadata.request(3, "new.mp4".into(), false, None, &ctx);
        let state = shared.0.lock().unwrap();
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].path, PathBuf::from("new.mp4"));
        drop(state);
        let mut cache = Cache::default();
        for index in 0..1000 {
            cache.insert(Cached {
                path: format!("{index}.mp4").into(),
                ffmpeg: None,
                wav: false,
                image: false,
                snapshot: Snapshot(vec![]),
                info: MediaInfo::default(),
            });
        }
        assert_eq!(cache.0.len(), CACHE_CAPACITY);
        assert_eq!(cache.0.front().unwrap().path, PathBuf::from("872.mp4"));
    }
    #[test]
    fn cache_rejects_replaced_source_and_engine() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("video.mp4");
        let ffmpeg = folder.path().join("engine.exe");
        std::fs::write(&path, "source").unwrap();
        std::fs::write(&ffmpeg, "engine").unwrap();
        let request = Request {
            id: 1,
            path: path.clone(),
            ffmpeg: Some(ffmpeg.clone()),
            wav: false,
            image: false,
            ctx: Default::default(),
        };
        let mut cache = Cache::default();
        for changed in [&path, &ffmpeg] {
            cache.insert(Cached {
                path: path.clone(),
                ffmpeg: Some(ffmpeg.clone()),
                wav: false,
                image: false,
                snapshot: Snapshot::read([&path, &ffmpeg]).unwrap(),
                info: MediaInfo::default(),
            });
            assert!(cache.get(&request).is_some());
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(changed)
                .unwrap()
                .write_all(b"changed")
                .unwrap();
            assert!(cache.get(&request).is_none());
        }
    }
    #[test]
    fn drops_are_sorted_by_type_or_refused_whole() {
        let folder = tempfile::tempdir().unwrap();
        let file = |name: &str| {
            let path = folder.path().join(name);
            std::fs::write(&path, []).unwrap();
            Some(path)
        };
        let (video, image, wav, srt) = (
            file("clip.MP4"),
            file("poster.JpG"),
            file("audio.WaV"),
            file("words.SRT"),
        );
        // A mixed drop fills the strip, the song and the lyrics at once.
        let plan = classify(
            &[
                video.clone(),
                image.clone(),
                wav.clone(),
                srt.clone(),
                video.clone(),
            ],
            false,
            true,
        )
        .unwrap();
        assert_eq!(plan.visuals.len(), 3);
        assert_eq!((plan.pictures, plan.videos), (1, 2));
        assert_eq!(plan.song, wav);
        assert_eq!(plan.lyrics, srt);
        assert!(item_for_path(image.clone().unwrap()).is_image());
        let refused = |paths: &[Option<PathBuf>], has_song: bool| {
            classify(paths, has_song, true).unwrap_err().key
        };
        assert_eq!(refused(&[], false), "drop.empty");
        assert_eq!(
            refused(&[wav.clone(), wav.clone()], false),
            "drop.rejected_songs"
        );
        assert_eq!(
            refused(&[srt.clone(), srt.clone(), wav.clone()], false),
            "drop.rejected_lyrics"
        );
        assert_eq!(refused(&[srt.clone()], false), "drop.lyrics_need_song");
        assert!(classify(&[srt.clone()], true, true).is_ok());
        let unsupported = classify(&[video.clone(), file("notes.txt")], false, true).unwrap_err();
        assert_eq!(unsupported.key, "drop.rejected_unsupported");
        assert_eq!(unsupported.args, ["notes.txt"]);
        let folder_path = Some(folder.path().join("album.wav"));
        std::fs::create_dir(folder_path.as_ref().unwrap()).unwrap();
        assert_eq!(
            refused(&[folder_path.clone()], false),
            "drop.rejected_unsupported"
        );
        // Hovering reads names only: a folder named like a WAV is not probed.
        assert!(classify(&[folder_path], false, false).is_ok());
        assert_eq!(refused(&[None], false), "drop.rejected_unsupported");
        // Every accepted extension, in any case; APNG and others are refused.
        for (name, kind) in [
            ("a.png", Kind::Picture),
            ("a.JPG", Kind::Picture),
            ("a.jpeg", Kind::Picture),
            ("a.mp4", Kind::Video),
            ("a.MOV", Kind::Video),
            ("a.mkv", Kind::Video),
            ("a.webm", Kind::Video),
            ("a.avi", Kind::Video),
            ("a.m4v", Kind::Video),
            ("a.wav", Kind::Song),
            ("a.Srt", Kind::Lyrics),
        ] {
            assert_eq!(kind_of(Path::new(name)), Some(kind), "{name}");
        }
        for name in ["a.apng", "a.gif", "a.mp3", "a.txt", "a"] {
            assert_eq!(kind_of(Path::new(name)), None, "{name}");
            assert_eq!(
                refused(&[file(name)], false),
                "drop.rejected_unsupported",
                "{name}"
            );
        }
    }
}
