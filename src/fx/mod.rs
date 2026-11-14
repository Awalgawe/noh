//! Caption effects engine (feature `effects`): text layout and glyph atlases,
//! declarative effect plugins with optional WGSL hooks, an OpenGL compositor shared
//! by the preview and the export, and the export's YUV mix and Matroska writer.
//! No egui code lives here; the GUI draws the compositor's output itself.
pub mod catalog;
pub mod export;
pub mod gl;
pub mod line;
pub mod mkv;
pub mod plugin;
pub mod plugin_shader;
pub mod shaders;
pub mod text;

#[cfg(test)]
mod classic_tests;
#[cfg(test)]
mod expr_tests;
#[cfg(test)]
mod overflow_tests;
#[cfg(test)]
mod space_tests;
#[cfg(test)]
mod stills_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod window_tests;
