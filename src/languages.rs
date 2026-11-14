//! Language packs (Task 41 S6b): `languages/<code>/language.json` beside the
//! executable, on the effects' model. A pack is data only: the language's
//! syllable rule and, when built, a dictionary file. The dictionaries are public
//! datasets: they are never committed, `tools/build_syllable_dicts.py` builds
//! them from D:\data next to the manifests of a build.
use crate::syllables::{self, Language, Rule, Source};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_DICTIONARY: u64 = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    id: String,
    name: BTreeMap<String, String>,
    version: String,
    syllables: Rule,
    #[serde(default)]
    dictionary: Option<DictionaryInfo>,
}

/// Where a pack's dictionary comes from, shown with its proposals.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DictionaryInfo {
    pub file: String,
    pub name: String,
    pub source: String,
    pub licence: String,
    #[serde(default)]
    pub attribution: String,
}

pub struct Pack {
    pub id: String,
    pub name: BTreeMap<String, String>,
    pub version: String,
    pub rule: Rule,
    pub dictionary: Option<DictionaryInfo>,
    dir: PathBuf,
    /// Sorted (word, split) pairs, read on first use; None when the file is
    /// missing or invalid (the rule alone then).
    words: OnceLock<Option<Vec<(String, String)>>>,
}

impl Pack {
    fn words(&self) -> Option<&[(String, String)]> {
        self.words
            .get_or_init(|| {
                let info = self.dictionary.as_ref()?;
                let path = self.dir.join(&info.file);
                if std::fs::metadata(&path).ok()?.len() > MAX_DICTIONARY {
                    return None;
                }
                let text = std::fs::read_to_string(path).ok()?;
                let mut words: Vec<(String, String)> = text
                    .lines()
                    .filter_map(|l| l.split_once('\t'))
                    .map(|(w, s)| (w.to_string(), s.to_string()))
                    .collect();
                words.sort_by(|a, b| a.0.cmp(&b.0));
                Some(words)
            })
            .as_deref()
    }

    /// The dictionary's split of a lower-case word.
    pub fn lookup(&self, word: &str) -> Option<String> {
        let words = self.words()?;
        words
            .binary_search_by(|(w, _)| w.as_str().cmp(word))
            .ok()
            .map(|i| words[i].1.clone())
    }

    /// Whether the dictionary file is there and readable.
    pub fn has_dictionary(&self) -> bool {
        self.words().is_some()
    }
}

#[derive(Default)]
pub struct Packs {
    pub packs: Vec<Pack>,
    /// Folders that are not valid packs, and why.
    pub rejected: Vec<(PathBuf, String)>,
}

/// `languages/` next to the executable (like `effects/`).
pub fn bundled_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|folder| folder.join("languages"))
}

/// Read every pack under `roots`; the first pack of a language wins.
pub fn scan(roots: &[PathBuf]) -> Packs {
    let mut out = Packs::default();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            match load(&dir) {
                Ok(pack) if out.get(&pack.id).is_none() => out.packs.push(pack),
                Ok(_) => {}
                Err(reason) => out.rejected.push((dir, reason)),
            }
        }
    }
    out
}

