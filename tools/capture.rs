//! Deterministic GUI capture matrix runner.
use super::{Result, process};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const PROCESS_DEADLINE: Duration = Duration::from_secs(30);
const DEFAULT_STATES: &[&str] = &[
    "empty",
    "ready",
    "mixed",
    "analyzing",
    "preview",
    "cancelling",
    "failed",
    "succeeded",
];
const STATES: &[&str] = &[
    "empty",
    "ready",
    "mixed",
    "analyzing",
    "preview",
    "preview-stale",
    "cancelling",
    "failed",
    "succeeded",
    "succeeded-stale",
    // Single-screen layout states used by the fixed capture campaign.
    "components",
    "partial",
    "song-only",
    "lyrics-menu",
    "lyrics-generating",
    "lyrics-track-menu",
    "short",
    "exporting",
    "done",
    "stale",
    "exists",
    "unreadable",
    "details",
    "options",
    "resources",
    "resources-options",
];
const DEFAULT_LANGUAGES: &[&str] = &["fr", "de", "ja"];
const LANGUAGES: &[&str] = &["en", "fr", "de", "es", "ja", "ko", "zh"];
const WIDE: (u32, u32) = (980, 850);
const COMPACT: (u32, u32) = (420, 540);
/// One capture per fixed layout case: state, tweaks, language, theme, size, reference_name.
type LayoutCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    (u32, u32),
    &'static str,
);
const LAYOUT: &[LayoutCase] = &[
    ("empty", "", "fr", "dark", WIDE, "empty-fr-dark-980"),
    ("partial", "", "de", "light", WIDE, "partial-de-light-980"),
    ("empty", "", "ja", "dark", COMPACT, "empty-ja-dark-420"),
    (
        "partial",
        "",
        "fr",
        "light",
        COMPACT,
        "partial-fr-light-420",
    ),
    ("ready", "", "fr", "dark", WIDE, "ready-fr-dark-980"),
    ("ready", "", "ja", "light", WIDE, "ready-ja-light-980"),
    ("ready", "", "de", "dark", COMPACT, "ready-de-dark-420"),
    (
        "ready",
        "scroll=228",
        "fr",
        "light",
        COMPACT,
        "ready-scroll-228-fr-light-420",
    ),
    (
        "lyrics-menu",
        "",
        "fr",
        "dark",
        WIDE,
        "lyrics-menu-fr-dark-980",
    ),
    (
        "lyrics-generating",
        "",
        "de",
        "light",
        WIDE,
        "lyrics-generating-de-light-980",
    ),
    (
        "lyrics-track-menu",
        "",
        "ja",
        "dark",
        COMPACT,
        "lyrics-track-menu-ja-dark-420",
    ),
    (
        "lyrics-menu",
        "",
        "de",
        "light",
        COMPACT,
        "lyrics-menu-de-light-420",
    ),
    ("short", "", "fr", "dark", WIDE, "short-fr-dark-980"),
    (
        "short",
        "view=short",
        "de",
        "light",
        WIDE,
        "short-view-short-de-light-980",
    ),
    (
        "short",
        "scroll=320",
        "ja",
        "dark",
        COMPACT,
        "short-scroll-320-ja-dark-420",
    ),
    (
        "short",
        "view=short,framing=fill",
        "fr",
        "light",
        COMPACT,
        "short-view-short-framing-fill-fr-light-420",
    ),
    (
        "short",
        "restart=1",
        "fr",
        "dark",
        WIDE,
        "short-restart-1-fr-dark-980",
    ),
    (
        "short",
        "restart=1,view=short",
        "de",
        "light",
        WIDE,
        "short-restart-1-view-short-de-light-980",
    ),
    (
        "short",
        "restart=1,open=popover,scroll=320",
        "ja",
        "dark",
        COMPACT,
        "short-restart-1-open-popover-scroll-320-ja-dark-420",
    ),
    (
        "short",
        "restart=1,view=short,framing=fill",
        "fr",
        "light",
        COMPACT,
        "short-restart-1-view-short-framing-fill-fr-light-420",
    ),
    ("exporting", "", "ja", "dark", WIDE, "exporting-ja-dark-980"),
    (
        "exporting",
        "export=short",
        "fr",
        "light",
        WIDE,
        "exporting-export-short-fr-light-980",
    ),
    (
        "exporting",
        "",
        "de",
        "dark",
        COMPACT,
        "exporting-de-dark-420",
    ),
    (
        "exporting",
        "scroll=168",
        "ja",
        "light",
        COMPACT,
        "exporting-scroll-168-ja-light-420",
    ),
    ("done", "", "fr", "dark", WIDE, "done-fr-dark-980"),
    ("done", "", "de", "light", WIDE, "done-de-light-980"),
    ("done", "", "ja", "dark", COMPACT, "done-ja-dark-420"),
    ("done", "", "fr", "light", COMPACT, "done-fr-light-420"),
    ("stale", "", "de", "dark", WIDE, "stale-de-dark-980"),
    ("stale", "", "ja", "light", WIDE, "stale-ja-light-980"),
    ("stale", "", "fr", "dark", COMPACT, "stale-fr-dark-420"),
    ("stale", "", "de", "light", COMPACT, "stale-de-light-420"),
    ("exists", "", "fr", "dark", WIDE, "exists-fr-dark-980"),
    ("exists", "", "ja", "light", WIDE, "exists-ja-light-980"),
    ("exists", "", "de", "dark", COMPACT, "exists-de-dark-420"),
    ("exists", "", "fr", "light", COMPACT, "exists-fr-light-420"),
    (
        "unreadable",
        "",
        "ja",
        "dark",
        WIDE,
        "unreadable-ja-dark-980",
    ),
    (
        "unreadable",
        "",
        "de",
        "light",
        WIDE,
        "unreadable-de-light-980",
    ),
    (
        "unreadable",
        "",
        "fr",
        "dark",
        COMPACT,
        "unreadable-fr-dark-420",
    ),
    (
        "unreadable",
        "",
        "ja",
        "light",
        COMPACT,
        "unreadable-ja-light-420",
    ),
    ("details", "", "fr", "dark", WIDE, "details-fr-dark-980"),
    (
        "details",
        "",
        "de",
        "light",
        COMPACT,
        "details-de-light-420",
    ),
    ("options", "", "fr", "light", WIDE, "options-fr-light-980"),
    ("options", "", "ja", "dark", COMPACT, "options-ja-dark-420"),
    // Additional popup states.
    ("ready", "open=fades", "fr", "dark", WIDE, ""),
    ("ready", "open=fades", "de", "light", COMPACT, ""),
    // Soundtrack without visual media.
    ("song-only", "", "fr", "dark", WIDE, ""),
    ("song-only", "", "de", "light", COMPACT, ""),
];
const TWEAK_KEYS: &[&str] = &[
    "view", "framing", "restart", "open", "scroll", "export", "playhead", "preset",
];
const DEFAULT_THEMES: &[&str] = &["dark", "light"];
const DEFAULT_SCALES: &[&str] = &["1", "1.5", "2"];
const DEFAULT_SIZES: &[(u32, u32)] = &[(420, 540), (980, 850)];

