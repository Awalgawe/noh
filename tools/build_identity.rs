//! Local, deterministic build-script helpers; also compiled into focused library tests.
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

#[path = "process.rs"]
mod process;

const INPUT_DIRS: &[&str] = &["src", "tools", "tests", "assets", "locales"];
const INPUT_FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    ".cargo/config",
    ".cargo/config.toml",
];
pub const OPTION_ENV: &[&str] = &[
    "NOH_UPDATE_TRUST_JSON",
    "NOH_UPDATE_PACKAGE_ID",
    "NOH_UPDATE_QUALIFICATION_ROOT",
    "NOH_UPDATE_FEED_URL",
    "NOH_UPDATE_CHANNEL",
    "NOH_UPDATE_FORMAT",
    "OPT_LEVEL",
    "DEBUG",
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTFLAGS",
    "CARGO_CFG_TARGET_FEATURE",
    "CARGO_CFG_PANIC",
    "CARGO_CFG_DEBUG_ASSERTIONS",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
];

fn ignored(name: &OsStr) -> bool {
    matches!(
        name.to_str(),
        Some("target" | "dist" | ".git" | ".mcp-dev" | "__pycache__" | ".pytest_cache")
    ) || name
        .to_str()
        .is_some_and(|s| s.ends_with(".pyc") || s.ends_with(".pyo"))
}

fn walk(root: &Path, folder: &Path, files: &mut Vec<PathBuf>, depth: usize) -> io::Result<()> {
    if depth > 32 || files.len() > 65536 {
        return Err(io::Error::other(
            "Build inputs exceed their enumeration limit",
        ));
    }
    if !folder.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        if ignored(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(root, &path, files, depth + 1)?;
        } else if kind.is_file() {
            files.push(path.strip_prefix(root).unwrap().to_owned());
        } else {
            return Err(io::Error::other(
                "Build inputs must be regular files or directories",
            ));
        }
    }
    Ok(())
}

pub fn inputs(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for name in INPUT_FILES {
        if root.join(name).is_file() {
            files.push(PathBuf::from(name));
        }
    }
    for name in INPUT_DIRS {
        walk(root, &root.join(name), &mut files, 0)?;
    }
    files.sort_by_key(|path| relative_name(path));
    Ok(files)
}

