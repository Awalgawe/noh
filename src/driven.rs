//! The private channel between `noh-mcp` and the NOH windows it opens
//! (`noh-app --driven`): the window's standard input and output, unnamed
//! pipes only the parent holds. JSON, one message per line, each carrying the
//! protocol version and its `type`. Nothing listens, nothing is discovered.
//!
//! Both ends share the bounded line reader and the writer here. A writer never
//! blocks its caller: lines go through a bounded queue to a thread, which says
//! since when a write has been pending (a reader that stopped reading).
use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

/// The protocol version every message carries (`"v"`).
pub const VERSION: u32 = 1;
/// The longest line the window reads: an action may be as large as an MCP
/// request (docs/MCP.md, 4 MiB).
pub const MAX_TO_APP: usize = 4 * 1024 * 1024;
/// The longest line the server reads from a window.
pub const MAX_TO_SERVER: usize = 64 * 1024;
/// The first argument that makes `noh-app` a driven window.
pub const ARGUMENT: &str = "--driven";
/// The argument before the title the agent gave the window.
pub const TITLE_ARGUMENT: &str = "--title";
/// The longest title kept, in characters.
pub const MAX_TITLE: usize = 80;

/// From the server to a window.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToApp {
    /// Close the window, as the agent asks (`noh_app_close`): nothing unsaved
    /// is discarded; the answer is a `done` with the same id.
    Close { id: u64 },
    /// The agent's session ended: the window stays open, as an ordinary NOH.
    SessionEnd,
    /// One gesture of the agent, applied between two frames as the click it
    /// stands for; answered by `done` with the state revision it led to.
    Act { id: u64, action: Action },
    /// A PNG of what the window shows, written to `output` (a free path).
    Look {
        id: u64,
        #[serde(default)]
        scope: LookScope,
        /// Move the playhead there first, as `seek` would; else the screen as it is.
        #[serde(default)]
        at_ms: Option<u64>,
        output: std::path::PathBuf,
    },
    /// The preparation turn (note §5): `go`, this window may make its light
    /// copies and decode its waveform now; else it holds them. One window
    /// prepares at a time, never during a heavy job.
    Prepare { go: bool },
}

/// What `look` captures.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[cfg_attr(feature = "mcp", schemars(crate = "rmcp::schemars"))]
#[serde(rename_all = "snake_case")]
pub enum LookScope {
    /// The whole window, as the person sees it.
    #[default]
    Window,
    /// The monitor alone (the preview, captions drawn over it).
    Monitor,
}

/// A change to one lyrics line, by its rank in the lyrics (0 first).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[cfg_attr(feature = "mcp", schemars(crate = "rmcp::schemars"))]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum LineEdit {
    SetText {
        index: usize,
        text: String,
    },
    /// Move one edge to `ms` (song time); the line keeps a minimum length.
    SetStart {
        index: usize,
        ms: u64,
    },
    SetEnd {
        index: usize,
        ms: u64,
    },
    /// Move the whole line by `delta_ms`.
    Shift {
        index: usize,
        delta_ms: i64,
    },
    /// A new line starting at `ms`, in a gap long enough for it.
    Add {
        ms: u64,
        text: String,
    },
    Delete {
        index: usize,
    },
}

/// One answer to a question of the application. Answers that rewrite or
/// replace a file need `user_confirmed`: ask the person in the conversation
/// first.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[cfg_attr(feature = "mcp", schemars(crate = "rmcp::schemars"))]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// first_edit: change the .srt in place (the original is not kept).
    ModifySrt,
    /// first_edit: nothing is written, the edit that asked is taken back.
    Cancel,
    /// lyrics_truth: keep the lyrics file, rewrite the .srt from it.
    KeepLyrics,
    /// lyrics_truth: keep the .srt, rewrite the lyrics file from it.
    KeepSrt,
    /// lyrics_truth: decide later (the lyrics stay unopened or read-only).
    Later,
    /// first_edit: write the change to a new .srt at `path` (a free absolute
    /// path), which becomes the attached lyrics; the original is kept.
    WorkOnCopy,
    /// lyrics_action: go on with the lyrics action, dropping the unsaved
    /// changes (needs user_confirmed). unsaved_on_close: quit without saving
    /// (needs user_confirmed). close_while_working: stop the job and quit
    /// (needs user_confirmed).
    Discard,
    /// unsaved_on_close: save, then quit.
    Save,
    /// save_failed: try the write again.
    Retry,
    /// save_failed: close the message (the change stays unsaved).
    Dismiss,
}

