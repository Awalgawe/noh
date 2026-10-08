//! Standalone inspection contracts. Quick metadata never promises picture copying.
use crate::engine::{EngineError, Event, ExportRequest, MediaItem};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Quick,
    Exact,
}
impl std::str::FromStr for Level {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "quick" => Ok(Self::Quick),
            "exact" => Ok(Self::Exact),
            _ => Err("Expected quick or exact".into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InspectionRequest {
    Metadata {
        path: PathBuf,
        ffmpeg: Option<PathBuf>,
        wav: bool,
    },
    Image {
        path: PathBuf,
        ffmpeg: Option<PathBuf>,
    },
    Plan {
        request: ExportRequest,
        level: Level,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub path: PathBuf,
    pub resolved: PathBuf,
    pub bytes: u64,
    pub modified: SystemTime,
    pub created: Option<SystemTime>,
}
impl FileStamp {
    pub fn read(path: &Path) -> Result<Self, EngineError> {
        let read = || -> std::io::Result<Self> {
            let canonical = path.canonicalize()?;
            let meta = std::fs::metadata(&canonical)?;
            if !meta.is_file() {
                return Err(std::io::Error::other("Input is not a regular file"));
            }
            Ok(Self {
                path: std::path::absolute(path)?,
                resolved: canonical,
                bytes: meta.len(),
                modified: meta.modified()?,
                created: meta.created().ok(),
            })
        };
        read().map_err(|e| EngineError::new("error.input", "fingerprint", Some(path), e))
    }
}

/// Metadata stamps are invalidation hints, not content hashes or locked snapshots.
/// Exact export always reinspects and compares the decisions before rendering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot(pub Vec<FileStamp>);
impl Snapshot {
    pub fn project(request: &ExportRequest) -> Result<Self, EngineError> {
        Self::read(
            request
                .items
                .iter()
                .map(MediaItem::path)
                .chain([&request.wav, &resolve_ffmpeg(&request.ffmpeg)?]),
        )
    }
    pub fn read<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> Result<Self, EngineError> {
        paths
            .into_iter()
            .map(|p| FileStamp::read(p))
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }
    pub fn verify(&self) -> Result<(), EngineError> {
        for stamp in &self.0 {
            if FileStamp::read(&stamp.path).as_ref() != Ok(stamp) {
                return Err(stale(Some(&stamp.path)));
            }
        }
        Ok(())
    }
}

/// One operation's metadata-based identities and media results. Canonical paths
/// identify aliases; these stamps do not detect edits preserving file metadata.
pub(crate) struct SourceInspections {
    snapshot: Snapshot,
    sources: Vec<usize>,
    media: Vec<Option<crate::media::MediaInfo>>,
}
impl SourceInspections {
    #[cfg(test)]
    pub(crate) fn new(ffmpeg: &Path, videos: &[PathBuf]) -> Result<Self, EngineError> {
        Self::new_items(
            ffmpeg,
            &videos.iter().cloned().map(Into::into).collect::<Vec<_>>(),
        )
    }
    pub(crate) fn new_items(ffmpeg: &Path, items: &[MediaItem]) -> Result<Self, EngineError> {
        let ffmpeg = ffmpeg.to_path_buf();
        let snapshot = Snapshot::read(
            items
                .iter()
                .map(MediaItem::path)
                .chain(std::iter::once(&ffmpeg)),
        )?;
        let mut identities = HashMap::new();
        let mut sources = Vec::with_capacity(items.len());
        for (index, stamp) in snapshot.0[..items.len()].iter().enumerate() {
            let first = *identities
                .entry((stamp.resolved.clone(), items[index].is_image()))
                .or_insert(index);
            let original = &snapshot.0[first];
            if (stamp.bytes, stamp.modified, stamp.created)
                != (original.bytes, original.modified, original.created)
            {
                return Err(stale(Some(&stamp.path)));
            }
            sources.push(first);
        }
        Ok(Self {
            snapshot,
            sources,
            media: vec![None; items.len()],
        })
    }
    pub(crate) fn source(&self, index: usize) -> usize {
        self.sources[index]
    }
    pub(crate) fn verify_entry(&self, index: usize) -> Result<(), EngineError> {
        // Check both identities even when no subprocess is needed for this entry.
        for stamp in [&self.snapshot.0[index], self.snapshot.0.last().unwrap()] {
            if FileStamp::read(&stamp.path).as_ref() != Ok(stamp) {
                return Err(stale(Some(&stamp.path)));
            }
        }
        Ok(())
    }
    pub(crate) fn media(
        &mut self,
        index: usize,
        inspect: impl FnOnce() -> crate::Result<crate::media::MediaInfo>,
    ) -> crate::Result<crate::media::MediaInfo> {
        self.verify_entry(index)?;
        let source = self.source(index);
        if self.media[source].is_none() {
            let info = inspect()?;
            self.verify_entry(index)?;
            self.media[source] = Some(info);
        }
        Ok(self.media[source].as_ref().unwrap().clone())
    }
    pub(crate) fn verify(&self) -> Result<(), EngineError> {
        self.snapshot.verify()
    }
}

pub fn resolve_ffmpeg(path: &Path) -> Result<PathBuf, EngineError> {
    let path =
        crate::find_ffmpeg((!path.as_os_str().is_empty()).then(|| path.as_os_str().to_owned()))
            .map_err(|e| EngineError::new("error.ffmpeg", "find_ffmpeg", Some(path), e))?;
    if path.is_file() {
        return Ok(path);
    }
    if path.components().count() == 1 {
        for folder in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            let candidate = folder.join(&path);
            if candidate.is_file() {
                return Ok(candidate);
            }
            #[cfg(windows)]
            if candidate.extension().is_none() && candidate.with_extension("exe").is_file() {
                return Ok(candidate.with_extension("exe"));
            }
        }
    }
    Err(EngineError::new(
        "error.ffmpeg",
        "find_ffmpeg",
        Some(&path),
        "FFmpeg executable not found",
    ))
}
pub(crate) fn stale(path: Option<&Path>) -> EngineError {
    EngineError::new(
        "error.stale_inspection",
        "inspect",
        path,
        "Inputs, settings or FFmpeg changed. Run diagnosis again.",
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnosis {
    pub version: u32,
    pub level: Level,
    pub request: ExportRequest,
    pub snapshot: Snapshot,
    pub duration: f64,
    pub container: String,
    pub audio: String,
    pub plan: Option<crate::plan::RenderPlan>,
    /// Quick durations are header estimates, not exact packet timing.
    pub quick_media: Vec<crate::media::MediaInfo>,
    pub notes: Vec<String>,
}
impl Diagnosis {
    pub fn validate(&self, request: &ExportRequest) -> Result<(), EngineError> {
        if self.version != 1
            || self.level != Level::Exact
            || !self.request.same_render_settings(request)
            || self.plan.is_none()
        {
            return Err(stale(None));
        }
        if Snapshot::project(request).as_ref() != Ok(&self.snapshot) {
            return Err(stale(None));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InspectionResult {
    Metadata {
        info: crate::media::MediaInfo,
        snapshot: Snapshot,
    },
    Plan(Box<Diagnosis>),
}

/// Synchronous inspection for embeddings; use `Job::inspect` for cancellable isolation.
pub fn inspect(
    request: &InspectionRequest,
    emit: &mut dyn FnMut(Event),
) -> Result<InspectionResult, EngineError> {
    match request {
        InspectionRequest::Image { path, ffmpeg } => {
            let ffmpeg = resolve_ffmpeg(ffmpeg.as_deref().unwrap_or(Path::new("")))?;
            let snapshot = Snapshot::read(std::iter::once(path).chain(std::iter::once(&ffmpeg)))?;
            let info = crate::media::image_info(&ffmpeg, path)
                .map_err(|e| EngineError::wrap("inspect_image", Some(path), e.as_ref()))?;
            snapshot.verify()?;
            Ok(InspectionResult::Metadata { info, snapshot })
        }
        InspectionRequest::Metadata { path, ffmpeg, wav } => {
            let ffmpeg = if *wav {
                None
            } else {
                Some(resolve_ffmpeg(ffmpeg.as_deref().unwrap_or(Path::new("")))?)
            };
            let snapshot = Snapshot::read(std::iter::once(path).chain(ffmpeg.iter()))?;
            let info = if let Some(ffmpeg) = ffmpeg {
                crate::media::quick_info(&ffmpeg, path, false)
            } else {
                crate::wav_duration(path).map(|seconds| crate::media::MediaInfo {
                    seconds,
                    ..Default::default()
                })
            }
            .map_err(|e| {
                EngineError::new(
                    if *wav { "error.wav" } else { "error.video" },
                    "inspect",
                    Some(path),
                    e,
                )
            })?;
            snapshot.verify()?;
            Ok(InspectionResult::Metadata { info, snapshot })
        }
        InspectionRequest::Plan { request, level } => {
            request.validate()?;
            let snapshot = Snapshot::project(request)?;
            let ffmpeg = resolve_ffmpeg(&request.ffmpeg)?;
            let duration = crate::wav_duration(&request.wav).map_err(|e| {
                EngineError::new("error.wav", "read_soundtrack", Some(&request.wav), e)
            })?;
            if request.fade_in + request.fade_out > duration {
                return Err(EngineError::new(
                    "error.fades",
                    "plan",
                    None,
                    "Combined fades exceed the WAV duration.",
                ));
            }
            let container = request
                .output
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let partial = request.partial_fades
                && (request.fade_in > 0.0 || request.fade_out > 0.0)
                && !request.preview
                && !request.force_encode;
            if !["mp4", "mkv"].contains(&container.as_str())
                || (container != "mp4"
                    && (request.items.len() > 1
                        || request.items[0].is_image()
                        || request.clip_audio
                        || partial))
            {
                return Err(EngineError::new(
                    "error.output_format",
                    "plan",
                    Some(&request.output),
                    "Sequences, mixed audio and partial fades require MP4; direct export supports MP4 or MKV.",
                ));
            }
            let mut quick_media = Vec::new();
            let plan = if *level == Level::Exact {
                Some(crate::engine::inspect_and_plan(request, emit)?)
            } else {
                let mut sources = SourceInspections::new_items(&ffmpeg, &request.items)?;
                for (index, item) in request.items.iter().enumerate() {
                    let path = item.path();
                    let mut info = sources
                        .media(index, || {
                            if item.is_image() {
                                crate::media::image_info(&ffmpeg, path)
                            } else {
                                crate::media::quick_info(&ffmpeg, path, false).map_err(|e| {
                                    EngineError::new("error.video", "inspect", Some(path), e).into()
                                })
                            }
                        })
                        .map_err(|e| EngineError::wrap("inspect", Some(path), e.as_ref()))?;
                    if let MediaItem::Image { duration, .. } = item {
                        info.seconds = *duration;
                    }
                    quick_media.push(info);
                }
                sources.verify()?;
                None
            };
            snapshot.verify()?;
            let mut notes = vec!["diagnosis.color_limit".into()];
            if partial {
                notes.push("diagnosis.partial".into());
            }
            if *level == Level::Quick {
                notes.push("diagnosis.quick_limit".into());
            }
            Ok(InspectionResult::Plan(Box::new(Diagnosis {
                version: 1,
                level: *level,
                request: request.clone(),
                snapshot,
                duration,
                audio: if container == "mkv" {
                    "copy"
                } else if request.preview {
                    "aac_160"
                } else {
                    "aac_320"
                }
                .into(),
                container,
                plan,
                quick_media,
                notes,
            })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::MediaInfo;

    fn inputs() -> (tempfile::TempDir, PathBuf, Vec<PathBuf>) {
        let folder = tempfile::tempdir().unwrap();
        let ffmpeg = folder.path().join("ffmpeg");
        let a = folder.path().join("a");
        let b = folder.path().join("b");
        for path in [&ffmpeg, &a, &b] {
            std::fs::write(path, b"synthetic").unwrap();
        }
        std::fs::create_dir(folder.path().join("alias")).unwrap();
        let alias = folder.path().join("alias").join("..").join("a");
        (folder, ffmpeg, vec![a.clone(), b.clone(), alias, b, a])
    }

    fn assert_stale(error: Box<dyn std::error::Error>) {
        assert_eq!(
            error.downcast_ref::<EngineError>().unwrap().code,
            "error.stale_inspection"
        );
    }

    #[test]
    fn repeated_media_preserves_order_and_starts_fresh_each_operation() {
        let (_folder, ffmpeg, paths) = inputs();
        let mut calls = 0;
        for _ in 0..2 {
            let mut sources = SourceInspections::new(&ffmpeg, &paths).unwrap();
            let mut seconds = Vec::new();
            for (index, path) in paths.iter().enumerate() {
                seconds.push(
                    sources
                        .media(index, || {
                            calls += 1;
                            Ok(MediaInfo {
                                seconds: if path.file_name().unwrap() == "a" {
                                    1.0
                                } else {
                                    2.0
                                },
                                ..Default::default()
                            })
                        })
                        .unwrap()
                        .seconds,
                );
                assert_eq!(
                    sources.snapshot.0[index].path,
                    std::path::absolute(path).unwrap()
                );
            }
            assert_eq!(seconds, [1.0, 2.0, 1.0, 2.0, 1.0]);
            sources.verify().unwrap();
        }
        assert_eq!(calls, 4);
    }

    #[test]
    fn media_failure_preserves_error_and_is_not_retained() {
        let (_folder, ffmpeg, paths) = inputs();
        let mut sources = SourceInspections::new(&ffmpeg, &paths).unwrap();
        let error = sources
            .media(0, || {
                Err(EngineError::new(
                    "error.video",
                    "inspect_video",
                    Some(&paths[0]),
                    "broken source",
                )
                .into())
            })
            .unwrap_err();
        let error = error.downcast_ref::<EngineError>().unwrap();
        assert_eq!(error.code, "error.video");
        assert_eq!(error.operation, "inspect_video");
        assert_eq!(error.path.as_ref(), Some(&paths[0]));
        let mut retried = false;
        sources
            .media(2, || {
                retried = true;
                Ok(MediaInfo::default())
            })
            .unwrap();
        assert!(retried);
    }

    #[test]
    fn source_or_ffmpeg_mutation_rejects_reuse_before_callback() {
        for engine_changed in [false, true] {
            let (_folder, ffmpeg, paths) = inputs();
            let mut sources = SourceInspections::new(&ffmpeg, &paths).unwrap();
            sources.media(0, || Ok(MediaInfo::default())).unwrap();
            let changed = if engine_changed { &ffmpeg } else { &paths[0] };
            std::fs::write(changed, b"changed and longer").unwrap();
            let mut called = false;
            assert_stale(
                sources
                    .media(2, || {
                        called = true;
                        Ok(MediaInfo::default())
                    })
                    .unwrap_err(),
            );
            assert!(!called);
        }
    }

    #[test]
    fn mutation_during_inspection_is_not_retained() {
        for engine_changed in [false, true] {
            let (_folder, ffmpeg, paths) = inputs();
            let mut sources = SourceInspections::new(&ffmpeg, &paths).unwrap();
            assert_stale(
                sources
                    .media(0, || {
                        let changed = if engine_changed { &ffmpeg } else { &paths[0] };
                        std::fs::write(changed, b"changed and longer").unwrap();
                        Ok(MediaInfo::default())
                    })
                    .unwrap_err(),
            );
            assert!(sources.media[0].is_none());
        }
    }

    #[test]
    fn final_verification_includes_previously_inspected_sources() {
        let (_folder, ffmpeg, paths) = inputs();
        let mut sources = SourceInspections::new(&ffmpeg, &paths).unwrap();
        for index in 0..paths.len() {
            sources.media(index, || Ok(MediaInfo::default())).unwrap();
        }
        std::fs::remove_file(&paths[0]).unwrap();
        assert_eq!(sources.verify().unwrap_err().code, "error.stale_inspection");
    }
}
