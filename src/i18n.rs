//! Embedded catalogs and stable message IDs. No dependency on the GUI or media engine.
use std::{collections::HashMap, path::PathBuf, sync::OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    En,
    Fr,
    De,
    Es,
    Ja,
    Ko,
    Zh,
}
impl Language {
    pub const ALL: [Self; 7] = [
        Self::En,
        Self::Fr,
        Self::De,
        Self::Es,
        Self::Ja,
        Self::Ko,
        Self::Zh,
    ];
    pub fn code(self) -> &'static str {
        ["en", "fr", "de", "es", "ja", "ko", "zh"][self as usize]
    }
    pub fn name(self) -> &'static str {
        [
            "English",
            "Français",
            "Deutsch",
            "Español",
            "日本語",
            "한국어",
            "简体中文",
        ][self as usize]
    }
    pub fn from_locale(locale: &str) -> Option<Self> {
        let code = locale
            .trim()
            .split(['-', '_', '.', '@'])
            .next()?
            .to_ascii_lowercase();
        Self::ALL.into_iter().find(|l| l.code() == code)
    }
    pub fn text(self, key: &str) -> &'static str {
        let catalogs = CATALOGS.get_or_init(|| SOURCES.map(parse));
        catalogs[self as usize]
            .get(key)
            .or_else(|| catalogs[0].get(key))
            .copied()
            .unwrap_or("[missing translation]")
    }
    pub fn format(self, key: &str, args: &[String]) -> String {
        // Single pass: filenames or OS errors containing braces are never interpolated again.
        let mut output = String::new();
        let mut rest = self.text(key);
        while let Some(start) = rest.find('{') {
            output.push_str(&rest[..start]);
            rest = &rest[start..];
            if let Some(end) = rest.find('}') {
                if let Some(value) = rest[1..end].parse::<usize>().ok().and_then(|i| args.get(i)) {
                    output.push_str(value);
                    rest = &rest[end + 1..];
                    continue;
                }
            }
            output.push('{');
            rest = &rest[1..];
        }
        output.push_str(rest);
        output
    }
    /// `key.one` or `key.other` with the count as `{0}`. French uses the
    /// singular for 0 and 1; Japanese, Korean and Chinese have no plural.
    pub fn plural(self, key: &str, count: usize) -> String {
        let one = match self {
            Self::Fr => count <= 1,
            Self::Ja | Self::Ko | Self::Zh => false,
            Self::En | Self::De | Self::Es => count == 1,
        };
        let form = if one { "one" } else { "other" };
        self.format(&format!("{key}.{form}"), &[count.to_string()])
    }
    pub fn decimal(self, value: f64, precision: usize) -> String {
        self.localize_decimal(format!("{value:.precision$}"))
    }
    pub fn localize_decimal(self, value: String) -> String {
        if matches!(self, Self::Fr | Self::De | Self::Es) {
            value.replace('.', ",")
        } else {
            value
        }
    }
}

pub const SOURCES: [&str; 7] = [
    include_str!("../locales/en.lang"),
    include_str!("../locales/fr.lang"),
    include_str!("../locales/de.lang"),
    include_str!("../locales/es.lang"),
    include_str!("../locales/ja.lang"),
    include_str!("../locales/ko.lang"),
    include_str!("../locales/zh.lang"),
];
static CATALOGS: OnceLock<[HashMap<&'static str, &'static str>; 7]> = OnceLock::new();
fn parse(source: &'static str) -> HashMap<&'static str, &'static str> {
    source
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub key: String,
    pub args: Vec<String>,
}
impl Message {
    pub fn new(key: &str, args: &[String]) -> Self {
        Self {
            key: key.into(),
            args: args.to_vec(),
        }
    }
    pub fn render(&self, language: Language) -> String {
        language.format(&self.key, &self.args)
    }
}
impl From<&str> for Message {
    fn from(value: &str) -> Self {
        let mut parts = value.split('|');
        Self {
            key: parts.next().unwrap_or("error.engine").into(),
            args: parts.map(str::to_owned).collect(),
        }
    }
}
impl From<String> for Message {
    fn from(value: String) -> Self {
        value.as_str().into()
    }
}

/// None means follow the operating system, including subsequent launches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choice(pub Option<Language>);
impl Choice {
    pub fn parse(value: &str) -> Self {
        Self(Language::ALL.into_iter().find(|l| l.code() == value.trim()))
    }
    pub fn code(self) -> &'static str {
        self.0.map_or("system", Language::code)
    }
    pub fn resolve(self, locales: &[String]) -> Language {
        self.0.unwrap_or_else(|| {
            locales
                .iter()
                .find_map(|s| Language::from_locale(s))
                .unwrap_or(Language::En)
        })
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
}
pub fn preferences_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base =
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Application Support"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")));
    base.map(|p| p.join("noh/language"))
}

