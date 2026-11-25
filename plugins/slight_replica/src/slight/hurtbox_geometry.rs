//! Experimental per-fighter hurtbox geometry preview (Smash 13.0.5).
//! Changes are applied on the fighter game thread; no extra hooks or PRC writes.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::LazyLock;

use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use smash::app::{self, lua_bind::HitModule, BattleObjectModuleAccessor};
use smash::phx::{Hash40, Vector3f};

#[derive(Clone, Debug, Deserialize)]
pub struct GeometryRule {
    pub fighter_kind: i32,
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
            && self.index < 128
            && self.bone > 0 && self.bone <= 0xffffffffff
            && self.radius.is_finite()
            && self.radius > 0.0
            && self.radius <= 50.0
            && (0..=3).contains(&self.status)
            && self.endpoint1.iter().chain(self.endpoint2.iter())
                .all(|v| v.is_finite() && v.abs() <= 200.0)
    }
}

static RULES: LazyLock<RwLock<Vec<GeometryRule>>> = LazyLock::new(|| RwLock::new(Vec::new()));
static REVISION: AtomicU64 = AtomicU64::new(0);
static HAS_DISABLED: AtomicBool = AtomicBool::new(false);
// Object ID + fighter kind prevent reusing a latch for a different fighter instance.
static APPLIED: LazyLock<Mutex<HashMap<u32, (i32, u64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_rules(rules: Vec<GeometryRule>) {
    let mut seen = HashSet::new();
    if rules.len() > 128
        || rules.iter().any(|r| !r.valid()
            || !seen.insert((r.fighter_kind, r.index)))
    {
        crate::slight::diag::note("hurtbox geometry rules rejected (index, value or duplicate)");
        return;
    }
    let has_disabled = rules.iter().any(|r| r.disabled);
    *RULES.write() = rules;
    HAS_DISABLED.store(has_disabled, Ordering::Release);
    REVISION.fetch_add(1, Ordering::Release);
}

/// One-shot per rule revision and fighter instance. An explicit empty snapshot restores
/// vanilla hit_data, while an edited snapshot updates only its named entries.
pub unsafe fn tick(boma: *mut BattleObjectModuleAccessor) {
    if boma.is_null() || !crate::slight::ver::is_13_0_5() {
        return;
    }
    if app::utility::get_category(&mut *boma)
        != *smash::lib::lua_const::BATTLE_OBJECT_CATEGORY_FIGHTER
    {
        return;
    }
    let kind = app::utility::get_kind(&mut *boma);
    let boid = (*boma).battle_object_id;
    let revision = REVISION.load(Ordering::Acquire);
    let previous = APPLIED.lock().get(&boid).copied();
    if previous == Some((kind, revision)) && !HAS_DISABLED.load(Ordering::Acquire) {
        return;
    }
    let rules: Vec<_> = RULES.read().iter()
        .filter(|r| r.fighter_kind == kind).cloned().collect();
    let already_applied = previous == Some((kind, revision));
    if already_applied && !rules.iter().any(|r| r.disabled) {
        return;
    }
    if rules.is_empty() {
        if previous.is_some_and(|(old_kind, _)| old_kind == kind) {
            app::FighterUtil::reset_hit_data(boma);
            APPLIED.lock().remove(&boid);
        }
        return;
    }

    for rule in &rules {
        if rule.disabled {
            // Status by joint avoids assuming that a PRC list index is a native index.
            // Keep enforcing OFF because an ACMD animation can restore that joint.
            HitModule::set_status_joint(boma, Hash40::new_raw(rule.bone), app::HitStatus(3), 0);
            continue;
        }
        if already_applied {
            continue;
        }
        if !rule.apply_geometry {
            // Re-enable a previously disabled joint without touching its native geometry.
            HitModule::set_status_joint(
                boma, Hash40::new_raw(rule.bone), app::HitStatus(rule.status), 0,
            );
            continue;
        }
        let start = Vector3f {
            x: rule.endpoint1[0], y: rule.endpoint1[1], z: rule.endpoint1[2],
        };
        let end = Vector3f {
            x: rule.endpoint2[0], y: rule.endpoint2[1], z: rule.endpoint2[2],
        };
        // Ghidra 13.0.5: FighterUtil::set_hit_data (main+0x693550), HitModule vtable +0x90.
        // The second integer is the HitModule update flag; keep 0 for this guarded probe.
        app::FighterUtil::set_hit_data(
            boma,
            rule.index as i32,
            0,
            &start,
            &end,
            rule.radius,
            Hash40::new_raw(rule.bone),
            app::CollisionPart(1),
            app::HitHeight(1),
            app::HitStatus(rule.status),
            app::CollisionShapeType(2),
        );
    }
    if !already_applied {
        APPLIED.lock().insert(boid, (kind, revision));
        crate::slight::diag::note(format!(
        "hurtbox geometry applied: fighter {kind}, {} capsule(s), revision {revision}",
        rules.len()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_rules_reject_unsafe_values() {
        let rule = GeometryRule {
            fighter_kind: 0,
            index: 0,
            bone: 1,
            endpoint1: [0.0; 3],
            endpoint2: [1.0; 3],
            radius: 4.0,
            status: 0,
            disabled: false,
            apply_geometry: false,
        };
        assert!(rule.valid());
        assert!(!GeometryRule { radius: f32::NAN, ..rule.clone() }.valid());
        assert!(!GeometryRule { index: 128, ..rule.clone() }.valid());
        assert!(!GeometryRule { endpoint1: [f32::INFINITY; 3], ..rule }.valid());
    }
}
