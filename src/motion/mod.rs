//! Motion scenes (feature `motion`, experimental): a scene is a sandboxed script
//! whose `frame(t, ctx)` describes, for any instant, what to draw over the video.
//!
//! - `script`: the script engine with its allow-list and counted limits. A frame
//!   it evaluates is host data of the drawing engine (`crate::fx::draw`), checked
//!   by the engine's counted checks (`crate::fx::check`) on **every** frame that
//!   is evaluated (preview and export alike), not only on the frames validated
//!   in advance: a pure script can still exceed a cap between two of them.
//! - `validate`: the pass run before a scene is shown or exported: every frame of
//!   a list of instants, checked, then asked again in another order.
//! - `thread`: the one thread that loads and evaluates a scene, with a stack of
//!   a fixed size, the same in the export worker and in the window.
//!
//! - `song`, `layer`, `run`: a scene over a source video, in the worker: its
//!   inputs gathered from the job's files, its overlay for the export (painted
//!   by `crate::fx::paint`), and the check and look jobs.
//!
//! What a scene draws, its shapes, its layer shaders and the picture's framing
//! (`video()`) live in the engine, where caption styles can reach them too.
pub mod audio;
pub mod layer;
pub mod quick;
pub mod run;
pub mod script;
pub mod song;
pub mod thread;
pub mod validate;

#[cfg(test)]
pub(crate) mod tests;

/// One lyrics line as a scene reads it: its times in seconds on the song's
/// clock, its text, and each word with its times.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub words: Vec<(String, f64, f64)>,
}

/// What a scene is evaluated against. A validation holds for these inputs only:
/// another size, safe area, lyrics or duration is another validation.
#[derive(Clone, Debug, PartialEq)]
pub struct Inputs {
    /// The output frame in pixels.
    pub w: u32,
    pub h: u32,
    /// The source picture in pixels.
    pub src_w: u32,
    pub src_h: u32,
    /// Seconds.
    pub duration: f64,
    pub cues: Vec<Cue>,
    /// Whether the word times are spread over each line by letter count rather
    /// than measured (the author is told: too loose for word-by-word effects).
    pub words_estimated: bool,
    /// The surface text should stay in: x0, y0, x1, y1, fractions of the frame.
    pub safe: [f32; 4],
    /// The layer shaders beside the scene, by name (validated before loading).
    pub shaders: Vec<String>,
}
