//! Runtime identity shared by all scoped live ACMD rule families.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeTarget {
    pub fighter_kind: i32,
    pub costume: i32,
}

impl RuntimeTarget {
    pub const UNKNOWN: Self = Self { fighter_kind: -1, costume: -1 };

    pub fn matches(self, fighter_kind: Option<i32>, slots: Option<&[u8]>) -> bool {
        self.fighter_kind >= 0
            && fighter_kind == Some(self.fighter_kind)
            && u8::try_from(self.costume).ok().is_some_and(|costume| {
                slots.is_some_and(|slots| slots.contains(&costume))
            })
    }
}

/// Scoped snapshots are accepted only with an explicit valid owner on every rule.
/// Legacy envelopes are clear-only, so an older desktop cannot widen a rule.
pub fn acmd_snapshot(value: &serde_json::Value, family: &str) -> Option<serde_json::Value> {
    if let Some(rules) = value.get(format!("{family}_scoped")) {
        let valid = rules.as_array().is_some_and(|rules| {
            rules.iter().all(|rule| {
                rule.get("fighter_kind").and_then(|kind| kind.as_i64())
                    .is_some_and(|kind| (0..=i32::MAX as i64).contains(&kind))
                && rule.get("costume_slots").and_then(|slots| slots.as_array())
                    .is_some_and(|slots| !slots.is_empty() && slots.iter().all(|slot| {
                        slot.as_u64().is_some_and(|slot| slot <= u8::MAX as u64)
                    }))
            })
        });
        return Some(if valid { rules.clone() } else { serde_json::json!([]) });
    }
    value.get(family).map(|_| serde_json::json!([]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_acmd_receiver_never_accepts_legacy_or_invalid_owners() {
        for family in ["hitbox_rules", "spawn_rules", "effect_control_rules"] {
            let valid = serde_json::json!([{"fighter_kind": 0, "costume_slots": [8, 9]}]);
            let scoped = format!("{family}_scoped");
            assert_eq!(acmd_snapshot(&serde_json::json!({(scoped.clone()): valid.clone()}), family), Some(valid.clone()));
            assert_eq!(acmd_snapshot(&serde_json::json!({(family): valid.clone()}), family), Some(serde_json::json!([])));
            for invalid in [
                serde_json::json!([{"fighter_kind": 0}]),
                serde_json::json!([{"fighter_kind": 0, "costume_slots": []}]),
                serde_json::json!([{"fighter_kind": -1, "costume_slots": [8]}]),
                serde_json::json!([{"fighter_kind": 0, "costume_slots": [256]}]),
                serde_json::json!([{"costume_slots": [8]}]),
                serde_json::json!({"fighter_kind": 0, "costume_slots": [8]}),
            ] {
                assert_eq!(acmd_snapshot(&serde_json::json!({(scoped.clone()): invalid}), family), Some(serde_json::json!([])));
            }
            assert_eq!(acmd_snapshot(&serde_json::json!({}), family), None);
        }
    }

    #[test]
    fn scoped_acmd_target_requires_fighter_and_explicit_costume() {
        let target = RuntimeTarget { fighter_kind: 0, costume: 9 };
        assert!(target.matches(Some(0), Some(&[8, 9, 10])));
        assert!(!target.matches(Some(0), Some(&[0, 1, 2, 3, 4, 5, 6, 7])));
        assert!(!target.matches(Some(1), Some(&[8, 9, 10])));
        assert!(!target.matches(None, Some(&[9])));
        assert!(!target.matches(Some(0), None));
        assert!(!target.matches(Some(0), Some(&[])));
        assert!(!RuntimeTarget::UNKNOWN.matches(Some(-1), Some(&[255])));
        assert!(!RuntimeTarget { fighter_kind: 0, costume: 256 }.matches(Some(0), Some(&[0])));
        assert!(RuntimeTarget { fighter_kind: 0, costume: 255 }.matches(Some(0), Some(&[255])));
    }
}
