//! Where a montage played by libmpv stands, shared between the controller,
//! which talks to libmpv, and the interface, which draws.
//!
//! The interface takes this state's lock at every frame, on the thread libmpv
//! waits for to render its pictures. A question put to libmpv while that lock
//! is held stops the controller, libmpv and the interface for good: the
//! controller waits for libmpv's core, the core for the render thread, the
//! render thread for the lock. It happened once, through a journal line that
//! read a property there.
//!
//! So the lock lives in this module, which knows nothing of libmpv. Both
//! sides hand plain values over and get plain values back; no closure comes
//! in. A call into libmpv under this lock cannot be written.
use crate::{
    playback::{Frame, LiveFailure, Phase, SeekTiming, Snapshot},
    project::ProjectRequest,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

/// What the interface asks of the controller.
pub(super) enum Command {
    Play {
        project: Box<ProjectRequest>,
        from: f64,
        playing: bool,
        volume: f32,
        generation: u64,
    },
    Playing(bool),
    Seek {
        seconds: f64,
        id: u64,
        at: Instant,
    },
    Volume(f32),
    /// Stops the montage of that generation: not one opened since.
    Stop(u64),
}

/// What the controller saw of libmpv on one of its turns.
#[derive(Default)]
pub(super) struct Seen {
    /// No picture of the montage has been rendered yet.
    pub loading: bool,
    /// The clock of the picture now rendered, when a new one is.
    pub shown: Option<f64>,
    /// libmpv's position.
    pub position: Option<f64>,
    pub paused: bool,
    pub ended: bool,
    /// Pictures rendered so far.
    pub rendered: u64,
    /// The last seek sent to libmpv, and whether one is waiting or in flight.
    pub seek: Option<SeekTiming>,
    pub seeking: bool,
    /// libmpv has a sound output and a sound track, once that is known.
    pub sound: Option<bool>,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
struct State {
    generation: u64,
    loading: bool,
    duration: f64,
    position: f64,
    /// The clock of the picture last rendered: not the position just asked
    /// for, which its picture has yet to follow.
    shown: f64,
    playing: bool,
    ended: bool,
    volume: f32,
    sound: bool,
    error: Option<String>,
    /// The error is a refusal: nothing of the montage was asked of libmpv.
    refused: bool,
    /// Pictures rendered so far: a new one is a new frame for the interface.
    rendered: u64,
    seek: Option<SeekTiming>,
}

#[derive(Default)]
pub(super) struct Exchange {
    state: Mutex<Option<State>>,
    queue: Mutex<VecDeque<Command>>,
    opened: AtomicU64,
}
impl Exchange {
    // The interface's side.

    /// A new montage to play, from `from` seconds of the selection. Returns
    /// its generation, by which every later call names it.
    pub fn open(&self, project: ProjectRequest, from: f64, playing: bool, volume: f32) -> u64 {
        let generation = self.opened.fetch_add(1, Ordering::Relaxed) + 1;
        let mut state = self.state.lock().unwrap();
        *state = Some(State {
            generation,
            loading: true,
            position: from,
            shown: from,
            playing,
            volume,
            ..Default::default()
        });
        self.queue.lock().unwrap().push_back(Command::Play {
            project: Box::new(project),
            from,
            playing,
            volume,
            generation,
        });
        generation
    }
    /// Play or pause. What is asked is read back at once, and written and
    /// queued under one lock: the controller, which looks at the queue under
    /// that lock, then never writes what it saw earlier over it.
    pub fn set_playing(&self, generation: u64, playing: bool) {
        self.ask(generation, Command::Playing(playing), |state| {
            state.playing = playing && !state.ended;
        });
    }
    pub fn seek(&self, generation: u64, seconds: f64, id: u64) {
        let command = Command::Seek {
            seconds,
            id,
            at: Instant::now(),
        };
        self.ask(generation, command, |state| {
            // The length is not known before libmpv has been asked.
            state.position = if state.duration > 0.0 {
                seconds.min(state.duration)
            } else {
                seconds
            };
            state.ended = false;
        });
    }
    pub fn set_volume(&self, generation: u64, volume: f32) {
        self.ask(generation, Command::Volume(volume), |state| {
            state.volume = volume;
        });
    }
    /// Private, so that no caller's code runs under the lock. What is asked
    /// of a montage that is not the open one is dropped: the controller applies
    /// a command to the montage it plays, whichever asked.
    fn ask(&self, generation: u64, command: Command, change: impl FnOnce(&mut State)) {
        let mut state = self.state.lock().unwrap();
        if let Some(state) = state.as_mut().filter(|s| s.generation == generation) {
            change(state);
            self.queue.lock().unwrap().push_back(command);
        }
    }
    /// The montage of that generation is over for the interface.
    pub fn close(&self, generation: u64) {
        let mut state = self.state.lock().unwrap();
        if state.as_ref().is_some_and(|s| s.generation == generation) {
            *state = None;
        }
        self.queue
            .lock()
            .unwrap()
            .push_back(Command::Stop(generation));
    }
    /// The player's snapshot of that montage; an empty one once it is closed.
    pub fn snapshot(&self, generation: u64) -> Snapshot {
        let state = self.state.lock().unwrap().clone();
        let Some(state) = state.filter(|s| s.generation == generation) else {
            return Snapshot::default();
        };
        Snapshot {
            // Told apart from the other player's generations by a high bit.
            generation: (1 << 62) | state.generation,
            path: None,
            phase: if state.error.is_some() {
                Phase::Error
            } else if state.loading {
                Phase::Loading
            } else if state.ended {
                Phase::Ended
            } else {
                Phase::Ready
            },
            duration: state.duration,
            position: state.position,
            playing: state.playing && !state.ended && state.error.is_none(),
            volume: state.volume,
            audio_available: state.sound,
            warning: None,
            error: state.error,
            // libmpv draws the picture itself: a frame with no pixels.
            frame: (!state.loading).then(|| {
                Arc::new(Frame {
                    id: state.rendered,
                    width: 0,
                    height: 0,
                    rgb: Vec::new(),
                    position: state.shown,
                })
            }),
            seek: state.seek,
        }
    }
    /// How that montage stopped being served, once it has an error.
    pub fn failure(&self, generation: u64) -> Option<LiveFailure> {
        let state = self.state.lock().unwrap();
        let state = state.as_ref().filter(|s| s.generation == generation)?;
        state.error.as_ref().map(|_| LiveFailure {
            refused: state.refused,
            playing: state.playing,
        })
    }
    /// Whether the open montage has pictures to give now: it plays, loads or
    /// seeks. Kept open and paused, it asks for nothing.
    pub fn wants_pictures(&self) -> bool {
        self.state.lock().unwrap().as_ref().is_some_and(|state| {
            state.error.is_none()
                && (state.loading
                    || (state.playing && !state.ended)
                    || state.seek.is_some_and(|seek| seek.frame_ms.is_none()))
        })
    }

    // The controller's side.

    /// The commands asked since the last turn, in order.
    pub fn commands(&self) -> Vec<Command> {
        self.queue.lock().unwrap().drain(..).collect()
    }
    /// libmpv has been asked for the montage, which lasts `duration` seconds.
    pub fn asked_of_libmpv(&self, generation: u64, duration: f64) {
        if let Some(state) = self.of(generation).as_mut() {
            state.duration = duration;
        }
    }
    /// libmpv is not asked for the montage: it would not play it. What was
    /// asked, playing or paused, stays known for whoever plays it instead.
    pub fn refused(&self, generation: u64, error: String) {
        if let Some(state) = self.of(generation).as_mut() {
            state.error = Some(error);
            state.refused = true;
            state.loading = false;
        }
    }
    /// One turn of the controller. True when the loading has just ended.
    pub fn saw(&self, generation: u64, seen: Seen) -> bool {
        let mut guard = self.of(generation);
        let Some(state) = guard.as_mut() else {
            return false;
        };
        let was_loading = state.loading;
        state.loading = seen.loading;
        if let Some(shown) = seen.shown {
            state.shown = shown.min(state.duration);
        }
        state.rendered = seen.rendered;
        // A command still in the queue is newer than what libmpv says: the
        // interface has written what it asked. A seek asked or in flight keeps
        // the position asked for, and so does a pause: the playhead stays
        // where it was put. The same seek takes the montage off its end.
        let queued = !self.queue.lock().unwrap().is_empty();
        if !queued && !seen.seeking {
            state.ended = seen.ended && !seen.loading;
        }
        if let Some(position) = seen
            .position
            .filter(|_| !queued && !seen.seeking && !seen.loading && !seen.paused)
        {
            state.position = position.min(state.duration);
        }
        if !seen.loading && !queued {
            state.playing = !seen.paused && !seen.ended;
        }
        state.seek = seen.seek;
        if let Some(sound) = seen.sound {
            state.sound = sound;
        }
        // What was asked, playing or paused, stays known past the error: the
        // snapshot itself reads not playing.
        if let Some(error) = seen.error {
            state.error = Some(error);
        }
        was_loading && !seen.loading
    }
    /// The state of that generation, locked: none once another is open.
    fn of(&self, generation: u64) -> Guard<'_> {
        Guard {
            guard: self.state.lock().unwrap(),
            generation,
        }
    }
}
struct Guard<'a> {
    guard: std::sync::MutexGuard<'a, Option<State>>,
    generation: u64,
}
impl Guard<'_> {
    fn as_mut(&mut self) -> Option<&mut State> {
        let generation = self.generation;
        self.guard.as_mut().filter(|s| s.generation == generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectRequest {
        serde_json::from_value(serde_json::json!({
            "montage": {
                "videos": ["a.mp4"], "wav": "song.wav", "output": "out.mp4", "ffmpeg": "",
                "fade_in": 0, "fade_out": 0, "partial_fades": false, "preview": true,
                "clip_audio": false, "force_encode": true
            }
        }))
        .unwrap()
    }
    /// A montage open and past its loading, playing at `position`.
    fn playing(live: &Exchange, position: f64) -> u64 {
        let generation = live.open(project(), 0.0, true, 0.7);
        live.commands();
        live.asked_of_libmpv(generation, 180.0);
        let started = live.saw(
            generation,
            Seen {
                position: Some(position),
                shown: Some(position),
                rendered: 1,
                sound: Some(true),
                ..Default::default()
            },
        );
        assert!(started, "the loading ends with the first picture");
        generation
    }

    #[test]
    fn what_the_interface_asks_is_read_back_at_once_and_not_written_over() {
        let live = Exchange::default();
        let generation = playing(&live, 12.0);
        live.seek(generation, 40.0, 1);
        assert_eq!(live.snapshot(generation).position, 40.0);
        // The controller has not read the queue yet and still sees the old
        // position: it does not write it back.
        live.saw(
            generation,
            Seen {
                position: Some(12.1),
                rendered: 2,
                ..Default::default()
            },
        );
        assert_eq!(live.snapshot(generation).position, 40.0);
        // It reads the queue and the seek is in flight: still the position asked.
        assert!(
            matches!(live.commands()[..], [Command::Seek { seconds, id: 1, .. }] if seconds == 40.0)
        );
        live.saw(
            generation,
            Seen {
                position: Some(40.0),
                seeking: true,
                rendered: 2,
                ..Default::default()
            },
        );
        assert_eq!(live.snapshot(generation).position, 40.0);
        // Landed and playing: libmpv's position is followed again.
        live.saw(
            generation,
            Seen {
                position: Some(40.5),
                shown: Some(40.5),
                rendered: 3,
                ..Default::default()
            },
        );
        let state = live.snapshot(generation);
        assert_eq!(
            (state.position, state.frame.unwrap().position),
            (40.5, 40.5)
        );
    }

    #[test]
    fn a_pause_asked_is_not_undone_by_what_the_controller_saw_before_it() {
        let live = Exchange::default();
        let generation = playing(&live, 5.0);
        assert!(live.snapshot(generation).playing);
        live.set_playing(generation, false);
        assert!(!live.snapshot(generation).playing);
        live.saw(
            generation,
            Seen {
                position: Some(5.1),
                rendered: 2,
                ..Default::default()
            },
        );
        assert!(
            !live.snapshot(generation).playing,
            "the pause waits in the queue"
        );
        live.commands();
        // Paused, the playhead stays where it was put, whatever libmpv reads.
        live.saw(
            generation,
            Seen {
                position: Some(5.2),
                paused: true,
                rendered: 2,
                ..Default::default()
            },
        );
        let state = live.snapshot(generation);
        assert!(!state.playing && state.position == 5.0);
        assert!(
            !live.wants_pictures(),
            "open and paused, it asks for nothing"
        );
    }

    #[test]
    fn the_snapshot_says_loading_ready_ended_or_failed_and_whether_sound_is_there() {
        let live = Exchange::default();
        let generation = live.open(project(), 3.0, true, 0.7);
        let state = live.snapshot(generation);
        assert_eq!(state.phase, Phase::Loading);
        assert!(state.frame.is_none() && state.position == 3.0 && !state.audio_available);
        assert!(live.wants_pictures());
        live.commands();
        live.asked_of_libmpv(generation, 10.0);
        live.saw(
            generation,
            Seen {
                position: Some(3.0),
                shown: Some(3.0),
                rendered: 4,
                ..Default::default()
            },
        );
        let state = live.snapshot(generation);
        assert_eq!((state.phase, state.duration), (Phase::Ready, 10.0));
        let frame = state.frame.unwrap();
        assert_eq!((frame.id, frame.width, frame.rgb.len()), (4, 0, 0));
        assert!(
            !state.audio_available,
            "not said before libmpv has an output"
        );
        live.saw(
            generation,
            Seen {
                position: Some(3.5),
                rendered: 5,
                sound: Some(true),
                ..Default::default()
            },
        );
        assert!(live.snapshot(generation).audio_available);
        // The end: paused by libmpv at the last picture.
        live.saw(
            generation,
            Seen {
                position: Some(10.0),
                paused: true,
                ended: true,
                rendered: 9,
                ..Default::default()
            },
        );
        let state = live.snapshot(generation);
        assert!(state.phase == Phase::Ended && !state.playing);
        assert!(!live.wants_pictures());
        // Off the end by a seek: what libmpv still says of the end is not
        // written back while the seek waits or is in flight.
        live.seek(generation, 2.0, 1);
        assert_eq!(live.snapshot(generation).phase, Phase::Ready);
        let still_at_the_end = || Seen {
            position: Some(10.0),
            paused: true,
            ended: true,
            rendered: 9,
            ..Default::default()
        };
        live.saw(generation, still_at_the_end());
        assert_eq!(live.snapshot(generation).phase, Phase::Ready);
        live.commands();
        live.saw(
            generation,
            Seen {
                seeking: true,
                ..still_at_the_end()
            },
        );
        assert_eq!(live.snapshot(generation).phase, Phase::Ready);
        assert_eq!(live.failure(generation), None);
        // Given up in the middle: an error, and what was asked stays known.
        live.set_playing(generation, true);
        live.commands();
        live.saw(
            generation,
            Seen {
                position: Some(2.5),
                rendered: 12,
                error: Some("No sound output".into()),
                ..Default::default()
            },
        );
        let state = live.snapshot(generation);
        assert_eq!(state.phase, Phase::Error);
        assert_eq!(state.error.as_deref(), Some("No sound output"));
        assert!(!state.playing && !live.wants_pictures());
        assert_eq!(
            live.failure(generation),
            Some(LiveFailure {
                refused: false,
                playing: true
            })
        );
    }

    #[test]
    fn a_refusal_is_told_apart_from_a_montage_given_up() {
        let live = Exchange::default();
        let generation = live.open(project(), 3.0, false, 0.7);
        live.commands();
        live.refused(generation, "a light copy is not ready".into());
        let state = live.snapshot(generation);
        assert_eq!(state.phase, Phase::Error);
        assert!(!live.wants_pictures());
        assert_eq!(
            live.failure(generation),
            Some(LiveFailure {
                refused: true,
                playing: false
            })
        );
        // A seek before the length is known keeps the position asked.
        let generation = live.open(project(), 0.0, true, 0.7);
        live.seek(generation, 42.0, 1);
        assert_eq!(live.snapshot(generation).position, 42.0);
        live.asked_of_libmpv(generation, 10.0);
        live.seek(generation, 42.0, 2);
        assert_eq!(live.snapshot(generation).position, 10.0);
    }

    #[test]
    fn an_older_montage_changes_nothing_once_another_is_open() {
        let live = Exchange::default();
        let old = playing(&live, 8.0);
        let new = live.open(project(), 1.0, true, 0.7);
        assert!(new != old);
        // The old one reads empty and its late updates and commands are ignored.
        assert_eq!(live.snapshot(old).phase, Phase::Empty);
        live.saw(
            old,
            Seen {
                position: Some(9.0),
                rendered: 7,
                ..Default::default()
            },
        );
        live.refused(old, "late".into());
        live.asked_of_libmpv(old, 99.0);
        // What the old one asks does not reach the controller, which would
        // apply it to the new one: only the new montage's Play is queued.
        live.seek(old, 50.0, 3);
        live.set_playing(old, false);
        live.set_volume(old, 0.1);
        assert!(matches!(
            live.commands()[..],
            [Command::Play { generation, .. }] if generation == new
        ));
        let state = live.snapshot(new);
        assert_eq!(
            (state.phase, state.position, state.duration, state.volume),
            (Phase::Loading, 1.0, 0.0, 0.7)
        );
        // Closing the old one leaves the new one open; its stop is still queued
        // for the controller, by generation.
        live.close(old);
        assert_eq!(live.snapshot(new).phase, Phase::Loading);
        assert!(
            live.commands()
                .iter()
                .any(|c| matches!(c, Command::Stop(g) if *g == old))
        );
        live.close(new);
        assert_eq!(live.snapshot(new).phase, Phase::Empty);
    }
}
