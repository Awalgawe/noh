//! Light working copies of video sources, kept on disk between sessions.
//!
//! A copy is found by the SIGNATURE of its source, a hash of the file's content,
//! never by its name: a renamed, moved or duplicated file keeps its copy. The
//! store is only an accelerator. Every caller must work while a copy is absent
//! or being made, and when the store itself cannot be used. Exports never read
//! these files.
use crate::inspection::FileStamp;
use sha2_11::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// FFmpeg stops writing a copy at this size; such a copy is refused.
pub const COPY_BYTES: u64 = 128 * 1024 * 1024;
/// Beyond this size the store deletes the copies used least recently.
const STORE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Names the encoding below. Change it with the encoding: older copies are
/// then no longer found, and leave with the least recently used.
const RECIPE: &str = "1 libx264 fast crf18 g1 yuv420p 640";
/// The index of known signatures keeps this many files.
const INDEX_FILES: usize = 4096;
/// One copy may take this long to encode.
const COPY_TIME: Duration = Duration::from_secs(60);
/// A source too large to have a copy is not tried again for this long.
const REFUSAL_AGE: Duration = Duration::from_secs(30 * 86_400);

/// No copy of this source with this recipe, for a reason worth remembering,
/// unlike a cancelled or failed encoding. Its SIZE belongs to the source: the
/// store keeps that for `REFUSAL_AGE`. The TIME it took belongs to this machine
/// at that moment: it is kept for this session only, and the next one tries again.
#[derive(Debug)]
struct Refusal {
    reason: &'static str,
    lasting: bool,
}
const TOO_LARGE: Refusal = Refusal {
    reason: "Preview working copy exceeds its size limit",
    lasting: true,
};
const TOO_SLOW: Refusal = Refusal {
    reason: "Preview working copy takes too long to prepare",
    lasting: false,
};
impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.reason)
    }
}
impl std::error::Error for Refusal {}

/// A light copy of one source: 640 pixels at most, every frame a keyframe.
pub struct Light {
    pub path: PathBuf,
    pub bytes: u64,
    /// Actual presentation times, including variable-rate gaps.
    pub times: Vec<f64>,
    /// A copy made outside the store lives as long as this folder.
    _temporary: Option<tempfile::TempDir>,
}
impl Light {
    /// Where to ask FFmpeg for the picture this copy shows at `seconds`: just
    /// before its first frame at or after that time, its last frame past the
    /// end. The frame times are the copy's own, so no rounding of the request
    /// can step over a frame.
    pub fn seek(&self, seconds: f64) -> f64 {
        let index = self
            .times
            .partition_point(|pts| *pts < seconds - 1e-7)
            .min(self.times.len().saturating_sub(1));
        (self.times.get(index).copied().unwrap_or(0.0) - 0.0005).max(0.0)
    }
}

/// A finished copy of the store and what it was made from, for a reader that
/// uses a copy only when it is there and never waits for one.
struct Held {
    source: FileStamp,
    origin: f64,
    copy: Arc<Light>,
}
fn held() -> &'static Mutex<HashMap<PathBuf, Held>> {
    static HELD: OnceLock<Mutex<HashMap<PathBuf, Held>>> = OnceLock::new();
    HELD.get_or_init(Default::default)
}
/// The copy of `source` made with this `origin`, if this session has it ready
/// and its file is still there. The live player asks at each seek: it reads
/// the source until then, and the copy from the next seek on.
pub fn ready(source: &FileStamp, origin: f64) -> Option<Arc<Light>> {
    let name = named(&source.path);
    let mut held = held().lock().unwrap_or_else(|e| e.into_inner());
    let Some(found) = held.get(&name) else {
        crate::profile!(
            "copy: none ready for {} ({:?})",
            name.display(),
            state(&name)
        );
        return None;
    };
    if found.source != *source || (found.origin - origin).abs() > 1e-9 {
        crate::profile!(
            "copy: not for this reader, {}: file changed {}, origin {} asked {}",
            name.display(),
            found.source != *source,
            found.origin,
            origin
        );
        return None;
    }
    if !found.copy.path.is_file() {
        crate::profile!("copy: file gone, {}", found.copy.path.display());
        held.remove(&name);
        drop(held);
        set_state(&source.path, State::Absent);
        return None;
    }
    Some(found.copy.clone())
}

