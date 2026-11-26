//! Resolve a visible character into an explicit, costume-limited edit target.
//! This module defines the contract only; no live rules are sent through it yet.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{EntryOrigin, RosterBacking, RosterKey};
use super::index::RosterIndex;

/// A saved binding must identify both the engine fighter and the allowed costume slots.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditScope {
    pub key: RosterKey,
    pub fighter: String,
    pub slots: Vec<u8>,
}

impl EditScope {
    /// An unknown costume never matches. A fighter name alone is not enough.
    pub fn matches(&self, fighter: &str, slot: Option<u8>) -> bool {
        self.fighter.eq_ignore_ascii_case(fighter)
            && slot.is_some_and(|slot| self.slots.binary_search(&slot).is_ok())
    }

    /// A saved project may not silently inherit a newly resolved or wider scope.
    pub fn verify_saved(&self, saved: Option<&Self>) -> Result<(), ScopeError> {
        match saved {
            None => Err(ScopeError::MissingSavedBinding),
            Some(saved) if saved != self => Err(ScopeError::ChangedSavedBinding),
            Some(_) => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeError {
    UnknownTarget(RosterKey),
    DuplicateTarget(RosterKey),
    MissingFighter(RosterKey),
    InvalidIdentity(RosterKey),
    UnknownBaseSlots(String),
    InvalidSlots(RosterKey),
    ImportedNeedsBinding(RosterKey),
    SlotConflict {
        fighter: String,
        slot: u8,
        first: RosterKey,
        second: RosterKey,
    },
    MissingSavedBinding,
    ChangedSavedBinding,
}

/// Resolve against the current roster and trusted *vanilla* costume lists.
///
/// `base_slots` must come from the original game data, not from a mod-augmented
/// fighter scan. Missing base data and unclassified imported fighters are rejected.
/// The returned scope is not yet used by the runtime or the exporter.
pub fn resolve(
    roster: &RosterIndex,
    selected: &RosterKey,
    base_slots: &BTreeMap<String, Vec<u8>>,
) -> Result<EditScope, ScopeError> {
    let mut entries = roster.entries.iter().filter(|entry| &entry.key == selected);
    let entry = entries
        .next()
        .ok_or_else(|| ScopeError::UnknownTarget(selected.clone()))?;
    if entries.next().is_some() {
        return Err(ScopeError::DuplicateTarget(selected.clone()));
    }
    let fighter = entry
        .fighter
        .as_deref()
        .filter(|fighter| !fighter.is_empty())
        .ok_or_else(|| ScopeError::MissingFighter(selected.clone()))?
        .to_ascii_lowercase();

    // Even a slot clone needs the base set to detect a claim on a vanilla costume.
    let base = base_slots
        .get(&fighter)
        .ok_or_else(|| ScopeError::UnknownBaseSlots(fighter.clone()))?;
    let base = checked_slots(base, &RosterKey::fighter(&fighter))?;

    let slots = match &entry.backing {
        RosterBacking::Fighter => {
            if entry.key != RosterKey::fighter(&fighter) {
                return Err(ScopeError::InvalidIdentity(selected.clone()));
            }
            if entry.origin == EntryOrigin::Imported {
                return Err(ScopeError::ImportedNeedsBinding(selected.clone()));
            }
            base.clone()
        }
        RosterBacking::SlotClone { donor, slot, slots } => {
            if !donor.eq_ignore_ascii_case(&fighter)
                || entry.key != RosterKey::slot(donor, *slot)
            {
                return Err(ScopeError::InvalidIdentity(selected.clone()));
            }
            let mut all = Vec::with_capacity(slots.len() + 1);
            all.push(*slot);
            all.extend(slots);
            checked_slots(&all, selected)?
        }
    };

    // An overlapping owner blocks both sides. Never pick a winner by load order.
    for other in &roster.entries {
        if other.key == *selected {
            continue;
        }
        if other.fighter.as_deref().is_none_or(|name| !name.eq_ignore_ascii_case(&fighter)) {
            continue;
        }
        let other_slots = match &other.backing {
            RosterBacking::Fighter => {
                if other.key != RosterKey::fighter(&fighter) {
                    return Err(ScopeError::InvalidIdentity(other.key.clone()));
                }
                if other.origin == EntryOrigin::Imported {
                    return Err(ScopeError::ImportedNeedsBinding(other.key.clone()));
                }
                base.clone()
            }
            RosterBacking::SlotClone { donor, slot, slots } => {
                if !donor.eq_ignore_ascii_case(&fighter)
                    || other.key != RosterKey::slot(donor, *slot)
                {
                    return Err(ScopeError::InvalidIdentity(other.key.clone()));
                }
                let mut all = Vec::with_capacity(slots.len() + 1);
                all.push(*slot);
                all.extend(slots);
                checked_slots(&all, &other.key)?
            }
        };
        if let Some(slot) = slots.iter().find(|slot| other_slots.binary_search(slot).is_ok()) {
            return Err(ScopeError::SlotConflict {
                fighter: fighter.clone(),
                slot: *slot,
                first: selected.clone(),
                second: other.key.clone(),
            });
        }
    }

    Ok(EditScope {
        key: selected.clone(),
        fighter,
        slots,
    })
}

/// Fighter-wide edits cannot be isolated if another logical owner or an unknown
/// extra costume shares the engine fighter. No guessed/default costume list.
pub fn exclusive_fighter(index: &RosterIndex, fighter: &str, base: &[u8], available: &[u8]) -> bool {
    let mut owners = index.entries.iter().filter(|entry|
        entry.fighter.as_deref().is_some_and(|name| name.eq_ignore_ascii_case(fighter)));
    let Some(entry) = owners.next() else { return false; };
    if owners.next().is_some() || entry.key != RosterKey::fighter(fighter)
        || entry.origin != EntryOrigin::Vanilla || entry.backing != RosterBacking::Fighter
    {
        return false;
    }
    let base_set: BTreeSet<_> = base.iter().copied().collect();
    let available_set: BTreeSet<_> = available.iter().copied().collect();
    !base_set.is_empty() && base_set.len() == base.len()
        && available_set.len() == available.len() && base_set == available_set
}

pub fn exclusive_fighter_in_roots(
    index: &RosterIndex, fighter: &str, roots: &[std::path::PathBuf],
    data_root: Option<&std::path::PathBuf>,
) -> bool {
    let Some(root) = data_root else { return false; };
    let base = crate::data::discover_costume_slots(std::slice::from_ref(root), fighter);
    let available = crate::data::discover_costume_slots(roots, fighter);
    exclusive_fighter(index, fighter, &base, &available)
}

fn checked_slots(slots: &[u8], owner: &RosterKey) -> Result<Vec<u8>, ScopeError> {
    if slots.is_empty() {
        return Err(ScopeError::InvalidSlots(owner.clone()));
    }
    let unique: BTreeSet<u8> = slots.iter().copied().collect();
    if unique.len() != slots.len() {
        return Err(ScopeError::InvalidSlots(owner.clone()));
    }
    Ok(unique.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::{RosterEntry, RosterEntryId};

    fn entry(key: RosterKey, fighter: &str, backing: RosterBacking, origin: EntryOrigin) -> RosterEntry {
        RosterEntry {
            id: RosterEntryId(0),
            key,
            name_id: None,
            backing,
            fighter: Some(fighter.into()),
            display_name: fighter.into(),
            css_order: None,
            providers: Vec::new(),
            origin,
            hidden: false,
            on_roster: false,
        }
    }

    fn base(fighter: &str) -> RosterEntry {
        entry(
            RosterKey::fighter(fighter),
            fighter,
            RosterBacking::Fighter,
            EntryOrigin::Vanilla,
        )
    }

    fn modded(fighter: &str, first: u8, extra: &[u8]) -> RosterEntry {
        entry(
            RosterKey::slot(fighter, first),
            fighter,
            RosterBacking::SlotClone {
                donor: fighter.into(),
                slot: first,
                slots: extra.to_vec(),
            },
            EntryOrigin::Authored,
        )
    }

    fn vanilla() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([("mario".into(), (0..8).collect())])
    }

    #[test]
    fn original_and_modded_character_are_isolated() {
        let roster = RosterIndex {
            entries: vec![base("mario"), modded("mario", 8, &[9, 10, 11])],
            ..Default::default()
        };
        let original = resolve(&roster, &RosterKey::fighter("mario"), &vanilla()).unwrap();
        let custom = resolve(&roster, &RosterKey::slot("mario", 8), &vanilla()).unwrap();
        assert_eq!(original.slots, (0..8).collect::<Vec<_>>());
        assert_eq!(custom.slots, vec![8, 9, 10, 11]);
        assert!(original.matches("mario", Some(7)));
        assert!(!original.matches("mario", Some(8)));
        assert!(custom.matches("mario", Some(11)));
        assert!(!custom.matches("mario", Some(0)));
        assert!(!custom.matches("donkey", Some(8)));
        assert!(!custom.matches("mario", None));
    }

    #[test]
    fn two_modded_characters_on_one_fighter_stay_separate() {
        let roster = RosterIndex {
            entries: vec![
                base("mario"),
                modded("mario", 8, &[9, 10, 11]),
                modded("mario", 12, &[13]),
            ],
            ..Default::default()
        };
        let first = resolve(&roster, &RosterKey::slot("mario", 8), &vanilla()).unwrap();
        let second = resolve(&roster, &RosterKey::slot("mario", 12), &vanilla()).unwrap();
        assert!(!first.matches("mario", Some(12)));
        assert!(!second.matches("mario", Some(8)));
        assert!(second.matches("mario", Some(13)));
    }

    #[test]
    fn overlapping_mods_are_refused() {
        let roster = RosterIndex {
            entries: vec![base("mario"), modded("mario", 8, &[9]), modded("mario", 9, &[10])],
            ..Default::default()
        };
        for key in [RosterKey::slot("mario", 8), RosterKey::slot("mario", 9)] {
            assert!(matches!(
                resolve(&roster, &key, &vanilla()),
                Err(ScopeError::SlotConflict { slot: 9, .. })
            ));
        }
    }

    #[test]
    fn vanilla_costume_claimed_by_mod_is_refused_for_both() {
        let roster = RosterIndex {
            entries: vec![base("mario"), modded("mario", 5, &[8])],
            ..Default::default()
        };
        for key in [RosterKey::fighter("mario"), RosterKey::slot("mario", 5)] {
            assert!(matches!(
                resolve(&roster, &key, &vanilla()),
                Err(ScopeError::SlotConflict { slot: 5, .. })
            ));
        }
    }

    #[test]
    fn missing_or_ambiguous_ownership_fails_closed() {
        let roster = RosterIndex {
            entries: vec![base("mario"), modded("mario", 8, &[])],
            ..Default::default()
        };
        assert!(matches!(
            resolve(&roster, &RosterKey::slot("mario", 8), &BTreeMap::new()),
            Err(ScopeError::UnknownBaseSlots(_))
        ));
        assert!(matches!(
            resolve(&roster, &RosterKey::slot("mario", 20), &vanilla()),
            Err(ScopeError::UnknownTarget(_))
        ));
        let imported = RosterIndex {
            entries: vec![entry(
                RosterKey::fighter("mario"),
                "mario",
                RosterBacking::Fighter,
                EntryOrigin::Imported,
            )],
            ..Default::default()
        };
        assert!(matches!(
            resolve(&imported, &RosterKey::fighter("mario"), &vanilla()),
            Err(ScopeError::ImportedNeedsBinding(_))
        ));
    }

    #[test]
    fn stale_or_legacy_saved_binding_never_expands_automatically() {
        let initial = RosterIndex {
            entries: vec![base("mario"), modded("mario", 8, &[9])],
            ..Default::default()
        };
        let scope = resolve(&initial, &RosterKey::slot("mario", 8), &vanilla()).unwrap();
        let json = serde_json::to_string(&scope).unwrap();
        let saved: EditScope = serde_json::from_str(&json).unwrap();
        assert!(scope.verify_saved(Some(&saved)).is_ok());
        assert_eq!(scope.verify_saved(None), Err(ScopeError::MissingSavedBinding));

        let changed = RosterIndex {
            entries: vec![base("mario"), modded("mario", 16, &[17])],
            ..Default::default()
        };
        let new_scope = resolve(&changed, &RosterKey::slot("mario", 16), &vanilla()).unwrap();
        assert_eq!(
            new_scope.verify_saved(Some(&saved)),
            Err(ScopeError::ChangedSavedBinding)
        );
    }

    #[test]
    fn invalid_slot_sets_are_rejected_not_normalized_silently() {
        assert_eq!(
            checked_slots(&[], &RosterKey::fighter("mario")),
            Err(ScopeError::InvalidSlots(RosterKey::fighter("mario")))
        );
        assert!(matches!(
            checked_slots(&[8, 8], &RosterKey::slot("mario", 8)),
            Err(ScopeError::InvalidSlots(_))
        ));
    }
}

#[cfg(test)]
#[test]
fn scoped_acmd_shared_traits_require_one_verified_engine_owner() {
    use super::{RosterEntry, RosterEntryId};
    let entry = |key, backing, origin| RosterEntry {
        id: RosterEntryId(0), key, name_id: None, backing, fighter: Some("mario".into()),
        display_name: "Mario".into(), css_order: None, providers: Vec::new(),
        origin, hidden: false, on_roster: true,
    };
    let mut index = RosterIndex::default();
    index.entries.push(entry(RosterKey::fighter("mario"), RosterBacking::Fighter, EntryOrigin::Vanilla));
    assert!(exclusive_fighter(&index, "mario", &[0, 1], &[1, 0]));
    assert!(!exclusive_fighter(&index, "mario", &[0, 1], &[0, 1, 8]));
    assert!(!exclusive_fighter(&index, "mario", &[], &[0, 1]));
    index.entries.push(entry(RosterKey::slot("mario", 8), RosterBacking::SlotClone {
        donor: "mario".into(), slot: 8, slots: vec![9]
    }, EntryOrigin::Authored));
    assert!(!exclusive_fighter(&index, "mario", &[0, 1], &[0, 1, 8, 9]));
    assert!(!exclusive_fighter(&index, "mario", &[0, 1], &[0, 1]));
}