#[derive(clap::Args)]
pub struct Options {
    #[arg(long)]
    pub app: PathBuf,
    #[arg(long)]
    pub ffmpeg: PathBuf,
    #[arg(long)]
    pub output: PathBuf,
    #[arg(long)]
    pub states: Option<String>,
    #[arg(long)]
    pub languages: Option<String>,
    #[arg(long)]
    pub themes: Option<String>,
    #[arg(long)]
    pub scales: Option<String>,
    #[arg(long)]
    pub sizes: Option<String>,
    /// Capture the fixed layout cases instead of the combinatorial matrix; --states filters it.
    #[arg(long, value_parser = ["layout"])]
    pub matrix: Option<String>,
    /// Folder of rendered reference images (`<reference_name>.png`) shown beside each capture.
    #[arg(long)]
    pub reference: Option<PathBuf>,
    /// Layout tweaks for every case, e.g. `view=short,restart=1`.
    #[arg(long)]
    pub tweaks: Option<String>,
    /// Real project JSON (`cargo dev fixtures layout`) that states are applied over.
    #[arg(long)]
    pub project: Option<PathBuf>,
}

#[derive(Clone)]
struct Case {
    state: String,
    language: String,
    theme: String,
    scale: String,
    width: u32,
    height: u32,
    tweaks: String,
    reference_name: Option<String>,
    stem: String,
}