/// Where the copy of a source stands in this session, for the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not asked for, or its preparation failed: the source itself is used.
    Absent,
    /// Waiting, being read for its signature, or being encoded.
    Preparing,
    Ready,
}
fn states() -> &'static Mutex<HashMap<PathBuf, State>> {
    static STATES: OnceLock<Mutex<HashMap<PathBuf, State>>> = OnceLock::new();
    STATES.get_or_init(Default::default)
}
fn named(source: &Path) -> PathBuf {
    std::path::absolute(source).unwrap_or_else(|_| source.to_path_buf())
}
/// The state of the copy of `source`, by the path the interface knows it by.
pub fn state(source: &Path) -> State {
    states()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&named(source))
        .copied()
        .unwrap_or(State::Absent)
}
fn set_state(source: &Path, state: State) {
    let mut states = states().lock().unwrap_or_else(|e| e.into_inner());
    if state == State::Absent {
        states.remove(&named(source));
    } else {
        states.insert(named(source), state);
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Known {
    bytes: u64,
    modified: (u64, u32),
    /// As in `FileStamp`, whose comparison includes it.
    #[serde(default)]
    created: Option<(u64, u32)>,
    signature: String,
    /// When this file was last asked for, to forget the oldest first.
    seen: u64,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Described {
    bytes: u64,
    times: Vec<f64>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The folder of copies and what this process is doing in it. Several
/// processes may share the folder: files appear by rename, and a lost update
/// of the index only costs a second reading of a source.
pub struct Store {
    folder: PathBuf,
    limit: u64,
    index: Mutex<()>,
    building: Mutex<HashSet<String>>,
    released: Condvar,
    /// Copies handed out in this session, by key, and files used where they
    /// were written, by name: never deleted by it.
    used: Mutex<HashSet<String>>,
    /// Copies that took too long in this session: not tried again by it.
    slow: Mutex<HashSet<String>>,
    partials: AtomicU64,
}
impl Store {
    pub fn at(folder: PathBuf, limit: u64) -> std::io::Result<Self> {
        std::fs::create_dir_all(&folder)?;
        Ok(Self {
            folder,
            limit,
            index: Mutex::new(()),
            building: Mutex::new(HashSet::new()),
            released: Condvar::new(),
            used: Mutex::new(HashSet::new()),
            slow: Mutex::new(HashSet::new()),
            partials: AtomicU64::new(0),
        })
    }
    /// The user's cache folder (`NOH_CACHE_DIR` replaces it). None when there
    /// is none or it cannot be created, and in this library's own tests. Any
    /// other run, a capture included, uses the real folder unless that variable
    /// is set.
    pub fn shared() -> Option<&'static Store> {
        static STORE: OnceLock<Option<Store>> = OnceLock::new();
        STORE
            .get_or_init(|| {
                if cfg!(test) {
                    return None;
                }
                let folder = cache_folder()?.join("copies");
                match Store::at(folder.clone(), STORE_BYTES) {
                    Ok(store) => Some(store),
                    Err(error) => {
                        tracing::warn!("Preview copies stay temporary: {error}");
                        None
                    }
                }
            })
            .as_ref()
    }

    fn index_path(&self) -> PathBuf {
        self.folder.join("index.json")
    }
    fn read_index(&self) -> HashMap<String, Known> {
        std::fs::read(self.index_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
    /// The signature of `source`: the SHA-256 of its content. A file whose
    /// path, size and date are unchanged since it was last read is not read
    /// again; that shortcut is only a hint, the signature names the copy.
    pub fn signature(&self, source: &FileStamp, stop: &dyn Fn() -> bool) -> Result<String> {
        let name = source.resolved.to_string_lossy().into_owned();
        let since = |time: SystemTime| {
            time.duration_since(UNIX_EPOCH)
                .map_or((0, 0), |d| (d.as_secs(), d.subsec_nanos()))
        };
        let modified = since(source.modified);
        let created = source.created.map(since);
        {
            let _guard = self.index.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(known) = self.read_index().get(&name)
                && known.bytes == source.bytes
                && known.modified == modified
                && known.created == created
            {
                crate::profile!("copy: signature known for {}", source.path.display());
                return Ok(known.signature.clone());
            }
        }
        let reading = Instant::now();
        let signature = hash(&source.resolved, stop)?;
        crate::profile!(
            "copy: signature read in {:.0} ms, {} bytes, {}",
            reading.elapsed().as_secs_f64() * 1000.0,
            source.bytes,
            source.path.display()
        );
        if FileStamp::read(&source.path)? != *source {
            return Err("Preview input changed".into());
        }
        let _guard = self.index.lock().unwrap_or_else(|e| e.into_inner());
        let mut index = self.read_index();
        index.insert(
            name,
            Known {
                bytes: source.bytes,
                modified,
                created,
                signature: signature.clone(),
                seen: now(),
            },
        );
        while index.len() > INDEX_FILES {
            let Some(oldest) = index
                .iter()
                .min_by_key(|(_, known)| known.seen)
                .map(|(name, _)| name.clone())
            else {
                break;
            };
            index.remove(&oldest);
        }
        // Best effort: without the index the next session reads the file again.
        let partial = self
            .folder
            .join(format!("index-{}.partial", std::process::id()));
        if let Ok(text) = serde_json::to_vec(&index)
            && std::fs::write(&partial, text).is_ok()
            && std::fs::rename(&partial, self.index_path()).is_err()
        {
            let _ = std::fs::remove_file(&partial);
        }
        Ok(signature)
    }

    fn video(&self, key: &str) -> PathBuf {
        self.folder.join(format!("{key}.mp4"))
    }
    fn described(&self, key: &str) -> PathBuf {
        self.folder.join(format!("{key}.json"))
    }
    /// A complete copy under `key`, marked as just used.
    pub fn get(&self, key: &str) -> Option<Light> {
        let path = self.video(key);
        let described: Described =
            serde_json::from_slice(&std::fs::read(self.described(key)).ok()?).ok()?;
        let bytes = std::fs::metadata(&path).ok()?.len();
        if bytes != described.bytes || described.times.is_empty() {
            return None;
        }
        if let Ok(file) = std::fs::File::options().append(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        self.used
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_owned());
        Some(Light {
            path,
            bytes,
            times: described.times,
            _temporary: None,
        })
    }
    /// A name for a copy being written; it becomes a copy only by `put`. The
    /// name is new each time: a leftover of a dead process cannot stand in the way.
    fn partial(&self) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        self.folder.join(format!(
            "partial-{}-{nanos}-{}.mp4",
            std::process::id(),
            self.partials.fetch_add(1, Ordering::Relaxed)
        ))
    }
    fn refusal(&self, key: &str) -> PathBuf {
        self.folder.join(format!("{key}.refused"))
    }
    /// Why no copy is made under `key`: found too large recently, or too slow
    /// in this session.
    pub fn refused(&self, key: &str) -> Option<String> {
        if self
            .slow
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(key)
        {
            return Some(TOO_SLOW.reason.into());
        }
        let path = self.refusal(key);
        let recent = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok()?
            .elapsed()
            .is_ok_and(|age| age < REFUSAL_AGE);
        recent
            .then(|| std::fs::read_to_string(&path).ok())
            .flatten()
    }
    /// Remembers why the source under `key` has no copy, so that it is not
    /// encoded again: at each launch if it is too large, in this session if it
    /// was only too slow.
    fn refuse(&self, key: &str, refusal: &Refusal) {
        if refusal.lasting {
            let _ = std::fs::write(self.refusal(key), refusal.reason);
        } else {
            self.slow
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key.to_owned());
        }
    }
    /// Gives the finished file at `partial` its name. If the store refuses,
    /// the file is used where it is: the caller never loses its copy.
    fn put(&self, key: &str, partial: &Path, bytes: u64, times: Vec<f64>) -> Light {
        let path = self.video(key);
        let described = Described { bytes, times };
        let filed = std::fs::rename(partial, &path).is_ok()
            && serde_json::to_vec(&described).is_ok_and(|text| {
                let staged = self.folder.join(format!("{key}.json.partial"));
                std::fs::write(&staged, text).is_ok()
                    && std::fs::rename(&staged, self.described(key)).is_ok()
            });
        let copy = if filed {
            self.get(key)
        } else if partial.is_file() {
            // Not renamed. Another process may have filed this copy meanwhile:
            // then its file and its description are used, never a mix of both.
            self.get(key).inspect(|_| {
                let _ = std::fs::remove_file(partial);
            })
        } else {
            None
        };
        self.used
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_owned());
        let copy = copy.unwrap_or_else(|| {
            let path = if partial.is_file() { partial } else { &path };
            tracing::warn!("Preview copy {key} could not be filed; it is used in place");
            if let Some(name) = path.file_name() {
                self.used
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(name.to_string_lossy().into_owned());
            }
            Light {
                path: path.to_path_buf(),
                bytes: described.bytes,
                times: described.times,
                _temporary: None,
            }
        });
        self.trim();
        copy
    }
    /// Deletes the copies used least recently beyond the limit, never one this
    /// session handed out, and the leftovers of interrupted writes.
    pub fn trim(&self) {
        let Ok(entries) = std::fs::read_dir(&self.folder) else {
            return;
        };
        let used = self.used.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut copies = Vec::new();
        let mut total = 0u64;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let modified = meta.modified().unwrap_or(UNIX_EPOCH);
            if name.ends_with(".refused") {
                if modified.elapsed().is_ok_and(|age| age > REFUSAL_AGE) {
                    let _ = std::fs::remove_file(entry.path());
                }
                continue;
            }
            if name.starts_with("partial-") || name.ends_with(".partial") {
                // Another process may still be writing a recent one.
                if !used.contains(&name)
                    && modified
                        .elapsed()
                        .is_ok_and(|age| age > Duration::from_secs(86_400))
                {
                    let _ = std::fs::remove_file(entry.path());
                }
                continue;
            }
            if let Some(key) = name.strip_suffix(".mp4") {
                total += meta.len();
                copies.push((modified, key.to_owned(), meta.len()));
            }
        }
        copies.sort();
        for (_, key, bytes) in copies {
            if total <= self.limit {
                break;
            }
            if used.contains(&key) {
                continue;
            }
            // A copy another process plays cannot be deleted on Windows: it stays.
            if std::fs::remove_file(self.video(&key)).is_ok() {
                let _ = std::fs::remove_file(self.described(&key));
                total -= bytes;
            }
        }
    }
    /// One maker per copy in this process: a second caller waits for the first.
    fn claim(&self, key: &str, stop: &dyn Fn() -> bool) -> Result<Claim<'_>> {
        let mut building = self.building.lock().unwrap_or_else(|e| e.into_inner());
        while building.contains(key) {
            if stop() {
                return Err("Preview preparation cancelled or timed out".into());
            }
            building = self
                .released
                .wait_timeout(building, Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        building.insert(key.to_owned());
        Ok(Claim {
            store: self,
            key: key.to_owned(),
        })
    }
}
/// Offers a finished copy of `source` to `ready`, as `prepare` does, for the
/// tests of its readers. The file at `path` must exist.
#[cfg(test)]
pub(crate) fn offer_for_test(
    source: &FileStamp,
    origin: f64,
    path: PathBuf,
    times: Vec<f64>,
) -> Arc<Light> {
    let copy = Arc::new(Light {
        path,
        bytes: 0,
        times,
        _temporary: None,
    });
    held().lock().unwrap_or_else(|e| e.into_inner()).insert(
        named(&source.path),
        Held {
            source: source.clone(),
            origin,
            copy: copy.clone(),
        },
    );
    copy
}
#[cfg(test)]
impl Light {
    /// A copy in a temporary folder of its own, for the tests of its users.
    pub(crate) fn temporary_for_test(bytes: u64) -> Self {
        let directory = tempfile::tempdir().unwrap();
        Self {
            path: directory.path().join("preview.mp4"),
            bytes,
            times: vec![0.0],
            _temporary: Some(directory),
        }
    }
}
struct Claim<'a> {
    store: &'a Store,
    key: String,
}
impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.store
            .building
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
        self.store.released.notify_all();
    }
}

