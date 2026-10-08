use assert_cmd::assert::OutputAssertExt;
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};

pub trait Argument {
    fn argument(&self) -> OsString;
}
impl<T: Argument + ?Sized> Argument for &T {
    fn argument(&self) -> OsString {
        (*self).argument()
    }
}
macro_rules! path_arg { ($($t:ty),*) => { $(impl Argument for $t { fn argument(&self) -> OsString { self.as_os_str().to_owned() } })* }; }
path_arg!(Path, PathBuf);
impl Argument for OsStr {
    fn argument(&self) -> OsString {
        self.to_owned()
    }
}
impl Argument for OsString {
    fn argument(&self) -> OsString {
        self.clone()
    }
}
macro_rules! display_arg { ($($t:ty),*) => { $(impl Argument for $t { fn argument(&self) -> OsString { self.to_string().into() } })* }; }
display_arg!(str, String, i32, usize, f64, bool);
macro_rules! args { ($($v:expr),* $(,)?) => { vec![$(crate::support::Argument::argument(&$v)),*] }; }

pub struct Fixture {
    pub folder: tempfile::TempDir,
    pub ffmpeg: PathBuf,
    pub exe: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        Self {
            folder: tempfile::Builder::new()
                .prefix("noh-media-")
                .tempdir()
                .unwrap(),
            ffmpeg: noh::find_ffmpeg(None).unwrap(),
            exe: std::env::var_os("NOH_EXE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_noh"))),
        }
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.folder.path().join(name)
    }
    pub fn run(&self, program: &Path, args: Vec<OsString>) -> Output {
        let mut cmd = Command::new(program);
        cmd.args(args).env("NOH_FFMPEG", &self.ffmpeg);
        crate::process::run(cmd, Duration::from_secs(60)).unwrap()
    }
    pub fn cli(&self, args: Vec<OsString>) -> Output {
        let output = self.run(&self.exe, args);
        output.clone().assert().success();
        output
    }
    pub fn ff(&self, args: Vec<OsString>) -> Output {
        let mut all = args!["-hide_banner", "-nostdin", "-v", "error", "-n"];
        all.extend(args);
        let output = self.run(&self.ffmpeg, all);
        output.clone().assert().success();
        output
    }
    pub fn wav(&self, name: &str, seconds: f64) -> PathBuf {
        let path = self.path(name);
        self.ff(args![
            "-f",
            "lavfi",
            "-i",
            format!("sine=duration={seconds}"),
            "-c:a",
            "pcm_s24le",
            &path
        ]);
        path
    }
    pub fn packets(&self, path: &Path, slice: bool, decode: bool, audio: bool) -> String {
        let mut flags = args!["-i", path, "-map", if audio { "0:a:0" } else { "0:v:0" }];
        if !decode {
            flags.extend(args!["-c", "copy"]);
        }
        if slice && !decode {
            flags.extend(args!["-bsf:v", "filter_units=pass_types=1-5"]);
        }
        flags.extend(args!["-fps_mode", "passthrough", "-f", "framehash", "-"]);
        let out = self.ff(flags);
        assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
        text(&out.stdout)
    }
    pub fn hashes(&self, path: &Path, slice: bool, decode: bool, audio: bool) -> Vec<String> {
        rows(&self.packets(path, slice, decode, audio))
            .into_iter()
            .map(|r| r.last().unwrap().clone())
            .collect()
    }
    pub fn duration(&self, path: &Path) -> f64 {
        let out = self.ff(args![
            "-v", "info", "-i", path, "-map", "0:v:0", "-c", "copy", "-f", "null", "-"
        ]);
        let log = text(&out.stderr);
        let time = log
            .split("Duration: ")
            .nth(1)
            .unwrap()
            .split(',')
            .next()
            .unwrap();
        let parts: Vec<f64> = time.split(':').map(|p| p.parse().unwrap()).collect();
        parts[0] * 3600.0 + parts[1] * 60.0 + parts[2]
    }
    pub fn gray(&self, path: &Path) -> Vec<u8> {
        let mut flags = args![];
        // This demuxer option applies to MP4, not Matroska.
        if path.extension().is_some_and(|ext| ext == "mp4") {
            flags.extend(args!["-ignore_editlist", "1"]);
        }
        flags.extend(args![
            "-i",
            path,
            "-map",
            "0:v:0",
            "-vf",
            "scale=1:1,format=gray",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-"
        ]);
        self.ff(flags).stdout
    }
    pub fn clean(&self) {
        assert!(!std::fs::read_dir(self.folder.path()).unwrap().any(|p| {
            p.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".noh-")
        }));
    }
}
pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
pub fn log(output: &Output) -> String {
    format!("{}{}", text(&output.stdout), text(&output.stderr))
}
pub fn rows(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split(',').map(|v| v.trim().to_owned()).collect())
        .collect()
}
pub fn near(actual: f64, expected: f64, delta: f64) {
    assert!(
        (actual - expected).abs() <= delta,
        "{actual} != {expected} ± {delta}"
    );
}
pub fn progress(log: &str) {
    let values: Vec<u32> = log
        .lines()
        .filter_map(|l| l.strip_prefix("NOH_PROGRESS|"))
        .map(|l| l.split('|').next().unwrap().parse().unwrap())
        .collect();
    assert!(!values.is_empty());
    assert!(values.windows(2).all(|w| w[0] <= w[1]));
    assert!(values.iter().all(|&v| v <= 100));
    assert_eq!(values.last(), Some(&100));
}
