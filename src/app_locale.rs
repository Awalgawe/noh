//! Language and theme preferences and fonts are loaded once, outside the render loop.
use super::*;
use app_style as style;
use noh::i18n::{self, Choice};

pub struct Locale {
    pub choice: Choice,
    pub language: Language,
    /// The interface theme (maintainer, 2026-11-12), saved beside the language.
    pub theme: Theme,
    system: Vec<String>,
    pub(super) path: Option<PathBuf>,
}
impl Locale {
    pub fn load() -> Self {
        let path = i18n::preferences_path();
        let system = i18n::system_locales();
        let mut choice = path.as_ref().map(|p| Choice::read(p)).unwrap_or_default();
        // Reproducible visual QA, without writing the user's preference file.
        if std::env::var_os("NOH_CAPTURE_UI").is_some()
            && let Ok(code) = std::env::var("NOH_LANGUAGE")
        {
            choice = Choice::parse(&code);
        }
        // Captures draw the theme they ask for (dark unless light), and Options
        // says that one, whatever the user's file or the system says.
        let theme = if std::env::var_os("NOH_CAPTURE_UI").is_some() {
            match std::env::var("NOH_CAPTURE_THEME").as_deref() {
                Ok("light") => Theme::Light,
                _ => Theme::Dark,
            }
        } else {
            path.as_deref()
                .map(|p| Theme::read(&Theme::path(p)))
                .unwrap_or_default()
        };
        Self {
            language: choice.resolve(&system),
            choice,
            theme,
            system,
            path,
        }
    }
}