pub(crate) fn cache_folder() -> Option<PathBuf> {
    if let Some(folder) = std::env::var_os("NOH_CACHE_DIR").filter(|p| !p.is_empty()) {
        return Some(folder.into());
    }
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Caches"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache")));
    base.map(|p| p.join("noh").join("cache"))
}

/// SHA-256 of the whole file, in hexadecimal.
fn hash(path: &Path, stop: &dyn Fn() -> bool) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        if stop() {
            return Err("Preview preparation cancelled or timed out".into());
        }
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex(hasher.finalize().as_slice()))
}
/// The name of a copy: its source's signature and how the copy is made.
pub fn key(signature: &str, recipe: &str) -> String {
    format!(
        "{signature}-{}",
        &hex(Sha256::digest(recipe).as_slice())[..16]
    )
}

/// The scaling of a copy. A 640-wide copy would be read as BT.601 by mpv: the
/// source's matrix is tagged.
fn filter(ffmpeg: &Path, source: &Path) -> String {
    let color = crate::color::proxy_tail(ffmpeg, source)
        .map(|tail| format!(",{tail}"))
        .unwrap_or_default();
    format!(
        "setpts=PTS-STARTPTS,scale=w='max(2,trunc(min(640,640*dar)/2)*2)':h='max(2,trunc(min(640,640/dar)/2)*2)',setsar=1{color}"
    )
}

