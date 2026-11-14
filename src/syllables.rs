//! The proposed split of a sung word into syllables (Task 41 S6b), always
//! corrected by hand with hyphens (subtitle_edit::parse_syllables).
//!
//! Order: Chinese, Korean and Japanese by character; otherwise the language
//! pack's dictionary (with its elision, clitic and suffix rules); otherwise the
//! pack's vowel rule; otherwise the generic vowel rule. Every rule is data from
//! the pack (`languages.rs`), never code. The result always spells the word.
use serde::Deserialize;

/// Latin and Cyrillic vowels, for a language without a pack.
const GENERIC_VOWELS: &str = "aeiouyàâäáãåæéèêëíìîïóòôöõœúùûüýÿаеёиоуыэюя";

/// A language's vowel rule and word-shape rules, from its pack.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// The letters that make a syllable's nucleus.
    #[serde(default)]
    pub vowels: String,
    /// The vowel groups sung as one nucleus ("au", "ie", "ue"). Without the
    /// list, a run of vowels is one nucleus; with it, a run is cut into these
    /// groups (longest first) and single vowels: Frau|en, dí|a.
    #[serde(default)]
    pub vowel_groups: Option<Vec<String>>,
    /// Consonant groups that begin a syllable together ("tr", "sch").
    #[serde(default)]
    pub onsets: Vec<String>,
    /// All consonants between two vowels go to the next syllable (Russian),
    /// rather than only the last one or its onset group.
    #[serde(default)]
    pub open_syllables: bool,
    /// Letters that always stay with the syllable before them ("йьъ").
    #[serde(default)]
    pub keep_with_previous: String,
    /// A vowel letter that is a consonant after one of these, before a vowel
    /// ("u" after "q" or "g": que, guerre).
    #[serde(default)]
    pub glides: Vec<Glide>,
    /// Final endings whose "e" is not sung ("give", "chante" said, not sung).
    #[serde(default)]
    pub silent_final: Option<SilentFinal>,
    /// A lone "e" after one consonant inside a word is silent ("some-thing").
    #[serde(default)]
    pub compound_silent_e: bool,
    /// Elided words before an apostrophe that join the first syllable ("l'", "qu'").
    #[serde(default)]
    pub elisions: Vec<String>,
    /// Endings after an apostrophe ("'s", "n't").
    #[serde(default)]
    pub clitics: Vec<Affix>,
    /// Inflections tried when the dictionary lacks the word, in order.
    #[serde(default)]
    pub suffixes: Vec<Affix>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Glide {
    pub letter: char,
    pub after: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SilentFinal {
    /// "e", "es".
    pub endings: Vec<String>,
    /// Not silent after these endings of the stem ("-es" after "s", "ch").
    #[serde(default)]
    pub sung_after: Vec<String>,
    /// "-le" after a consonant is sung ("peo-ple").
    #[serde(default)]
    pub syllabic_le: bool,
}

/// A clitic or suffix: its own syllable, or joined to the last one.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Affix {
    pub text: String,
    /// "own" or "join".
    pub syllable: String,
    /// "own" only when the stem ends with one of these (otherwise "join").
    #[serde(default)]
    pub own_after: Vec<String>,
    /// How to find the dictionary word from the stem, in order:
    /// "plain", "undouble" (runn → run), "add_e" (mak → make), "i_to_y" (carri → carry).
    #[serde(default)]
    pub bases: Vec<String>,
}

/// A language's sources: its rule, and a dictionary lookup (lower-case word to
/// its hyphenated split), when it has one.
pub struct Language<'a> {
    pub rule: Option<&'a Rule>,
    pub dictionary: Option<&'a dyn Fn(&str) -> Option<String>>,
}

/// Where a proposal came from (the field's help names it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Script,
    Dictionary,
    Rule,
}

fn cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF)
}

/// Small kana and the long mark join the kana before them (きょ, ラー).
fn joins_previous_kana(c: char) -> bool {
    "ぁぃぅぇぉゃゅょゎっァィゥェォャュョヮッー".contains(c)
}

