//! Desktop preview ownership. Job lifecycle is shared by all adapters.
pub use noh::engine::{Event, ExportRequest as Request};
pub use noh::jobs::Job;
use std::path::{Path, PathBuf};
/// A preview belongs to an application instance, never to the soundtrack folder.
pub struct PreviewFile {
    pub path: PathBuf,
    _folder: tempfile::TempDir,
}
impl PreviewFile {
    pub fn new() -> std::io::Result<Self> {
        Self::in_folder(&std::env::temp_dir())
    }
    pub fn in_folder(parent: &Path) -> std::io::Result<Self> {
        let folder = tempfile::Builder::new()
            .prefix("noh-preview-")
            .tempdir_in(parent)?;
        Ok(Self {
            path: folder.path().join("preview.mp4"),
            _folder: folder,
        })
    }
}
