//! Spellcheck: the bundled en_US Hunspell dictionary (via `spellbook`),
//! loaded once on a background thread, plus the writer's custom words.
//!
//! The dictionary is process-wide; custom words, session ignores, and the
//! check cache live in a gpui global so every editor shares them.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use gpui_kit::{App, Global};
use spellbook::Dictionary;
use unicode_segmentation::UnicodeSegmentation;

const AFF: &str = include_str!("../assets/dict/en_US.aff");
const DIC: &str = include_str!("../assets/dict/en_US.dic");

static DICT: OnceLock<Arc<Dictionary>> = OnceLock::new();
static LOADING: AtomicBool = AtomicBool::new(false);

/// Start building the dictionary on a background thread (idempotent).
pub fn ensure_loading() {
    if DICT.get().is_some() || LOADING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("wordy-spell".into())
        .spawn(|| {
            let t = std::time::Instant::now();
            match Dictionary::new(AFF, DIC) {
                Ok(d) => {
                    let _ = DICT.set(Arc::new(d));
                    tracing::info!("dictionary loaded in {:?}", t.elapsed());
                }
                Err(e) => tracing::error!("dictionary failed to parse: {e}"),
            }
        })
        .expect("spawn spell thread");
}

/// The dictionary, once loaded.
pub fn dictionary() -> Option<Arc<Dictionary>> {
    DICT.get().cloned()
}

/// Shared spelling state: custom words, session ignores, memoized checks.
#[derive(Default)]
pub struct SpellState {
    custom: HashSet<String>,
    ignored: HashSet<String>,
    cache: HashMap<String, bool>,
}

impl Global for SpellState {}

impl SpellState {
    pub fn custom_words(cx: &App) -> Vec<String> {
        let mut v: Vec<String> = cx
            .try_global::<SpellState>()
            .map(|s| s.custom.iter().cloned().collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Replace the custom word list (loaded from the project's dictionary file).
    pub fn set_custom_words(cx: &mut App, words: impl IntoIterator<Item = String>) {
        let s = cx.default_global::<SpellState>();
        s.custom = words
            .into_iter()
            .map(|w| normalize(&w))
            .filter(|w| !w.is_empty())
            .collect();
        s.cache.clear();
    }

    pub fn add_custom_word(cx: &mut App, word: &str) {
        let s = cx.default_global::<SpellState>();
        s.custom.insert(normalize(word));
        s.cache.clear();
    }

    pub fn ignore(cx: &mut App, word: &str) {
        let s = cx.default_global::<SpellState>();
        s.ignored.insert(normalize(word));
        s.cache.clear();
    }

    /// Whether `word` is spelled correctly. `None` while the dictionary is
    /// still loading.
    pub fn check(cx: &mut App, word: &str) -> Option<bool> {
        let dict = dictionary()?;
        let s = cx.default_global::<SpellState>();
        let key = normalize(word);
        if let Some(ok) = s.cache.get(&key) {
            return Some(*ok);
        }
        let ok = s.custom.contains(&key)
            || s.ignored.contains(&key)
            || s.custom.contains(&key.to_lowercase())
            || dict.check(&key)
            || is_dictionary_word_with_suffix(&dict, &key);
        if s.cache.len() > 50_000 {
            s.cache.clear();
        }
        s.cache.insert(key, ok);
        Some(ok)
    }

    /// Suggestions for a misspelled word, best first.
    pub fn suggest(word: &str, limit: usize) -> Vec<String> {
        let Some(dict) = dictionary() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        dict.suggest(&normalize(word), &mut out);
        out.truncate(limit);
        out
    }
}

/// Curly apostrophes to straight so the dictionary recognizes contractions.
pub fn normalize(word: &str) -> String {
    word.trim_matches(|c: char| c == '\'' || c == '\u{2019}')
        .replace('\u{2019}', "'")
}

/// "Sam's", "readers'" and similar possessive forms of known words.
fn is_dictionary_word_with_suffix(dict: &Dictionary, word: &str) -> bool {
    if let Some(stem) = word.strip_suffix("'s") {
        return dict.check(stem);
    }
    if let Some(stem) = word.strip_suffix('\'') {
        return dict.check(stem);
    }
    false
}

/// Words worth checking in `text`, as byte ranges. Skips numbers, acronyms,
/// and anything with digits or without a letter.
pub fn checkable_words(text: &str) -> Vec<(Range<usize>, &str)> {
    text.split_word_bound_indices()
        .filter_map(|(i, w)| {
            let trimmed = w.trim_matches(|c: char| c == '\'' || c == '\u{2019}');
            if trimmed.is_empty() {
                return None;
            }
            let start = i + (w.len() - w.trim_start_matches(|c: char| c == '\'' || c == '\u{2019}').len());
            let end = start + trimmed.len();
            let mut letters = 0;
            let mut lower = false;
            for c in trimmed.chars() {
                if c.is_numeric() {
                    return None;
                }
                if c.is_alphabetic() {
                    letters += 1;
                    if c.is_lowercase() {
                        lower = true;
                    }
                } else if c != '\'' && c != '\u{2019}' && c != '-' {
                    return None;
                }
            }
            if letters < 2 || !lower {
                // Single letters and ALLCAPS acronyms are not checked.
                return None;
            }
            Some((start..end, trimmed))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_skips_noise() {
        let words: Vec<&str> = checkable_words("Don’t read 42 NASA pages, 'quoted' word-ish x.")
            .into_iter()
            .map(|(_, w)| w)
            .collect();
        assert_eq!(words, vec!["Don’t", "read", "pages", "quoted", "word", "ish"]);
        let r = &checkable_words("say 'hi'")[1].0;
        assert_eq!(&"say 'hi'"[r.clone()], "hi");
    }

    #[test]
    fn dictionary_checks_and_suggests() {
        let d = Dictionary::new(AFF, DIC).unwrap();
        assert!(d.check("house"));
        assert!(d.check("Houses"));
        assert!(d.check(&normalize("don’t")));
        assert!(!d.check("hosue"));
        assert!(is_dictionary_word_with_suffix(&d, "lamp's"));
        let mut s = Vec::new();
        d.suggest("hosue", &mut s);
        assert!(s.iter().any(|w| w == "house"), "{s:?}");
    }
}