impl Answer {
    /// Whether this answer drops work or rewrites a file the person has.
    pub fn destructive(self) -> bool {
        matches!(
            self,
            Answer::ModifySrt | Answer::KeepLyrics | Answer::KeepSrt | Answer::Discard
        )
    }
}

/// A gesture of the agent. Each calls what the matching click calls.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "mcp", derive(rmcp::schemars::JsonSchema))]
#[cfg_attr(feature = "mcp", schemars(crate = "rmcp::schemars"))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Add files as a drop would: videos and pictures to the montage, a WAV
    /// as the song, an .srt or lyrics file as the lyrics. Absolute paths.
    Add {
        paths: Vec<std::path::PathBuf>,
    },
    /// A picture's duration in the montage, by its clip id (`state.clips`).
    SetPhotoDuration {
        clip_id: u64,
        seconds: f64,
    },
    /// One lyrics line. `based_on`: the lyrics revision read in `state`; if
    /// the lyrics were reloaded since (changed on disk), nothing is changed.
    EditLine {
        edit: LineEdit,
        #[serde(default)]
        based_on: Option<u64>,
    },
    /// Undo or redo the last lyrics change.
    Undo {},
    Redo {},
    /// The caption effect, by id and preset (`noh_list_effects`); no id for none.
    ChooseEffect {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        preset: Option<String>,
    },
    /// The short's safe area.
    SetSafeArea {
        area: crate::safe_area::SafeArea,
    },
    /// The short's interval on the song; both absent to remove it.
    SetRange {
        #[serde(default)]
        start_ms: Option<u64>,
        #[serde(default)]
        end_ms: Option<u64>,
    },
    /// Move the playhead (paused there). Waits while the person navigates.
    Seek {
        ms: u64,
    },
    Play {},
    Pause {},
    /// Answer the open question (`state.question`). `path`: for work_on_copy.
    Answer {
        answer: Answer,
        #[serde(default)]
        user_confirmed: bool,
        #[serde(default)]
        path: Option<std::path::PathBuf>,
    },
    /// Remove a clip from the montage, by its id.
    RemoveClip {
        clip_id: u64,
    },
    /// Move a clip to `to_index` in the montage (0 first).
    MoveClip {
        clip_id: u64,
        to_index: usize,
    },
    /// The picture fades in seconds (0: none) and whether the clips' own sound
    /// is mixed in; what is absent stays as it is.
    SetFades {
        #[serde(default)]
        fade_in_seconds: Option<f64>,
        #[serde(default)]
        fade_out_seconds: Option<f64>,
        #[serde(default)]
        clip_audio: Option<bool>,
    },
    /// Convert every clip (true), or only those that need it (false, automatic).
    SetConvertAll {
        convert_all: bool,
    },
    /// The chosen effect's preset and settings (`noh_list_effects`), Classique's
    /// size and placement, and whether the captions are drawn on the video.
    SetEffectSettings {
        #[serde(default)]
        preset: Option<String>,
        #[serde(default)]
        values: std::collections::BTreeMap<String, serde_json::Value>,
        #[serde(default)]
        size: Option<crate::captions::CaptionSize>,
        #[serde(default)]
        placement: Option<crate::captions::CaptionPlacement>,
        #[serde(default)]
        show_on_video: Option<bool>,
    },
    /// The short's framing and whether its loop restarts the montage.
    SetFraming {
        #[serde(default)]
        framing: Option<crate::shorts::Framing>,
        #[serde(default)]
        restart_loops: Option<bool>,
    },
    /// Where the export goes: an absolute .mp4 path; `short_name`, the short's
    /// file name in the same folder.
    SetDestination {
        #[serde(default)]
        path: Option<std::path::PathBuf>,
        #[serde(default)]
        short_name: Option<String>,
    },
    /// Export the montage, or the short (`short`). Started or not, the answer
    /// says; the progress and the end follow in `state`.
    Export {
        #[serde(default)]
        short: bool,
    },
    /// Cancel the job running.
    Cancel {},
}

