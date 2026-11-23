//! The caption effect the command line and the MCP choose, by id only: its folder
//! comes from the effect folders the application reads (`style::catalog`), never
//! from the request. Everything is checked before a job starts (id, preset, each
//! value against its declared type) and nothing is corrected: a wrong request is
//! refused with what was found instead.
//!
//! Classique (`noh.classic`) stays on today's route: its values become the caption
//! style (size, placement, line width), so its libass fallback draws the same.
use crate::captions::CaptionStyle;
use crate::engine::EngineError;
use serde::Serialize;
use serde_json::Value as J;
use std::path::PathBuf;

/// A setting as the request gives it: typed on the command line, JSON in the MCP.
#[derive(Clone, Debug, PartialEq)]
pub enum Given {
    Text(String),
    Json(J),
}

/// The effect a request asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wanted {
    pub id: String,
    pub preset: Option<String>,
    pub values: Vec<(String, Given)>,
}

/// Which of Classique's own options the request set explicitly (a default does
/// not count): with another effect they are refused, with Classique they must not
/// also come as effect values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub size: bool,
    pub placement: bool,
}

/// What draws the captions.
#[derive(Clone, Debug, PartialEq)]
pub enum Chosen {
    /// Classique, with the request's values folded into the style.
    Classic(CaptionStyle),
    /// An effect plugin, drawn by the GPU engine (no libass fallback).
    Plugin(crate::project::EffectChoice),
}

fn refused(code: &str, detail: impl ToString) -> EngineError {
    EngineError::new(code, "choose_effect", None, detail)
}

/// Resolve against the folders the application reads. `project_preview`: the
/// request is a project preview, which draws no effect outside the app.
pub fn resolve(
    wanted: &Wanted,
    style: CaptionStyle,
    flags: Flags,
    project_preview: bool,
) -> Result<Chosen, EngineError> {
    #[cfg(feature = "effects")]
    let roots = crate::style::catalog::default_roots();
    #[cfg(not(feature = "effects"))]
    let roots = Vec::new();
    resolve_in(wanted, style, flags, project_preview, &roots)
}

#[cfg(not(feature = "effects"))]
pub fn resolve_in(
    _: &Wanted,
    _: CaptionStyle,
    _: Flags,
    _: bool,
    _: &[PathBuf],
) -> Result<Chosen, EngineError> {
    Err(refused(
        "error.effects_unavailable",
        "This build of NOH has no caption effects engine: no effect can be chosen.",
    ))
}