/// Validate `key=value` pairs; the application parses the same syntax.
fn tweaks(value: &str) -> Result<String> {
    let mut keys = BTreeSet::new();
    for pair in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (key, item) = pair
            .split_once('=')
            .ok_or_else(|| format!("Invalid tweak {pair:?}; expected key=value"))?;
        if !TWEAK_KEYS.contains(&key) || item.is_empty() || !keys.insert(key) {
            return Err(format!(
                "Invalid tweak {pair:?}; keys are {} (each once)",
                TWEAK_KEYS.join(", ")
            )
            .into());
        }
    }
    Ok(value.trim().to_owned())
}

fn stem_part(tweaks: &str) -> String {
    tweaks.replace([',', '='], "_")
}

fn layout_cases(options: &Options) -> Result<Vec<Case>> {
    let filter = options
        .states
        .clone()
        .map(|s| comma_values(Some(s), &[], STATES, "state"))
        .transpose()?;
    let scale = comma_values(options.scales.clone(), &["1"], &["1", "1.5", "2"], "scale")?;
    let mut cases = Vec::new();
    for (state, extra, language, theme, (width, height), reference_name) in LAYOUT {
        if filter
            .as_ref()
            .is_some_and(|f| !f.iter().any(|s| s == state))
        {
            continue;
        }
        for scale in &scale {
            let label = if reference_name.is_empty() {
                format!(
                    "{state}-{}-{language}-{theme}-{width}x{height}",
                    stem_part(extra)
                )
            } else {
                (*reference_name).to_owned()
            };
            cases.push(Case {
                state: (*state).to_owned(),
                language: (*language).to_owned(),
                theme: (*theme).to_owned(),
                scale: scale.clone(),
                width: *width,
                height: *height,
                tweaks: tweaks(extra)?,
                reference_name: (!reference_name.is_empty()).then(|| (*reference_name).to_owned()),
                stem: format!("{label}-{}", scale.replace('.', "p")),
            });
        }
    }
    if cases.is_empty() {
        return Err("The state filter selects no layout case".into());
    }
    Ok(cases)
}

fn comma_values(
    value: Option<String>,
    defaults: &[&str],
    allowed: &[&str],
    label: &str,
) -> Result<Vec<String>> {
    let values: Vec<String> = value
        .map(|v| v.split(',').map(str::trim).map(str::to_owned).collect())
        .unwrap_or_else(|| defaults.iter().map(|s| (*s).to_owned()).collect());
    let mut unique = BTreeSet::new();
    for item in &values {
        if item.is_empty() || !allowed.contains(&item.as_str()) {
            return Err(format!(
                "Invalid {label} value {item:?}; expected one of {}",
                allowed.join(", ")
            )
            .into());
        }
        if !unique.insert(item.clone()) {
            return Err(format!("Duplicate {label} value {item:?}").into());
        }
    }
    if values.is_empty() {
        return Err(format!("At least one {label} is required").into());
    }
    Ok(values)
}

fn sizes(value: Option<String>) -> Result<Vec<(u32, u32)>> {
    let parsed = if let Some(value) = value {
        let mut result = Vec::new();
        for item in value.split(',').map(str::trim) {
            let (w, h) = item
                .split_once('x')
                .or_else(|| item.split_once('X'))
                .ok_or_else(|| format!("Invalid size {item:?}; expected WIDTHxHEIGHT"))?;
            let width: u32 = w.parse().map_err(|_| format!("Invalid size {item:?}"))?;
            let height: u32 = h.parse().map_err(|_| format!("Invalid size {item:?}"))?;
            if width == 0 || height == 0 {
                return Err(format!("Size dimensions must be positive: {item}").into());
            }
            result.push((width, height));
        }
        result
    } else {
        DEFAULT_SIZES.to_vec()
    };
    let mut seen = BTreeSet::new();
    for size in &parsed {
        if !seen.insert(*size) {
            return Err(format!("Duplicate size {}x{}", size.0, size.1).into());
        }
    }
    if parsed.is_empty() {
        return Err("At least one size is required".into());
    }
    Ok(parsed)
}