/// The proposed syllables of `word` and their source. Punctuation stays on its
/// side, the word's case is kept, and the parts always spell the word.
pub fn propose(word: &str, language: &Language<'_>) -> (Vec<String>, Source) {
    let chars: Vec<char> = word.chars().collect();
    let Some(first) = chars.iter().position(|c| c.is_alphanumeric()) else {
        return (vec![word.to_string()], Source::Rule);
    };
    let last = chars.iter().rposition(|c| c.is_alphanumeric()).unwrap();
    let core: Vec<char> = chars[first..=last].to_vec();
    let (parts, source) = if core.iter().any(|c| cjk(*c)) {
        (by_character(&core), Source::Script)
    } else {
        let lower: String = core.iter().flat_map(|c| c.to_lowercase()).collect();
        // Lower-casing changed the length (rare): keep the word whole.
        if lower.chars().count() != core.len() {
            return (vec![word.to_string()], Source::Rule);
        }
        let (lower_parts, source) = split_lower(&lower, language);
        // Back to the word's own letters, by character counts.
        let mut at = 0;
        let parts = lower_parts
            .iter()
            .map(|p| {
                let n = p.chars().count();
                let s: String = core[at..at + n].iter().collect();
                at += n;
                s
            })
            .collect();
        (parts, source)
    };
    let mut parts: Vec<String> = parts;
    let lead: String = chars[..first].iter().collect();
    let trail: String = chars[last + 1..].iter().collect();
    parts[0].insert_str(0, &lead);
    parts.last_mut().unwrap().push_str(&trail);
    debug_assert_eq!(parts.concat(), word);
    (parts, source)
}

/// One syllable per Han character or Hangul block, one per kana (small kana
/// and ー join the kana before them). Other letters in the run stay together.
fn by_character(core: &[char]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for &c in core {
        let join = match out.last() {
            None => false,
            Some(prev) => {
                let p = prev.chars().last().unwrap();
                joins_previous_kana(c) || (!cjk(c) && !cjk(p))
            }
        };
        if join {
            out.last_mut().unwrap().push(c);
        } else {
            out.push(c.to_string());
        }
    }
    out
}

/// The split of a lower-case word: dictionary (as is, after an elision, a
/// clitic or a suffix), else the rule. Parts spell `lower`.
fn split_lower(lower: &str, language: &Language<'_>) -> (Vec<String>, Source) {
    let rule = language.rule;
    if let Some(parts) = from_dictionary(lower, language) {
        return (parts, Source::Dictionary);
    }
    let apostrophe = |c: char| c == '\'' || c == '’';
    if let Some(rule) = rule {
        // An elided word before an apostrophe joins the first syllable (l'a-mour).
        for e in &rule.elisions {
            let stem = e.trim_end_matches(apostrophe);
            if let Some(rest) = lower.strip_prefix(stem)
                && let Some(a) = rest.chars().next()
                && apostrophe(a)
            {
                let rest = &rest[a.len_utf8()..];
                if rest.is_empty() {
                    continue;
                }
                let (mut parts, source) = split_lower(rest, language);
                parts[0].insert_str(0, &lower[..lower.len() - rest.len()]);
                return (parts, source);
            }
        }
        // A clitic after an apostrophe ("'s", "n't").
        for c in &rule.clitics {
            let ending = c.text.replace('’', "'");
            let norm = lower.replace('’', "'");
            if norm.len() > ending.len() && norm.ends_with(&ending) {
                let cut = lower
                    .char_indices()
                    .nth(lower.chars().count() - ending.chars().count())
                    .map_or(lower.len(), |(i, _)| i);
                let (stem, tail) = lower.split_at(cut);
                let (mut parts, source) = split_lower(stem, language);
                attach(&mut parts, tail, own(c, stem));
                return (parts, source);
            }
        }
        // An inflection of a dictionary word.
        if language.dictionary.is_some() {
            for s in &rule.suffixes {
                let Some(stem) = lower.strip_suffix(s.text.as_str()) else {
                    continue;
                };
                if stem.chars().count() < 2 {
                    continue;
                }
                for base in bases(stem, &s.bases) {
                    if let Some(mut parts) = from_dictionary(&base, language) {
                        // The base's split, mapped back onto the stem's letters; a
                        // doubled letter goes with the suffix (run|ning).
                        let Some(extra) = reshape(&mut parts, stem) else {
                            continue;
                        };
                        let tail = format!("{extra}{}", &lower[stem.len()..]);
                        attach(&mut parts, &tail, own(s, stem));
                        return (parts, Source::Dictionary);
                    }
                }
            }
        }
    }
    (by_rule(lower, rule), Source::Rule)
}