#[cfg(feature = "effects")]
pub fn resolve_in(
    wanted: &Wanted,
    mut style: CaptionStyle,
    flags: Flags,
    project_preview: bool,
    roots: &[PathBuf],
) -> Result<Chosen, EngineError> {
    use crate::project::{CLASSIC, EffectChoice};
    use crate::style::catalog;
    let bad = |detail: String| refused("error.effect", detail);
    let id = wanted.id.trim();
    if id.is_empty() {
        return Err(bad("A caption effect needs a plugin id.".into()));
    }
    if id == CLASSIC {
        let effect = catalog::builtin_classic();
        check_preset(&effect, wanted.preset.as_deref()).map_err(bad)?;
        let values = typed(&effect, &wanted.values).map_err(bad)?;
        for (key, given) in [("size", flags.size), ("placement", flags.placement)] {
            if given && values.contains_key(key) {
                return Err(bad(format!(
                    "Classique's {key} is given twice: as a caption option and as an effect value."
                )));
            }
        }
        validate(&effect, wanted.preset.as_deref(), &values).map_err(bad)?;
        let mut all: std::collections::BTreeMap<String, J> = wanted
            .preset
            .as_deref()
            .and_then(|p| effect.presets.iter().find(|(id, _)| id == p))
            .map(|(_, values)| values.clone().into_iter().collect())
            .unwrap_or_default();
        all.extend(values);
        for (key, value) in all {
            fold_classic(&mut style, &key, &value).map_err(bad)?;
        }
        return Ok(Chosen::Classic(style));
    }
    if flags.size || flags.placement {
        return Err(bad(format!(
            "Size and placement are Classique's options; \"{id}\" has its own settings: give them as effect values."
        )));
    }
    if project_preview {
        return Err(refused(
            "error.effect_preview",
            "A project preview draws no caption effect: export without preview, or preview with the caption burn or the short.",
        ));
    }
    let found = catalog::scan(roots);
    let Some(entry) = found.find(id) else {
        let mut ids: Vec<&str> = found.effects.iter().map(|e| e.effect.id.as_str()).collect();
        ids.push(CLASSIC);
        ids.sort_unstable();
        ids.dedup();
        let folders: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
        let mut detail = format!(
            "No caption effect \"{id}\" in {}. Effects found: {}.",
            if folders.is_empty() {
                "any effect folder".to_string()
            } else {
                folders.join(", ")
            },
            ids.join(", ")
        );
        if !found.rejected.is_empty() {
            let why: Vec<String> = found
                .rejected
                .iter()
                .map(|r| format!("{} ({})", r.dir.display(), r.message))
                .collect();
            detail.push_str(&format!(" Refused folders: {}.", why.join("; ")));
        }
        return Err(bad(detail));
    };
    let effect = &entry.effect;
    check_preset(effect, wanted.preset.as_deref()).map_err(bad)?;
    let values = typed(effect, &wanted.values).map_err(bad)?;
    validate(effect, wanted.preset.as_deref(), &values).map_err(bad)?;
    Ok(Chosen::Plugin(EffectChoice {
        id: id.to_string(),
        preset: wanted.preset.clone(),
        dir: entry.dir.clone(),
        values,
    }))
}

#[cfg(feature = "effects")]
fn check_preset(effect: &crate::style::plugin::Effect, preset: Option<&str>) -> Result<(), String> {
    match preset {
        Some(p) if !effect.presets.iter().any(|(id, _)| id == p) => {
            let ids: Vec<&str> = effect.presets.iter().map(|(id, _)| id.as_str()).collect();
            Err(format!(
                "\"{}\" has no preset \"{p}\". Its presets: {}.",
                effect.id,
                ids.join(", ")
            ))
        }
        _ => Ok(()),
    }
}

/// Each given setting read by its declared type.
#[cfg(feature = "effects")]
fn typed(
    effect: &crate::style::plugin::Effect,
    given: &[(String, Given)],
) -> Result<std::collections::BTreeMap<String, J>, String> {
    let mut out = std::collections::BTreeMap::new();
    for (key, value) in given {
        let Some(p) = effect.params.iter().find(|p| &p.id == key) else {
            let ids: Vec<&str> = effect.params.iter().map(|p| p.id.as_str()).collect();
            return Err(format!(
                "\"{}\" has no setting \"{key}\". Its settings: {}.",
                effect.id,
                ids.join(", ")
            ));
        };
        let value = match value {
            Given::Json(v) => v.clone(),
            Given::Text(t) => parse(&p.id, &p.kind, t)?,
        };
        if out.insert(key.clone(), value).is_some() {
            return Err(format!("The setting \"{key}\" is given twice."));
        }
    }
    Ok(out)
}