/// Encodes the copy of `source` into `output` and reads its frame times.
fn build(
    source: &FileStamp,
    ffmpeg: &Path,
    origin: f64,
    filter: &str,
    output: &Path,
    stop: &dyn Fn() -> bool,
    cancel: &AtomicBool,
) -> Result<(u64, Vec<f64>)> {
    let stderr = std::fs::File::create(output.with_extension("log"))?;
    let mut child = crate::command(ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-n",
            "-threads",
            "2",
            "-ss",
            &format!("{origin:.9}"),
            "-i",
        ])
        .arg(&source.resolved)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-sn",
            "-dn",
            "-vf",
            filter,
            "-fps_mode",
            "passthrough",
            "-c:v",
            "libx264",
            "-threads",
            "2",
            "-preset",
            "fast",
            "-crf",
            "18",
            "-g",
            "1",
            "-pix_fmt",
            "yuv420p",
            "-fs",
            &COPY_BYTES.to_string(),
        ])
        .arg(output)
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()?;
    // Closing NOH stops the copy and waits for this encoder; a NOH that is
    // killed takes it along too.
    crate::process::adopt(&child);
    let began = Instant::now();
    let status = loop {
        let late = began.elapsed() > COPY_TIME;
        if stop() || late {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if late {
                TOO_SLOW.into()
            } else {
                "Preview preparation cancelled or timed out".into()
            });
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.into());
            }
        }
    };
    if !status.success() {
        return Err("Preview working copy could not be prepared".into());
    }
    let bytes = std::fs::metadata(output)?.len();
    if bytes >= COPY_BYTES - 1024 * 1024 {
        return Err(TOO_LARGE.into());
    }
    if FileStamp::read(&source.path)? != *source {
        return Err("Preview input changed".into());
    }
    Ok((bytes, frame_times(ffmpeg, output, cancel)?))
}

