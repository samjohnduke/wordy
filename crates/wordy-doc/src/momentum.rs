//! Momentum tools: writing goals, daily sessions and streaks, the task list,
//! scene versions, and the placeholder scan. All of it lives in the project
//! doc so it syncs with the manuscript.

use anyhow::{anyhow, Result};
use chrono::{Days, NaiveDate};
use loro::{ContainerTrait as _, Frontiers, LoroDoc, LoroMap, LoroText, LoroValue, TreeID, ValueOrContainer};

use crate::node::NodeKind;
use crate::project::{now_ms, Project};
use crate::schema;

// ---- helpers --------------------------------------------------------------

fn get_str(map: &LoroMap, key: &str) -> Option<String> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    }
}
fn get_i64(map: &LoroMap, key: &str) -> Option<i64> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::I64(i))) => Some(i),
        Some(ValueOrContainer::Value(LoroValue::Double(d))) => Some(d as i64),
        _ => None,
    }
}
fn get_bool(map: &LoroMap, key: &str) -> Option<bool> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::Bool(b))) => Some(b),
        _ => None,
    }
}
fn get_bytes(map: &LoroMap, key: &str) -> Option<Vec<u8>> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::Binary(b))) => Some(b.to_vec()),
        _ => None,
    }
}
fn child_map(map: &LoroMap, key: &str) -> Option<LoroMap> {
    match map.get(key) {
        Some(ValueOrContainer::Container(c)) => c.into_map().ok(),
        _ => None,
    }
}
/// Every child map of `map`, keyed.
fn child_maps(map: &LoroMap) -> Vec<(String, LoroMap)> {
    let keys: Vec<String> = map.keys().map(|k| k.to_string()).collect();
    keys.into_iter()
        .filter_map(|k| child_map(map, &k).map(|m| (k, m)))
        .collect()
}

pub fn date_str(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}
pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

// ---- settings -------------------------------------------------------------

/// Project-wide writing goals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Goals {
    /// Words per day; `None` means no daily goal.
    pub daily: Option<i64>,
    /// Target manuscript length.
    pub manuscript: Option<i64>,
    /// Date the manuscript should reach its goal.
    pub deadline: Option<NaiveDate>,
}

impl Goals {
    /// Words per day needed from `today` to hit `manuscript` by `deadline`.
    /// `None` without both a manuscript goal and a future deadline.
    pub fn required_pace(&self, manuscript_words: i64, today: NaiveDate) -> Option<i64> {
        let (goal, deadline) = (self.manuscript?, self.deadline?);
        let remaining = (goal - manuscript_words).max(0);
        let days = (deadline - today).num_days();
        if days < 0 {
            return None;
        }
        let days = days.max(0) + 1;
        Some((remaining + days - 1) / days)
    }
}

mod keys {
    pub const DAILY_GOAL: &str = "daily_goal";
    pub const MANUSCRIPT_GOAL: &str = "manuscript_goal";
    pub const DEADLINE: &str = "deadline";

    pub const WORDS_START: &str = "words_start";
    pub const WORDS_END: &str = "words_end";
    pub const SECONDS: &str = "seconds";

    pub const TEXT: &str = "text";
    pub const DONE: &str = "done";
    pub const NODE: &str = "node";
    pub const CREATED: &str = "created";

    pub const LABEL: &str = "label";
    pub const FRONTIERS: &str = "frontiers";
}

/// One day's writing session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub date: NaiveDate,
    /// Manuscript word count when the day started.
    pub words_start: i64,
    /// Manuscript word count at the last save that day.
    pub words_end: i64,
    /// Seconds of active editing.
    pub seconds: i64,
}

impl Session {
    pub fn words(&self) -> i64 {
        self.words_end - self.words_start
    }
}

/// A to-do item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub text: String,
    pub done: bool,
    pub created: i64,
    /// Node the task is about, if any.
    pub node: Option<TreeID>,
}

/// A bookmarked state of one node's body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub id: String,
    pub node: TreeID,
    pub label: String,
    pub created: i64,
    frontiers: Vec<u8>,
}