fn build_cases(options: &Options) -> Result<Vec<Case>> {
    if options.matrix.is_some() {
        return layout_cases(options);
    }
    let extra = tweaks(options.tweaks.as_deref().unwrap_or(""))?;
    let states = comma_values(options.states.clone(), DEFAULT_STATES, STATES, "state")?;
    let languages = comma_values(
        options.languages.clone(),
        DEFAULT_LANGUAGES,
        LANGUAGES,
        "language",
    )?;
    let themes = comma_values(
        options.themes.clone(),
        DEFAULT_THEMES,
        &["dark", "light"],
        "theme",
    )?;
    let scales = comma_values(
        options.scales.clone(),
        DEFAULT_SCALES,
        &["1", "1.5", "2"],
        "scale",
    )?;
    let sizes = sizes(options.sizes.clone())?;
    let mut cases = Vec::new();
    for state in states {
        for language in &languages {
            for theme in &themes {
                for scale in &scales {
                    for (width, height) in &sizes {
                        let scale_label = scale.replace('.', "p");
                        let variant = if extra.is_empty() {
                            String::new()
                        } else {
                            format!("-{}", stem_part(&extra))
                        };
                        let stem = format!(
                            "{state}{variant}-{language}-{theme}-{scale_label}-{}x{}",
                            width, height
                        );
                        cases.push(Case {
                            state: state.clone(),
                            language: language.clone(),
                            theme: theme.clone(),
                            scale: scale.clone(),
                            width: *width,
                            height: *height,
                            tweaks: extra.clone(),
                            reference_name: None,
                            stem,
                        });
                    }
                }
            }
        }
    }
    Ok(cases)
}

fn logical_dimensions(report: &Value) -> Option<(f64, f64)> {
    let viewport = &report["viewport"];
    let width = viewport["logical_width"]
        .as_f64()
        .or_else(|| viewport["width"].as_f64())
        .or_else(|| report["logical_width"].as_f64());
    let height = viewport["logical_height"]
        .as_f64()
        .or_else(|| viewport["height"].as_f64())
        .or_else(|| report["logical_height"].as_f64());
    Some((width?, height?))
}

fn pixels_per_point(report: &Value) -> Option<f64> {
    report["pixels_per_point"]
        .as_f64()
        .or_else(|| report["viewport"]["pixels_per_point"].as_f64())
}

fn validate_report(case: &Case, report: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let actual_state = report["requested_state"].as_str().unwrap_or("");
    if actual_state != case.state {
        errors.push(format!(
            "report state was {actual_state:?}, expected {:?}",
            case.state
        ));
    }
    let captured_state = report["captured_state"].as_str().unwrap_or("");
    if captured_state != case.state {
        errors.push(format!(
            "captured state was {captured_state:?}, expected {:?}",
            case.state
        ));
    }
    let actual_tweaks = report["requested_tweaks"].as_str().unwrap_or("");
    if actual_tweaks != case.tweaks {
        errors.push(format!(
            "report tweaks were {actual_tweaks:?}, expected {:?}",
            case.tweaks
        ));
    }
    if report["fixture"].as_bool() != Some(true) {
        errors.push("application report does not confirm a deterministic fixture".to_owned());
    }
    match logical_dimensions(report) {
        Some((width, height)) => {
            let scale = case.scale.parse::<f64>().unwrap_or(1.0);
            if (width - f64::from(case.width)).abs() * scale > 1.0
                || (height - f64::from(case.height)).abs() * scale > 1.0
            {
                errors.push(format!(
                    "reported logical size was {width}x{height}, expected {}x{}",
                    case.width, case.height
                ));
            }
        }
        None => errors.push("report omitted logical viewport width/height".to_owned()),
    }
    match pixels_per_point(report) {
        Some(actual) => {
            let expected: f64 = case.scale.parse().unwrap_or_default();
            if (actual - expected).abs() > 0.05 {
                errors.push(format!("reported scale was {actual}, expected {expected}"));
            }
        }
        None => errors.push("report omitted pixels_per_point".to_owned()),
    }
    match report["capture_ready"].as_bool() {
        Some(true) => {}
        Some(false) => errors.push("application report says capture was not ready".to_owned()),
        None => errors.push("report omitted boolean capture_ready".to_owned()),
    }
    match report["timed_out"].as_bool() {
        Some(false) => {}
        Some(true) => errors.push("application reported that fixture setup timed out".to_owned()),
        None => errors.push("report omitted boolean timed_out".to_owned()),
    }
    errors
}

