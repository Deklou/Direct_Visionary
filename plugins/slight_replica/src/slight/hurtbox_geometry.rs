//! Session-only fighter hurtbox overrides for Smash 13.0.5.
//! Network threads stage verified costume-scoped rules; native calls run on fighter frames.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use smash::app::{self, lua_bind::HitModule, BattleObjectModuleAccessor};
use smash::phx::{Hash40, Vector3f};

use crate::slight::rule_scope::RuntimeTarget;

#[derive(Clone, Debug, Deserialize)]
pub struct GeometryRule {
    pub fighter_kind: i32,
    /// Missing scope is never a wildcard (older senders must clear, not apply).
    #[serde(default)]
    pub costume_slots: Vec<u8>,
    pub index: u32,
    pub bone: u64,
    pub endpoint1: [f32; 3],
    pub endpoint2: [f32; 3],
    pub radius: f32,
    pub status: i32,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub apply_geometry: bool,
}

impl GeometryRule {
    fn valid(&self) -> bool {
        (0..=127).contains(&self.fighter_kind)
            && !self.costume_slots.is_empty()
            && self.index < 128
            && self.bone > 0 && self.bone <= 0xffffffffff
            && self.radius.is_finite()
            && self.radius > 0.0
            && self.radius <= 50.0
            && (0..=3).contains(&self.status)
            && self.endpoint1.iter().chain(self.endpoint2.iter())
                .all(|v| v.is_finite() && v.abs() <= 200.0)
    }

    fn matches_target(&self, target: RuntimeTarget) -> bool {
        target.matches(Some(self.fighter_kind), Some(&self.costume_slots))
    }
}

/// No two logical owners may alter the same native capsule on one costume.
fn valid_snapshot(rules: &[GeometryRule]) -> bool {
    if rules.len() > 128 {
        return false;
    }
    let mut seen = HashSet::new();
    rules.iter().all(|rule| {
        rule.valid() && rule.costume_slots.iter()
            .all(|slot| seen.insert((rule.fighter_kind, *slot, rule.index)))
    })
}

#[derive(Clone, Debug)]
struct AppliedState {
    target: RuntimeTarget,
    revision: u64,
    /// Saved default joint statuses, used to undo OFF when a rule disappears.
    disabled_joints: Vec<(u64, i32)>,
    changed_geometry: bool,
}

static RULES: LazyLock<RwLock<Vec<GeometryRule>>> = LazyLock::new(|| RwLock::new(Vec::new()));
static REVISION: AtomicU64 = AtomicU64::new(0);
static APPLIED: LazyLock<Mutex<HashMap<u32, AppliedState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_rules(rules: Vec<GeometryRule>) {
    let valid = valid_snapshot(&rules);
    *RULES.write() = if valid { rules } else { Vec::new() };
    REVISION.fetch_add(1, Ordering::Release);
    if !valid {
        crate::slight::diag::note("hurtbox geometry rules rejected; cleared (scope or value conflict)");
    }
}

/// Drop per-instance latches on a new match; keep the session's authored rules.
pub fn forget_match() {
    APPLIED.lock().clear();
}