/// The interface theme: follow the operating system (the default, as NOH always
/// did), or keep dark or light whatever the system says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}
impl Theme {
    /// The theme file, beside the language file (`<config>/noh/theme`).
    pub fn path(language: &std::path::Path) -> PathBuf {
        language.with_file_name("theme")
    }
    fn code(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Theme::System => "theme.system",
            Theme::Dark => "theme.dark",
            Theme::Light => "theme.light",
        }
    }
    /// An unknown or missing value follows the system.
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "dark" => Theme::Dark,
            "light" => Theme::Light,
            _ => Theme::System,
        }
    }
    pub fn read(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .map(|s| Self::parse(&s))
            .unwrap_or_default()
    }
    pub fn save(self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.code())
    }
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Theme::System => egui::ThemePreference::System,
            Theme::Dark => egui::ThemePreference::Dark,
            Theme::Light => egui::ThemePreference::Light,
        }
    }
}
impl NohApp {
    pub fn language_selector(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let l = self.locale.language;
        let before = self.locale.choice;
        let label = self.locale.choice.0.map_or_else(
            || format!("{} · {}", l.text("system"), l.name()),
            |language| language.name().into(),
        );
        style::framed_select(ui, |ui| {
            egui::ComboBox::from_id_salt("language-choice")
                .icon(style::select_chevron)
                .selected_text(label)
                .width(120.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.locale.choice,
                        Choice::default(),
                        l.text("system"),
                    );
                    ui.separator();
                    for language in Language::ALL {
                        ui.selectable_value(
                            &mut self.locale.choice,
                            Choice(Some(language)),
                            language.name(),
                        );
                    }
                })
                .response
                .on_hover_text(l.text("language"))
        });
        if self.locale.choice != before {
            self.locale.language = self.locale.choice.resolve(&self.locale.system);
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(
                self.locale.language.text("title").into(),
            ));
            let saved = self
                .locale
                .path
                .as_ref()
                .ok_or_else(|| std::io::Error::other("No user configuration directory"))
                .and_then(|path| self.locale.choice.save(path));
            if let Err(error) = saved {
                self.notice = Some(Message::new("error.preferences", &[error.to_string()]));
            }
            ctx.request_repaint();
        }
    }

    /// The theme: System (closed, it says what the system gives), Dark, Light.
    /// Applied at once and saved beside the language.
    pub fn theme_selector(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let l = self.locale.language;
        let before = self.locale.theme;
        let label = match (self.locale.theme, ctx.system_theme()) {
            (Theme::System, Some(system)) => {
                let shown = match system {
                    egui::Theme::Dark => Theme::Dark,
                    egui::Theme::Light => Theme::Light,
                };
                format!("{} · {}", l.text("theme.system"), l.text(shown.key()))
            }
            (theme, _) => l.text(theme.key()).into(),
        };
        style::framed_select(ui, |ui| {
            egui::ComboBox::from_id_salt("theme-choice")
                .icon(style::select_chevron)
                .selected_text(label)
                .width(120.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.locale.theme,
                        Theme::System,
                        l.text("theme.system"),
                    );
                    ui.separator();
                    for theme in [Theme::Dark, Theme::Light] {
                        ui.selectable_value(&mut self.locale.theme, theme, l.text(theme.key()));
                    }
                })
                .response
                .on_hover_text(l.text("theme"))
        });
        if self.locale.theme != before {
            ctx.set_theme(self.locale.theme.preference());
            let saved = self
                .locale
                .path
                .as_deref()
                .ok_or_else(|| std::io::Error::other("No user configuration directory"))
                .and_then(|path| self.locale.theme.save(&Theme::path(path)));
            if let Err(error) = saved {
                self.notice = Some(Message::new(
                    "error.theme_preferences",
                    &[error.to_string()],
                ));
            }
            ctx.request_repaint();
        }
    }
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // Body text is Noto Sans Regular, ahead of egui's defaults (kept for symbols).
    fonts.font_data.insert(
        "noto-regular".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/NotoSans-Regular.ttf"
        ))),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "noto-regular".into());
    fonts.font_data.insert(
        "noto-semibold".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/NotoSans-SemiBold.ttf"
        ))),
    );
    fonts.font_data.insert(
        "noh-cjk".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/NOHCJK.otf"
        ))),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("noh-cjk".into());
    }
    let mut semibold = vec!["noto-semibold".into()];
    semibold.extend(
        fonts.families[&egui::FontFamily::Proportional]
            .iter()
            .cloned(),
    );
    fonts
        .families
        .insert(egui::FontFamily::Name("semibold".into()), semibold);
    // The NOH wordmark only (assets/NOTICE.md).
    fonts.font_data.insert(
        "jost-extrabold".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/Jost-ExtraBold.ttf"
        ))),
    );
    fonts.families.insert(
        egui::FontFamily::Name("logo".into()),
        vec!["jost-extrabold".into()],
    );
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;
    use skrifa::MetadataProvider;

    const REGULAR: &[u8] = include_bytes!("../assets/NotoSans-Regular.ttf");
    const SEMIBOLD: &[u8] = include_bytes!("../assets/NotoSans-SemiBold.ttf");

    fn advance(font: &skrifa::FontRef, c: char, size: f32) -> f32 {
        let glyph = font.charmap().map(c).expect("glyph");
        font.glyph_metrics(
            skrifa::instance::Size::new(size),
            skrifa::instance::LocationRef::default(),
        )
        .advance_width(glyph)
        .unwrap()
    }

    /// The pinned Noto Sans Regular, first in the family,
    /// lays out Latin, Greek and Cyrillic text with its own metrics.
    #[test]
    fn noto_sans_regular_lays_out_with_its_own_metrics() {
        let face = skrifa::FontRef::new(REGULAR).unwrap();
        assert_eq!(face.attributes().weight, skrifa::attribute::Weight::NORMAL);
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "noto-regular".into(),
            std::sync::Arc::new(egui::FontData::from_static(REGULAR)),
        );
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "noto-regular".into());
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        ctx.begin_pass(Default::default());
        for text in [
            "Déposez vos images",
            "Ωμέγα λέξη",
            "Привет мир",
            "0:13,0 – 0:18,0",
        ] {
            let expected: f32 = text.chars().map(|c| advance(&face, c, 14.0)).sum();
            let width = ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(
                        text.into(),
                        egui::FontId::proportional(14.0),
                        egui::Color32::WHITE,
                    )
                    .size()
                    .x
            });
            // Kerning and pixel rounding may differ slightly from raw advances.
            assert!(
                (width - expected).abs() <= 1.0 + expected * 0.02,
                "{text}: egui {width} vs Noto {expected}"
            );
        }
        ctx.end_pass().textures_delta.clear();
    }

    #[test]
    fn body_text_uses_noto_regular_and_headings_noto_semibold() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        ctx.begin_pass(Default::default());
        ctx.fonts_mut(|fonts| {
            let families = &fonts.definitions().families;
            let proportional = &families[&egui::FontFamily::Proportional];
            assert_eq!(proportional[0], "noto-regular");
            assert_eq!(proportional.last().unwrap(), "noh-cjk");
            let semibold = &families[&egui::FontFamily::Name("semibold".into())];
            assert_eq!(semibold[..2], ["noto-semibold", "noto-regular"]);
        });
        ctx.end_pass().textures_delta.clear();
    }

    /// Noto Sans digits share one advance, so time readouts
    /// do not jitter inside their fixed-width slot.
    #[test]
    fn noto_sans_digits_are_tabular_in_both_weights() {
        for data in [REGULAR, SEMIBOLD] {
            let face = skrifa::FontRef::new(data).unwrap();
            let widths: Vec<f32> = ('0'..='9').map(|c| advance(&face, c, 1000.0)).collect();
            assert!(
                widths.iter().all(|w| (w - widths[0]).abs() < 0.01),
                "digit advances differ: {widths:?}"
            );
        }
    }
    #[test]
    fn embedded_fonts_cover_every_translation_and_language_name() {
        let face = skrifa::FontRef::new(include_bytes!("../assets/NotoSans-SemiBold.ttf")).unwrap();
        assert_eq!(
            face.attributes().weight,
            skrifa::attribute::Weight::SEMI_BOLD
        );
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        ctx.begin_pass(Default::default());
        let mut chars = std::collections::BTreeSet::new();
        chars.extend(['⚙', 'ⓘ', '⚠', '✓', '↑', '↓', '↻', '×', '♪']);
        for source in i18n::SOURCES {
            for line in source.lines().filter(|s| !s.starts_with('#')) {
                if let Some((_, value)) = line.split_once('=') {
                    chars.extend(value.chars());
                }
            }
        }
        for language in Language::ALL {
            chars.extend(language.name().chars());
        }
        ctx.fonts_mut(|fonts| {
            let definitions = fonts.definitions();
            for family in [
                egui::FontFamily::Proportional,
                egui::FontFamily::Monospace,
                egui::FontFamily::Name("semibold".into()),
            ] {
                // Inspect charmaps directly: egui 0.36.2's has_glyph compares
                // face IDs and gives false negatives for the primary face.
                let faces: Vec<_> = definitions.families[&family]
                    .iter()
                    .map(|name| {
                        let data = &definitions.font_data[name];
                        skrifa::FontRef::from_index(data.font.as_ref(), data.index).unwrap()
                    })
                    .collect();
                for c in &chars {
                    assert!(
                        faces.iter().any(|font| font.charmap().map(*c).is_some()),
                        "Missing glyph {c}"
                    );
                }
            }
        });
        // This test inspects fonts without a renderer to consume texture updates.
        ctx.end_pass().textures_delta.clear();
    }
}
