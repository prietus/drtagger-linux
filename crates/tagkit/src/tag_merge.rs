//! Overwrite policy: fields the candidate provides replace the existing
//! ones; identity fields it does not provide are cleared (they described
//! another release); everything else is preserved; locked fields never
//! change.

use crate::tag_set::{TagSet, field};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Removed,
    Changed,
    Unchanged,
}

/// One field before and after a merge, for the preview.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TagChange {
    pub name: String,
    pub old: Vec<String>,
    pub new: Vec<String>,
}

impl TagChange {
    pub fn kind(&self) -> ChangeKind {
        match (self.old.is_empty(), self.new.is_empty()) {
            (true, false) => ChangeKind::Added,
            (false, true) => ChangeKind::Removed,
            _ if self.old == self.new => ChangeKind::Unchanged,
            _ => ChangeKind::Changed,
        }
    }
}

pub struct TagMerge;

impl TagMerge {
    pub fn merge(
        existing: &TagSet,
        proposed: &TagSet,
        locked: &HashSet<String>,
        kept: &HashSet<String>,
    ) -> (TagSet, Vec<TagChange>) {
        let mut result = existing.clone();
        let locked: HashSet<String> = locked.iter().map(|n| n.to_uppercase()).collect();
        for name in proposed.names() {
            if !locked.contains(&name) {
                result.put(&name, proposed.get(&name));
            }
        }
        if !proposed.is_empty() {
            for name in field::IDENTITY_FIELDS {
                if !kept.contains(*name) && proposed.get(name).is_empty() && !locked.contains(*name) {
                    result.remove(name);
                }
            }
        }
        let names = ordered_union(&existing.names(), &result.names());
        let changes = names
            .into_iter()
            .map(|n| TagChange { old: existing.get(&n).to_vec(), new: result.get(&n).to_vec(), name: n })
            .collect();
        (result, changes)
    }
}

fn ordered_union(a: &[String], b: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let union: Vec<&String> = a.iter().chain(b).filter(|n| seen.insert(n.as_str())).collect();
    let mut out: Vec<String> =
        field::CANONICAL_ORDER.iter().filter(|n| seen.contains(**n)).map(|n| n.to_string()).collect();
    let mut rest: Vec<String> =
        union.into_iter().filter(|n| !field::CANONICAL_ORDER.contains(&n.as_str())).cloned().collect();
    rest.sort();
    out.extend(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_replaces_clears_and_locks() {
        let mut existing = TagSet::new();
        existing.set(field::TITLE, Some("so what (old)"));
        existing.set(field::MB_ALBUM_ID, Some("stale-release"));
        existing.set("REPLAYGAIN_TRACK_GAIN", Some("-6.1 dB"));
        existing.set(field::COMPOSER, Some("Somebody"));
        existing.set(field::COMMENT, Some("keep me"));
        let mut proposed = TagSet::new();
        proposed.set(field::TITLE, Some("So What"));
        proposed.set(field::ARTIST, Some("Miles Davis"));
        let locked: HashSet<String> = [field::COMMENT.to_string(), "title".to_string()].into();
        let (result, changes) = TagMerge::merge(&existing, &proposed, &locked, &HashSet::new());
        assert_eq!(result.first(field::TITLE), Some("so what (old)"), "locked field untouched");
        assert_eq!(result.first(field::ARTIST), Some("Miles Davis"));
        assert!(result.get(field::MB_ALBUM_ID).is_empty(), "identity field the candidate lacks is cleared");
        assert_eq!(result.first("REPLAYGAIN_TRACK_GAIN"), Some("-6.1 dB"), "unmapped field preserved");
        assert_eq!(result.first(field::COMPOSER), Some("Somebody"), "credit field preserved when absent");
        let kind = |n: &str| changes.iter().find(|c| c.name == n).unwrap().kind();
        assert_eq!(kind(field::MB_ALBUM_ID), ChangeKind::Removed);
        assert_eq!(kind(field::ARTIST), ChangeKind::Added);
        assert_eq!(kind(field::TITLE), ChangeKind::Unchanged);
    }
}
