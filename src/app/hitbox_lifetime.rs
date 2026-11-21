//! Live lifetime rules for ordinary ATTACK hitboxes.
use super::VisionaryApp;
use crate::data::Hitbox;
use crate::game_link::{HitboxRuleWire, InjectRuleWire, LuaArgWire, CAT_ATTACK_LIFETIME};

impl VisionaryApp {
    /// Keep a box through its original clear_all, or clear it earlier, by ID.
    /// Reused IDs cannot overlap the scheduled clear: the game has only one slot per ID.
    fn attack_lifetime_rules(
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
        let clear_frame = Self::script_to_motion_frame(edited.active_end.saturating_add(1));
        rules.push(HitboxRuleWire {
            motion,
            category: CAT_ATTACK_LIFETIME,
            hitbox_id: Some(edited.id as u64),
            suppress: false,
            frame_start: None,
            frame_end: Some(clear_frame + 0.5),
            overrides: None,
            inject: Some(InjectRuleWire {
                frame: clear_frame,
                args: vec![LuaArgWire::Int(edited.id as i64)],
                command: Some("ATTACK_CLEAR_ID".into()),
            }),
            func: None,
        });
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
}
