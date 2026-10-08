//! Relocate the native media dependency closure into an application bundle.
use super::{Result, run};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Default, Debug)]
struct MachO {
    dependencies: Vec<String>,
    rpaths: Vec<String>,
    id: Option<String>,
    minimum: [u32; 3],
}

fn parse_load_commands(text: &str) -> Result<MachO> {
    let mut info = MachO::default();
    let mut command = "";
    for line in text.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("cmd ") {
            command = value;
        } else if let Some(value) = line.strip_prefix("name ") {
            let value = value.split(" (offset ").next().unwrap_or(value).to_owned();
            match command {
                "LC_ID_DYLIB" => info.id = Some(value),
                "LC_LOAD_DYLIB"
                | "LC_LOAD_WEAK_DYLIB"
                | "LC_REEXPORT_DYLIB"
                | "LC_LOAD_UPWARD_DYLIB" => info.dependencies.push(value),
                _ => {}
            }
        } else if command == "LC_RPATH" {
            if let Some(value) = line.strip_prefix("path ") {
                info.rpaths
                    .push(value.split(" (offset ").next().unwrap_or(value).to_owned());
            }
        } else if let Some(value) = match command {
            "LC_BUILD_VERSION" => line.strip_prefix("minos "),
            "LC_VERSION_MIN_MACOSX" => line.strip_prefix("version "),
            _ => None,
        } {
            let mut minimum = [0; 3];
            for (index, number) in value.split('.').enumerate() {
                *minimum
                    .get_mut(index)
                    .ok_or("Invalid macOS minimum version")? = number.parse()?;
            }
            info.minimum = info.minimum.max(minimum);
        }
    }
    Ok(info)
}

fn inspect(path: &Path) -> Result<MachO> {
    let mut command = Command::new("/usr/bin/otool");
    command.arg("-l").arg(path);
    parse_load_commands(&String::from_utf8(run(command)?.stdout)?)
}

fn system(path: &str) -> bool {
    path.starts_with("/usr/lib/") || path.starts_with("/System/Library/")
}

fn loader_path(value: &str, owner: &Path) -> Option<PathBuf> {
    if let Some(relative) = value.strip_prefix("@loader_path/") {
        Some(owner.parent()?.join(relative))
    } else if Path::new(value).is_absolute() {
        Some(value.into())
    } else {
        None
    }
}

fn resolve(value: &str, owner: &Path, info: &MachO) -> Result<PathBuf> {
    let candidate = if let Some(relative) = value.strip_prefix("@rpath/") {
        info.rpaths
            .iter()
            .filter_map(|rpath| loader_path(rpath, owner))
            .map(|folder| folder.join(relative))
            .find(|path| path.is_file())
    } else {
        loader_path(value, owner)
    };
    candidate
        .ok_or_else(|| format!("Cannot resolve {value} in {}", owner.display()))?
        .canonicalize()
        .map_err(Into::into)
}

fn relative(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut result = PathBuf::new();
    for _ in common..from.len() {
        result.push("..");
    }
    for part in &to[common..] {
        result.push(part.as_os_str());
    }
    result
}

fn copy_notices(source: &Path, resources: &Path, copied: &mut BTreeSet<PathBuf>) -> Result<()> {
    let Some(keg) = source
        .ancestors()
        .find(|path| path.join("INSTALL_RECEIPT.json").is_file())
    else {
        // NOH executables have their own build identity and source inventory.
        return Ok(());
    };
    if !copied.insert(keg.to_owned()) {
        return Ok(());
    }
    let formula = keg
        .parent()
        .and_then(Path::file_name)
        .ok_or("Missing formula name")?;
    let destination = resources
        .join("licenses/homebrew")
        .join(formula)
        .join(keg.file_name().ok_or("Missing formula version")?);
    fs::create_dir_all(&destination)?;
    for entry in fs::read_dir(keg)? {
        let entry = entry?;
        let name = entry.file_name();
        let lower = name.to_string_lossy().to_ascii_lowercase();
        if entry.file_type()?.is_file()
            && (lower.starts_with("license")
                || lower.starts_with("copying")
                || lower.starts_with("copyright")
                || lower.starts_with("notice")
                || lower == "install_receipt.json"
                || lower == "sbom.spdx.json")
        {
            fs::copy(entry.path(), destination.join(&name))?;
        }
    }
    // Preserve the exact local build recipe when Homebrew supplies it.
    if let Ok(entries) = fs::read_dir(keg.join(".brew")) {
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.path().extension().is_some_and(|ext| ext == "rb")
            {
                fs::copy(entry.path(), destination.join(entry.file_name()))?;
            }
        }
    }
    Ok(())
}