/// Apply after receiving a new full snapshot, and reassert disabled joints every frame.
/// Old rules are restored only on the same engine fighter, never on a recycled ID
/// belonging to a different fighter.
pub unsafe fn tick(boma: *mut BattleObjectModuleAccessor) {
    if boma.is_null() || !crate::slight::ver::is_13_0_5() {
        return;
    }
    let Some(target) = crate::slight::hitbox_viewer::rule_target(boma) else {
        return;
    };
    let boid = (*boma).battle_object_id;
    let revision = REVISION.load(Ordering::Acquire);
    let previous = APPLIED.lock().get(&boid).cloned();

    if let Some(state) = previous.as_ref() {
        if state.target == target && state.revision == revision {
            for (bone, _) in &state.disabled_joints {
                HitModule::set_status_joint(boma, Hash40::new_raw(*bone), app::HitStatus(3), 0);
            }
            return;
        }
    }

    let rules: Vec<_> = RULES.read().iter()
        .filter(|rule| rule.matches_target(target)).cloned().collect();
    let mut disabled_joints = Vec::new();
    for rule in &rules {
        if rule.disabled && !disabled_joints.iter().any(|(bone, _)| *bone == rule.bone) {
            disabled_joints.push((rule.bone, rule.status));
        }
    }

    // Undo the preceding snapshot before applying the replacement. Do not write
    // old-fighter bones into a new fighter when battle_object_id is recycled.
    if let Some(state) = previous {
        if state.target.fighter_kind == target.fighter_kind {
            if state.changed_geometry {
                app::FighterUtil::reset_hit_data(boma);
            }
            for (bone, status) in state.disabled_joints {
                if !disabled_joints.iter().any(|(new_bone, _)| *new_bone == bone) {
                    HitModule::set_status_joint(boma, Hash40::new_raw(bone), app::HitStatus(status), 0);
                }
            }
        }
    }

    let mut changed_geometry = false;
    for rule in &rules {
        if rule.disabled || !rule.apply_geometry {
            continue;
        }
        let start = Vector3f {
            x: rule.endpoint1[0], y: rule.endpoint1[1], z: rule.endpoint1[2],
        };
        let end = Vector3f {
            x: rule.endpoint2[0], y: rule.endpoint2[1], z: rule.endpoint2[2],
        };
        // Native geometry write remains experimental until its ABI and PRC index
        // correspondence have been verified independently in-game.
        app::FighterUtil::set_hit_data(
            boma, rule.index as i32, 0, &start, &end, rule.radius,
            Hash40::new_raw(rule.bone), app::CollisionPart(1),
            app::HitHeight(1), app::HitStatus(rule.status), app::CollisionShapeType(2),
        );
        changed_geometry = true;
    }

    for (bone, _) in &disabled_joints {
        HitModule::set_status_joint(boma, Hash40::new_raw(*bone), app::HitStatus(3), 0);
    }

    if rules.is_empty() {
        APPLIED.lock().remove(&boid);
    } else {
        APPLIED.lock().insert(boid, AppliedState {
            target, revision, disabled_joints, changed_geometry,
        });
        crate::slight::diag::note(format!(
            "hurtbox geometry applied: fighter {} costume {} {} rule(s), revision {}",
            target.fighter_kind, target.costume, rules.len(), revision,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(kind: i32, slots: Vec<u8>, index: u32) -> GeometryRule {
        GeometryRule {
            fighter_kind: kind,
            costume_slots: slots,
            index,
            bone: 1,
            endpoint1: [0.0; 3],
            endpoint2: [1.0; 3],
            radius: 4.0,
            status: 0,
            disabled: false,
            apply_geometry: false,
        }
    }

    #[test]
    fn geometry_rules_reject_unsafe_values() {
        let rule = rule(0, vec![0], 0);
        assert!(rule.valid());
        assert!(!GeometryRule { radius: f32::NAN, ..rule.clone() }.valid());
        assert!(!GeometryRule { index: 128, ..rule.clone() }.valid());
        assert!(!GeometryRule { endpoint1: [f32::INFINITY; 3], ..rule }.valid());
        assert!(!valid_snapshot(&[rule(0, Vec::new(), 0)]));
    }

    #[test]
    fn geometry_rules_are_fighter_and_costume_scoped() {
        let original = rule(25, vec![0, 1, 2, 3, 4, 5, 6, 7], 1);
        let modded = rule(25, vec![8, 9], 1);
        assert!(original.matches_target(RuntimeTarget { fighter_kind: 25, costume: 0 }));
        assert!(!original.matches_target(RuntimeTarget { fighter_kind: 25, costume: 8 }));
        assert!(modded.matches_target(RuntimeTarget { fighter_kind: 25, costume: 8 }));
        assert!(!modded.matches_target(RuntimeTarget { fighter_kind: 26, costume: 8 }));
        assert!(!modded.matches_target(RuntimeTarget::UNKNOWN));
        assert!(valid_snapshot(&[original.clone(), modded.clone()]));
        assert!(!valid_snapshot(&[original.clone(), original]));
        assert!(!valid_snapshot(&[modded, rule(25, vec![9, 10], 1)]));
        assert!(!valid_snapshot(&[rule(25, vec![8, 8], 1)]));
    }

    #[test]
    fn legacy_unscoped_hurtboxes_fail_closed() {
        let old_json = serde_json::json!({
            "fighter_kind": 25, "index": 1, "bone": 1,
            "endpoint1": [0.0, 0.0, 0.0], "endpoint2": [1.0, 0.0, 0.0],
            "radius": 4.0, "status": 0, "disabled": true,
        });
        let old: GeometryRule = serde_json::from_value(old_json).unwrap();
        assert!(!old.valid());
        assert!(!valid_snapshot(&[old]));
    }
}