fn load(dir: &Path) -> Result<Pack, String> {
    let path = dir.join("language.json");
    let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if size > MAX_MANIFEST {
        return Err(format!(
            "language.json is {size} bytes, more than {MAX_MANIFEST}"
        ));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let m: Manifest = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let folder = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if m.id != folder
        || !(2..=3).contains(&m.id.len())
        || !m.id.bytes().all(|b| b.is_ascii_lowercase())
    {
        return Err(format!(
            "id {:?} must be the folder's name, 2 or 3 lower-case letters",
            m.id
        ));
    }
    if let Some(d) = &m.dictionary
        && (d.file.contains(['/', '\\']) || d.file.starts_with('.'))
    {
        return Err(format!(
            "dictionary file {:?} must be a plain file name",
            d.file
        ));
    }
    Ok(Pack {
        id: m.id,
        name: m.name,
        version: m.version,
        rule: m.syllables,
        dictionary: m.dictionary,
        dir: dir.to_path_buf(),
        words: OnceLock::new(),
    })
}

impl Packs {
    pub fn get(&self, id: &str) -> Option<&Pack> {
        self.packs.iter().find(|p| p.id == id)
    }

    /// The proposed syllables of `word` in `language` (a code such as "fr"),
    /// with their source and, for a dictionary, its name.
    pub fn propose(
        &self,
        word: &str,
        language: Option<&str>,
    ) -> (Vec<String>, Source, Option<String>) {
        let pack = language.and_then(|l| self.get(l));
        let lookup = |w: &str| pack.and_then(|p| p.lookup(w));
        let dictionary: Option<&dyn Fn(&str) -> Option<String>> =
            pack.filter(|p| p.has_dictionary()).map(|_| &lookup as _);
        let (parts, source) = syllables::propose(
            word,
            &Language {
                rule: pack.map(|p| &p.rule),
                dictionary,
            },
        );
        let name = (source == Source::Dictionary)
            .then(|| {
                pack.and_then(|p| p.dictionary.as_ref())
                    .map(|d| d.name.clone())
            })
            .flatten();
        (parts, source, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(root: &Path, id: &str, manifest: &str, dictionary: Option<&str>) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("language.json"), manifest).unwrap();
        if let Some(d) = dictionary {
            std::fs::write(dir.join("dictionary.txt"), d).unwrap();
        }
    }

    const EN: &str = r#"{"id": "en", "name": {"en": "English", "fr": "Anglais"}, "version": "1",
        "syllables": {"vowels": "aeiouy", "suffixes": [{"text": "ing", "syllable": "own", "bases": ["plain", "undouble"]}]},
        "dictionary": {"file": "dictionary.txt", "name": "Moby", "source": "test", "licence": "public domain"}}"#;

    #[test]
    fn packs_load_validate_and_fall_back_to_their_rule() {
        let root = tempfile::tempdir().unwrap();
        pack(
            root.path(),
            "en",
            EN,
            Some("remember\tre-mem-ber\nrun\trun\nabsent-minded\tab-sent--mind-ed\n"),
        );
        pack(root.path(), "fr", &EN.replace("\"en\"", "\"fr\""), None);
        pack(
            root.path(),
            "xx",
            r#"{"id": "xx", "name": {}, "version": "1", "syllables": {}, "plugin": "code.dll"}"#,
            None,
        );
        pack(
            root.path(),
            "de",
            r#"{"id": "en", "name": {}, "version": "1", "syllables": {}}"#,
            None,
        );
        let packs = scan(&[root.path().to_path_buf()]);
        assert_eq!(
            packs
                .packs
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["en", "fr"]
        );
        assert_eq!(
            packs.rejected.len(),
            2,
            "an unknown key and an id that is not the folder: {:?}",
            packs.rejected
        );
        let (parts, source, name) = packs.propose("Remember", Some("en"));
        assert_eq!(
            (parts.join("|"), source, name.as_deref()),
            ("Re|mem|ber".into(), Source::Dictionary, Some("Moby"))
        );
        assert_eq!(packs.propose("running", Some("en")).0.join("|"), "run|ning");
        assert_eq!(
            packs.propose("absent-minded", Some("en")).0.join("|"),
            "ab|sent-|mind|ed",
            "the word's own hyphen, doubled in the file"
        );
        // Its dictionary is not built: the rule, said as such.
        let (parts, source, name) = packs.propose("banana", Some("fr"));
        assert_eq!(
            (parts.join("|"), source, name),
            ("ba|na|na".into(), Source::Rule, None)
        );
        assert!(!packs.get("fr").unwrap().has_dictionary());
        // No pack for the language: the generic rule; CJK by script.
        assert_eq!(packs.propose("banana", Some("it")).0.join("|"), "ba|na|na");
        assert_eq!(packs.propose("夜空", None).1, Source::Script);
    }

    #[test]
    fn an_oversized_dictionary_is_not_read() {
        let root = tempfile::tempdir().unwrap();
        pack(root.path(), "en", EN, None);
        let file = std::fs::File::create(root.path().join("en").join("dictionary.txt")).unwrap();
        file.set_len(MAX_DICTIONARY + 1).unwrap();
        let packs = scan(&[root.path().to_path_buf()]);
        assert!(!packs.get("en").unwrap().has_dictionary());
    }

    /// The bundled manifests (assets/languages) all load, without dictionaries.
    #[test]
    fn bundled_manifests_load() {
        let packs = scan(&[Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("languages")]);
        assert!(packs.rejected.is_empty(), "{:?}", packs.rejected);
        let mut ids: Vec<&str> = packs.packs.iter().map(|p| p.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["de", "en", "es", "fr", "ru"]);
        let fr = |w| packs.propose(w, Some("fr")).0.join("|");
        assert_eq!(fr("l'amour"), "l'a|mour");
        assert_eq!(fr("oiseau"), "oi|seau");
        let de = |w| packs.propose(w, Some("de")).0.join("|");
        assert_eq!(de("Frauen"), "Frau|en");
        let es = |w| packs.propose(w, Some("es")).0.join("|");
        assert_eq!((es("día"), es("bueno")), ("dí|a".into(), "bue|no".into()));
        let ru = |w| packs.propose(w, Some("ru")).0.join("|");
        assert_eq!(ru("сильно"), "силь|но");
        let en = |w| packs.propose(w, Some("en")).0.join("|");
        assert_eq!(
            (en("didn't"), en("people")),
            ("did|n't".into(), "peo|ple".into())
        );
    }
}