pub(super) fn sign(path: &Path) -> Result<()> {
    let mut command = Command::new("/usr/bin/codesign");
    command
        .args(["--force", "--sign", "-", "--timestamp=none"])
        .arg(path);
    run(command)?;
    Ok(())
}

/// Copy every non-system load dependency, rewrite its references and sign each
/// Mach-O. All work stays in staging; installed Homebrew files are never edited.
pub(super) fn embed(app: &Path, roots: &[(PathBuf, PathBuf)]) -> Result<serde_json::Value> {
    let frameworks = app.join("Contents/Frameworks");
    let resources = app.join("Contents/Resources");
    fs::create_dir_all(&frameworks)?;
    let mpv = if let Some(path) = std::env::var_os("NOH_LIBMPV").filter(|p| !p.is_empty()) {
        PathBuf::from(path)
    } else {
        [
            "/opt/homebrew/lib/libmpv.2.dylib",
            "/usr/local/lib/libmpv.2.dylib",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .ok_or("Portable macOS packaging needs native libmpv: brew install mpv or set NOH_LIBMPV")?
    };
    let mut pending: VecDeque<_> = roots.iter().cloned().collect();
    pending.push_back((mpv, frameworks.join("libmpv.2.dylib")));
    let mut nodes = BTreeMap::<PathBuf, (PathBuf, MachO)>::new();
    let mut destinations = BTreeMap::<PathBuf, PathBuf>::new();
    let mut minimum = [0; 3];
    let architecture = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        std::env::consts::ARCH
    };
    while let Some((source, destination)) = pending.pop_front() {
        let source = source.canonicalize()?;
        if nodes.contains_key(&source) {
            continue;
        }
        if let Some(previous) = destinations.insert(destination.clone(), source.clone()) {
            return Err(format!(
                "Conflicting libraries {} and {} map to {}",
                previous.display(),
                source.display(),
                destination.display()
            )
            .into());
        }
        let mut command = Command::new("/usr/bin/lipo");
        command.arg(&source).args(["-verify_arch", architecture]);
        run(command)?;
        let info = inspect(&source)?;
        minimum = minimum.max(info.minimum);
        // Homebrew's SDL2 compatibility library dlopens SDL3 by this exact
        // loader-relative name. It has no LC_LOAD_DYLIB entry for that runtime.
        if source
            .components()
            .any(|part| part.as_os_str() == "sdl2-compat")
        {
            let cellar = source
                .ancestors()
                .find(|path| path.file_name().is_some_and(|name| name == "Cellar"))
                .ok_or("Cannot locate SDL compatibility runtime prefix")?;
            let sdl3 = cellar
                .parent()
                .ok_or("Missing Homebrew prefix")?
                .join("opt/sdl3/lib/libSDL3.dylib");
            pending.push_back((sdl3, frameworks.join("libSDL3.dylib")));
        }
        for dependency in &info.dependencies {
            if system(dependency) {
                continue;
            }
            let resolved = resolve(dependency, &source, &info)?;
            let name = resolved.file_name().ok_or("Missing library filename")?;
            if resolved.extension().is_none_or(|ext| ext != "dylib") {
                return Err(
                    format!("Unsupported non-system dependency: {}", resolved.display()).into(),
                );
            }
            pending.push_back((resolved.clone(), frameworks.join(name)));
        }
        if destination.exists() {
            let mut permissions = fs::metadata(&destination)?.permissions();
            permissions.set_mode(permissions.mode() | 0o200);
            fs::set_permissions(&destination, permissions)?;
        }
        fs::copy(&source, &destination).map_err(|error| {
            format!(
                "Cannot stage {} at {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
        let mut permissions = fs::metadata(&destination)?.permissions();
        permissions.set_mode(permissions.mode() | 0o200);
        fs::set_permissions(&destination, permissions)?;
        nodes.insert(source, (destination, info));
    }

    let mut provenance = Vec::new();
    let mut notices = BTreeSet::new();
    for (source, (destination, info)) in &nodes {
        let mut command = Command::new("/usr/bin/install_name_tool");
        let mut changed = false;
        if info.id.is_some() {
            command.arg("-id").arg(format!(
                "@loader_path/{}",
                destination.file_name().unwrap().to_string_lossy()
            ));
            changed = true;
        }
        for dependency in &info.dependencies {
            if system(dependency) {
                continue;
            }
            let resolved = resolve(dependency, source, info)?;
            let bundled = &nodes
                .get(&resolved)
                .ok_or("Incomplete dependency closure")?
                .0;
            command.arg("-change").arg(dependency).arg(format!(
                "@loader_path/{}",
                relative(destination.parent().unwrap(), bundled).display()
            ));
            changed = true;
        }
        // Every load reference is now either absolute system code or loader-relative.
        for rpath in &info.rpaths {
            command.arg("-delete_rpath").arg(rpath);
            changed = true;
        }
        if changed {
            command.arg(destination);
            run(command)?;
        }
        let relocated = inspect(destination)?;
        for dependency in &relocated.dependencies {
            if system(dependency) {
                continue;
            }
            if !dependency.starts_with("@loader_path/") {
                return Err(format!("Non-portable dependency remains: {dependency}").into());
            }
            let resolved = resolve(dependency, destination, &relocated)?;
            if !resolved.starts_with(app.canonicalize()?) {
                return Err(format!("Dependency escapes the bundle: {dependency}").into());
            }
        }
        if !relocated.rpaths.is_empty() {
            return Err("External run paths remain".into());
        }
        sign(destination)?;
        copy_notices(source, &resources, &mut notices)?;
        provenance.push(serde_json::json!({
            "path": destination.strip_prefix(app)?, "source": source,
            "source_sha256": format!("{:x}", Sha256::digest(fs::read(source)?)),
            "dependencies": relocated.dependencies
        }));
    }
    Ok(serde_json::json!({
        "schema_version": 1, "architecture": architecture,
        "minimum_macos": format!("{}.{}.{}", minimum[0], minimum[1], minimum[2]),
        "signature": "ad-hoc; not Developer ID signed or notarized",
        "mach_o": provenance
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_identity_dependencies_rpaths_and_minimum_os() {
        let info = parse_load_commands("cmd LC_ID_DYLIB\n name /opt/lib.dylib (offset 24)\ncmd LC_LOAD_DYLIB\n name /usr/lib/libSystem.B.dylib (offset 24)\ncmd LC_LOAD_WEAK_DYLIB\n name @rpath/with spaces.dylib (offset 24)\ncmd LC_RPATH\n path @loader_path/../lib (offset 12)\ncmd LC_BUILD_VERSION\n minos 26.6.2\n sdk 27.0\n").unwrap();
        assert_eq!(info.id.as_deref(), Some("/opt/lib.dylib"));
        assert_eq!(
            info.dependencies,
            ["/usr/lib/libSystem.B.dylib", "@rpath/with spaces.dylib"]
        );
        assert_eq!(info.rpaths, ["@loader_path/../lib"]);
        assert_eq!(info.minimum, [26, 6, 2]);
    }

    #[test]
    fn resolves_loader_and_run_paths_without_guessing_host_locations() {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("lib.dylib");
        fs::write(&library, []).unwrap();
        let owner = root.path().join("other.dylib");
        let info = MachO {
            rpaths: vec!["@loader_path/../missing".into(), "@loader_path/".into()],
            ..MachO::default()
        };
        assert_eq!(
            resolve("@rpath/lib.dylib", &owner, &info).unwrap(),
            library.canonicalize().unwrap()
        );
        assert!(resolve("@rpath/missing.dylib", &owner, &info).is_err());
        assert!(resolve("lib.dylib", &owner, &info).is_err());
        assert_eq!(
            relative(
                Path::new("/app/Contents/MacOS/bin"),
                Path::new("/app/Contents/Frameworks/lib.dylib")
            ),
            Path::new("../../Frameworks/lib.dylib")
        );
        assert!(!system("/usr/local/lib/lib.dylib"));
    }
}