fn frame_times(ffmpeg: &Path, path: &Path, cancel: &AtomicBool) -> Result<Vec<f64>> {
    let mut command = crate::command(ffmpeg);
    command
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-c:v",
            "copy",
            "-f",
            "framehash",
            "-hash",
            "adler32",
            "-",
        ]);
    let mut time_base = None;
    let mut times = Vec::new();
    let (status, _) =
        crate::process::stream_interruptible(command, Duration::from_secs(15), cancel, |line| {
            if let Some(value) = line.strip_prefix("#tb 0: ") {
                let (num, den) = value.split_once('/').ok_or("Invalid preview time base")?;
                let tb = num.parse::<f64>()? / den.parse::<f64>()?;
                if !tb.is_finite() || tb <= 0.0 {
                    return Err("Invalid preview time base".into());
                }
                time_base = Some(tb);
            } else if !line.starts_with('#') && !line.trim().is_empty() {
                if times.len() >= 1_000_000 {
                    return Err("Preview frame index exceeds its size limit".into());
                }
                let pts = line
                    .split(',')
                    .nth(2)
                    .ok_or("Missing preview PTS")?
                    .trim()
                    .parse::<i64>()?;
                let seconds = pts as f64 * time_base.ok_or("Missing preview time base")?;
                if !seconds.is_finite() || times.last().is_some_and(|last| seconds <= *last) {
                    return Err("Invalid preview frame order".into());
                }
                times.push(seconds);
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    if !status.success() || times.is_empty() {
        return Err("Preview frame index unavailable".into());
    }
    Ok(times)
}

/// A copy in a folder of its own, deleted with it: what every copy was before
/// the store, and what one still is when the store cannot be used.
fn temporary(
    source: &FileStamp,
    ffmpeg: &Path,
    origin: f64,
    filter: &str,
    stop: &dyn Fn() -> bool,
    cancel: &AtomicBool,
) -> Result<Light> {
    let directory = tempfile::Builder::new().prefix("noh-scrub-").tempdir()?;
    let path = directory.path().join("preview.mp4");
    let (bytes, times) = build(source, ffmpeg, origin, filter, &path, stop, cancel)?;
    Ok(Light {
        path,
        bytes,
        times,
        _temporary: Some(directory),
    })
}

/// Finds the copy of `source` or makes it, waiting if it is being made.
/// `origin` is where the source's pictures start (its inspection gives it).
/// `stop` is polled throughout; `cancel` interrupts the reading of frame times.
pub fn prepare(
    source: &FileStamp,
    ffmpeg: &Path,
    origin: f64,
    stop: &dyn Fn() -> bool,
    cancel: &AtomicBool,
) -> Result<Arc<Light>> {
    prepare_in(Store::shared(), source, ffmpeg, origin, stop, cancel)
}
fn prepare_in(
    store: Option<&Store>,
    source: &FileStamp,
    ffmpeg: &Path,
    origin: f64,
    stop: &dyn Fn() -> bool,
    cancel: &AtomicBool,
) -> Result<Arc<Light>> {
    set_state(&source.path, State::Preparing);
    let began = Instant::now();
    let filter = filter(ffmpeg, &source.resolved);
    let made = (|| {
        let Some(store) = store else {
            return temporary(source, ffmpeg, origin, &filter, stop, cancel);
        };
        let signature = store.signature(source, stop)?;
        let key = key(&signature, &format!("{RECIPE}|{origin:.9}|{filter}"));
        let _claim = store.claim(&key, stop)?;
        if let Some(copy) = store.get(&key) {
            crate::profile!("copy: found in the store, {key}");
            return Ok(copy);
        }
        crate::profile!("copy: to be made, {key}");
        // Known to be too large, or too slow today: not encoded again, here or
        // ahead of need.
        if let Some(reason) = store.refused(&key) {
            return Err(reason.into());
        }
        let partial = store.partial();
        let built = build(source, ffmpeg, origin, &filter, &partial, stop, cancel);
        let _ = std::fs::remove_file(partial.with_extension("log"));
        match built {
            Ok((bytes, times)) => Ok(store.put(&key, &partial, bytes, times)),
            Err(error) => {
                let _ = std::fs::remove_file(&partial);
                if let Some(refusal) = error.downcast_ref::<Refusal>() {
                    store.refuse(&key, refusal);
                }
                Err(error)
            }
        }
    })();
    let made = made.map(Arc::new);
    match &made {
        Ok(copy) => crate::profile!(
            "copy: ready in {:.0} ms, {} bytes, {} frames, {} -> {}",
            began.elapsed().as_secs_f64() * 1000.0,
            copy.bytes,
            copy.times.len(),
            source.path.display(),
            copy.path.display()
        ),
        Err(error) => crate::profile!(
            "copy: none after {:.0} ms for {}: {error}",
            began.elapsed().as_secs_f64() * 1000.0,
            source.path.display()
        ),
    }
    set_state(
        &source.path,
        if made.is_ok() {
            State::Ready
        } else {
            State::Absent
        },
    );
    // Only a copy of the store is offered to other readers: a temporary one
    // belongs to whoever asked for it and goes with them.
    if let Ok(copy) = &made
        && copy._temporary.is_none()
    {
        held().lock().unwrap_or_else(|e| e.into_inner()).insert(
            named(&source.path),
            Held {
                source: source.clone(),
                origin,
                copy: copy.clone(),
            },
        );
    }
    made
}

/// Makes copies ahead of need, one at a time, as soon as a source is known.
/// Without a store it does nothing: a copy made ahead would have nowhere to
/// stay, and the preview makes its own on first use, as before. While held it
/// encodes nothing, as the preview decodes nothing during an export. Dropping
/// it stops the copy in progress and waits for its encoder to be gone.
pub struct Warmer {
    store: Option<&'static Store>,
    held: Arc<AtomicBool>,
    sender: Option<mpsc::Sender<(PathBuf, PathBuf)>>,
    worker: Option<thread::JoinHandle<()>>,
    quit: Arc<AtomicBool>,
    asked: Mutex<HashSet<PathBuf>>,
}
impl Warmer {
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        Self::new_in(Store::shared(), wake)
    }
    fn new_in(store: Option<&'static Store>, wake: impl Fn() + Send + 'static) -> Self {
        let quit = Arc::new(AtomicBool::new(false));
        let held = Arc::new(AtomicBool::new(false));
        let mut warmer = Self {
            store,
            held: held.clone(),
            sender: None,
            worker: None,
            quit: quit.clone(),
            asked: Mutex::new(HashSet::new()),
        };
        let Some(store) = store else {
            return warmer;
        };
        let (sender, receiver) = mpsc::channel::<(PathBuf, PathBuf)>();
        let spawned = thread::Builder::new()
            .name("noh-preview-copies".into())
            .spawn(move || {
                while let Ok((source, ffmpeg)) = receiver.recv() {
                    let result = loop {
                        while held.load(Ordering::Relaxed) && !quit.load(Ordering::Relaxed) {
                            thread::sleep(Duration::from_millis(100));
                        }
                        if quit.load(Ordering::Relaxed) {
                            return;
                        }
                        let result = warm(store, &source, &ffmpeg, &quit, &held);
                        // Interrupted by a hold: the same source again, afterwards.
                        if result.is_ok() || !held.load(Ordering::Relaxed) {
                            break result;
                        }
                        set_state(&source, State::Preparing);
                    };
                    if let Err(error) = result {
                        // The source is then read directly, as before.
                        set_state(&source, State::Absent);
                        tracing::debug!("No preview copy of {}: {error}", source.display());
                    }
                    wake();
                }
            });
        match spawned {
            Ok(worker) => {
                warmer.sender = Some(sender);
                warmer.worker = Some(worker);
            }
            Err(error) => tracing::warn!("Preview copies are made on demand only: {error}"),
        }
        warmer
    }
    /// Asks once per session for the copy of the video at `source`.
    pub fn request(&self, source: &Path, ffmpeg: &Path) {
        let Some(sender) = &self.sender else {
            return;
        };
        if !self
            .asked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(named(source))
        {
            return;
        }
        crate::profile!("copy: asked ahead for {}", source.display());
        if sender
            .send((source.to_path_buf(), ffmpeg.to_path_buf()))
            .is_ok()
            && state(source) == State::Absent
        {
            set_state(source, State::Preparing);
        }
    }
    /// Holds the work, the copy in progress included, while `held` is true: the
    /// app sets it during an export or another job, and releases it after.
    pub fn hold(&self, held: bool) {
        if self.held.swap(held, Ordering::Relaxed) != held {
            crate::profile!(
                "copy: making ahead {}",
                if held { "held" } else { "released" }
            );
        }
    }
    /// Whether copies are made ahead of need in this session.
    pub fn active(&self) -> bool {
        self.store.is_some() && self.sender.is_some()
    }
}
impl Drop for Warmer {
    fn drop(&mut self) {
        // Every wait of the worker polls this flag (reading per mebibyte,
        // encoding every 20 ms, a claim every 50 ms), so the join is short. It
        // is what kills the encoder: nothing else would after this process.
        self.quit.store(true, Ordering::Relaxed);
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn warm(
    store: &Store,
    source: &Path,
    ffmpeg: &Path,
    quit: &AtomicBool,
    held: &AtomicBool,
) -> Result<()> {
    let stamp = FileStamp::read(source).map_err(|e| e.to_string())?;
    let ffmpeg = crate::inspection::resolve_ffmpeg(ffmpeg).map_err(|e| e.to_string())?;
    // The same inspection as the preview's, so both name the same copy.
    let timed = crate::media::inspect_timed_interruptible(
        &ffmpeg,
        &stamp.resolved,
        quit,
        Duration::from_secs(15),
    )
    .map_err(|e| e.to_string())?;
    prepare_in(
        Some(store),
        &stamp,
        &ffmpeg,
        timed.video_start,
        &|| quit.load(Ordering::Relaxed) || held.load(Ordering::Relaxed),
        quit,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(folder: &Path, name: &str, content: &[u8]) -> FileStamp {
        let path = folder.join(name);
        std::fs::write(&path, content).unwrap();
        FileStamp::read(&path).unwrap()
    }
    fn file_copy(store: &Store, key: &str, bytes: usize) -> Light {
        let partial = store.partial();
        std::fs::write(&partial, vec![7u8; bytes]).unwrap();
        store.put(key, &partial, bytes as u64, vec![0.0, 0.5])
    }
    fn leaked(folder: &Path) -> &'static Store {
        Box::leak(Box::new(Store::at(folder.to_path_buf(), 1 << 30).unwrap()))
    }
    /// Entries of a store folder other than the index of signatures.
    fn files(folder: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(folder)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "index.json")
            .collect();
        names.sort();
        names
    }
    /// A real FFmpeg and a one-second 320x240 clip made with it, for the tests
    /// that encode. None where no FFmpeg is found: those tests then pass empty.
    fn clip(folder: &Path, name: &str) -> Option<(PathBuf, FileStamp)> {
        let ffmpeg = crate::find_ffmpeg(std::env::var_os("NOH_FFMPEG")).ok()?;
        let path = folder.join(name);
        let made = crate::command(&ffmpeg)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("testsrc2=size=320x240:rate=24:duration=1")
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .ok()?;
        made.success()
            .then(|| (ffmpeg, FileStamp::read(&path).unwrap()))
    }

    #[test]
    fn a_renamed_or_duplicated_file_keeps_its_signature_and_a_changed_one_does_not() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::at(folder.path().join("store"), 1 << 20).unwrap();
        let never = || false;
        let first = stamp(folder.path(), "first.mp4", b"the same pictures");
        let signature = store.signature(&first, &never).unwrap();
        // SHA-256 of the content, whatever the name.
        assert_eq!(
            signature,
            hex(Sha256::digest(b"the same pictures").as_slice())
        );
        let renamed = folder.path().join("renamed.mp4");
        std::fs::rename(&first.path, &renamed).unwrap();
        let renamed = FileStamp::read(&renamed).unwrap();
        assert_eq!(store.signature(&renamed, &never).unwrap(), signature);
        let twin = stamp(folder.path(), "twin.mp4", b"the same pictures");
        assert_eq!(store.signature(&twin, &never).unwrap(), signature);
        let other = stamp(folder.path(), "first.mp4", b"other pictures");
        assert_ne!(store.signature(&other, &never).unwrap(), signature);
    }

    #[test]
    fn an_unchanged_file_is_not_read_again_and_a_stopped_reading_fails() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::at(folder.path().join("store"), 1 << 20).unwrap();
        let source = stamp(folder.path(), "clip.mp4", b"pictures");
        assert!(store.signature(&source, &|| true).is_err());
        let signature = store.signature(&source, &|| false).unwrap();
        // Known by path, size and dates: answered without reading, so `stop`
        // is never asked.
        assert_eq!(store.signature(&source, &|| true).unwrap(), signature);
        // Another creation date is another file, as for `FileStamp`.
        let mut recreated = source.clone();
        recreated.created = Some(UNIX_EPOCH);
        assert!(store.signature(&recreated, &|| true).is_err());
    }

    #[test]
    fn a_copy_is_found_by_its_key_only_when_complete() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::at(folder.path().to_path_buf(), 1 << 20).unwrap();
        let name = key("abc", "recipe");
        assert_ne!(name, key("abc", "another recipe"));
        assert!(store.get(&name).is_none());
        let copy = file_copy(&store, &name, 10);
        assert_eq!(copy.path, store.video(&name));
        let found = store.get(&name).unwrap();
        assert_eq!((found.bytes, found.times.len()), (10, 2));
        // Truncated behind the store's back: no longer a copy.
        std::fs::write(store.video(&name), b"cut").unwrap();
        assert!(store.get(&name).is_none());
    }

    #[test]
    fn a_copy_filed_meanwhile_by_another_process_is_used_whole() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::at(folder.path().to_path_buf(), 1 << 20).unwrap();
        let first = file_copy(&store, "key", 10);
        // The same key built a second time, differently, while the first is
        // open (as a player holds it): the rename is refused on Windows and
        // replaces the file elsewhere. Either way the copy returned is one
        // file with ITS OWN description, and no partial stays.
        let held = std::fs::File::open(&first.path).unwrap();
        let partial = store.partial();
        std::fs::write(&partial, vec![9u8; 20]).unwrap();
        let second = store.put("key", &partial, 20, vec![0.0, 0.25, 0.5]);
        drop(held);
        assert_eq!(second.path, store.video("key"));
        assert_eq!(std::fs::metadata(&second.path).unwrap().len(), second.bytes);
        assert_eq!(second.times.len(), if second.bytes == 10 { 2 } else { 3 });
        assert_eq!(files(folder.path()), ["key.json", "key.mp4"]);
    }

    #[test]
    fn the_store_drops_the_least_recently_used_copies_but_none_used_in_this_session() {
        let folder = tempfile::tempdir().unwrap();
        let write = |key: &str, age: u64| {
            let path = folder.path().join(format!("{key}.mp4"));
            std::fs::write(&path, vec![1u8; 400]).unwrap();
            std::fs::write(
                folder.path().join(format!("{key}.json")),
                br#"{"bytes":400,"times":[0.0]}"#,
            )
            .unwrap();
            let file = std::fs::File::options().append(true).open(&path).unwrap();
            file.set_modified(SystemTime::now() - Duration::from_secs(age))
                .unwrap();
        };
        write("oldest", 300);
        write("older", 200);
        write("recent", 100);
        let store = Store::at(folder.path().to_path_buf(), 1000).unwrap();
        // The oldest is in use by this session: the next oldest leaves instead.
        assert!(store.get("oldest").is_some());
        write("oldest", 300);
        store.trim();
        assert!(store.get("oldest").is_some());
        assert!(store.get("older").is_none());
        assert!(store.get("recent").is_some());
        assert!(!folder.path().join("older.json").exists());
    }

    #[test]
    fn a_copy_is_asked_for_just_before_its_first_frame_at_or_after_a_time() {
        let copy = Light {
            path: PathBuf::new(),
            bytes: 0,
            // 24 pictures a second with one gap, as a variable-rate source has.
            times: vec![0.0, 1.0 / 24.0, 2.0 / 24.0, 0.5, 13.0 / 24.0],
            _temporary: None,
        };
        let frame = |seconds: f64| {
            let asked = copy.seek(seconds);
            // What FFmpeg then outputs first: the first frame not before `asked`.
            copy.times.iter().position(|pts| *pts >= asked).unwrap()
        };
        assert_eq!(copy.seek(0.0), 0.0);
        assert_eq!(frame(0.0), 0);
        // A time written with nine decimals still lands on its own frame.
        assert_eq!(frame(0.041666667), 1);
        assert_eq!(frame(0.041666666), 1);
        assert_eq!(frame(0.05), 2);
        assert_eq!(frame(0.3), 3);
        assert_eq!(frame(0.5), 3);
        assert_eq!(frame(9.0), 4);
    }

    #[test]
    fn a_second_maker_of_the_same_copy_waits_for_the_first() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::at(folder.path().to_path_buf(), 1 << 20).unwrap();
        let first = store.claim("key", &|| false).unwrap();
        // Still claimed: a caller told to stop gives up instead of waiting.
        assert!(store.claim("key", &|| true).is_err());
        assert!(store.claim("another", &|| true).is_ok());
        drop(first);
        assert!(store.claim("key", &|| true).is_ok());
    }

    #[test]
    fn the_interface_sees_a_copy_being_prepared_then_ready_or_absent() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("clip.mp4");
        assert_eq!(state(&source), State::Absent);
        set_state(&source, State::Preparing);
        assert_eq!(state(&source), State::Preparing);
        set_state(&source, State::Ready);
        assert_eq!(state(&source), State::Ready);
        set_state(&source, State::Absent);
        assert_eq!(state(&source), State::Absent);
    }

    #[test]
    fn a_source_that_cannot_be_encoded_leaves_nothing_behind_and_reads_as_absent() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::at(folder.path().join("store"), 1 << 20).unwrap();
        let source = stamp(folder.path(), "clip.mp4", b"not a video");
        let cancel = AtomicBool::new(false);
        let made = prepare_in(
            Some(&store),
            &source,
            Path::new("no-such-ffmpeg"),
            0.0,
            &|| false,
            &cancel,
        );
        assert!(made.is_err());
        assert_eq!(state(&source.path), State::Absent);
        // A failure that may not repeat (no encoder, a cancellation) is not
        // remembered either: the next attempt encodes again.
        assert!(files(&folder.path().join("store")).is_empty());
    }

    #[test]
    fn a_source_known_to_have_no_possible_copy_is_not_encoded_again() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::at(folder.path().join("store"), 1 << 20).unwrap();
        let source = stamp(folder.path(), "long.mp4", b"hours of pictures");
        let cancel = AtomicBool::new(false);
        let ffmpeg = Path::new("no-such-ffmpeg");
        let signature = store.signature(&source, &|| false).unwrap();
        let name = key(
            &signature,
            &format!("{RECIPE}|{:.9}|{}", 0.0, filter(ffmpeg, &source.resolved)),
        );
        store.refuse(&name, &TOO_LARGE);
        // The encoder is never started: with this FFmpeg the error would be a
        // spawn failure, not the remembered reason.
        let error = prepare_in(Some(&store), &source, ffmpeg, 0.0, &|| false, &cancel)
            .err()
            .unwrap()
            .to_string();
        assert_eq!(error, "Preview working copy exceeds its size limit");
        // Only a refusal is remembered, and its size not for ever.
        let typed: Box<dyn std::error::Error + Send + Sync> = TOO_LARGE.into();
        assert!(typed.downcast_ref::<Refusal>().is_some());
        let plain: Box<dyn std::error::Error + Send + Sync> = "cancelled".into();
        assert!(plain.downcast_ref::<Refusal>().is_none());
        let file = std::fs::File::options()
            .append(true)
            .open(store.refusal(&name))
            .unwrap();
        file.set_modified(SystemTime::now() - REFUSAL_AGE - Duration::from_secs(60))
            .unwrap();
        assert!(store.refused(&name).is_none());
        store.trim();
        assert!(files(&folder.path().join("store")).is_empty());
    }

    #[test]
    fn a_copy_that_was_only_slow_is_tried_again_by_the_next_session() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::at(folder.path().join("store"), 1 << 20).unwrap();
        let source = stamp(folder.path(), "busy.mp4", b"pictures");
        let cancel = AtomicBool::new(false);
        let ffmpeg = Path::new("no-such-ffmpeg");
        let signature = store.signature(&source, &|| false).unwrap();
        let recipe = format!("{RECIPE}|{:.9}|{}", 0.0, filter(ffmpeg, &source.resolved));
        let name = key(&signature, &recipe);
        store.refuse(&name, &TOO_SLOW);
        // Not encoded again in this session…
        let error = prepare_in(Some(&store), &source, ffmpeg, 0.0, &|| false, &cancel)
            .err()
            .unwrap()
            .to_string();
        assert_eq!(error, TOO_SLOW.reason);
        // …and nothing on disk says so: another session finds no refusal.
        assert!(files(&folder.path().join("store")).is_empty());
        let next = Store::at(folder.path().join("store"), 1 << 20).unwrap();
        assert!(next.refused(&name).is_none());
    }

    #[test]
    fn a_held_warmer_encodes_nothing_until_released() {
        let folder = tempfile::tempdir().unwrap();
        let store = leaked(&folder.path().join("store"));
        let source = stamp(folder.path(), "clip.mp4", b"not a video");
        let warmer = Warmer::new_in(Some(store), || {});
        warmer.hold(true);
        warmer.request(&source.path, Path::new("no-such-ffmpeg"));
        thread::sleep(Duration::from_millis(300));
        // Still waiting: with this FFmpeg an attempt would already have failed.
        assert_eq!(state(&source.path), State::Preparing);
        warmer.hold(false);
        let began = Instant::now();
        while state(&source.path) == State::Preparing && began.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(state(&source.path), State::Absent);
        // A warmer dropped while held ends too.
        warmer.hold(true);
        drop(warmer);
    }

    #[test]
    fn without_a_store_nothing_is_made_ahead_of_need() {
        let folder = tempfile::tempdir().unwrap();
        let source = stamp(folder.path(), "clip.mp4", b"pictures");
        let warmer = Warmer::new_in(None, || {});
        assert!(!warmer.active());
        warmer.request(&source.path, Path::new("no-such-ffmpeg"));
        // No thread, no encoding, and the tile shows no preparation.
        assert_eq!(state(&source.path), State::Absent);
        assert!(warmer.worker.is_none());
    }

    #[test]
    fn dropping_the_warmer_ends_its_worker_before_returning() {
        let folder = tempfile::tempdir().unwrap();
        let store = leaked(&folder.path().join("store"));
        let source = stamp(folder.path(), "clip.mp4", b"not a video");
        let alive = Arc::new(());
        let held = alive.clone();
        let warmer = Warmer::new_in(Some(store), move || {
            let _ = &held;
        });
        assert!(warmer.active());
        warmer.request(&source.path, Path::new("no-such-ffmpeg"));
        assert_eq!(Arc::strong_count(&alive), 2);
        drop(warmer);
        // The worker owned the other reference: it has ended, joined by Drop.
        assert_eq!(Arc::strong_count(&alive), 1);
        assert!(files(&folder.path().join("store")).is_empty());
    }

    #[test]
    fn a_copy_is_made_once_then_found_again_under_any_name_of_its_source() {
        let folder = tempfile::tempdir().unwrap();
        let Some((ffmpeg, source)) = clip(folder.path(), "clip.mp4") else {
            return;
        };
        let store = Store::at(folder.path().join("store"), 1 << 30).unwrap();
        let cancel = AtomicBool::new(false);
        let made = prepare_in(Some(&store), &source, &ffmpeg, 0.0, &|| false, &cancel).unwrap();
        assert_eq!(state(&source.path), State::Ready);
        assert_eq!(made.times.len(), 24);
        assert_eq!(std::fs::metadata(&made.path).unwrap().len(), made.bytes);
        let stored = files(&folder.path().join("store"));
        assert_eq!(stored.len(), 2, "{stored:?}");
        // The same content under another name: the same file, and no encoding.
        let renamed = folder.path().join("renamed.mp4");
        std::fs::copy(&source.path, &renamed).unwrap();
        let renamed = FileStamp::read(&renamed).unwrap();
        let unusable = Path::new("no-such-ffmpeg");
        // Its colour tail is read with the real FFmpeg and remembered by path;
        // the lookup then runs with an FFmpeg that could not encode anything.
        let _ = filter(&ffmpeg, &renamed.resolved);
        let found = prepare_in(Some(&store), &renamed, unusable, 0.0, &|| false, &cancel).unwrap();
        assert_eq!(found.path, made.path);
        assert_eq!(files(&folder.path().join("store")), stored);
        // A reader that never waits gets it under either name, and only for the
        // file and the origin it was made from.
        assert_eq!(ready(&source, 0.0).unwrap().path, made.path);
        assert_eq!(ready(&renamed, 0.0).unwrap().path, made.path);
        assert!(ready(&source, 0.5).is_none());
        let mut touched = source.clone();
        touched.bytes += 1;
        assert!(ready(&touched, 0.0).is_none());
        // Its file gone: no longer offered, and the tile no longer says ready.
        std::fs::remove_file(&made.path).unwrap();
        assert!(ready(&source, 0.0).is_none());
        assert_eq!(state(&source.path), State::Absent);
    }

    #[test]
    fn without_a_store_a_copy_lives_in_a_folder_of_its_own() {
        let folder = tempfile::tempdir().unwrap();
        let Some((ffmpeg, source)) = clip(folder.path(), "clip.mp4") else {
            return;
        };
        let cancel = AtomicBool::new(false);
        let made = prepare_in(None, &source, &ffmpeg, 0.0, &|| false, &cancel).unwrap();
        // Not offered to other readers: it goes with whoever asked for it.
        assert!(ready(&source, 0.0).is_none());
        let directory = made.path.parent().unwrap().to_path_buf();
        assert!(made.path.is_file());
        drop(made);
        assert!(!directory.exists());
    }

    #[test]
    #[ignore = "opt-in measurement of the signature's reading rate; requires NOH_HASH_FILE"]
    fn signature_reading_rate() {
        let path = PathBuf::from(std::env::var_os("NOH_HASH_FILE").expect("NOH_HASH_FILE"));
        let bytes = std::fs::metadata(&path).unwrap().len();
        for pass in 1..=3 {
            let began = Instant::now();
            let signature = hash(&path, &|| false).unwrap();
            let seconds = began.elapsed().as_secs_f64();
            println!(
                "pass {pass}: {bytes} bytes in {seconds:.2} s = {:.0} MB/s ({})",
                bytes as f64 / 1e6 / seconds,
                &signature[..12]
            );
        }
    }
}
