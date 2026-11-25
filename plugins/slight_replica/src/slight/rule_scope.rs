//! Runtime identity shared by all scoped live ACMD rule families.

// The effect manager's kind table, legacy pins and EFF carrier have no verified
// per-character resource identity. Keep their saved data, but do not activate
// them in the scoped editing build. ACMD rules and ordinary exported files are separate.
pub const LEGACY_GLOBAL_EFFECTS_ENABLED: bool = false;
/// Explicit next-match EFF preview. The path is derived, never supplied by the wire.
/// The editor must verify that the whole fighter resource has one logical owner.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct ScopedEffPreview {
    pub fighter: String,
    pub fighter_kind: i32,
    pub costume_slots: Vec<u8>,
    pub token: String,
}

impl ScopedEffPreview {
    pub fn valid(&self) -> bool {
        self.fighter_kind >= 0
            && !self.fighter.is_empty() && self.fighter.len() <= 32
            && self.fighter.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
            // The preview is path-wide, not costume-scoped at the resource loader.
            // Only the full standard vanilla costume group is eligible for this experiment.
            && self.costume_slots.as_slice() == &[0, 1, 2, 3, 4, 5, 6, 7]
            && self.token.len() == 32
            && self.token.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    }

    pub fn arc_path(&self) -> String {
        format!("effect/fighter/{0}/ef_{0}.eff", self.fighter)
    }

    pub fn sd_rel_path(&self) -> String {
        format!("effect_viewer/scoped_live_eff/ef_{}_{}.eff", self.fighter, self.token)
    }
    /// Never publish a private clone at the old path-wide preview location.
    pub fn private_sd_rel_path(&self) -> String {
        format!("effect_viewer/private_live_eff/ef_{}_{}.eff", self.fighter, self.token)
    }
}

#[cfg(test)]
#[test]
fn scoped_acmd_eff_preview_requires_canonical_identity() {
    let p = ScopedEffPreview {
        fighter: "mario".into(), fighter_kind: 0, costume_slots: vec![0, 1, 2, 3, 4, 5, 6, 7],
        token: "0123456789abcdef0123456789abcdef".into(),
    };
    assert!(p.valid());
    assert_eq!(p.arc_path(), "effect/fighter/mario/ef_mario.eff");
    assert_eq!(p.sd_rel_path(), "effect_viewer/scoped_live_eff/ef_mario_0123456789abcdef0123456789abcdef.eff");
    for fighter in ["", "../mario", "Mario", "assist/bomberman", "mario\\x"] {
        assert!(!ScopedEffPreview { fighter: fighter.into(), ..p.clone() }.valid());
    }
    for costume_slots in [vec![], vec![0, 1], vec![0, 0], vec![1, 0], (0..=8).collect()] {
        assert!(!ScopedEffPreview { costume_slots, ..p.clone() }.valid());
    }
    assert!(!ScopedEffPreview { token: "../old.eff".into(), ..p.clone() }.valid());
    assert!(!ScopedEffPreview { fighter_kind: -1, ..p }.valid());
}

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

/// A redirect belongs to a fighter and an explicit set of costumes.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct EffectAlias {
    pub from: u64,
    pub to: u64,
    pub fighter_kind: i32,
    pub costume_slots: Vec<u8>,
}

impl EffectAlias {
    pub fn valid(&self) -> bool {
        self.fighter_kind >= 0 && !self.costume_slots.is_empty()
            && self.from != self.to
            && self.from > 0 && self.from <= 0xff_ffff_ffff
            && self.to > 0 && self.to <= 0xff_ffff_ffff
    }
}

/// Conflicting destinations fail closed rather than taking the first owner's edit.
pub fn effect_alias_for(aliases: &[EffectAlias], from: u64, target: RuntimeTarget) -> Option<u64> {
    let mut matches = aliases.iter().filter(|alias| {
        alias.valid() && alias.from == from
            && target.matches(Some(alias.fighter_kind), Some(&alias.costume_slots))
    });
    let first = matches.next()?.to;
    matches.all(|alias| alias.to == first).then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_acmd_receiver_never_accepts_legacy_or_invalid_owners() {
        for family in ["hitbox_rules", "spawn_rules", "effect_control_rules", "effect_aliases"] {
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

#[cfg(test)]
#[test]
fn scoped_acmd_aliases_require_exact_owner_and_refuse_conflicts() {
    let alias = |fighter_kind, costume_slots, to| EffectAlias { from: 123, to, fighter_kind, costume_slots };
    let mut rules = vec![alias(1, vec![0, 1], 111), alias(1, vec![8, 9], 222), alias(2, vec![8], 333)];
    let lookup = |rules: &[EffectAlias], fighter_kind, costume| effect_alias_for(rules, 123, RuntimeTarget { fighter_kind, costume });
    assert_eq!(lookup(&rules, 1, 0), Some(111));
    assert_eq!(lookup(&rules, 1, 8), Some(222));
    assert_eq!(lookup(&rules, 2, 8), Some(333));
    assert_eq!(lookup(&rules, 3, 8), None);
    assert_eq!(lookup(&rules, 1, -1), None);
    assert_eq!(lookup(&rules, 1, 264), None);
    rules.push(alias(1, vec![8], 444));
    assert_eq!(lookup(&rules, 1, 8), None);
    assert_eq!(lookup(&rules, 1, 9), Some(222));
    assert_eq!(lookup(&[alias(1, vec![], 111)], 1, 0), None);
}