pub fn system_locales() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetUserPreferredUILanguages(
                flags: u32,
                count: *mut u32,
                buffer: *mut u16,
                size: *mut u32,
            ) -> i32;
            fn GetUserDefaultLocaleName(buffer: *mut u16, size: i32) -> i32;
        }
        let (mut count, mut size) = (0, 0);
        unsafe {
            GetUserPreferredUILanguages(8, &mut count, std::ptr::null_mut(), &mut size);
        }
        if size > 0 && size < 65536 {
            let mut buffer = vec![0u16; size as usize];
            if unsafe { GetUserPreferredUILanguages(8, &mut count, buffer.as_mut_ptr(), &mut size) }
                != 0
            {
                return buffer
                    .split(|v| *v == 0)
                    .filter(|s| !s.is_empty())
                    .map(String::from_utf16_lossy)
                    .collect();
            }
        }
        let mut buffer = [0u16; 85];
        let len = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
        if len > 1 {
            return vec![String::from_utf16_lossy(&buffer[..len as usize - 1])];
        }
    }
    #[cfg(target_os = "macos")]
    if let Ok(output) = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
    {
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|s| s.trim().trim_matches([',', '"', '(', ')']).to_owned())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(key) {
            if !value.is_empty() {
                return vec![value];
            }
        }
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_forms_follow_each_language() {
        for (language, count, form) in [
            (Language::En, 0, "other"),
            (Language::En, 1, "one"),
            (Language::Fr, 0, "one"),
            (Language::Fr, 1, "one"),
            (Language::Fr, 2, "other"),
            (Language::De, 1, "one"),
            (Language::Ja, 1, "other"),
        ] {
            let key = "ui.conversion";
            assert_eq!(
                language.plural(key, count),
                language.format(&format!("{key}.{form}"), &[count.to_string()]),
                "{language:?} {count}"
            );
        }
    }
    fn placeholders(value: &str) -> std::collections::BTreeSet<&str> {
        value
            .split('{')
            .skip(1)
            .filter_map(|s| s.split_once('}').map(|(p, _)| p))
            .collect()
    }
    #[test]
    fn catalogs_are_complete_and_placeholders_match() {
        let base = parse(SOURCES[0]);
        for (language, source) in Language::ALL.into_iter().zip(SOURCES) {
            let catalog = parse(source);
            assert_eq!(
                catalog.len(),
                source
                    .lines()
                    .filter(|s| !s.is_empty() && !s.starts_with('#'))
                    .count(),
                "duplicate or invalid key in {}",
                language.code()
            );
            assert_eq!(catalog.len(), base.len(), "{}", language.code());
            for (key, value) in &base {
                let translated = catalog
                    .get(key)
                    .unwrap_or_else(|| panic!("{} missing {key}", language.code()));
                assert!(!translated.is_empty());
                assert_eq!(
                    placeholders(value),
                    placeholders(translated),
                    "{}: {key}",
                    language.code()
                );
            }
        }
    }
    /// Key prefixes built at run time rather than written literally.
    /// `reason.` comes from `plan::reason_key`, applied to engine reason IDs.
    const DYNAMIC_PREFIXES: [&str; 1] = ["reason."];
    fn sources(dir: &std::path::Path, text: &mut String) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, text);
            } else if path.extension().is_some_and(|e| e == "rs") {
                text.push_str(&std::fs::read_to_string(path).unwrap());
            }
        }
    }
    #[test]
    fn catalog_keys_are_reachable() {
        // A key is reachable when src/ names it as a string literal, as the
        // ID of a "key|arg" message, or as the base of a `.one`/`.other`
        // plural. Parity makes the English catalog stand for all seven.
        let mut text = String::new();
        sources(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut text,
        );
        let mut unreachable: Vec<_> = parse(SOURCES[0])
            .into_keys()
            .filter(|key| {
                let base = key
                    .strip_suffix(".one")
                    .or_else(|| key.strip_suffix(".other"))
                    .unwrap_or(key);
                !DYNAMIC_PREFIXES.iter().any(|p| key.starts_with(p))
                    && !text.contains(&format!("\"{key}\""))
                    && !text.contains(&format!("\"{key}|"))
                    && !text.contains(&format!("\"{base}\""))
            })
            .collect();
        unreachable.sort_unstable();
        assert!(unreachable.is_empty(), "unreachable keys: {unreachable:?}");
    }
    #[test]
    fn locales_fallback_and_manual_override() {
        for (locale, language) in [
            ("fr-FR", Language::Fr),
            ("de_DE.UTF-8", Language::De),
            ("ES-mx", Language::Es),
            ("ja-JP", Language::Ja),
            ("ko_KR", Language::Ko),
            ("zh-Hant-TW", Language::Zh),
        ] {
            assert_eq!(Choice::default().resolve(&[locale.into()]), language);
        }
        assert_eq!(Choice::default().resolve(&["pt-BR".into()]), Language::En);
        assert_eq!(
            Choice(Some(Language::Ko)).resolve(&["fr-FR".into()]),
            Language::Ko
        );
        assert_eq!(Choice::parse("invalid"), Choice::default());
    }
    #[test]
    fn choice_persists_and_can_return_to_system() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("language");
        assert_eq!(Choice::read(&path), Choice::default());
        for choice in [Choice(Some(Language::Ja)), Choice::default()] {
            choice.save(&path).unwrap();
            assert_eq!(Choice::read(&path), choice);
        }
    }
    #[test]
    fn interpolation_preserves_user_text() {
        assert!(
            Language::En
                .format("error.open", &["file {1}.mp4".into()])
                .contains("file {1}.mp4")
        );
        assert_eq!(Language::Fr.decimal(1.25, 2), "1,25");
        assert_eq!(Language::En.decimal(1.25, 2), "1.25");
    }
}