/// A line read by a window.
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    Message(ToApp),
    /// A message this window cannot serve (a later protocol step): when it
    /// carries an id, its `done` says so.
    Unsupported {
        id: Option<u64>,
    },
}

/// Why a window refused a request, in `done`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Refusal {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl Refusal {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }
}

/// From a window to the server.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToServer {
    /// The window is open and reads its input.
    Ready { pid: u32, version: String },
    /// The answer to the request `id`: its result, or why it was refused.
    Done {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<Refusal>,
    },
    /// The person stopped the piloting (« Arrêter le pilotage », or a close
    /// of the window, which stops it first): no request is served any more.
    Released,
    /// The person closed the window.
    Closed,
    /// The window could not start (no OpenGL 2.0, no display…): it ends
    /// without showing the dialog an ordinary start shows, since nobody may
    /// be there to close it.
    Failed { reason: String },
    /// The window has copies or a waveform to prepare and waits for its turn;
    /// `urgent`: the person plays it and waits for them.
    NeedsPrepare { urgent: bool },
    /// Nothing left to prepare: the turn can go to the next window.
    Prepared,
    /// The heavy job the agent started here ended: `succeeded`, `failed` or
    /// `cancelled`. Sent even after the person took the window back.
    JobDone { outcome: String },
    /// What the window shows, as data; only the latest counts.
    State {
        revision: u64,
        state: serde_json::Value,
    },
}

/// One line for `message`, version included, newline-terminated.
pub fn line<T: Serialize>(message: &T) -> Vec<u8> {
    let mut value = serde_json::to_value(message).expect("protocol messages serialize");
    if let serde_json::Value::Object(map) = &mut value {
        map.insert("v".into(), VERSION.into());
    }
    let mut bytes = serde_json::to_vec(&value).expect("a JSON value serializes");
    bytes.push(b'\n');
    bytes
}

/// The JSON object of a line of this protocol version, else nothing.
fn object(line: &[u8]) -> Option<serde_json::Map<String, serde_json::Value>> {
    match serde_json::from_slice::<serde_json::Value>(line).ok()? {
        serde_json::Value::Object(map)
            if map.get("v").and_then(serde_json::Value::as_u64) == Some(u64::from(VERSION)) =>
        {
            Some(map)
        }
        _ => None,
    }
}

/// A line read by a window. Not JSON, another version, no type: ignored.
pub fn parse_to_app(line: &[u8]) -> Option<Inbound> {
    let mut map = object(line)?;
    map.get("type")?.as_str()?;
    let id = map.get("id").and_then(serde_json::Value::as_u64);
    map.remove("v");
    Some(
        match serde_json::from_value::<ToApp>(serde_json::Value::Object(map)) {
            Ok(message) => Inbound::Message(message),
            Err(_) => Inbound::Unsupported { id },
        },
    )
}

/// A line read by the server. Anything else a window writes is ignored.
pub fn parse_to_server(line: &[u8]) -> Option<ToServer> {
    let mut map = object(line)?;
    map.remove("v");
    serde_json::from_value(serde_json::Value::Object(map)).ok()
}

/// The title the agent gave, as the window shows it: one line, bounded.
pub fn clean_title(title: &str) -> String {
    title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TITLE)
        .collect()
}

/// Windows: this process's own standard handles are not passed on to the
/// children it starts. Whoever started it with pipes made them inheritable,
/// and a child started with inheritance (as every child with pipes is) would
/// get them too: a window started by noh-mcp would keep the MCP client's pipe
/// open after the server ends; a helper started by a driven window would hold
/// its channel. A child given `Stdio::inherit` still gets a copy made for it.
pub fn keep_standard_handles_private() {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        const HANDLE_FLAG_INHERIT: u32 = 1;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(which: u32) -> *mut c_void;
            fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
        }
        // STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE.
        for which in [-10i32, -11, -12] {
            let handle = unsafe { GetStdHandle(which as u32) };
            if !handle.is_null() && handle as isize != -1 {
                unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
            }
        }
    }
}