/// A typed setting: a number (one decimal comma accepted), true/false, a palette
/// as comma-separated colors, a landscape/portrait pair as JSON, anything else as
/// the text itself. `validate` then checks it against the setting.
pub fn parse(id: &str, kind: &str, text: &str) -> Result<J, String> {
    let t = text.trim();
    let wrong = || format!("\"{id}\" takes a {kind}: \"{text}\" is not one.");
    if t.starts_with('{') {
        return serde_json::from_str(t)
            .map_err(|e| format!("\"{id}\": \"{text}\" is not a landscape/portrait pair ({e})."));
    }
    Ok(match kind {
        "number" => {
            let one = t.matches(',').count() <= 1 && !(t.contains(',') && t.contains('.'));
            let n = one
                .then(|| t.replace(',', ".").parse::<f64>().ok())
                .flatten()
                .filter(|n| n.is_finite())
                .ok_or_else(wrong)?;
            J::from(n)
        }
        "bool" => match t {
            "true" => J::Bool(true),
            "false" => J::Bool(false),
            _ => return Err(wrong()),
        },
        "palette" => J::Array(t.split(',').map(|c| J::String(c.trim().into())).collect()),
        _ => J::String(text.to_string()),
    })
}

/// The plugin's own check (`PluginScene::with_values`): unknown setting, wrong
/// type, out of range, an installed family where no font is taken.
#[cfg(feature = "effects")]
fn validate(
    effect: &crate::style::plugin::Effect,
    preset: Option<&str>,
    values: &std::collections::BTreeMap<String, J>,
) -> Result<(), String> {
    crate::style::plugin::PluginScene::new(effect.clone(), 1920, 1080, Vec::new(), preset)
        .with_values(values)
        .map(|_| ())
}

#[cfg(feature = "effects")]
fn fold_classic(style: &mut CaptionStyle, key: &str, value: &J) -> Result<(), String> {
    use crate::captions::{CaptionPlacement, CaptionSize};
    match (key, value.as_str()) {
        ("size", Some("small")) => style.size = CaptionSize::Small,
        ("size", Some("medium")) => style.size = CaptionSize::Medium,
        ("size", Some("large")) => style.size = CaptionSize::Large,
        ("placement", Some("bottom")) => style.placement = CaptionPlacement::Bottom,
        ("placement", Some("top")) => style.placement = CaptionPlacement::Top,
        ("width", _) => {
            let width = value
                .as_f64()
                .ok_or_else(|| "Classique's width takes one number.".to_string())?;
            style.line_width = Some((width * 100.0).round() as u32);
        }
        _ => return Err(format!("Classique cannot take {key} = {value}.")),
    }
    Ok(())
}