fn relative_name(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

pub fn source_hash(root: &Path, files: &[PathBuf]) -> io::Result<String> {
    let mut hash = Sha256::new();
    hash.update(b"noh-source-v1\0");
    let mut files = files.to_vec();
    files.sort_by_key(|path| relative_name(path));
    let mut total = 0u64;
    let mut buffer = [0; 65536];
    for relative in files {
        field(&mut hash, relative_name(&relative).as_bytes());
        let mut file = fs::File::open(root.join(relative))?;
        let length = file.metadata()?.len();
        total = total
            .checked_add(length)
            .ok_or_else(|| io::Error::other("Build input size overflow"))?;
        if total > 1024 * 1024 * 1024 {
            return Err(io::Error::other("Build inputs exceed 1 GiB"));
        }
        hash.update(length.to_le_bytes());
        let mut read = 0;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            read += count as u64;
            if read > length {
                return Err(io::Error::other("Build input changed during hashing"));
            }
            hash.update(&buffer[..count]);
        }
        if read != length {
            return Err(io::Error::other("Build input changed during hashing"));
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn input_watches(root: &Path) -> Vec<PathBuf> {
    INPUT_FILES
        .iter()
        .chain(INPUT_DIRS)
        .chain([&".cargo"])
        .map(|name| root.join(name))
        .filter(|path| path.exists())
        .collect()
}

pub fn vcs_watches(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if root.join("design").is_dir() {
        paths.push(root.join("design"));
    }
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && (path.extension().is_some_and(|e| e == "md" || e == "ps1")
                    || path.file_name() == Some(OsStr::new(".gitignore")))
            {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

struct Output {
    bytes: Vec<u8>,
    truncated: bool,
}
fn run(command: Command) -> Option<Output> {
    let captured = process::run(command, Duration::from_secs(5)).ok()?;
    if !captured.status.success() {
        return None;
    }
    let mut bytes = captured.stdout;
    let truncated = bytes.len() > 65536;
    bytes.truncate(65536);
    Some(Output { bytes, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(root: &Path) {
        for folder in INPUT_DIRS {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        fs::create_dir_all(root.join(".cargo")).unwrap();
        for (name, content) in [
            ("Cargo.toml", "manifest"),
            ("Cargo.lock", "lock"),
            ("build.rs", "build"),
            ("src/lib.rs", "source"),
            ("tools/dev.rs", "tool"),
            ("tests/check.rs", "test"),
            ("assets/font.ttf", "font"),
            ("locales/en.lang", "translation"),
            (".cargo/config.toml", "options"),
        ] {
            fs::write(root.join(name), content).unwrap();
        }
    }
    fn digest(root: &Path) -> String {
        source_hash(root, &inputs(root).unwrap()).unwrap()
    }
    fn identity() -> Identity {
        Identity {
            package_version: "0.1.0".into(),
            git_revision: Some("0123456789abcdef0123456789abcdef01234567".into()),
            git_dirty: Some(false),
            source_sha256: "a".repeat(64),
            rustc_version: Some("rustc 1.98".into()),
            cargo_version: Some("cargo 1.98".into()),
            target: "x86_64-pc-windows-gnu".into(),
            profile: "release".into(),
            features: vec!["mcp".into(), "gui".into()],
            options: vec![("OPT_LEVEL".into(), "s".into())],
        }
    }
    fn fingerprint(info: &Identity) -> String {
        info.render()
            .split("build_fingerprint: \"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn source_hash_is_relocation_order_and_timestamp_independent() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        project(a.path());
        project(b.path());
        let original = digest(a.path());
        assert_eq!(original, digest(b.path()));
        let mut paths = inputs(a.path()).unwrap();
        paths.reverse();
        assert_eq!(original, source_hash(a.path(), &paths).unwrap());
        fs::write(a.path().join("src/lib.rs"), b"source").unwrap();
        assert_eq!(original, digest(a.path()));
    }

    #[test]
    fn changed_and_new_build_inputs_invalidate_source_hash() {
        for name in [
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            "src/lib.rs",
            "tools/dev.rs",
            "tests/check.rs",
            "assets/font.ttf",
            "locales/en.lang",
            ".cargo/config.toml",
        ] {
            let root = tempfile::tempdir().unwrap();
            project(root.path());
            let original = digest(root.path());
            fs::write(root.path().join(name), b"changed").unwrap();
            assert_ne!(original, digest(root.path()), "{name}");
        }
        let root = tempfile::tempdir().unwrap();
        project(root.path());
        let original = digest(root.path());
        fs::write(root.path().join("src/new.rs"), b"new module").unwrap();
        assert_ne!(original, digest(root.path()));
        let watches = input_watches(root.path());
        assert!(watches.contains(&root.path().join("src")));
        assert!(watches.contains(&root.path().join("locales")));
        assert!(!watches.contains(&root.path().to_path_buf()));
        assert!(watches.iter().all(|path| path.exists()));
    }

    #[test]
    fn caches_and_non_compilation_docs_do_not_enter_source_hash() {
        let root = tempfile::tempdir().unwrap();
        project(root.path());
        let original = digest(root.path());
        for directory in ["target", "dist", ".mcp-dev", "assets/__pycache__"] {
            fs::create_dir_all(root.path().join(directory)).unwrap();
            fs::write(root.path().join(directory).join("generated"), b"cache").unwrap();
        }
        fs::write(root.path().join("README.md"), b"docs").unwrap();
        assert_eq!(original, digest(root.path()));
        let watches = vcs_watches(root.path());
        assert!(watches.contains(&root.path().join("README.md")));
        assert!(!watches.contains(&root.path().join("target")));
        assert!(watches.iter().all(|path| path.exists()));
    }

    #[test]
    fn identity_changes_for_sources_toolchain_target_profile_features_and_options() {
        let original = fingerprint(&identity());
        for change in 0..9 {
            let mut info = identity();
            match change {
                0 => info.source_sha256 = "b".repeat(64),
                1 => info.rustc_version = None,
                2 => info.cargo_version = None,
                3 => info.target = "other-target".into(),
                4 => info.profile = "debug".into(),
                5 => info.features.clear(),
                6 => info.options[0].1 = "3".into(),
                7 => info.git_dirty = Some(true),
                _ => info.git_revision = None,
            }
            assert_ne!(original, fingerprint(&info));
        }
        let mut reordered = identity();
        reordered.features.reverse();
        assert_eq!(original, fingerprint(&reordered));
        let mut unknown = identity();
        unknown.git_revision = None;
        unknown.git_dirty = None;
        assert!(
            unknown
                .render()
                .contains("revision unavailable state unavailable")
        );
        assert!(identity().render().contains("01234567 clean; build"));
    }

    #[test]
    fn workspace_paths_are_normalized_without_exposing_flags() {
        assert!(OPTION_ENV.iter().all(|name| is_option(name)));
        assert!(is_option("CARGO_PROFILE_RELEASE_LTO"));
        assert!(!is_option("PATH"));
        assert!(!is_option("UNRELATED_PRIVATE_VALUE"));
        assert!(!is_option("NOH_UPDATE_TOKEN"));
        let a = Path::new("C:/first/user/noh");
        let b = Path::new("C:/second/user/noh");
        assert_eq!(
            normalize_option("-L C:/first/user/noh/lib", a),
            normalize_option("-L C:/second/user/noh/lib", b)
        );
        let mut info = identity();
        info.options
            .push(("RUSTFLAGS".into(), "private compiler flags".into()));
        assert!(!info.render().contains("private compiler flags"));
    }
    #[test]
    fn update_feed_credentials_are_rejected_before_embedding() {
        assert!(credential_free_update_feed(
            "https://api.github.com/repos/example/noh/releases/assets/1"
        ));
        for value in [
            "https://github.com/a?token=synthetic-secret",
            "https://synthetic-secret@api.github.com/a",
            "https://github.com/a#private",
            "http://github.com/a",
        ] {
            assert!(!credential_free_update_feed(value));
        }
    }

    #[test]
    fn unavailable_vcs_and_compiler_are_explicit() {
        let root = tempfile::tempdir().unwrap();
        let info = git_info(root.path());
        assert_eq!(info.revision, None);
        assert_eq!(info.dirty, None);
        assert!(info.watches.iter().all(|path| path.exists()));
        assert_eq!(
            compiler_info(Some(root.path().join("missing-rustc").as_os_str())),
            None
        );
        assert_eq!(cargo_info(None), None);
    }

    // Fixture creation writes objects/indexes and can exceed the production
    // metadata probe's five-second budget on hosted Windows runners. Keep it
    // bounded, but preserve stderr/status instead of converting errors to None.
    fn fixture_git(root: &Path, args: &[&str]) {
        let output = process::run(git_command(root, args), Duration::from_secs(30))
            .unwrap_or_else(|error| panic!("Git fixture {args:?}: {error}"));
        assert!(
            output.status.success(),
            "Git fixture {args:?} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fixture_git(root.path(), &["init", "--quiet"]);
        project(root.path());
        fs::write(root.path().join(".gitignore"), b"/target/\n").unwrap();
        let hooks = root.path().join("disabled-hooks");
        fixture_git(
            root.path(),
            &["config", "core.hooksPath", hooks.to_str().unwrap()],
        );
        fixture_git(root.path(), &["config", "core.autocrlf", "false"]);
        fixture_git(root.path(), &["add", "."]);
        fixture_git(
            root.path(),
            &[
                "-c",
                "user.name=NOH test",
                "-c",
                "user.email=noh@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "--no-verify",
                "-m",
                "synthetic test repository",
            ],
        );
        root
    }

    #[test]
    fn git_records_clean_dirty_and_ignored_files_without_changing_revision() {
        let root = repository();
        let clean = git_info(root.path());
        assert!(clean.revision.is_some());
        assert_eq!(clean.dirty, Some(false));
        fs::create_dir(root.path().join("target")).unwrap();
        fs::write(root.path().join("target/generated"), b"ignored").unwrap();
        assert_eq!(git_info(root.path()).dirty, Some(false));
        fs::write(root.path().join("src/lib.rs"), b"modified").unwrap();
        let dirty = git_info(root.path());
        assert_eq!(dirty.revision, clean.revision);
        assert_eq!(dirty.dirty, Some(true));
        fixture_git(root.path(), &["checkout", "--", "src/lib.rs"]);
        fs::write(root.path().join("src/new.rs"), b"untracked").unwrap();
        assert_eq!(git_info(root.path()).dirty, Some(true));
        assert!(clean.watches.contains(&root.path().join(".git/index")));
        assert!(!clean.watches.contains(&root.path().join(".git")));
        assert!(clean.watches.iter().all(|path| path.exists()));
    }

    #[test]
    fn worktree_revision_and_watches_use_indirected_git_and_common_dirs() {
        let root = repository();
        let worktree = tempfile::tempdir().unwrap();
        fixture_git(
            root.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                worktree.path().to_str().unwrap(),
                "HEAD",
            ],
        );
        let original = git_info(root.path());
        let info = git_info(worktree.path());
        assert_eq!(info.revision, original.revision);
        assert_eq!(info.dirty, Some(false));
        assert!(info.watches.contains(&worktree.path().join(".git")));
        // Git resolves macOS's /var -> /private/var temporary-directory alias.
        let refs = root.path().join(".git/refs").canonicalize().unwrap();
        assert!(
            info.watches
                .iter()
                .any(|path| path.canonicalize().ok().as_ref() == Some(&refs))
        );
        assert!(
            info.watches
                .iter()
                .any(|p| p.ends_with("HEAD") && p.to_string_lossy().contains("worktrees"))
        );
        assert_eq!(digest(root.path()), digest(worktree.path()));
    }
}

fn version(executable: Option<&OsStr>, flag: &str) -> Option<String> {
    let mut command = Command::new(executable?);
    command.arg(flag);
    let output = run(command)?;
    if output.truncated {
        return None;
    }
    let text = String::from_utf8(output.bytes).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}
pub fn compiler_info(executable: Option<&OsStr>) -> Option<String> {
    version(executable, "--version")
}
pub fn cargo_info(executable: Option<&OsStr>) -> Option<String> {
    version(executable, "--version")
}

#[derive(Default)]
pub struct GitInfo {
    pub revision: Option<String>,
    pub dirty: Option<bool>,
    pub watches: Vec<PathBuf>,
}
fn git_command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
        ])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
    ] {
        command.env_remove(key);
    }
    command
}

fn git(root: &Path, args: &[&str]) -> Option<Output> {
    run(git_command(root, args))
}

pub fn git_info(root: &Path) -> GitInfo {
    let mut result = GitInfo {
        watches: if root.join(".git").exists() {
            vec![root.join(".git")]
        } else {
            Vec::new()
        },
        ..Default::default()
    };
    let Some(output) = git(
        root,
        &[
            "rev-parse",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
        ],
    ) else {
        return result;
    };
    if output.truncated {
        return result;
    }
    let Ok(text) = String::from_utf8(output.bytes) else {
        return result;
    };
    let mut lines = text.lines();
    let (Some(top), Some(directory), Some(common)) = (lines.next(), lines.next(), lines.next())
    else {
        return result;
    };
    if Path::new(top).canonicalize().ok() != root.canonicalize().ok() {
        return result;
    }
    // In a normal checkout .git is a directory. Watch files, not its changing object store.
    result.watches.clear();
    if root.join(".git").is_file() {
        result.watches.push(root.join(".git"));
    }
    for folder in [root.join(directory), root.join(common)] {
        for name in ["HEAD", "index", "packed-refs", "refs", "config", "info"] {
            let path = folder.join(name);
            if path.exists() {
                result.watches.push(path);
            }
        }
    }
    result.watches.sort();
    result.watches.dedup();
    if let Some(output) = git(root, &["rev-parse", "--verify", "HEAD"]) {
        let revision = String::from_utf8_lossy(&output.bytes).trim().to_owned();
        if !output.truncated
            && matches!(revision.len(), 40 | 64)
            && revision.bytes().all(|b| b.is_ascii_hexdigit())
        {
            result.revision = Some(revision);
        }
    }
    if let Some(output) = git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=normal",
            "--ignore-submodules=all",
        ],
    ) {
        result.dirty = Some(!output.bytes.is_empty() || output.truncated);
    }
    result
}

pub fn is_option(key: &str) -> bool {
    OPTION_ENV.contains(&key) || key.starts_with("CARGO_PROFILE_")
}
/// Build-time credential guard, not the runtime URL/host authentication policy.
/// Do not embed common userinfo/query credentials even in an invalid feed config.
pub fn credential_free_update_feed(value: &str) -> bool {
    value.strip_prefix("https://").is_some_and(|rest| {
        !value.contains(['?', '#']) && !rest.split('/').next().unwrap_or_default().contains('@')
    })
}
pub fn normalize_option(value: &str, root: &Path) -> String {
    let path = root.to_string_lossy();
    value
        .replace(path.as_ref(), "$SOURCE")
        .replace(&path.replace('\\', "/"), "$SOURCE")
}

pub struct Identity {
    pub package_version: String,
    pub git_revision: Option<String>,
    pub git_dirty: Option<bool>,
    pub source_sha256: String,
    pub rustc_version: Option<String>,
    pub cargo_version: Option<String>,
    pub target: String,
    pub profile: String,
    pub features: Vec<String>,
    pub options: Vec<(String, String)>,
}
impl Identity {
    pub fn render(&self) -> String {
        let mut features = self.features.clone();
        features.sort();
        features.dedup();
        let mut options = self.options.clone();
        options.sort();
        let mut options_hash = Sha256::new();
        options_hash.update(b"noh-options-v1\0");
        for (key, value) in options {
            field(&mut options_hash, key.as_bytes());
            field(&mut options_hash, value.as_bytes());
        }
        let options_sha256 = format!("{:x}", options_hash.finalize());
        let mut hash = Sha256::new();
        hash.update(b"noh-build-v1\0");
        for value in [
            &self.package_version,
            &self.source_sha256,
            &self.target,
            &self.profile,
            &options_sha256,
        ] {
            field(&mut hash, value.as_bytes());
        }
        for value in [&self.git_revision, &self.rustc_version, &self.cargo_version] {
            field(&mut hash, format!("{value:?}").as_bytes());
        }
        field(&mut hash, format!("{:?}", self.git_dirty).as_bytes());
        field(&mut hash, format!("{features:?}").as_bytes());
        let fingerprint = format!("{:x}", hash.finalize());
        let revision = self
            .git_revision
            .as_ref()
            .map_or("revision unavailable", |r| &r[..r.len().min(8)]);
        let state = match self.git_dirty {
            Some(true) => "dirty",
            Some(false) => "clean",
            None => "state unavailable",
        };
        let label = format!(
            "{} ({revision} {state}; build {})",
            self.package_version,
            &fingerprint[..12]
        );
        format!(
            "pub const LABEL: &str = {label:?};\npub const VERSION: &str = LABEL;\nstatic BUILD_INFO: BuildInfo = BuildInfo {{\n\
            schema_version: 1, package_version: {:?}, git_revision: {:?}, git_dirty: {:?},\n\
            source_sha256: {:?}, build_fingerprint: {fingerprint:?}, rustc_version: {:?}, cargo_version: {:?},\n\
            target: {:?}, profile: {:?}, features: &{features:?}, options_sha256: {options_sha256:?},\n}};\n",
            self.package_version,
            self.git_revision,
            self.git_dirty,
            self.source_sha256,
            self.rustc_version,
            self.cargo_version,
            self.target,
            self.profile
        )
    }
}