/// Lines of at most `limit` bytes (newline excluded). A longer line is
/// skipped to its end, never kept whole in memory.
pub struct Lines<R> {
    reader: BufReader<R>,
    limit: usize,
}

impl<R: Read> Lines<R> {
    pub fn new(reader: R, limit: usize) -> Self {
        Self {
            reader: BufReader::new(reader),
            limit,
        }
    }

    /// The next line within the limit; `None` at the end of the input.
    pub fn next_line(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut line = Vec::new();
        let mut skipping = false;
        loop {
            let available = match self.reader.fill_buf() {
                Ok(available) => available,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if available.is_empty() {
                // The end: a last line without newline still counts.
                return Ok((!skipping && !line.is_empty()).then_some(line));
            }
            let (taken, end) = match available.iter().position(|&b| b == b'\n') {
                Some(at) => (at + 1, Some(at)),
                None => (available.len(), None),
            };
            if !skipping {
                line.extend_from_slice(&available[..end.unwrap_or(taken)]);
                if line.len() > self.limit {
                    skipping = true;
                    line = Vec::new();
                }
            }
            self.reader.consume(taken);
            if end.is_some() {
                if skipping {
                    skipping = false;
                    continue;
                }
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(line));
            }
        }
    }
}

/// Read `reader` line by line on a thread of its own: each line within
/// `limit` goes to `on_line`; `on_end` runs once at the end of the input or
/// on a read error.
pub fn spawn_reader<R, L, E>(name: &str, reader: R, limit: usize, mut on_line: L, on_end: E)
where
    R: Read + Send + 'static,
    L: FnMut(Vec<u8>) + Send + 'static,
    E: FnOnce() + Send + 'static,
{
    let started = thread::Builder::new().name(name.into()).spawn(move || {
        let mut lines = Lines::new(reader, limit);
        while let Ok(Some(line)) = lines.next_line() {
            on_line(line);
        }
        on_end();
    });
    if let Err(error) = started {
        tracing::warn!("driven channel reader not started: {error}");
    }
}

/// Why a line was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsent {
    /// The queue is full: the reader is not reading.
    Full,
    /// The writer stopped (the pipe broke) or was closed.
    Gone,
}

#[derive(Default)]
struct WriterState {
    /// Since when the line being written has been pending.
    since: Option<Instant>,
    /// The writer thread ended.
    ended: bool,
}

/// The sending end of a writer thread. Never blocks.
pub struct Outbox {
    queue: Option<SyncSender<Vec<u8>>>,
    state: Arc<(Mutex<WriterState>, Condvar)>,
}

