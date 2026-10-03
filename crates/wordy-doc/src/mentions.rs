//! Entity mention detection: an Aho-Corasick automaton over every entity's
//! title and aliases, with whole-word boundary checks after each match.
//!
//! Shared by the editor (auto-link decorations) and the index (auto backlinks).

use std::collections::HashMap;
use std::ops::Range;

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use loro::TreeID;

/// One entity's names, as fed to the matcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityNames {
    pub id: TreeID,
    pub names: Vec<String>,
}

/// A detected mention: a byte range into the scanned text and every entity
/// whose name matches there. More than one candidate means the mention is
/// ambiguous and needs pinning to an explicit link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub range: Range<usize>,
    pub candidates: Vec<TreeID>,
}

impl Mention {
    pub fn is_ambiguous(&self) -> bool {
        self.candidates.len() > 1
    }
}

#[derive(Debug, Clone)]
pub struct Matcher {
    ac: Option<AhoCorasick>,
    /// Pattern index → entities sharing that (case-folded) name.
    owners: Vec<Vec<TreeID>>,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Matcher {
    pub fn empty() -> Self {
        Self {
            ac: None,
            owners: Vec::new(),
        }
    }

    pub fn new(entries: &[EntityNames]) -> Self {
        let mut by_name: HashMap<String, Vec<TreeID>> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for e in entries {
            for n in &e.names {
                let n = n.trim();
                if n.len() < 2 {
                    continue;
                }
                let key = n.to_lowercase();
                let owners = by_name.entry(key.clone()).or_insert_with(|| {
                    order.push(key.clone());
                    Vec::new()
                });
                if !owners.contains(&e.id) {
                    owners.push(e.id);
                }
            }
        }
        if order.is_empty() {
            return Self::empty();
        }
        let owners: Vec<Vec<TreeID>> = order.iter().map(|k| by_name[k].clone()).collect();
        let ac = AhoCorasickBuilder::new()
            .ascii_case_insensitive(true)
            .match_kind(MatchKind::LeftmostLongest)
            .build(&order)
            .ok();
        Self { ac, owners }
    }

    pub fn is_empty(&self) -> bool {
        self.ac.is_none()
    }

    /// Scan `text` for whole-word mentions. Ranges are byte offsets into `text`.
    pub fn scan(&self, text: &str) -> Vec<Mention> {
        let Some(ac) = &self.ac else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for m in ac.find_iter(text) {
            let (start, end) = (m.start(), m.end());
            let before_ok = text[..start]
                .chars()
                .next_back()
                .map(|c| !is_word_char(c))
                .unwrap_or(true);
            let after_ok = text[end..].chars().next().map(|c| !is_word_char(c)).unwrap_or(true);
            if !before_ok || !after_ok {
                continue;
            }
            // Non-ASCII letters are matched byte-exact; make sure the hit is the
            // same word under full case folding too.
            let pat = &text[start..end];
            let owners = &self.owners[m.pattern().as_usize()];
            if pat.to_lowercase() != pat.to_lowercase() {
                continue;
            }
            out.push(Mention {
                range: start..end,
                candidates: owners.clone(),
            });
        }
        out
    }

    /// Scan, dropping candidates equal to `exclude` (an entity's own sheet
    /// should not mention itself).
    pub fn scan_excluding(&self, text: &str, exclude: Option<TreeID>) -> Vec<Mention> {
        let mut v = self.scan(text);
        if let Some(x) = exclude {
            for m in &mut v {
                m.candidates.retain(|c| *c != x);
            }
            v.retain(|m| !m.candidates.is_empty());
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::TreeID;

    fn tid(n: u32) -> TreeID {
        TreeID {
            peer: 1,
            counter: n as i32,
        }
    }

    #[test]
    fn whole_word_case_insensitive() {
        let m = Matcher::new(&[
            EntityNames {
                id: tid(1),
                names: vec!["Anna".into(), "the Captain".into()],
            },
            EntityNames {
                id: tid(2),
                names: vec!["Annabel".into()],
            },
        ]);
        let hits = m.scan("anna met Annabel, the captain. Hannah too.");
        let ranges: Vec<_> = hits.iter().map(|h| (h.range.clone(), h.candidates.clone())).collect();
        assert_eq!(
            ranges,
            vec![(0..4, vec![tid(1)]), (9..16, vec![tid(2)]), (18..29, vec![tid(1)])]
        );
    }

    #[test]
    fn ambiguous_and_exclusion() {
        let m = Matcher::new(&[
            EntityNames {
                id: tid(1),
                names: vec!["Sam".into()],
            },
            EntityNames {
                id: tid(2),
                names: vec!["Sam".into(), "Samantha".into()],
            },
        ]);
        let hits = m.scan("Sam and Samantha");
        assert!(hits[0].is_ambiguous());
        assert_eq!(hits[1].candidates, vec![tid(2)]);
        let hits = m.scan_excluding("Sam and Samantha", Some(tid(2)));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].candidates, vec![tid(1)]);
    }

    #[test]
    fn possessives_match_the_name() {
        let m = Matcher::new(&[EntityNames {
            id: tid(1),
            names: vec!["Sam".into()],
        }]);
        let hits = m.scan("Sam's lamp. Sam\u{2019}s boat. Samson's.");
        let ranges: Vec<_> = hits.iter().map(|h| h.range.clone()).collect();
        assert_eq!(ranges, vec![0..3, 12..15]);
    }

    #[test]
    fn empty_matcher() {
        assert!(Matcher::new(&[]).scan("anything").is_empty());
    }
}
