//! Incremental delta playbook for checkpoints (#541, EFF-4 — ACE principle).
//!
//! ACE (Agentic Context Engineering, 2510.04618) showed that monolithic
//! checkpoint rewrites cause *brevity bias* (repeated summarization loses
//! detail) and *context collapse* (an observed 18k -> 122 token implosion,
//! −29% accuracy). The cure: contexts grow as structured, itemized delta
//! entries with stable IDs that are never rewritten — only appended, bumped
//! (dedup-confirm), voted on, and locally evicted.
//!
//! sobre wires this into `ctx_compress`: every checkpoint distills the
//! session into playbook deltas instead of re-summarizing prior summaries.
//! Renders are ordered by stable ID, so unchanged prefixes stay prefix-cache
//! friendly across checkpoints.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// An approach that worked (decisions, successful tactics).
    Strategy,
    /// Something that bit us (gotchas, bounces, failed edits).
    Pitfall,
    /// A stable observation about the codebase or domain.
    Fact,
    /// A file worth remembering, with why.
    FileRef,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Strategy => "Strategy",
            EntryKind::Pitfall => "Pitfall",
            EntryKind::Fact => "Fact",
            EntryKind::FileRef => "FileRef",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybookEntry {
    /// Stable, monotonically assigned — never reused, never rewritten.
    pub id: u32,
    pub kind: EntryKind,
    pub content: String,
    pub created_turn: u32,
    pub last_confirmed_turn: u32,
    pub helpful_votes: u32,
    pub harmful_votes: u32,
}

impl PlaybookEntry {
    fn salience(&self) -> i64 {
        i64::from(self.helpful_votes) - i64::from(self.harmful_votes)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Playbook {
    pub entries: Vec<PlaybookEntry>,
    pub next_id: u32,
}

impl Playbook {
    /// Vote on an entry by stable ID (agent feedback via sobre_session).
    #[cfg(test)]
    pub fn vote(&mut self, id: u32, helpful: bool) -> bool {
        match self.entries.iter_mut().find(|e| e.id == id) {
            Some(e) => {
                if helpful {
                    e.helpful_votes += 1;
                } else {
                    e.harmful_votes += 1;
                }
                true
            }
            None => false,
        }
    }

    /// Render ordered by stable ID (prefix-cache friendly: old entries keep
    /// their byte positions). When `top_k` is exceeded, the lowest-salience
    /// entries are elided — never rewritten.
    pub fn render(&self, top_k: usize) -> String {
        if self.entries.is_empty() {
            return String::new();
        }
        let mut selected: Vec<&PlaybookEntry> = self.entries.iter().collect();
        let elided = if selected.len() > top_k {
            selected.sort_by_key(|e| std::cmp::Reverse((e.salience(), e.last_confirmed_turn)));
            let n = selected.len() - top_k;
            selected.truncate(top_k);
            n
        } else {
            0
        };
        selected.sort_by_key(|e| e.id);

        let mut out = String::from("PLAYBOOK (delta log, stable IDs):\n");
        for e in selected {
            let votes = if e.helpful_votes > 0 || e.harmful_votes > 0 {
                format!(" (+{}/-{})", e.helpful_votes, e.harmful_votes)
            } else {
                String::new()
            };
            out.push_str(&format!(
                "[P{}] {}: {}{votes}\n",
                e.id,
                e.kind.as_str(),
                e.content
            ));
        }
        if elided > 0 {
            out.push_str(&format!(
                "… {elided} low-salience entries elided (recall via sobre_session)\n"
            ));
        }
        out
    }

    /// Total content volume (entries × chars) — used by the brevity-bias
    /// regression test: repeated checkpoints must never shrink this.
    #[cfg(test)]
    pub fn information_volume(&self) -> usize {
        self.entries.iter().map(|e| e.content.len()).sum()
    }
}
