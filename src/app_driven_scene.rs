//! The motion scene a driven window follows (motion overlay plan §5.6, M8):
//! `set_scene` attaches the file the agent writes; the window looks at it every
//! 2 s, as at the song, and when it changed checks that it loads and draws its
//! first frame (in a thread, never in paint), and says so in `state.scene`. A
//! `noh_scene_look` with this window's id moves the playhead to the instant it
//! looks at and draws the frame's grid, and on the short the safe area, over
//! the monitor while the playhead stays there. The window does not draw the
//! scene itself yet (the preview's half, M4).
use super::NohApp;
use eframe::egui;
use noh::driven::Refusal;
use noh::safe_area::SafeArea;
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime},
};

/// The file is looked at this often, as the song is.
const POLL: Duration = Duration::from_secs(2);

/// A file's identity as the poll compares it.
type Stamp = Option<(SystemTime, u64)>;

/// What the light check found.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum SceneStatus {
    Checking,
    Loads,
    Refused(String),
    Missing,
}

/// The scene the window follows.
pub(super) struct SceneWatch {
    pub path: PathBuf,
    /// Counts the versions of the file seen (1: the first).
    pub revision: u64,
    pub status: SceneStatus,
    stamp: Stamp,
    polled: Option<Instant>,
    /// The check under way: the file's stamp, and what the check said when it
    /// changed (None: unchanged).
    work: Option<mpsc::Receiver<(Stamp, Option<Result<(), String>>)>>,
}

/// Where the agent looks: the instant, and what is drawn over the monitor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SceneMarker {
    pub at_ms: u64,
    pub safe_area: SafeArea,
    pub grid: bool,
}

fn stamp(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

impl NohApp {
    /// `set_scene`: attach the scene at `path`, or detach it.
    pub(super) fn driven_set_scene(
        &mut self,
        path: Option<PathBuf>,
    ) -> Result<serde_json::Value, Refusal> {
        if !cfg!(feature = "motion") {
            return Err(Refusal::new(
                "app.scene_unavailable",
                "This build of NOH cannot draw scenes.",
            ));
        }
        let Some(d) = &mut self.driven else {
            return Ok(serde_json::Value::Null);
        };
        match path {
            None => d.scene = None,
            Some(path) => {
                if !path.is_absolute() || !path.is_file() {
                    return Err(Refusal::new(
                        "app.refused",
                        "The scene must be an absolute path to a file that exists.",
                    ));
                }
                d.scene = Some(SceneWatch {
                    path,
                    revision: 0,
                    status: SceneStatus::Checking,
                    stamp: None,
                    polled: None,
                    work: None,
                });
            }
        }
        d.marker = None;
        self.driven_mark();
        Ok(serde_json::Value::Null)
    }

    /// Each frame: the poll of the scene file, and what its check said.
    pub(super) fn driven_scene_frame(&mut self, ctx: &egui::Context) {
        let size = self.scene_frame_size();
        let duration = self.project.duration_ms as f64 / 1000.0;
        let track = self.lyrics_track();
        let Some(d) = &mut self.driven else {
            return;
        };
        let Some(w) = &mut d.scene else {
            return;
        };
        let mut changed = false;
        if let Some(rx) = &w.work {
            match rx.try_recv() {
                Ok((new, outcome)) => {
                    w.work = None;
                    w.stamp = new;
                    if new.is_none() {
                        changed = w.status != SceneStatus::Missing;
                        w.status = SceneStatus::Missing;
                    } else if let Some(outcome) = outcome {
                        w.revision += 1;
                        w.status = match outcome {
                            Ok(()) => SceneStatus::Loads,
                            Err(why) => SceneStatus::Refused(why),
                        };
                        changed = true;
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => w.work = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if w.work.is_none() && w.polled.is_none_or(|t| t.elapsed() >= POLL) {
            w.polled = Some(Instant::now());
            let (tx, rx) = mpsc::sync_channel(1);
            w.work = Some(rx);
            let (path, seen) = (w.path.clone(), w.stamp);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let now = stamp(&path);
                let outcome = (now.is_some() && now != seen)
                    .then(|| check(&path, track.as_deref(), size, duration));
                let _ = tx.send((now, outcome));
                ctx.request_repaint();
            });
        }
        ctx.request_repaint_after(POLL);
        if changed {
            self.driven_mark();
        }
    }

    /// The frame a scene would draw on here: the montage's shape, 1920 wide.
    fn scene_frame_size(&self) -> (u32, u32) {
        let aspect = self.canvas_aspect().max(0.1);
        let h = ((1920.0 / aspect / 2.0).round() as u32 * 2).max(2);
        (1920, h)
    }

    /// `state.scene`.
    pub(super) fn driven_scene_state(&self) -> serde_json::Value {
        let Some(w) = self.driven.as_ref().and_then(|d| d.scene.as_ref()) else {
            return serde_json::Value::Null;
        };
        let (status, reason) = match &w.status {
            SceneStatus::Checking => ("checking", None),
            SceneStatus::Loads => ("loads", None),
            SceneStatus::Refused(why) => ("refused", Some(why.clone())),
            SceneStatus::Missing => ("missing", None),
        };
        serde_json::json!({
            "path": w.path,
            "revision": w.revision,
            "status": status,
            "reason": reason,
        })
    }

    /// The marker of the agent's scene look, while the playhead stays at its
    /// instant (paused there).
    pub(super) fn driven_scene_marker(&self) -> Option<SceneMarker> {
        let m = self.driven.as_ref()?.marker?;
        (!self.mini_playing() && self.mini_preview.cursor_ms == m.at_ms).then_some(m)
    }
}

#[cfg(feature = "motion")]
fn check(
    path: &Path,
    track: Option<&noh::subtitle_track::SubtitleTrack>,
    size: (u32, u32),
    duration: f64,
) -> Result<(), String> {
    noh::motion::quick::loads(path, track, size, duration)
}

#[cfg(not(feature = "motion"))]
fn check(
    _: &Path,
    _: Option<&noh::subtitle_track::SubtitleTrack>,
    _: (u32, u32),
    _: f64,
) -> Result<(), String> {
    Err("This build of NOH cannot draw scenes.".into())
}

/// A line every tenth of the monitor, as a scene look's `grid`.
pub(super) fn paint_grid(ui: &egui::Ui, rect: egui::Rect) {
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70));
    let painter = ui.painter_at(rect);
    for k in 1..10 {
        let f = k as f32 / 10.0;
        let x = rect.left() + rect.width() * f;
        let y = rect.top() + rect.height() * f;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            stroke,
        );
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            stroke,
        );
    }
    painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
}
