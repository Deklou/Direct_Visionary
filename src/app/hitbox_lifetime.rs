//! Live lifetime rules for ordinary ATTACK hitboxes.
use super::VisionaryApp;
use crate::data::Hitbox;
use crate::game_link::{HitboxRuleWire, InjectRuleWire, LuaArgWire, CAT_ATTACK_LIFETIME};
use std::collections::BTreeSet;


/// Duration rules for moved ATTACK spawns, indexed by edited hitbox.
/// Unsafe same-ID retimings are ignored instead of half-applied.
pub(super) struct RetimedAttackPlan {
    pub(super) rules_by_index: Vec<Vec<HitboxRuleWire>>,
    pub(super) skipped_ids: BTreeSet<u32>,
    pub(super) error: Option<&'static str>,
}

impl VisionaryApp {
    fn attack_clear_id_rule(motion: u64, id: u32, end: u32) -> HitboxRuleWire {
        let clear_frame = Self::script_to_motion_frame(end.saturating_add(1));
        HitboxRuleWire {
            motion,
            category: CAT_ATTACK_LIFETIME,
            hitbox_id: Some(id as u64),
            suppress: false,
            frame_start: None,
            frame_end: Some(clear_frame + 0.5),
            overrides: None,
            inject: Some(InjectRuleWire {
                frame: clear_frame,
                args: vec![LuaArgWire::Int(id as i64)],
                command: Some("ATTACK_CLEAR_ID".into()),
            }),
            func: None,
        }
    }

    /// When the start frame changes, the original spawn is suppressed and the
    /// edited hitbox injected. Reuse its lifetime only for a unique same-ID pair.
    pub(super) fn plan_retimed_attack_lifetimes(
        motion: u64,
        pristine: &[Hitbox],
        pristine_used: &[bool],
        current: &[Hitbox],
        current_used: &[bool],
    ) -> RetimedAttackPlan {
        let mut plan = RetimedAttackPlan {
            rules_by_index: vec![Vec::new(); current.len()],
            skipped_ids: BTreeSet::new(),
            error: None,
        };
        for (ci, edited) in current.iter().enumerate() {
            if current_used[ci] || edited.category != 0 {
                continue;
            }
            let sources: Vec<&Hitbox> = pristine
                .iter()
                .enumerate()
                .filter(|(i, original)| {
                    !pristine_used[*i] && original.category == 0 && original.id == edited.id
                })
                .map(|(_, original)| original)
                .collect();
            if sources.is_empty() {
                // A newly added hitbox has no original clear to preserve.
                continue;
            }
            let destinations = current
                .iter()
                .enumerate()
                .filter(|(i, other)| {
                    !current_used[*i] && other.category == 0 && other.id == edited.id
                })
                .count();
            if sources.len() != 1 || destinations != 1 {
                plan.skipped_ids.insert(edited.id);
                plan.error = Some("Cannot retime ATTACK: multiple unmatched hitboxes share this ID.");
                continue;
            }
            // An ID clear on end+1 must not erase a neighbouring same-ID phase.
            // Nor may an earlier phase's clear erase the retimed spawn.
            let overlap = current.iter().any(|other| {
                !std::ptr::eq(other, edited)
                    && other.category == 0
                    && other.id == edited.id
                    && other.active_start <= edited.active_end.saturating_add(1)
                    && other.active_end.saturating_add(1) >= edited.active_start
            });
            let lifetime = if overlap {
                Err("Cannot retime ATTACK: overlaps another phase with the same ID.")
            } else if edited.id > 7
                || edited.active_end < edited.active_start
                || edited.active_end >= 9999
            {
                Err("Live ATTACK retiming requires ID 0-7 and a finite end frame.")
            } else {
                Self::attack_lifetime_rules(motion, sources[0], edited, current)
            };
            match lifetime {
                Ok(mut rules) => {
                    // A moved spawn needs its own clear even if its end is unchanged.
                    if rules.is_empty() {
                        rules.push(Self::attack_clear_id_rule(motion, edited.id, edited.active_end));
                    }
                    plan.rules_by_index[ci] = rules;
                }
                Err(error) => {
                    plan.skipped_ids.insert(edited.id);
                    plan.error = Some(error);
                }
            }
        }
        plan
    }

    /// Keep a box through its original clear_all, or clear it earlier, by ID.
    /// Reused IDs cannot overlap the scheduled clear: the game has only one slot per ID.
    pub(super) fn attack_lifetime_rules(
        motion: u64,
        original: &Hitbox,
        edited: &Hitbox,
        siblings: &[Hitbox],
    ) -> Result<Vec<HitboxRuleWire>, &'static str> {
        if original.category != 0 || edited.category != 0 || original.active_end == edited.active_end {
            return Ok(Vec::new());
        }
        if edited.id > 7 || edited.active_end < edited.active_start || edited.active_end >= 9999 {
            return Err("Live ATTACK duration requires ID 0–7 and a finite, valid end frame.");
        }
        if siblings.iter().any(|other| {
            other.category == 0
                && other.id == edited.id
                && other.active_start > edited.active_start
                && other.active_start <= edited.active_end.saturating_add(1)
        }) {
            return Err("Live ATTACK duration overlaps the next hitbox with the same ID.");
        }