fn ensure_new(path: &Path) -> Result<()> {
    if path.exists() {
        return Err(format!(
            "Refusing to overwrite existing capture output: {}",
            path.display()
        )
        .into());
    }
    Ok(())
}

fn capture_case(options: &Options, case: &Case) -> Value {
    let ppm = options.output.join(format!("{}.ppm", case.stem));
    let app_report = options.output.join(format!("{}.json", case.stem));
    let png = options.output.join(format!("{}.png", case.stem));
    let mut errors = Vec::new();

    let mut command = Command::new(&options.app);
    match &options.project {
        Some(project) => command.env("NOH_CAPTURE_PROJECT", project),
        None => command.env_remove("NOH_CAPTURE_PROJECT"),
    };
    command
        .env_remove("NOH_CAPTURE_SETTINGS")
        .env_remove("NOH_CAPTURE_SECTION")
        .env("NOH_CAPTURE_TWEAKS", &case.tweaks)
        .env("NOH_FFMPEG", &options.ffmpeg)
        .env("NOH_CAPTURE_UI", &ppm)
        .env("NOH_LANGUAGE", &case.language)
        .env("NOH_CAPTURE_STATE", &case.state)
        .env("NOH_CAPTURE_WIDTH", case.width.to_string())
        .env("NOH_CAPTURE_HEIGHT", case.height.to_string())
        .env("NOH_CAPTURE_SCALE", &case.scale)
        .env("NOH_CAPTURE_THEME", &case.theme);
    match process::run(command, PROCESS_DEADLINE) {
        Ok(output) if output.status.success() => {}
        Ok(output) => errors.push(format!(
            "GUI exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => errors.push(format!("GUI launch/capture failed: {error}")),
    }

    let app_json = match fs::read(&app_report) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => {
                errors.extend(validate_report(case, &value));
                Some(value)
            }
            Err(error) => {
                errors.push(format!("invalid app report JSON: {error}"));
                None
            }
        },
        Err(error) => {
            errors.push(format!("app report missing/unreadable: {error}"));
            None
        }
    };

    let image = if ppm.is_file() {
        let mut command = Command::new(&options.ffmpeg);
        command
            .args(["-hide_banner", "-loglevel", "error", "-n", "-i"])
            .arg(&ppm)
            .args(["-frames:v", "1"])
            .arg(&png);
        match process::run(command, PROCESS_DEADLINE) {
            Ok(output) if output.status.success() && png.is_file() => {
                if let Err(error) = fs::remove_file(&ppm) {
                    errors.push(format!("could not remove converted PPM: {error}"));
                }
                Some(format!("{}.png", case.stem))
            }
            Ok(output) => {
                errors.push(format!(
                    "FFmpeg conversion failed ({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
                None
            }
            Err(error) => {
                errors.push(format!("FFmpeg conversion failed: {error}"));
                None
            }
        }
    } else {
        errors.push("GUI did not produce a PPM image".to_owned());
        None
    };

    // Copy the reference image beside the capture so the sheet is self-contained.
    let reference = match (&options.reference, &case.reference_name) {
        (Some(folder), Some(reference_name)) => {
            let source = folder.join(format!("{reference_name}.png"));
            let copy = format!("{}.reference.png", case.stem);
            match fs::copy(&source, options.output.join(&copy)) {
                Ok(_) => Some(copy),
                Err(error) => {
                    errors.push(format!(
                        "reference {} unavailable: {error}",
                        source.display()
                    ));
                    None
                }
            }
        }
        _ => None,
    };

    json!({
        "state": case.state,
        "tweaks": case.tweaks,
        "reference_name": case.reference_name,
        "reference": reference,
        "language": case.language,
        "theme": case.theme,
        "requested_scale": case.scale,
        "requested_width": case.width,
        "requested_height": case.height,
        "image": image,
        "app_report": app_json,
        "errors": errors,
    })
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn write_html(path: &Path, cases: &[Value]) -> Result<()> {
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>NOH UI captures</title><style>body{font:14px system-ui,sans-serif;margin:24px;background:#17191d;color:#eee}h1{font-size:1.5rem}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:16px}.card{background:#25282e;border:1px solid #41454e;border-radius:8px;padding:12px}.card img{display:block;width:100%;height:auto;background:#111}.meta{line-height:1.5}.error{color:#ff9d9d;white-space:pre-wrap}</style><h1>NOH UI captures</h1><div class=\"grid\">",
    );
    for case in cases {
        let variant = [
            case["tweaks"].as_str().unwrap_or(""),
            case["reference_name"].as_str().unwrap_or(""),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        let title = format!(
            "{}{} · {} · {} · {}×{} @ {}x",
            case["state"].as_str().unwrap_or("?"),
            if variant.is_empty() {
                String::new()
            } else {
                format!(" ({variant})")
            },
            case["language"].as_str().unwrap_or("?"),
            case["theme"].as_str().unwrap_or("?"),
            case["requested_width"].as_u64().unwrap_or(0),
            case["requested_height"].as_u64().unwrap_or(0),
            case["requested_scale"].as_str().unwrap_or("?")
        );
        html.push_str("<section class=\"card\"><div class=\"meta\"><strong>");
        html.push_str(&html_escape(&title));
        html.push_str("</strong>");
        if let Some((width, height)) = logical_dimensions(&case["app_report"]) {
            let actual_scale = pixels_per_point(&case["app_report"])
                .map(|scale| format!("{scale}x"))
                .unwrap_or_else(|| "unknown scale".to_owned());
            html.push_str(&format!(
                "<br>Actual: {}×{} logical px @ {}",
                html_escape(&format_number(width)),
                html_escape(&format_number(height)),
                html_escape(&actual_scale)
            ));
        }
        html.push_str("</div>");
        if let Some(image) = case["image"].as_str() {
            html.push_str("<a href=\"");
            html.push_str(&html_escape(image));
            html.push_str("\"><img loading=\"lazy\" src=\"");
            html.push_str(&html_escape(image));
            html.push_str("\" alt=\"");
            html.push_str(&html_escape(&title));
            html.push_str("\"></a>");
        }
        if let Some(reference) = case["reference"].as_str() {
            html.push_str("<p class=\"meta\">Reference image</p><a href=\"");
            html.push_str(&html_escape(reference));
            html.push_str("\"><img loading=\"lazy\" src=\"");
            html.push_str(&html_escape(reference));
            html.push_str("\" alt=\"Reference image\"></a>");
        }
        if let Some(errors) = case["errors"].as_array().filter(|items| !items.is_empty()) {
            html.push_str("<p class=\"error\">");
            for (index, error) in errors.iter().enumerate() {
                if index > 0 {
                    html.push_str("<br>");
                }
                html.push_str(&html_escape(error.as_str().unwrap_or("capture error")));
            }
            html.push_str("</p>");
        }
        html.push_str("</section>");
    }
    html.push_str("</div></html>");
    fs::write(path, html)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_readiness_does_not_require_a_completed_analysis() {
        let case = Case {
            state: "analyzing".into(),
            language: "ja".into(),
            theme: "light".into(),
            scale: "1.5".into(),
            width: 420,
            height: 540,
            tweaks: String::new(),
            reference_name: None,
            stem: "analysis".into(),
        };
        let mut report = json!({
            "requested_state": "analyzing", "captured_state": "analyzing", "requested_tweaks": "",
            "fixture": true, "capture_ready": true, "timed_out": false,
            "viewport": {"width": 420, "height": 540},
            "pixels_per_point": 1.5, "ready": false
        });
        assert!(validate_report(&case, &report).is_empty());
        // Observed Cocoa rounding at 1.5x is just under one physical pixel.
        report["viewport"]["height"] = 540.65625.into();
        assert!(validate_report(&case, &report).is_empty());
        report["viewport"]["height"] = 541.into();
        assert_eq!(validate_report(&case, &report).len(), 1);
        report["viewport"]["height"] = 540.into();
        report["captured_state"] = "empty".into();
        report["viewport"]["height"] = 400.into();
        report["pixels_per_point"] = 1.into();
        assert_eq!(validate_report(&case, &report).len(), 3);
        report["timed_out"] = true.into();
        assert_eq!(validate_report(&case, &report).len(), 4);
    }

    fn options(states: Option<&str>) -> Options {
        Options {
            app: PathBuf::new(),
            ffmpeg: PathBuf::new(),
            output: PathBuf::new(),
            states: states.map(Into::into),
            languages: None,
            themes: None,
            scales: None,
            sizes: None,
            matrix: Some("layout".into()),
            reference: None,
            tweaks: None,
            project: None,
        }
    }

    #[test]
    fn layout_matrix_names_every_reference_once_with_valid_tweaks() {
        let cases = build_cases(&options(None)).unwrap();
        assert_eq!(cases.len(), LAYOUT.len());
        let boards: BTreeSet<_> = cases
            .iter()
            .filter_map(|c| c.reference_name.clone())
            .collect();
        assert_eq!(
            boards.len(),
            44,
            "40 state reference images + Details + Options"
        );
        let stems: BTreeSet<_> = cases.iter().map(|c| c.stem.clone()).collect();
        assert_eq!(stems.len(), cases.len());
        assert!(cases.iter().all(|c| STATES.contains(&c.state.as_str())));
        let shorts = build_cases(&options(Some("short"))).unwrap();
        assert_eq!(shorts.len(), 8);
        assert!(tweaks("view=short,view=video").is_err());
        assert!(tweaks("zoom=2").is_err());
        assert!(build_cases(&options(Some("mixed"))).is_err());
    }

    #[test]
    fn an_existing_capture_is_preserved() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("case.json");
        fs::write(&path, b"previous evidence").unwrap();
        assert!(ensure_new(&path).is_err());
        assert_eq!(fs::read(path).unwrap(), b"previous evidence");
    }
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

pub fn run(options: Options) -> Result<()> {
    if !options.app.is_file() {
        return Err(format!("GUI executable does not exist: {}", options.app.display()).into());
    }
    if !options.ffmpeg.is_file() {
        return Err(format!(
            "FFmpeg executable does not exist: {}",
            options.ffmpeg.display()
        )
        .into());
    }
    let cases = build_cases(&options)?;
    fs::create_dir_all(&options.output)?;
    ensure_new(&options.output.join("report.json"))?;
    ensure_new(&options.output.join("index.html"))?;
    for case in &cases {
        for extension in ["ppm", "json", "png", "reference.png"] {
            ensure_new(&options.output.join(format!("{}.{}", case.stem, extension)))?;
        }
    }

    eprintln!(
        "Capturing {} UI cases into {}",
        cases.len(),
        options.output.display()
    );
    let mut results = Vec::with_capacity(cases.len());
    let mut failures = 0usize;
    let mut current_state = String::new();
    for case in &cases {
        if current_state != case.state {
            current_state.clone_from(&case.state);
            eprintln!("State group: {}", case.state);
        }
        let result = capture_case(&options, case);
        if !result["errors"].as_array().is_none_or(Vec::is_empty) {
            failures += 1;
        }
        results.push(result);
    }
    let report = json!({
        "application": options.app,
        "ffmpeg": options.ffmpeg,
        "case_count": results.len(),
        "failed_cases": failures,
        "cases": results,
    });
    fs::write(
        options.output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    write_html(&options.output.join("index.html"), &results)?;
    eprintln!(
        "Capture complete: {} cases, {} failed; see {}",
        results.len(),
        failures,
        options.output.join("index.html").display()
    );
    if failures > 0 {
        return Err(format!(
            "{} capture case(s) failed; details are in report.json",
            failures
        )
        .into());
    }
    Ok(())
}
