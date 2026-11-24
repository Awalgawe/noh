//! The effect folders: the plugins bundled next to the executable and the user's
//! own, read on demand (never at application start). Nothing is imported or kept
//! between sessions: the folders are the only source.
use super::plugin::Effect;
use std::path::{Path, PathBuf};

/// A plugin that loaded and validated.
#[derive(Clone, Debug)]
pub struct Entry {
    pub dir: PathBuf,
    pub effect: Effect,
}

/// A plugin folder that was refused, with the message to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected {
    pub dir: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub effects: Vec<Entry>,
    pub rejected: Vec<Rejected>,
}

impl Catalog {
    pub fn find(&self, id: &str) -> Option<&Entry> {
        self.effects.iter().find(|e| e.effect.id == id)
    }
}

/// `effects/` next to the application: beside `noh.exe`, so one folder up for the
/// command line and the MCP, which live in `bin/`. In a macOS app the executables
/// are in `Contents/MacOS`, where only signed code may be: its effects are in
/// `Contents/Resources`. Cargo does not copy it there: a development build gets
/// it from `assets/effects/` by an explicit copy.
pub fn bundled_dir() -> Option<PathBuf> {
    bundled_dir_for(&std::env::current_exe().ok()?)
}

/// `bundled_dir` for the executable at `exe` (update/windows.rs uses the same rule).
pub fn bundled_dir_for(exe: &Path) -> Option<PathBuf> {
    let folder = exe.parent()?;
    let app = if folder.file_name() == Some(std::ffi::OsStr::new("bin")) {
        folder.parent().unwrap_or(folder)
    } else {
        folder
    };
    if cfg!(target_os = "macos")
        && app.file_name() == Some(std::ffi::OsStr::new("MacOS"))
        && let Some(contents) = app.parent()
        && contents.file_name() == Some(std::ffi::OsStr::new("Contents"))
    {
        return Some(contents.join("Resources/effects"));
    }
    Some(app.join("effects"))
}

/// The user's folder, beside the language preference (`noh/effects`).
pub fn user_dir() -> Option<PathBuf> {
    crate::i18n::preferences_path()?
        .parent()
        .map(|noh| noh.join("effects"))
}

/// The folders the application reads, bundled first.
pub fn default_roots() -> Vec<PathBuf> {
    bundled_dir().into_iter().chain(user_dir()).collect()
}

/// Every immediate subfolder of each root is one plugin. A missing root is not an
/// error. Two plugins with the same id are both refused: neither silently wins.
pub fn scan(roots: &[PathBuf]) -> Catalog {
    let mut loaded: Vec<Entry> = Vec::new();
    let mut rejected = Vec::new();
    for root in roots {
        let Ok(read) = std::fs::read_dir(root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = read
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            match Effect::load(&dir) {
                Ok(effect) => loaded.push(Entry { dir, effect }),
                Err(e) => rejected.push(Rejected { message: e.0, dir }),
            }
        }
    }
    let mut effects = Vec::new();
    for entry in &loaded {
        let same: Vec<&Entry> = loaded
            .iter()
            .filter(|o| o.effect.id == entry.effect.id)
            .collect();
        if same.len() == 1 {
            effects.push(entry.clone());
        } else {
            let others: Vec<String> = same
                .iter()
                .filter(|o| o.dir != entry.dir)
                .map(|o| o.dir.display().to_string())
                .collect();
            rejected.push(Rejected {
                dir: entry.dir.clone(),
                message: format!(
                    "id \"{}\" is also used by {}",
                    entry.effect.id,
                    others.join(", ")
                ),
            });
        }
    }
    Catalog { effects, rejected }
}

/// Classique's effect.json, built into NOH: the command line and the MCP draw
/// their captions with it when `effects/` is missing beside the executable. The
/// same file as the bundled plugin, so the two cannot drift.
pub const CLASSIC_JSON: &str = include_str!("../../assets/effects/classique/effect.json");

/// The built-in Classique (it has no shader file to read).
pub fn builtin_classic() -> Effect {
    Effect::from_json(Path::new(""), CLASSIC_JSON.as_bytes())
        .expect("the built-in Classique is valid")
}

/// Load one plugin folder for an export or a preview, after checking its id.
pub fn load_one(dir: &Path, id: &str) -> Result<Effect, String> {
    let effect = Effect::load(dir).map_err(|e| e.0)?;
    if effect.id != id {
        return Err(format!(
            "{}: plugin id \"{}\" differs from the requested \"{id}\"",
            dir.display(),
            effect.id
        ));
    }
    Ok(effect)
}