impl Version {
    pub fn frontiers(&self) -> Result<Frontiers> {
        Frontiers::decode(&self.frontiers).map_err(|e| anyhow!("decode frontiers: {e}"))
    }
}

/// A placeholder left in the prose: `TK`, `[TODO ...]`, `[?]`, `XXX`, `TBD`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    pub node: TreeID,
    pub title: String,
    /// Code point offset into the node's plain text.
    pub offset: usize,
    /// The placeholder text itself.
    pub marker: String,
    /// Surrounding text for display.
    pub snippet: String,
}

/// Scan `text` for placeholders. Offsets are code points.
pub fn scan_placeholders(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '[' {
            // Bracketed note: [TODO ...], [?], [fix this], up to 80 chars, same paragraph.
            if let Some(end) = (i + 1..chars.len().min(i + 82)).find(|&j| chars[j] == ']' || chars[j] == '\n') {
                if chars[end] == ']' {
                    let inner: String = chars[i + 1..end].iter().collect();
                    let t = inner.trim();
                    let upper = t.to_uppercase();
                    if t == "?"
                        || t == "??"
                        || upper.starts_with("TODO")
                        || upper.starts_with("TK")
                        || upper.starts_with("FIX")
                        || upper.starts_with("CHECK")
                        || upper.starts_with("NOTE")
                        || upper.starts_with("TBD")
                    {
                        out.push((i, chars[i..=end].iter().collect()));
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        if c.is_alphabetic() && (i == 0 || !is_word(chars[i - 1])) {
            let mut j = i;
            while j < chars.len() && is_word(chars[j]) {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            if matches!(word.as_str(), "TK" | "TKTK" | "XXX" | "TBD" | "TODO") {
                out.push((i, word));
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Project {
    // ---- settings ---------------------------------------------------------

    pub fn goals(&self) -> Goals {
        let s = self.settings_map();
        Goals {
            daily: get_i64(&s, keys::DAILY_GOAL).filter(|g| *g > 0),
            manuscript: get_i64(&s, keys::MANUSCRIPT_GOAL).filter(|g| *g > 0),
            deadline: get_str(&s, keys::DEADLINE).and_then(|d| parse_date(&d)),
        }
    }

    pub fn set_goals(&self, goals: &Goals) -> Result<()> {
        let s = self.settings_map();
        match goals.daily {
            Some(g) => s.insert(keys::DAILY_GOAL, g)?,
            None => s.delete(keys::DAILY_GOAL)?,
        }
        match goals.manuscript {
            Some(g) => s.insert(keys::MANUSCRIPT_GOAL, g)?,
            None => s.delete(keys::MANUSCRIPT_GOAL)?,
        }
        match goals.deadline {
            Some(d) => s.insert(keys::DEADLINE, date_str(d))?,
            None => s.delete(keys::DEADLINE)?,
        }
        self.commit_meta();
        Ok(())
    }

    /// Words in the manuscript that count toward goals (scenes included in compile).
    pub fn manuscript_word_count(&self) -> i64 {
        self.manuscript_scenes()
            .into_iter()
            .filter_map(|id| self.node(id).ok())
            .filter(|n| n.include_in_compile())
            .map(|n| n.word_count() as i64)
            .sum()
    }

    // ---- sessions ---------------------------------------------------------

    fn read_session(date: NaiveDate, m: &LoroMap) -> Session {
        let start = get_i64(m, keys::WORDS_START).unwrap_or(0);
        Session {
            date,
            words_start: start,
            words_end: get_i64(m, keys::WORDS_END).unwrap_or(start),
            seconds: get_i64(m, keys::SECONDS).unwrap_or(0),
        }
    }

    /// Every recorded day, oldest first.
    pub fn sessions(&self) -> Vec<Session> {
        let mut out: Vec<Session> = child_maps(&self.sessions_map())
            .into_iter()
            .filter_map(|(k, m)| parse_date(&k).map(|d| Self::read_session(d, &m)))
            .collect();
        out.sort_by_key(|s| s.date);
        out
    }

    pub fn session(&self, date: NaiveDate) -> Option<Session> {
        child_map(&self.sessions_map(), &date_str(date)).map(|m| Self::read_session(date, &m))
    }

    /// Make sure `date` has a session, starting from `words` if it is new.
    pub fn begin_session(&self, date: NaiveDate, words: i64) -> Result<Session> {
        let map = self.sessions_map();
        let key = date_str(date);
        if let Some(m) = child_map(&map, &key) {
            return Ok(Self::read_session(date, &m));
        }
        let m = map.insert_container(&key, LoroMap::new())?;
        m.insert(keys::WORDS_START, words)?;
        m.insert(keys::WORDS_END, words)?;
        m.insert(keys::SECONDS, 0i64)?;
        self.commit_meta();
        Ok(Self::read_session(date, &m))
    }

    /// Record the current word count and add active seconds to `date`'s session.
    pub fn update_session(&self, date: NaiveDate, words: i64, add_seconds: i64) -> Result<Session> {
        self.begin_session(date, words)?;
        let m = child_map(&self.sessions_map(), &date_str(date)).ok_or_else(|| anyhow!("session vanished"))?;
        m.insert(keys::WORDS_END, words)?;
        if add_seconds > 0 {
            let secs = get_i64(&m, keys::SECONDS).unwrap_or(0) + add_seconds;
            m.insert(keys::SECONDS, secs)?;
        }
        self.commit_meta();
        Ok(Self::read_session(date, &m))
    }

    /// Words written on `date` (0 when nothing recorded).
    pub fn words_on(&self, date: NaiveDate) -> i64 {
        self.session(date).map(|s| s.words()).unwrap_or(0)
    }

    /// Consecutive days with net words written, ending today or yesterday.
    pub fn streak(&self, today: NaiveDate) -> u32 {
        let wrote = |d: NaiveDate| self.words_on(d) > 0;
        let mut day = if wrote(today) {
            today
        } else {
            match today.checked_sub_days(Days::new(1)) {
                Some(y) if wrote(y) => y,
                _ => return 0,
            }
        };
        let mut n = 0;
        while wrote(day) {
            n += 1;
            match day.checked_sub_days(Days::new(1)) {
                Some(d) => day = d,
                None => break,
            }
        }
        n
    }

    // ---- tasks ------------------------------------------------------------

    fn read_task(id: String, m: &LoroMap) -> Task {
        Task {
            id,
            text: get_str(m, keys::TEXT).unwrap_or_default(),
            done: get_bool(m, keys::DONE).unwrap_or(false),
            created: get_i64(m, keys::CREATED).unwrap_or(0),
            node: get_str(m, keys::NODE).and_then(|s| TreeID::try_from(s.as_str()).ok()),
        }
    }

    /// Every task, oldest first.
    pub fn task_list(&self) -> Vec<Task> {
        let mut out: Vec<Task> = child_maps(&self.tasks())
            .into_iter()
            .map(|(k, m)| Self::read_task(k, &m))
            .collect();
        out.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
        out
    }

    pub fn add_task(&self, text: &str, node: Option<TreeID>) -> Result<Task> {
        let id = ulid::Ulid::new().to_string();
        let m = self.tasks().insert_container(&id, LoroMap::new())?;
        m.insert(keys::TEXT, text)?;
        m.insert(keys::DONE, false)?;
        m.insert(keys::CREATED, now_ms())?;
        if let Some(n) = node {
            m.insert(keys::NODE, n.to_string())?;
        }
        self.commit_meta();
        Ok(Self::read_task(id, &m))
    }

    pub fn set_task_done(&self, id: &str, done: bool) -> Result<()> {
        let m = child_map(&self.tasks(), id).ok_or_else(|| anyhow!("no task {id}"))?;
        m.insert(keys::DONE, done)?;
        self.commit_meta();
        Ok(())
    }

    pub fn set_task_text(&self, id: &str, text: &str) -> Result<()> {
        let m = child_map(&self.tasks(), id).ok_or_else(|| anyhow!("no task {id}"))?;
        m.insert(keys::TEXT, text)?;
        self.commit_meta();
        Ok(())
    }

    pub fn remove_task(&self, id: &str) -> Result<()> {
        self.tasks().delete(id)?;
        self.commit_meta();
        Ok(())
    }

    // ---- versions ---------------------------------------------------------

    fn read_version(id: String, m: &LoroMap) -> Option<Version> {
        let node = TreeID::try_from(get_str(m, keys::NODE)?.as_str()).ok()?;
        Some(Version {
            id,
            node,
            label: get_str(m, keys::LABEL).unwrap_or_default(),
            created: get_i64(m, keys::CREATED).unwrap_or(0),
            frontiers: get_bytes(m, keys::FRONTIERS)?,
        })
    }

    /// Bookmark the current state of `node`'s body under `label`.
    pub fn save_version(&self, node: TreeID, label: &str) -> Result<Version> {
        // Make sure everything pending is part of the recorded frontier.
        self.commit_meta();
        let frontiers = self.doc.state_frontiers().encode();
        let id = ulid::Ulid::new().to_string();
        let m = self.versions_map().insert_container(&id, LoroMap::new())?;
        m.insert(keys::NODE, node.to_string())?;
        m.insert(keys::LABEL, label)?;
        m.insert(keys::CREATED, now_ms())?;
        m.insert(keys::FRONTIERS, LoroValue::from(frontiers))?;
        self.commit_meta();
        Self::read_version(id, &m).ok_or_else(|| anyhow!("version not readable"))
    }

    /// Versions of `node`, newest first.
    pub fn versions_for(&self, node: TreeID) -> Vec<Version> {
        let mut out: Vec<Version> = child_maps(&self.versions_map())
            .into_iter()
            .filter_map(|(k, m)| Self::read_version(k, &m))
            .filter(|v| v.node == node)
            .collect();
        out.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
        out
    }

    pub fn version(&self, id: &str) -> Option<Version> {
        child_map(&self.versions_map(), id).and_then(|m| Self::read_version(id.to_string(), &m))
    }

    pub fn remove_version(&self, id: &str) -> Result<()> {
        self.versions_map().delete(id)?;
        self.commit_meta();
        Ok(())
    }

    /// A detached copy of the doc as it was when `v` was saved, with the
    /// node's body text in it. Read-only in practice: edits never come back.
    pub fn version_doc(&self, v: &Version) -> Result<(LoroDoc, LoroText)> {
        let frontiers = v.frontiers()?;
        let fork = self
            .doc
            .fork_at(&frontiers)
            .map_err(|e| anyhow!("fork at version: {e}"))?;
        schema::configure_text_styles(&fork);
        let body = self.node(v.node)?.body()?;
        let text = fork.get_text(body.id());
        Ok((fork, text))
    }

    /// Plain text of the body as it was in version `v`.
    pub fn version_text(&self, v: &Version) -> Result<String> {
        let (_, text) = self.version_doc(v)?;
        Ok(text.to_string())
    }

    /// Replace the node's current body with the version's content, as one
    /// edit committed under `origin` (so the editor can undo it).
    pub fn restore_version(&self, v: &Version, origin: &str) -> Result<()> {
        let (_fork, old) = self.version_doc(v)?;
        let delta = old.to_delta();
        let body = self.node(v.node)?.body()?;
        let len = body.len_unicode();
        if len > 0 {
            body.delete(0, len)?;
        }
        body.apply_delta(&delta)?;
        self.doc.commit_with(loro::CommitOptions::default().origin(origin));
        Ok(())
    }

    // ---- placeholders -----------------------------------------------------

    /// Every placeholder in every live node with a body, in tree order.
    pub fn placeholders(&self) -> Vec<Placeholder> {
        let mut out = Vec::new();
        for id in self.all_nodes() {
            let Ok(n) = self.node(id) else { continue };
            if !n.kind().has_body() || n.kind() == NodeKind::Entity && n.body_if_exists().is_none() {
                continue;
            }
            let text = n.plain_text();
            if text.is_empty() {
                continue;
            }
            let chars: Vec<char> = text.chars().collect();
            for (offset, marker) in scan_placeholders(&text) {
                let end = offset + marker.chars().count();
                let lo = chars[..offset]
                    .iter()
                    .rposition(|c| *c == '\n')
                    .map(|p| p + 1)
                    .unwrap_or(0);
                let hi = chars[end..]
                    .iter()
                    .position(|c| *c == '\n')
                    .map(|p| end + p)
                    .unwrap_or(chars.len());
                let lo = lo.max(offset.saturating_sub(40));
                let hi = hi.min(end + 60);
                let mut snippet: String = chars[lo..hi].iter().collect();
                if lo > 0 && chars[lo - 1] != '\n' {
                    snippet.insert(0, '…');
                }
                if hi < chars.len() && chars[hi] != '\n' {
                    snippet.push('…');
                }
                out.push(Placeholder {
                    node: id,
                    title: n.title(),
                    offset,
                    marker,
                    snippet,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::Space;

    fn d(s: &str) -> NaiveDate {
        parse_date(s).unwrap()
    }

    #[test]
    fn goals_round_trip_and_pace() {
        let p = Project::new_in_memory("T").unwrap();
        assert_eq!(p.goals(), Goals::default());
        let g = Goals {
            daily: Some(500),
            manuscript: Some(80_000),
            deadline: Some(d("2026-12-31")),
        };
        p.set_goals(&g).unwrap();
        assert_eq!(p.goals(), g);
        // 10 days inclusive, 1000 words to go → 100/day.
        assert_eq!(g.required_pace(79_000, d("2026-12-22")), Some(100));
        assert_eq!(g.required_pace(80_000, d("2026-12-22")), Some(0));
        assert_eq!(g.required_pace(0, d("2027-01-01")), None);
        p.set_goals(&Goals::default()).unwrap();
        assert_eq!(p.goals(), Goals::default());
    }

    #[test]
    fn sessions_and_streak() {
        let p = Project::new_in_memory("T").unwrap();
        p.begin_session(d("2026-03-01"), 100).unwrap();
        p.update_session(d("2026-03-01"), 350, 600).unwrap();
        p.begin_session(d("2026-03-02"), 350).unwrap();
        p.update_session(d("2026-03-02"), 400, 60).unwrap();
        p.update_session(d("2026-03-03"), 400, 0).unwrap(); // opened, wrote nothing
        let s = p.sessions();
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].words(), 250);
        assert_eq!(s[0].seconds, 600);
        assert_eq!(s[1].words(), 50);
        assert_eq!(p.streak(d("2026-03-02")), 2);
        assert_eq!(p.streak(d("2026-03-03")), 2); // yesterday counts
        assert_eq!(p.streak(d("2026-03-04")), 0);
        // begin_session is idempotent within a day.
        assert_eq!(p.begin_session(d("2026-03-01"), 999).unwrap().words_start, 100);
    }

    #[test]
    fn tasks_crud() {
        let p = Project::new_in_memory("T").unwrap();
        let sc = p.create_node(p.root(Space::Manuscript), NodeKind::Scene, "S").unwrap();
        let a = p.add_task("Fix the ending", Some(sc)).unwrap();
        let b = p.add_task("Name the dog", None).unwrap();
        let list = p.task_list();
        assert_eq!(list.len(), 2);
        let find = |l: &[Task], id: &str| l.iter().find(|t| t.id == id).cloned().unwrap();
        assert_eq!(find(&list, &a.id).node, Some(sc));
        assert_eq!(find(&list, &b.id).node, None);
        p.set_task_done(&a.id, true).unwrap();
        p.set_task_text(&b.id, "Name the cat").unwrap();
        let list = p.task_list();
        assert!(find(&list, &a.id).done);
        assert_eq!(find(&list, &b.id).text, "Name the cat");
        p.remove_task(&a.id).unwrap();
        assert_eq!(p.task_list().len(), 1);
    }

    #[test]
    fn versions_view_and_restore() {
        let p = Project::new_in_memory("T").unwrap();
        let sc = p.create_node(p.root(Space::Manuscript), NodeKind::Scene, "S").unwrap();
        let body = p.node(sc).unwrap().body().unwrap();
        body.delete(0, body.len_unicode()).unwrap();
        body.insert(0, "First draft.\n").unwrap();
        body.mark(0..5, "bold", true).unwrap();
        p.doc.commit();
        let v1 = p.save_version(sc, "draft 1").unwrap();
        body.delete(0, body.len_unicode()).unwrap();
        body.insert(0, "Second draft, longer.\n").unwrap();
        p.doc.commit();
        assert_eq!(p.node(sc).unwrap().plain_text(), "Second draft, longer.");
        assert_eq!(p.version_text(&v1).unwrap(), "First draft.\n");
        let vs = p.versions_for(sc);
        assert_eq!(vs.len(), 1);
        assert_eq!(vs[0].label, "draft 1");
        p.restore_version(&v1, "body:test").unwrap();
        assert_eq!(p.node(sc).unwrap().plain_text(), "First draft.");
        // Formatting came back too.
        let delta = p.node(sc).unwrap().body().unwrap().to_delta();
        let bold = delta.iter().any(|d| match d {
            loro::TextDelta::Insert {
                attributes: Some(a), ..
            } => a.contains_key("bold"),
            _ => false,
        });
        assert!(bold);
        // Still the live doc.
        assert!(!p.doc.is_detached());
        p.remove_version(&v1.id).unwrap();
        assert!(p.versions_for(sc).is_empty());
    }

    #[test]
    fn versions_survive_save_and_reopen() {
        let dir = std::env::temp_dir().join(format!("wordy-ver-{}", ulid::Ulid::new()));
        let p = Project::create(&dir, "T").unwrap();
        let sc = p.create_node(p.root(Space::Manuscript), NodeKind::Scene, "S").unwrap();
        let body = p.node(sc).unwrap().body().unwrap();
        body.delete(0, body.len_unicode()).unwrap();
        body.insert(0, "one\n").unwrap();
        p.doc.commit();
        let v = p.save_version(sc, "v").unwrap();
        p.node(sc).unwrap().body().unwrap().insert(0, "zero ").unwrap();
        p.save().unwrap();
        let q = Project::open(&dir).unwrap();
        assert_eq!(q.node(sc).unwrap().plain_text(), "zero one");
        let v2 = q.version(&v.id).unwrap();
        assert_eq!(q.version_text(&v2).unwrap(), "one\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn placeholder_scan() {
        let hits =
            scan_placeholders("She said TK and left. [TODO: name] Nothing here [maybe]. TKTK? [?] fine. Stalk TK.");
        let markers: Vec<&str> = hits.iter().map(|(_, m)| m.as_str()).collect();
        assert_eq!(markers, vec!["TK", "[TODO: name]", "TKTK", "[?]", "TK"]);
        assert_eq!(hits[0].0, 9);
    }

    #[test]
    fn placeholders_across_project() {
        let p = Project::new_in_memory("T").unwrap();
        let sc = p
            .create_node(p.root(Space::Manuscript), NodeKind::Scene, "Opening")
            .unwrap();
        p.node(sc)
            .unwrap()
            .body()
            .unwrap()
            .insert(0, "Line one.\nHe grabbed the TK and ran.\n")
            .unwrap();
        let note = p.create_node(p.root(Space::Notes), NodeKind::Note, "Ideas").unwrap();
        p.node(note)
            .unwrap()
            .body()
            .unwrap()
            .insert(0, "[TODO think about this]\n")
            .unwrap();
        let ph = p.placeholders();
        assert_eq!(ph.len(), 2);
        assert_eq!(ph[0].title, "Opening");
        assert_eq!(ph[0].marker, "TK");
        assert_eq!(ph[0].offset, 25);
        assert_eq!(ph[0].snippet, "He grabbed the TK and ran.");
        assert_eq!(ph[1].marker, "[TODO think about this]");
    }
}