impl Outbox {
    /// A writer thread over `writer`, `capacity` lines queued at most.
    pub fn spawn<W: Write + Send + 'static>(name: &str, writer: W, capacity: usize) -> Self {
        let (queue, lines) = mpsc::sync_channel::<Vec<u8>>(capacity);
        let state = Arc::new((Mutex::new(WriterState::default()), Condvar::new()));
        let shared = state.clone();
        let started = thread::Builder::new().name(name.into()).spawn(move || {
            let mut writer = writer;
            for line in lines {
                shared.0.lock().unwrap().since = Some(Instant::now());
                let written = writer.write_all(&line).and_then(|()| writer.flush());
                shared.0.lock().unwrap().since = None;
                if written.is_err() {
                    break;
                }
            }
            // Dropping the writer closes the pipe.
            drop(writer);
            let (lock, ended) = &*shared;
            lock.lock().unwrap().ended = true;
            ended.notify_all();
        });
        if started.is_err() {
            let (lock, _) = &*state;
            lock.lock().unwrap().ended = true;
        }
        Self {
            queue: Some(queue),
            state,
        }
    }

    pub fn send<T: Serialize>(&self, message: &T) -> Result<(), Unsent> {
        if self.state.0.lock().unwrap().ended {
            return Err(Unsent::Gone);
        }
        let Some(queue) = &self.queue else {
            return Err(Unsent::Gone);
        };
        queue.try_send(line(message)).map_err(|error| match error {
            TrySendError::Full(_) => Unsent::Full,
            TrySendError::Disconnected(_) => Unsent::Gone,
        })
    }

    /// How long the line being written has been pending, if one is.
    pub fn stalled_for(&self) -> Option<Duration> {
        self.state.0.lock().unwrap().since.map(|at| at.elapsed())
    }

    /// The writer thread ended (the pipe broke, or `close`).
    pub fn gone(&self) -> bool {
        self.state.0.lock().unwrap().ended
    }

    /// Stop queuing; the writer writes what is queued, then closes the pipe.
    /// Waits `limit` at most; true when the pipe is closed.
    pub fn close(&mut self, limit: Duration) -> bool {
        self.queue = None;
        let (lock, ended) = &*self.state;
        let state = lock.lock().unwrap();
        let (state, _) = ended
            .wait_timeout_while(state, limit, |s| !s.ended)
            .unwrap();
        state.ended
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8], limit: usize) -> Vec<Vec<u8>> {
        let mut reader = Lines::new(input, limit);
        let mut out = Vec::new();
        while let Some(line) = reader.next_line().unwrap() {
            out.push(line);
        }
        out
    }

    #[test]
    fn lines_are_bounded_and_a_longer_one_is_skipped_whole() {
        let long = vec![b'x'; 40];
        let mut input = b"one\r\n".to_vec();
        input.extend_from_slice(&long);
        input.extend_from_slice(b"\ntwo\nlast");
        assert_eq!(
            lines(&input, 16),
            vec![b"one".to_vec(), b"two".to_vec(), b"last".to_vec()]
        );
        // A long line split across the reader's buffer refills is skipped too.
        let mut big = vec![b'y'; 20_000];
        big.extend_from_slice(b"\nok\n");
        assert_eq!(lines(&big, 1024), vec![b"ok".to_vec()]);
        // At the limit exactly, the line is kept.
        assert_eq!(lines(b"abcd\n", 4), vec![b"abcd".to_vec()]);
        // An over-long last line without newline is dropped.
        assert!(lines(b"abcdef", 4).is_empty());
    }

    #[test]
    fn messages_carry_the_version_and_round_trip() {
        let close = line(&ToApp::Close { id: 7 });
        let text = String::from_utf8(close.clone()).unwrap();
        assert!(text.ends_with('\n'));
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(value["v"], 1);
        assert_eq!(value["type"], "close");
        assert_eq!(
            parse_to_app(text.trim().as_bytes()),
            Some(Inbound::Message(ToApp::Close { id: 7 }))
        );
        assert_eq!(
            parse_to_app(&line(&ToApp::SessionEnd)[..]),
            Some(Inbound::Message(ToApp::SessionEnd))
        );
        for message in [
            ToServer::Ready {
                pid: 42,
                version: "0.3.1".into(),
            },
            ToServer::Done {
                id: 3,
                result: Some(serde_json::json!({ "state": "closed" })),
                error: None,
            },
            ToServer::Done {
                id: 4,
                result: None,
                error: Some(Refusal::new("app.question_open", "A question is open.")),
            },
            ToServer::Released,
            ToServer::Closed,
        ] {
            assert_eq!(parse_to_server(&line(&message)), Some(message));
        }
    }

    #[test]
    fn gestures_and_looks_round_trip_and_reject_unknown_fields() {
        let act = ToApp::Act {
            id: 2,
            action: Action::EditLine {
                edit: LineEdit::SetText {
                    index: 3,
                    text: "New words".into(),
                },
                based_on: Some(4),
            },
        };
        assert_eq!(parse_to_app(&line(&act)), Some(Inbound::Message(act)));
        let look = ToApp::Look {
            id: 3,
            scope: LookScope::Monitor,
            at_ms: Some(1500),
            output: "look.png".into(),
        };
        assert_eq!(parse_to_app(&line(&look)), Some(Inbound::Message(look)));
        // An unknown field in an action is not served: it answers unsupported.
        assert_eq!(
            parse_to_app(br#"{"v":1,"type":"act","id":5,"action":{"type":"play","speed":2}}"#),
            Some(Inbound::Unsupported { id: Some(5) })
        );
        let state = ToServer::State {
            revision: 9,
            state: serde_json::json!({ "duration_ms": 1000 }),
        };
        assert_eq!(parse_to_server(&line(&state)), Some(state));
    }

    #[test]
    fn foreign_lines_are_ignored_and_unknown_requests_named() {
        // Not JSON, another version, no version, no type: nothing.
        for text in [
            "hello",
            r#"{"v":2,"type":"close","id":1}"#,
            r#"{"type":"close","id":1}"#,
            r#"{"v":1,"id":1}"#,
            r#"[1,2]"#,
        ] {
            assert_eq!(parse_to_app(text.as_bytes()), None, "{text}");
            assert_eq!(parse_to_server(text.as_bytes()), None, "{text}");
        }
        // A later step's request: unsupported, with its id when it has one.
        assert_eq!(
            parse_to_app(br#"{"v":1,"type":"act","id":9,"action":{}}"#),
            Some(Inbound::Unsupported { id: Some(9) })
        );
        assert_eq!(
            parse_to_app(br#"{"v":1,"type":"later_step","go":true}"#),
            Some(Inbound::Unsupported { id: None })
        );
        assert_eq!(
            parse_to_app(br#"{"v":1,"type":"prepare","go":true}"#),
            Some(Inbound::Message(ToApp::Prepare { go: true }))
        );
        // A known type with a malformed body is not served either.
        assert_eq!(
            parse_to_app(br#"{"v":1,"type":"close","id":"x"}"#),
            Some(Inbound::Unsupported { id: None })
        );
    }

    #[test]
    fn titles_are_one_bounded_line() {
        assert_eq!(clean_title("  Clip\n\tof   the day "), "Clip of the day");
        assert_eq!(clean_title(&"é".repeat(200)).chars().count(), MAX_TITLE);
    }

    /// A writer that blocks until told to go on.
    struct Gate(Arc<(Mutex<bool>, Condvar)>, Arc<Mutex<Vec<u8>>>);
    impl Write for Gate {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let (open, opened) = &*self.0;
            let _open = opened.wait_while(open.lock().unwrap(), |o| !*o).unwrap();
            self.1.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_stalled_reader_never_blocks_the_sender() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut outbox = Outbox::spawn("test-writer", Gate(gate.clone(), written.clone()), 2);
        // One line in the writer, two queued: the next is refused at once.
        outbox.send(&ToApp::Close { id: 0 }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while outbox.stalled_for().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        let start = Instant::now();
        let mut refused = None;
        for id in 1..10 {
            if let Err(unsent) = outbox.send(&ToApp::Close { id }) {
                refused = Some(unsent);
                break;
            }
        }
        assert_eq!(refused, Some(Unsent::Full));
        assert!(start.elapsed() < Duration::from_secs(1));
        thread::sleep(Duration::from_millis(30));
        assert!(outbox.stalled_for().unwrap() >= Duration::from_millis(20));
        // The reader reads again: what was queued is written, then the pipe closes.
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        assert!(outbox.close(Duration::from_secs(5)));
        assert!(outbox.gone());
        assert_eq!(outbox.send(&ToApp::SessionEnd), Err(Unsent::Gone));
        let text = String::from_utf8(written.lock().unwrap().clone()).unwrap();
        assert_eq!(text.lines().count(), 3);
    }

    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_broken_pipe_ends_the_writer_quietly() {
        let outbox = Outbox::spawn("test-broken", Broken, 4);
        outbox.send(&ToServer::Closed).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !outbox.gone() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(outbox.gone());
        assert_eq!(outbox.send(&ToServer::Closed), Err(Unsent::Gone));
    }
}
