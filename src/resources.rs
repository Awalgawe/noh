//! Availability diagnostics shared by adapters; callers run probes off their UI thread.
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub ffmpeg: Option<PathBuf>,
    pub speech: [PathBuf; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    Ffmpeg,
    Speech,
    Model,
    Vad,
    Preview,
}
impl Component {
    pub fn help_key(self) -> &'static str {
        match self {
            Self::Ffmpeg => "resources.ffmpeg",
            Self::Speech => "resources.speech",
            Self::Model => "resources.model",
            Self::Vad => "resources.vad",
            Self::Preview => "resources.preview",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub component: Component,
    pub path: Option<PathBuf>,
    pub detail: String,
}

/// Windows loader failures are recoverable child-process errors, including missing DLLs.
pub(crate) fn dependency_startup_failure(code: Option<i32>) -> bool {
    code.is_some_and(|code| matches!(code as u32, 0xc0000135 | 0xc000007b | 0xc0000142))
}

fn issue(component: Component, path: &Path, detail: impl ToString) -> Issue {
    Issue {
        component,
        path: (!path.as_os_str().is_empty()).then(|| path.to_path_buf()),
        detail: crate::engine::bounded(&detail.to_string(), 4096),
    }
}

fn file(component: Component, path: &Path, limit: u64) -> Result<(), Issue> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() > 0 && meta.len() <= limit => Ok(()),
        Ok(_) => Err(issue(
            component,
            path,
            "Required file is empty, not a regular file, or exceeds its size limit.",
        )),
        Err(error) => Err(issue(
            component,
            path,
            format!("Required file is unavailable: {error}"),
        )),
    }
}

fn program(
    component: Component,
    path: &Path,
    argument: &str,
    marker: &str,
    cancel: &AtomicBool,
) -> Result<(), Issue> {
    file(component, path, u64::MAX)?;
    if cancel.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut command = crate::command(path);
    command.arg(argument);
    let mut output = String::new();
    let (status, diagnostic) =
        crate::process::stream_interruptible(command, Duration::from_secs(8), cancel, |line| {
            if output.len() < 8192 {
                output.push_str(line);
                output.push('\n');
            }
            Ok(())
        })
        .map_err(|error| {
            issue(
                component,
                path,
                format!("Program could not start or complete its availability check: {error}"),
            )
        })?;
    output.push_str(&diagnostic);
    if !status.success() {
        return Err(issue(
            component,
            path,
            format!(
                "Program availability check failed ({status}). Required shared libraries may be missing or incompatible. {}",
                crate::engine::bounded(&output, 2048)
            ),
        ));
    }
    if !output.contains(marker) {
        return Err(issue(
            component,
            path,
            "The selected executable does not identify as the required media tool.",
        ));
    }
    Ok(())
}

/// Checks file availability and bounded executable startup, not model contents or media quality.
/// Cancellation and subprocess-tree ownership use the same core as media inspections.
pub fn check(inputs: &Inputs, cancel: &AtomicBool) -> Vec<Issue> {
    if cancel.load(Ordering::Acquire) {
        return Vec::new();
    }
    let mut issues = Vec::new();
    let ffmpeg = inputs.ffmpeg.as_deref().unwrap_or(Path::new(""));
    match crate::inspection::resolve_ffmpeg(ffmpeg) {
        Ok(path) => {
            if let Err(error) = program(
                Component::Ffmpeg,
                &path,
                "-version",
                "ffmpeg version",
                cancel,
            ) {
                issues.push(error);
            }
        }
        Err(error) => issues.push(issue(Component::Ffmpeg, ffmpeg, error)),
    }
    if cancel.load(Ordering::Acquire) {
        return Vec::new();
    }
    if let Err(error) = program(
        Component::Speech,
        &inputs.speech[0],
        "--help",
        "--model",
        cancel,
    ) {
        issues.push(error);
    }
    for (component, path, limit) in [
        (
            Component::Model,
            &inputs.speech[1],
            crate::subtitles::MAX_MODEL_BYTES,
        ),
        (
            Component::Vad,
            &inputs.speech[2],
            crate::subtitles::MAX_VAD_BYTES,
        ),
    ] {
        if let Err(error) = file(component, path, limit) {
            issues.push(error);
        }
    }
    if cancel.load(Ordering::Acquire) {
        return Vec::new();
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_resources_are_returned_as_recoverable_issues() {
        let folder = tempfile::tempdir().unwrap();
        let inputs = Inputs {
            ffmpeg: Some(folder.path().join("missing-ffmpeg.exe")),
            speech: [
                "missing-whisper.exe",
                "missing-model.bin",
                "missing-vad.bin",
            ]
            .map(|name| folder.path().join(name)),
        };
        let report = check(&inputs, &AtomicBool::new(false));
        assert_eq!(
            report.iter().map(|i| i.component).collect::<Vec<_>>(),
            [
                Component::Ffmpeg,
                Component::Speech,
                Component::Model,
                Component::Vad
            ]
        );
        assert!(
            report
                .iter()
                .all(|i| i.path.is_some() && !i.detail.is_empty())
        );
        let model = &inputs.speech[1];
        std::fs::write(model, b"synthetic availability fixture").unwrap();
        let recovered = check(&inputs, &AtomicBool::new(false));
        assert!(!recovered.iter().any(|i| i.component == Component::Model));
        assert!(recovered.iter().any(|i| i.component == Component::Vad));
    }
    #[test]
    fn cancellation_does_not_publish_a_false_missing_report() {
        assert!(check(&Inputs::default(), &AtomicBool::new(true)).is_empty());
    }
    #[test]
    fn dll_loader_failures_are_distinguished_from_recognition_errors() {
        for code in [0xc0000135u32, 0xc000007b, 0xc0000142] {
            assert!(dependency_startup_failure(Some(code as i32)));
        }
        assert!(!dependency_startup_failure(Some(1)));
        assert!(!dependency_startup_failure(None));
    }
    #[test]
    fn wrong_executable_is_reported_without_panicking() {
        let error = program(
            Component::Speech,
            &std::env::current_exe().unwrap(),
            "--help",
            "--model",
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.component, Component::Speech);
        assert!(error.detail.contains("does not identify"));
    }
}