        let mut rules = Vec::new();
        if edited.active_end > original.active_end {
            // A real clear_all may run more than once. Preserve only this ID until its new end.
            let begin = Self::script_to_motion_frame(original.active_end.saturating_add(1)) - 0.5;
            let end = Self::script_to_motion_frame(edited.active_end) + 0.5;
            rules.push(HitboxRuleWire {
                motion,
                category: CAT_ATTACK_LIFETIME,
                hitbox_id: Some(edited.id as u64),
                suppress: false,
                frame_start: Some(begin),
                frame_end: Some(end),
                overrides: None,
                inject: None,
                func: None,
            });
        }
        // Even for an extension: an independent ID-scoped clear ends it at the requested frame.
        rules.push(Self::attack_clear_id_rule(motion, edited.id, edited.active_end));
        Ok(rules)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_at(id: u32, start: u32, end: u32) -> Hitbox {
        Hitbox {
            category: 0,
            id,
            active_start: start,
            active_end: end,
            ..Default::default()
        }
    }

    #[test]
    fn shortening_injects_only_an_id_scoped_clear() {
        let original = box_at(0, 3, 9);
        let edited = box_at(0, 3, 5);
        let rules = VisionaryApp::attack_lifetime_rules(123, &original, &edited, &[edited.clone()])
            .unwrap();
        assert_eq!(rules.len(), 1);
        let injection = rules[0].inject.as_ref().unwrap();
        assert_eq!(injection.command.as_deref(), Some("ATTACK_CLEAR_ID"));
        assert_eq!(injection.frame, VisionaryApp::script_to_motion_frame(6));
        assert_eq!(rules[0].hitbox_id, Some(0));
    }

    #[test]
    fn extension_preserves_id_through_original_clear_then_ends_it() {
        let original = box_at(0, 3, 5);
        let edited = box_at(0, 3, 12);
        let rules = VisionaryApp::attack_lifetime_rules(123, &original, &edited, &[edited.clone()])
            .unwrap();
        assert_eq!(rules.len(), 2);
        assert!(rules[0].inject.is_none());
        assert_eq!(rules[0].hitbox_id, Some(0));
        assert!(rules[0].frame_start.unwrap() <= VisionaryApp::script_to_motion_frame(6));
        assert!(rules[0].frame_end.unwrap() >= VisionaryApp::script_to_motion_frame(12));
        assert_eq!(rules[1].inject.as_ref().unwrap().frame,
            VisionaryApp::script_to_motion_frame(13));
    }

    #[test]
    fn reused_id_cannot_be_cleared_during_its_next_hit() {
        let original = box_at(0, 3, 5);
        let edited = box_at(0, 3, 12);
        let next = box_at(0, 10, 15);
        assert!(VisionaryApp::attack_lifetime_rules(123, &original, &edited, &[edited.clone(), next]).is_err());
    }

    #[test]
    fn unchanged_duration_adds_no_rules() {
        let hitbox = box_at(0, 3, 5);
        assert!(VisionaryApp::attack_lifetime_rules(123, &hitbox, &hitbox, &[hitbox.clone()])
            .unwrap().is_empty());
    }
    #[test]
    fn retimed_start_and_extended_end_keep_the_lifetime_rules() {
        let original = box_at(0, 10, 12);
        let edited = box_at(0, 14, 18);
        let plan = VisionaryApp::plan_retimed_attack_lifetimes(
            123, &[original], &[false], &[edited], &[false],
        );
        assert!(plan.skipped_ids.is_empty());
        assert!(plan.error.is_none());
        assert_eq!(plan.rules_by_index[0].len(), 2);
        assert!(plan.rules_by_index[0][0].inject.is_none());
        assert_eq!(
            plan.rules_by_index[0][1].inject.as_ref().unwrap().frame,
            VisionaryApp::script_to_motion_frame(19)
        );
    }

    #[test]
    fn retimed_start_with_same_end_gets_a_clear_rule() {
        let original = box_at(0, 10, 18);
        let edited = box_at(0, 13, 18);
        let plan = VisionaryApp::plan_retimed_attack_lifetimes(
            123, &[original], &[false], &[edited], &[false],
        );
        assert!(plan.skipped_ids.is_empty());
        assert_eq!(plan.rules_by_index[0].len(), 1);
        assert_eq!(
            plan.rules_by_index[0][0].inject.as_ref().unwrap().command.as_deref(),
            Some("ATTACK_CLEAR_ID")
        );
    }

    #[test]
    fn ambiguous_retimed_id_is_blocked_without_partial_rules() {
        let pristine = [box_at(0, 4, 6), box_at(0, 11, 13)];
        let edited = [box_at(0, 5, 7), box_at(0, 12, 14)];
        let plan = VisionaryApp::plan_retimed_attack_lifetimes(
            123, &pristine, &[false, false], &edited, &[false, false],
        );
        assert!(plan.skipped_ids.contains(&0));
        assert!(plan.rules_by_index.iter().all(Vec::is_empty));
        assert!(plan.error.is_some());
    }

    #[test]
    fn retiming_into_next_same_id_phase_is_blocked() {
        let pristine = [box_at(0, 4, 6), box_at(0, 10, 12)];
        let edited = [box_at(0, 5, 10), box_at(0, 10, 12)];
        let plan = VisionaryApp::plan_retimed_attack_lifetimes(
            123, &pristine, &[false, true], &edited, &[false, true],
        );
        assert!(plan.skipped_ids.contains(&0));
        assert!(plan.rules_by_index.iter().all(Vec::is_empty));
    }


}