fn own(affix: &Affix, stem: &str) -> bool {
    affix.syllable == "own"
        && (affix.own_after.is_empty()
            || affix.own_after.iter().any(|a| stem.ends_with(a.as_str())))
}

fn attach(parts: &mut Vec<String>, tail: &str, own: bool) {
    if own
        && parts
            .last()
            .is_some_and(|p| p.chars().any(char::is_alphabetic))
        && tail.chars().any(char::is_alphabetic)
    {
        parts.push(tail.to_string());
    } else {
        parts.last_mut().unwrap().push_str(tail);
    }
}

/// Candidate dictionary words for an inflected stem.
fn bases(stem: &str, rules: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for r in rules {
        match r.as_str() {
            "plain" => out.push(stem.to_string()),
            "undouble" => {
                let c: Vec<char> = stem.chars().collect();
                if c.len() >= 3 && c[c.len() - 1] == c[c.len() - 2] {
                    out.push(c[..c.len() - 1].iter().collect());
                }
            }
            "add_e" => out.push(format!("{stem}e")),
            "i_to_y" => {
                if let Some(s) = stem.strip_suffix('i') {
                    out.push(format!("{s}y"));
                }
            }
            _ => {}
        }
    }
    out
}

/// The base word's syllables laid onto the stem's letters. The stem may be one
/// letter shorter (mak for make) or longer (runn for run) at the end; the
/// letters beyond the base are returned, to go with the suffix.
fn reshape(parts: &mut Vec<String>, stem: &str) -> Option<String> {
    let stem_chars: Vec<char> = stem.chars().collect();
    let base_len: usize = parts.iter().map(|p| p.chars().count()).sum();
    let used = base_len.min(stem_chars.len());
    let mut out = Vec::new();
    let mut at = 0;
    for (k, p) in parts.iter().enumerate() {
        let n = if k + 1 == parts.len() {
            used.saturating_sub(at)
        } else {
            p.chars().count()
        };
        if n == 0 || at + n > used {
            return None;
        }
        out.push(stem_chars[at..at + n].iter().collect::<String>());
        at += n;
    }
    *parts = out;
    Some(stem_chars[used..].iter().collect())
}

/// The dictionary's split, looked up with ' for ’ (the datasets use '), laid
/// back onto the word's own letters.
fn from_dictionary(lower: &str, language: &Language<'_>) -> Option<Vec<String>> {
    let key = lower.replace('’', "'");
    let split = (language.dictionary?)(&key)?;
    let parts = crate::subtitle_edit::parse_syllables(&split, &key)?;
    let letters: Vec<char> = lower.chars().collect();
    let mut at = 0;
    Some(
        parts
            .iter()
            .map(|p| {
                let n = p.chars().count();
                let s = letters[at..at + n].iter().collect();
                at += n;
                s
            })
            .collect(),
    )
}