/// What `--list-effects` and `noh_list_effects` show.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Listing {
    /// The folders read, bundled first, and whether each exists.
    pub folders: Vec<Folder>,
    pub effects: Vec<Listed>,
    /// Folders that are not valid effects, and why.
    pub refused: Vec<Refused>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Folder {
    pub path: PathBuf,
    pub exists: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Listed {
    pub id: String,
    /// The English name, or the id.
    pub name: String,
    /// None: built into NOH (Classique, when no folder holds it).
    pub folder: Option<PathBuf>,
    pub presets: Vec<String>,
    pub settings: Vec<Setting>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Setting {
    pub id: String,
    /// color, number, bool, palette, choice, font or text.
    #[serde(rename = "type")]
    pub kind: String,
    pub default: J,
    /// A choice's texts; a font's bundled faces (it also takes
    /// `system:<installed family>`).
    pub choices: Vec<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// 1: shown first in the app; 2: under "more settings".
    pub level: u8,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Refused {
    pub folder: PathBuf,
    pub reason: String,
}

/// The effects a request can name, from the folders the application reads.
pub fn list() -> Listing {
    #[cfg(feature = "effects")]
    let roots = crate::style::catalog::default_roots();
    #[cfg(not(feature = "effects"))]
    let roots = Vec::new();
    list_in(&roots)
}

#[cfg(not(feature = "effects"))]
pub fn list_in(roots: &[PathBuf]) -> Listing {
    Listing {
        folders: roots
            .iter()
            .map(|p| Folder {
                path: p.clone(),
                exists: p.is_dir(),
            })
            .collect(),
        effects: Vec::new(),
        refused: Vec::new(),
    }
}

#[cfg(feature = "effects")]
pub fn list_in(roots: &[PathBuf]) -> Listing {
    use crate::style::{catalog, plugin::Effect};
    let found = catalog::scan(roots);
    let listed = |effect: &Effect, folder: Option<PathBuf>| Listed {
        id: effect.id.clone(),
        name: effect
            .names
            .get("en")
            .cloned()
            .unwrap_or_else(|| effect.id.clone()),
        folder,
        presets: effect.presets.iter().map(|(id, _)| id.clone()).collect(),
        settings: effect
            .params
            .iter()
            .map(|p| Setting {
                id: p.id.clone(),
                kind: p.kind.clone(),
                default: p.default.clone(),
                choices: if p.kind == "font" {
                    crate::fx::text::FACE_NAMES
                        .iter()
                        .map(|s| s.to_string())
                        .collect()
                } else {
                    p.choices.clone()
                },
                min: p.min,
                max: p.max,
                level: p.level,
            })
            .collect(),
    };
    let mut effects: Vec<Listed> = found
        .effects
        .iter()
        .map(|e| listed(&e.effect, Some(e.dir.clone())))
        .collect();
    if found.find(crate::project::CLASSIC).is_none() {
        effects.insert(0, listed(&catalog::builtin_classic(), None));
    }
    Listing {
        folders: roots
            .iter()
            .map(|p| Folder {
                path: p.clone(),
                exists: p.is_dir(),
            })
            .collect(),
        effects,
        refused: found
            .rejected
            .iter()
            .map(|r| Refused {
                folder: r.dir.clone(),
                reason: r.message.clone(),
            })
            .collect(),
    }
}

/// The listing as text, for the terminal.
pub fn listing_text(listing: &Listing) -> String {
    let mut out = String::from("Effect folders read:\n");
    for f in &listing.folders {
        out.push_str(&format!(
            "  {}{}\n",
            f.path.display(),
            if f.exists { "" } else { " (missing)" }
        ));
    }
    out.push_str("\nEffects (--effect ID):\n");
    for e in &listing.effects {
        let folder = e
            .folder
            .as_ref()
            .map_or("built into NOH".to_string(), |p| p.display().to_string());
        out.push_str(&format!("  {}: {} ({folder})\n", e.id, e.name));
        if !e.presets.is_empty() {
            out.push_str(&format!(
                "    presets (--effect-preset): {}\n",
                e.presets.join(", ")
            ));
        }
        for s in &e.settings {
            let mut what = s.kind.clone();
            if !s.choices.is_empty() {
                what.push_str(&format!(": {}", s.choices.join(", ")));
            }
            if s.kind == "font" {
                what.push_str(", or system:<installed family>");
            }
            if let (Some(min), Some(max)) = (s.min, s.max) {
                what.push_str(&format!(", {min} to {max}"));
            }
            out.push_str(&format!(
                "    --effect-value {}=\u{2026}  ({what}; default {})\n",
                s.id, s.default
            ));
        }
    }
    if !listing.refused.is_empty() {
        out.push_str("\nRefused folders:\n");
        for r in &listing.refused {
            out.push_str(&format!("  {}: {}\n", r.folder.display(), r.reason));
        }
    }
    out
}

#[cfg(all(test, feature = "effects"))]
mod tests {
    use super::*;
    use crate::captions::{CaptionPlacement, CaptionSize};
    use std::path::Path;

    fn roots() -> Vec<PathBuf> {
        vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/effects")]
    }
    fn wanted(id: &str, values: &[(&str, &str)]) -> Wanted {
        Wanted {
            id: id.into(),
            preset: None,
            values: values
                .iter()
                .map(|(k, v)| (k.to_string(), Given::Text(v.to_string())))
                .collect(),
        }
    }
    fn go(w: &Wanted, flags: Flags, preview: bool) -> Result<Chosen, EngineError> {
        resolve_in(w, CaptionStyle::default(), flags, preview, &roots())
    }
    const SIZE: Flags = Flags {
        size: true,
        placement: false,
    };
    const PLACEMENT: Flags = Flags {
        size: false,
        placement: true,
    };

    /// Classique's values become the style, so libass's fallback draws the same;
    /// a setting given twice (option and value) is refused, never silently won.
    #[test]
    fn classique_values_become_the_caption_style_and_twice_is_refused() {
        let w = wanted(
            "noh.classic",
            &[("size", "large"), ("placement", "top"), ("width", "80")],
        );
        let Ok(Chosen::Classic(style)) = go(&w, Flags::default(), true) else {
            panic!("Classique stays on today's route, preview included");
        };
        assert_eq!(style.size, CaptionSize::Large);
        assert_eq!(style.placement, CaptionPlacement::Top);
        assert_eq!(style.line_width, Some(8_000));
        let twice = go(&wanted("noh.classic", &[("size", "large")]), SIZE, false).unwrap_err();
        assert!(twice.detail.contains("given twice"), "{}", twice.detail);
        // The option alone with a value for another setting is fine.
        assert!(go(&wanted("noh.classic", &[("placement", "top")]), SIZE, false).is_ok());
        let range = go(
            &wanted("noh.classic", &[("width", "20")]),
            Flags::default(),
            false,
        )
        .unwrap_err();
        assert!(range.detail.contains("width"), "{}", range.detail);
    }

    /// Another effect is found by id in the folders read; its folder comes from
    /// there, its values are typed by declaration and checked now.
    #[test]
    fn a_plugin_is_found_by_id_with_typed_checked_values() {
        let w = wanted(
            "noh.drop",
            &[("font", "system:Some Family"), ("size", "large")],
        );
        let Ok(Chosen::Plugin(choice)) = go(&w, Flags::default(), false) else {
            panic!("noh.drop is bundled");
        };
        assert_eq!(choice.id, "noh.drop");
        assert_eq!(choice.dir, roots()[0].join("drop"));
        assert_eq!(choice.values["font"], J::from("system:Some Family"));
        for (values, says) in [
            (&[("font", "Comic")][..], "font"),
            (&[("colour", "red")][..], "has no setting \"colour\""),
            (&[("size", "huge")][..], "size"),
        ] {
            let e = go(&wanted("noh.drop", values), Flags::default(), false).unwrap_err();
            assert_eq!(e.code, "error.effect");
            assert!(e.detail.contains(says), "{values:?}: {}", e.detail);
        }
        let mut preset = wanted("noh.drop", &[]);
        preset.preset = Some("nope".into());
        let e = go(&preset, Flags::default(), false).unwrap_err();
        assert!(e.detail.contains("no preset \"nope\""), "{}", e.detail);
    }

    /// Classique's options with another effect, and a project preview with one,
    /// are refused: never ignored, never a video without its captions.
    #[test]
    fn classique_options_and_project_previews_are_refused_with_another_effect() {
        let w = wanted("noh.drop", &[]);
        for flags in [SIZE, PLACEMENT] {
            let e = go(&w, flags, false).unwrap_err();
            assert!(e.detail.contains("Classique's options"), "{}", e.detail);
        }
        let e = go(&w, Flags::default(), true).unwrap_err();
        assert_eq!(e.code, "error.effect_preview");
    }

    /// An unknown id is refused with the ids found, the folders read and the
    /// folders refused (with their reason).
    #[test]
    fn an_unknown_id_lists_what_was_found_and_refused() {
        let broken = tempfile::tempdir().unwrap();
        std::fs::create_dir(broken.path().join("bad")).unwrap();
        std::fs::write(broken.path().join("bad/effect.json"), "{").unwrap();
        let roots = [roots()[0].clone(), broken.path().to_path_buf()];
        let e = resolve_in(
            &wanted("noh.nope", &[]),
            CaptionStyle::default(),
            Flags::default(),
            false,
            &roots,
        )
        .unwrap_err();
        for part in [
            "\"noh.nope\"",
            "noh.classic",
            "noh.drop",
            "noh.sweep",
            "Refused folders",
            "bad",
        ] {
            assert!(e.detail.contains(part), "{part}: {}", e.detail);
        }
        assert!(go(&wanted(" ", &[]), Flags::default(), false).is_err());
    }

    /// Values typed by the setting's declared type: a decimal comma, never a list
    /// read as a number; a choice or a text keeps its text.
    #[test]
    fn values_are_read_by_their_declared_type() {
        assert_eq!(parse("w", "number", "1,5"), Ok(J::from(1.5)));
        assert_eq!(parse("w", "number", " 80 "), Ok(J::from(80.0)));
        for bad in ["1,2,3", "1.2,3", "abc", "inf"] {
            assert!(parse("w", "number", bad).is_err(), "{bad}");
        }
        assert_eq!(parse("b", "bool", "true"), Ok(J::Bool(true)));
        assert!(parse("b", "bool", "1").is_err());
        assert_eq!(
            parse("p", "palette", "#FFFFFF, #000000"),
            Ok(serde_json::json!(["#FFFFFF", "#000000"]))
        );
        assert_eq!(parse("c", "choice", "1"), Ok(J::from("1")));
        assert_eq!(parse("t", "text", "true"), Ok(J::from("true")));
        assert_eq!(
            parse("w", "number", r#"{"landscape": 60, "portrait": 90}"#),
            Ok(serde_json::json!({"landscape": 60, "portrait": 90}))
        );
        assert!(parse("w", "number", "{oops").is_err());
    }

    /// The listing: the folders read, every effect with its presets and settings
    /// (a font names the bundled faces), Classique built in when no folder has it.
    #[test]
    fn the_listing_shows_folders_effects_settings_and_refusals() {
        let listing = list_in(&roots());
        assert_eq!(
            listing.folders,
            vec![Folder {
                path: roots()[0].clone(),
                exists: true
            }]
        );
        let ids: Vec<&str> = listing.effects.iter().map(|e| e.id.as_str()).collect();
        for id in [
            "noh.classic",
            "noh.drop",
            "noh.sweep",
            "noh.breath",
            "noh.block",
        ] {
            assert!(ids.contains(&id), "{id}: {ids:?}");
        }
        let drop = listing.effects.iter().find(|e| e.id == "noh.drop").unwrap();
        let font = drop.settings.iter().find(|s| s.id == "font").unwrap();
        assert_eq!(font.kind, "font");
        assert!(font.choices.contains(&"jost_extrabold".to_string()));
        let text = listing_text(&listing);
        assert!(text.contains("system:<installed family>"), "{text}");
        assert!(text.contains("--effect-value font="), "{text}");
        let none = Path::new(env!("CARGO_MANIFEST_DIR")).join("no-such-folder");
        let empty = list_in(std::slice::from_ref(&none));
        assert_eq!(
            empty.folders,
            vec![Folder {
                path: none,
                exists: false
            }]
        );
        assert_eq!(empty.effects.len(), 1);
        assert_eq!(empty.effects[0].id, "noh.classic");
        assert_eq!(empty.effects[0].folder, None);
        assert!(listing_text(&empty).contains("(missing)"));
    }

    #[test]
    fn every_face_name_is_a_bundled_face() {
        for name in crate::fx::text::FACE_NAMES {
            assert!(crate::fx::text::Face::named(name).is_some(), "{name}");
        }
    }
}

/// Without the effects engine nothing can be chosen, Classique included.
#[cfg(all(test, not(feature = "effects")))]
mod no_engine_tests {
    use super::*;

    #[test]
    fn no_effect_can_be_chosen_without_the_engine() {
        let w = Wanted {
            id: "noh.drop".into(),
            ..Wanted::default()
        };
        let e = resolve(&w, CaptionStyle::default(), Flags::default(), false).unwrap_err();
        assert_eq!(e.code, "error.effects_unavailable");
    }
}
