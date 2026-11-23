//! The drawing engine (feature `effects`): what a frame can draw over the video
//! and how it is drawn. Text layout and glyph atlases, the draws a frame hands
//! over as host data (text, rectangles, paths, particles, layers with an
//! optional shader, the picture's framing) with their counted checks, their
//! painter, an OpenGL compositor shared by the preview and the export, and the
//! export's YUV mix and Matroska writer.
//!
//! The engine knows no format: caption styles (`crate::style`, JSON) and motion
//! scenes (`crate::motion`, scripts) are front ends that describe frames to it.
//! No egui code lives here; the GUI draws the compositor's output itself.
pub mod check;
pub mod compose;
pub mod draw;
pub mod ease;
pub mod export;
pub mod gl;
pub mod layer_shader;
pub mod mkv;
pub mod paint;
pub mod quad;
pub mod reframe;
pub mod shaders;
pub mod shapes;
pub mod text;