/// The vowel rule: each run of vowels is a nucleus; between two nuclei, one
/// consonant starts the next syllable, of more, the last one (or its onset
/// group), or all of them with open syllables. Letters kept with the previous
/// syllable never start one.
fn by_rule(lower: &str, rule: Option<&Rule>) -> Vec<String> {
    let core: Vec<char> = lower.chars().collect();
    let n = core.len();
    let vowels = rule.map_or(GENERIC_VOWELS, |r| {
        if r.vowels.is_empty() {
            GENERIC_VOWELS
        } else {
            r.vowels.as_str()
        }
    });
    let is_v = |i: usize| -> bool {
        let c = core[i];
        if let Some(r) = rule
            && r.glides.iter().any(|g| {
                g.letter == c
                    && i > 0
                    && g.after.contains(core[i - 1])
                    && i + 1 < n
                    && vowels.contains(core[i + 1])
            })
        {
            return false;
        }
        vowels.contains(c)
    };
    let groups = rule.and_then(|r| r.vowel_groups.as_ref());
    let mut nuclei: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < n {
        if is_v(i) {
            let s = i;
            while i < n && is_v(i) {
                i += 1;
            }
            match groups {
                None => nuclei.push((s, i)),
                // The run cut into listed groups, longest first, else single vowels.
                Some(groups) => {
                    let mut k = s;
                    while k < i {
                        let len = groups
                            .iter()
                            .map(|g| g.chars().collect::<Vec<_>>())
                            .filter(|g| k + g.len() <= i && core[k..k + g.len()] == g[..])
                            .map(|g| g.len())
                            .max()
                            .unwrap_or(1);
                        nuclei.push((k, k + len));
                        k += len;
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    if let Some(sf) = rule.and_then(|r| r.silent_final.as_ref())
        && nuclei.len() > 1
    {
        let (s, e) = *nuclei.last().unwrap();
        let tail: String = core[s..].iter().collect();
        let stem: String = core[..s].iter().collect();
        let sung = sf
            .sung_after
            .iter()
            .any(|a| stem.ends_with(a.as_str()) && tail != "e")
            || (sf.syllabic_le && tail == "e" && s >= 2 && core[s - 1] == 'l' && !is_v(s - 2));
        if e - s == 1
            && core[s] == 'e'
            && sf.endings.iter().any(|x| *x == tail)
            && !is_v(s - 1)
            && !sung
        {
            nuclei.pop();
        }
    }
    if rule.is_some_and(|r| r.compound_silent_e) {
        let mut k = 1;
        while k + 1 < nuclei.len() {
            let (s, e) = nuclei[k];
            if e - s == 1
                && core[s] == 'e'
                && s >= 2
                && !is_v(s - 1)
                && is_v(s - 2)
                && nuclei[k + 1].0 > e
            {
                nuclei.remove(k);
            } else {
                k += 1;
            }
        }
    }
    let keep = rule.map_or("", |r| r.keep_with_previous.as_str());
    let mut cuts = Vec::new();
    for pair in nuclei.windows(2) {
        let (a, b) = (pair[0].1, pair[1].0);
        let k = b - a;
        // Two nuclei side by side (a hiatus): cut between them (Frau|en, dí|a).
        let mut cut = if k == 0 || rule.is_some_and(|r| r.open_syllables) {
            a
        } else if k == 1 {
            a
        } else {
            let cluster: String = core[a..b].iter().collect();
            let onset = rule.and_then(|r| {
                r.onsets
                    .iter()
                    .filter(|o| cluster.ends_with(o.as_str()) && o.chars().count() < k + 1)
                    .max_by_key(|o| o.chars().count())
            });
            match onset {
                Some(o) => b - o.chars().count(),
                None => b - 1,
            }
        };
        // A kept letter (й, ь, ъ) never starts a syllable, nor does the
        // consonant it marks (силь|но).
        while cut < b && (keep.contains(core[cut]) || (cut + 1 < b && keep.contains(core[cut + 1])))
        {
            cut += 1;
        }
        if cut > 0 && cut < n {
            cuts.push(cut);
        }
    }
    if cuts.is_empty() {
        return vec![lower.to_string()];
    }
    let mut out = Vec::new();
    let mut from = 0;
    for c in cuts {
        if c > from {
            out.push(core[from..c].iter().collect());
            from = c;
        }
    }
    out.push(core[from..].iter().collect());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(json: &str) -> Rule {
        serde_json::from_str(json).unwrap()
    }

    fn en() -> Rule {
        rule(
            r#"{
            "vowels": "aeiouy",
            "onsets": ["bl","br","ch","cl","cr","dr","fl","fr","gl","gr","ph","pl","pr","sh","th","tr","wh","wr"],
            "glides": [{"letter": "u", "after": "qg"}],
            "silent_final": {"endings": ["e","es"], "sung_after": ["s","z","x","ch","sh","c","g"], "syllabic_le": true},
            "compound_silent_e": true,
            "clitics": [
                {"text": "n't", "syllable": "own"},
                {"text": "'s", "syllable": "join"}, {"text": "'re", "syllable": "join"}, {"text": "'ll", "syllable": "join"},
                {"text": "'m", "syllable": "join"}, {"text": "'d", "syllable": "join"}, {"text": "'ve", "syllable": "join"}
            ],
            "suffixes": [
                {"text": "ing", "syllable": "own", "bases": ["plain","undouble","add_e"]},
                {"text": "ed", "syllable": "own", "own_after": ["t","d"], "bases": ["plain","undouble","add_e","i_to_y"]},
                {"text": "es", "syllable": "own", "own_after": ["s","x","z","ch","sh","c","g"], "bases": ["plain","add_e","i_to_y"]},
                {"text": "s", "syllable": "join", "bases": ["plain"]}
            ]
        }"#,
        )
    }

    fn fr() -> Rule {
        rule(
            r#"{
            "vowels": "aeiouyàâäéèêëîïôöùûüÿœæ",
            "vowel_groups": ["eau","au","ai","ei","oi","ou","eu","œu","ui","ie","ue"],
            "onsets": ["bl","br","ch","cl","cr","dr","fl","fr","gl","gn","gr","ph","pl","pr","tr","vr"],
            "glides": [{"letter": "u", "after": "qg"}],
            "elisions": ["l'","d'","j'","m'","n'","s'","t'","c'","qu'","jusqu'","lorsqu'","puisqu'"]
        }"#,
        )
    }

    fn dict(entries: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |w| {
            entries
                .iter()
                .find(|(k, _)| *k == w)
                .map(|(_, v)| v.to_string())
        }
    }

    fn split(word: &str, lang: &Language<'_>) -> String {
        let (parts, _) = propose(word, lang);
        assert_eq!(parts.concat(), word, "the parts spell the word");
        parts.join("|")
    }

    #[test]
    fn chinese_korean_and_japanese_split_by_character() {
        let none = Language {
            rule: None,
            dictionary: None,
        };
        assert_eq!(split("夜空に光る", &none), "夜|空|に|光|る");
        assert_eq!(split("안녕하세요", &none), "안|녕|하|세|요");
        assert_eq!(split("きょう", &none), "きょ|う");
        assert_eq!(split("ラーメン、", &none), "ラー|メ|ン、");
        assert_eq!(propose("夜", &none).1, Source::Script);
    }

    #[test]
    fn the_dictionary_comes_first_then_inflections_then_the_rule() {
        let r = en();
        let d = dict(&[
            ("remember", "re-mem-ber"),
            ("camera", "cam-er-a"),
            ("disappear", "dis-ap-pear"),
            ("box", "box"),
            ("wish", "wish"),
            ("run", "run"),
            ("make", "make"),
            ("carry", "car-ry"),
            ("did", "did"),
            ("i", "i"),
        ]);
        let en = Language {
            rule: Some(&r),
            dictionary: Some(&d),
        };
        assert_eq!(split("Remember,", &en), "Re|mem|ber,");
        assert_eq!(
            propose("camera", &en),
            (
                vec!["cam".into(), "er".into(), "a".into()],
                Source::Dictionary
            )
        );
        assert_eq!(split("disappearing", &en), "dis|ap|pear|ing");
        assert_eq!(split("disappears", &en), "dis|ap|pears");
        assert_eq!(split("boxes", &en), "box|es");
        assert_eq!(split("wishes", &en), "wish|es");
        assert_eq!(split("running", &en), "run|ning");
        assert_eq!(split("making", &en), "mak|ing");
        assert_eq!(split("carried", &en), "car|ried");
        assert_eq!(split("didn't", &en), "did|n't");
        assert_eq!(split("I'm", &en), "I'm");
        // Not in the dictionary: the rule.
        assert_eq!(
            propose("lovely", &en),
            (vec!["love".into(), "ly".into()], Source::Rule)
        );
        assert_eq!(split("people", &en), "peo|ple");
        assert_eq!(split("give", &en), "give");
    }

    #[test]
    fn french_elisions_join_the_first_syllable() {
        let r = fr();
        let d = dict(&[
            ("amour", "a-mour"),
            ("il", "il"),
            ("ai", "ai"),
            ("été", "é-té"),
            ("peut-être", "peut-être"),
        ]);
        let fr = Language {
            rule: Some(&r),
            dictionary: Some(&d),
        };
        assert_eq!(split("l'amour", &fr), "l'a|mour");
        assert_eq!(split("qu'il", &fr), "qu'il");
        assert_eq!(split("J'ai", &fr), "J'ai");
        assert_eq!(split("été", &fr), "é|té");
        assert_eq!(split("peut-être", &fr), "peut-être");
        // The typographic apostrophe finds the dictionary's ' entry.
        let d2 = dict(&[("aujourd'hui", "au-jourd'-hui")]);
        let fr2 = Language {
            rule: Some(&r),
            dictionary: Some(&d2),
        };
        assert_eq!(
            propose("aujourd’hui", &fr2),
            (
                vec!["au".into(), "jourd’".into(), "hui".into()],
                Source::Dictionary
            )
        );
        // Outside the dictionary: the rule, with its onsets.
        assert_eq!(split("chanson", &fr), "chan|son");
        assert_eq!(split("oiseau", &fr), "oi|seau");
        assert_eq!(split("soleil", &fr), "so|leil");
    }

    #[test]
    fn german_spanish_and_russian_follow_their_rule() {
        let de = rule(
            r#"{"vowels": "aeiouäöüy", "vowel_groups": ["ei","ai","au","eu","äu","ie","aa","ee","oo"], "onsets": ["sch","ch","pf","st","sp","bl","br","dr","fl","fr","gl","gr","kl","kr","pl","pr","tr"]}"#,
        );
        let es = rule(
            r#"{"vowels": "aeiouáéíóúüy", "vowel_groups": ["ia","ie","io","ua","ue","uo","ai","ei","oi","au","eu","ou","ui","iu","ay","ey","oy","ué","ió","ié","uí","iá","uá","ái","éi","ói","áu","éu"], "onsets": ["bl","br","ch","cl","cr","dr","fl","fr","gl","gr","ll","pl","pr","rr","tr"], "glides": [{"letter": "u", "after": "qg"}]}"#,
        );
        let ru = rule(
            r#"{"vowels": "аеёиоуыэюя", "open_syllables": true, "keep_with_previous": "йьъ"}"#,
        );
        let lang = |r| Language {
            rule: Some(r),
            dictionary: None,
        };
        assert_eq!(split("Tränen", &lang(&de)), "Trä|nen");
        assert_eq!(split("Liebe", &lang(&de)), "Lie|be");
        assert_eq!(split("Frauen", &lang(&de)), "Frau|en");
        assert_eq!(split("heute", &lang(&de)), "heu|te");
        assert_eq!(split("bueno", &lang(&es)), "bue|no");
        assert_eq!(split("cielo", &lang(&es)), "cie|lo");
        assert_eq!(split("día", &lang(&es)), "dí|a");
        assert_eq!(split("corazón", &lang(&es)), "co|ra|zón");
        assert_eq!(split("calle", &lang(&es)), "ca|lle");
        assert_eq!(split("молоко", &lang(&ru)), "мо|ло|ко");
        assert_eq!(split("сестра", &lang(&ru)), "се|стра");
        assert_eq!(split("мойка", &lang(&ru)), "мой|ка");
        assert_eq!(split("сильно", &lang(&ru)), "силь|но");
        // No pack at all: the generic vowels.
        assert_eq!(
            split(
                "banana",
                &Language {
                    rule: None,
                    dictionary: None
                }
            ),
            "ba|na|na"
        );
    }

    #[test]
    fn a_word_without_letters_or_with_a_changing_case_stays_whole() {
        let none = Language {
            rule: None,
            dictionary: None,
        };
        assert_eq!(split("...", &none), "...");
        assert_eq!(split("2024", &none), "2024");
    }
}
