//! Caption styles (feature `effects`): the declarative effect plugins
//! (`effect.json`, with optional WGSL hooks), the catalog that finds them, and the
//! lyrics lines they animate. A style is a front end of the drawing engine
//! (`crate::fx`): it lays a line out and hands the engine the letters to draw.
//! Nothing in `crate::fx` knows this format.
pub mod catalog;
pub mod line;
pub mod plugin;
pub mod plugin_shader;

#[cfg(test)]
mod classic_tests;
#[cfg(test)]
mod engine_features_tests;
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
