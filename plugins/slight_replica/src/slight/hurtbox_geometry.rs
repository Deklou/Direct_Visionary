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

// Group zero of HitModule in main 13.0.5. The native setter at +0x4697f0
// copies 0x38 bytes into record+8; the count getter at +0x46a8b0 reads group+6.
pub(super) type CapsuleGeometry = ([f32; 3], [f32; 3], f32);

#[derive(Clone, Copy, Debug)]
struct NativeCapsule {
    bone: u64,
    geometry: CapsuleGeometry,
    part: i32,
    height: i32,
    status: i32,
    shape: i32,
}

fn geometry_matches(a: CapsuleGeometry, b: CapsuleGeometry) -> bool {
    (a.2 - b.2).abs() <= 0.001
        && a.0.iter().zip(b.0.iter()).chain(a.1.iter().zip(b.1.iter()))
            .all(|(x, y)| (x - y).abs() <= 0.001)
}

fn valid_capsule_geometry(g: CapsuleGeometry) -> bool {
    g.2.is_finite() && g.2 > 0.0 && g.2 <= 50.0
        && g.0.iter().chain(g.1.iter()).all(|v| v.is_finite() && v.abs() <= 200.0)
}

impl NativeCapsule {
    fn same_metadata(&self, other: &Self) -> bool {
        self.bone == other.bone && self.part == other.part && self.height == other.height
            && self.status == other.status && self.shape == other.shape
    }
}

unsafe fn read_at<T: Copy>(base: *const u8, offset: usize) -> T {
    core::ptr::read_unaligned(base.add(offset).cast::<T>())
}

/// The vtable guard deliberately rejects unfamiliar HitModule implementations.
unsafe fn native_record(boma: *mut BattleObjectModuleAccessor, index: u32)
    -> Option<(*const u8, *const u8, *const u8)>
{
    if boma.is_null() || index >= 128 || !crate::slight::ver::is_13_0_5() {
        return None;
    }
    let module: *const u8 = read_at(boma.cast(), 0xb0);
    if module.is_null() { return None; }
    let vtable: *const u8 = read_at(module, 0);
    if vtable.is_null() { return None; }
    let text = skyline::hooks::getRegionAddress(skyline::hooks::Region::Text) as usize;
    if text == 0 || read_at::<usize>(vtable, 0x90) != text + 0x4697f0
        || read_at::<usize>(vtable, 0x170) != text + 0x46a8b0
        || read_at::<usize>(vtable, 0x148) != text + 0x46a800
    {
        return None;
    }
    let group: *const u8 = read_at(module, 0x60);
    let records: *const u8 = read_at(module, 0x70);
    if group.is_null() || records.is_null() { return None; }
    let first = read_at::<i16>(group, 4) as i32;
    let count = read_at::<i16>(group, 6) as i32;
    if !(0..=128).contains(&first) || !(1..=128).contains(&count)
        || index as i32 >= count || HitModule::get_part_size(boma, 0) as i32 != count
    {
        return None;
    }
    let record = records.add((first as usize + index as usize) * 0x50);
    Some((module, group, record))
}

unsafe fn native_capsule(boma: *mut BattleObjectModuleAccessor, index: u32) -> Option<NativeCapsule> {
    let (_, _, record) = native_record(boma, index)?;
    let capsule = NativeCapsule {
        bone: read_at(record, 0x28),
        geometry: (read_at(record, 0x08), read_at(record, 0x14), read_at(record, 0x20)),
        part: read_at(record, 0x30),
        height: read_at(record, 0x34),
        status: read_at(record, 0x38),
        shape: read_at(record, 0x3c),
    };
    // COLLISION_SHAPE_TYPE_CAPSULE = 2 in the 13.0.5 constant registration.
    (capsule.shape == 2 && capsule.bone > 0 && capsule.bone <= 0xffffffffff
        && valid_capsule_geometry(capsule.geometry)).then_some(capsule)
}

#[repr(C, align(16))]
struct PaddedVector {
    vector: Vector3f,
    padding: f32,
}

impl PaddedVector {
    fn new(v: [f32; 3]) -> Self {
        Self { vector: Vector3f { x: v[0], y: v[1], z: v[2] }, padding: 0.0 }
    }
}

/// Keep native metadata, including the default status. The public wrapper also
/// signals the game's geometry update work flag. Its vector loads are 16 bytes.
unsafe fn write_geometry(boma: *mut BattleObjectModuleAccessor, index: u32,
                         current: NativeCapsule, geometry: CapsuleGeometry,
                         action: &str) -> Option<NativeCapsule> {
    let start = PaddedVector::new(geometry.0);
    let end = PaddedVector::new(geometry.1);
    app::FighterUtil::set_hit_data(
        boma, index as i32, 0, &start.vector, &end.vector, geometry.2,
        Hash40::new_raw(current.bone), app::CollisionPart(current.part as _),
        app::HitHeight(current.height as _), app::HitStatus(current.status),
        app::CollisionShapeType(current.shape as _),
    );
    let after = native_capsule(boma, index);
    let ok = after.as_ref().is_some_and(|v| {
        current.same_metadata(v) && geometry_matches(v.geometry, geometry)
    });
    if !ok {
        crate::slight::diag::note(format!(
            "HURTBOX native {action} readback mismatch boid={} index={index} bone={:#x} requested={geometry:?} readback={after:?}",
            (*boma).battle_object_id, current.bone,
        ));
    }
    after.filter(|v| v.bone == current.bone)
}

#[derive(Clone, Copy, Debug)]
struct AppliedGeometry {
    index: u32,
    bone: u64,
    original: CapsuleGeometry,
    written: CapsuleGeometry,
}

unsafe fn apply_geometry(boma: *mut BattleObjectModuleAccessor, rule: &GeometryRule) -> Option<AppliedGeometry> {
    // PRC catalog and runtime capsule must agree before the first write.
    // The catalog is currently per fighter kind, so a different costume's
    // geometry is rejected rather than inferred.
    let reference = crate::slight::collision_view::reference_geometry(
        rule.fighter_kind, rule.index, rule.bone,
    );
    let native = native_capsule(boma, rule.index);
    let Some(current) = native.filter(|v| v.bone == rule.bone
        && reference.is_some_and(|r| geometry_matches(v.geometry, r))) else {
        crate::slight::diag::note(format!(
            "HURTBOX native apply refused fighter={} index={} bone={:#x} reference={reference:?} native={native:?}",
            rule.fighter_kind, rule.index, rule.bone,
        ));
        return None;
    };
    let requested = (rule.endpoint1, rule.endpoint2, rule.radius);
    if geometry_matches(current.geometry, requested) {
        return None;
    }
    let after = write_geometry(boma, rule.index, current, requested, "apply")?;
    let edit = AppliedGeometry {
        index: rule.index, bone: rule.bone, original: current.geometry, written: after.geometry,
    };
    crate::slight::diag::note(format!(
        "HURTBOX tracked boid={} accessor={:#x} index={} original={:?} written={:?}",
        (*boma).battle_object_id, boma as usize, edit.index, edit.original, edit.written,
    ));
    Some(edit)
}

unsafe fn restore_geometry(boma: *mut BattleObjectModuleAccessor, edit: AppliedGeometry) {
    let Some(current) = native_capsule(boma, edit.index).filter(|v| v.bone == edit.bone) else {
        crate::slight::diag::note(format!("HURTBOX native restore refused index={} (layout or bone)", edit.index));
        return;
    };
    if geometry_matches(current.geometry, edit.original) { return; }
    if !geometry_matches(current.geometry, edit.written) {
        crate::slight::diag::note(format!("HURTBOX native restore refused index={} (changed elsewhere)", edit.index));
        return;
    }
    // Restore only the geometry we changed, not the entire fighter's hit_data.
    let restored = write_geometry(boma, edit.index, current, edit.original, "restore");
    if restored.is_some_and(|v| geometry_matches(v.geometry, edit.original)) {
        crate::slight::diag::note(format!(
            "HURTBOX restored boid={} index={} geometry={:?}",
            (*boma).battle_object_id, edit.index, edit.original,
        ));
    }
}

/// None: unedited volume (keep PRC fallback). Some(None): an edited volume
/// cannot be read safely (omit the marker rather than display requested values).
pub(super) unsafe fn geometry_for_overlay(
    boma: *mut BattleObjectModuleAccessor, index: u32, bone: u64,
) -> Option<Option<CapsuleGeometry>> {
    let boid = (*boma).battle_object_id;
    let tracked = APPLIED.lock().get(&boid).is_some_and(|state| {
        state.accessor == boma as usize
            && state.geometry.iter().any(|edit| edit.index == index && edit.bone == bone)
    });
    if !tracked { return None; }
    Some(native_capsule(boma, index).filter(|v| v.bone == bone).map(|v| v.geometry))
}

#[derive(Clone, Debug)]
struct AppliedState {
    target: RuntimeTarget,
    revision: u64,
    /// Saved default joint statuses, used to undo OFF when a rule disappears.
    disabled_joints: Vec<(u64, i32)>,
    accessor: usize,
    geometry: Vec<AppliedGeometry>,
    geometry_pending: bool,
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

// A fight-start notification is not a lifetime boundary. A fighter can be
// temporarily inactive while its HitModule still owns the edited geometry.
// Only a verified callback with the same full ID can replace its saved
// identity; teardown clears all remaining records.

/// Actual match teardown, not the repeatable fight-start notification.
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
    // During reset, skip incomplete identity rather than losing the original.
    let Some(boid) = crate::slight::agents::boid_from_module(boma) else {
        return;
    };
    let Some(target) = crate::slight::hitbox_viewer::rule_target(boma) else {
        return;
    };
    if !(0..=127).contains(&target.fighter_kind) || u8::try_from(target.costume).is_err() {
        return;
    }
    let revision = REVISION.load(Ordering::Acquire);
    // Distinct full battle-object IDs must never evict each other. In
    // particular, their low nine bits are not a verified lifetime identity.
    let previous = APPLIED.lock().get(&boid).cloned();
    let training = app::smashball::is_training_mode();
    let roster_ready = training && crate::slight::collision_view::has_reference_roster(target.fighter_kind);

    if let Some(state) = previous.as_ref() {
        let ready = state.geometry_pending && roster_ready;
        if state.target == target && state.revision == revision && state.accessor == boma as usize && !ready {
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

    // Do not discard the saved original if HitModule is temporarily unavailable.
    // Wait until it can be read before processing a replacement snapshot.
    if let Some(state) = previous.as_ref() {
        if state.target == target && state.accessor == boma as usize
            && state.geometry.iter().any(|edit| native_capsule(boma, edit.index).is_none())
        {
            return;
        }
    }

    // Undo the preceding snapshot before applying the replacement. Do not write
    // old-fighter bones into a new fighter when battle_object_id is recycled.
    if let Some(state) = previous {
        if state.target != target || state.accessor != boma as usize {
            crate::slight::diag::note(format!(
                "HURTBOX identity replaced boid={boid} previous={:?}/0x{:x} current={target:?}/0x{:x}",
                state.target, state.accessor, boma as usize,
            ));
        }
        if state.target == target && state.accessor == boma as usize {
            for edit in state.geometry.iter().copied() {
                restore_geometry(boma, edit);
            }
        }
        if state.target == target && state.accessor == boma as usize {
            for (bone, status) in state.disabled_joints {
                if !disabled_joints.iter().any(|(new_bone, _)| *new_bone == bone) {
                    HitModule::set_status_joint(boma, Hash40::new_raw(bone), app::HitStatus(status), 0);
                }
            }
        }
    }

    let mut geometry = Vec::new();
    let geometry_pending = rules.iter().any(|rule| !rule.disabled && rule.apply_geometry)
        && !roster_ready;
    if training && !geometry_pending {
        for rule in &rules {
            if rule.disabled || !rule.apply_geometry { continue; }
            if let Some(edit) = apply_geometry(boma, rule) {
                geometry.push(edit);
            }
        }
    }

    for (bone, _) in &disabled_joints {
        HitModule::set_status_joint(boma, Hash40::new_raw(*bone), app::HitStatus(3), 0);
    }

    if rules.is_empty() {
        APPLIED.lock().remove(&boid);
    } else {
        crate::slight::diag::note(format!(
            "hurtbox geometry snapshot: boid={boid} accessor={:#x} fighter={} costume={} revision={revision} rules={} tracked={} disabled={} pending={geometry_pending}",
            boma as usize, target.fighter_kind, target.costume, rules.len(), geometry.len(), disabled_joints.len(),
        ));
        APPLIED.lock().insert(boid, AppliedState {
            target, revision, disabled_joints, accessor: boma as usize, geometry, geometry_pending,
        });
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
        let original = rule(0, vec![0], 0);
        assert!(original.valid());
        assert!(!GeometryRule { radius: f32::NAN, ..original.clone() }.valid());
        assert!(!GeometryRule { index: 128, ..original.clone() }.valid());
        assert!(!GeometryRule { endpoint1: [f32::INFINITY; 3], ..original }.valid());
        assert!(!valid_snapshot(&[rule(0, Vec::new(), 0)]));
    }

    #[test]
    fn native_geometry_comparison_checks_both_ends_and_radius() {
        let baseline = ([0.0, 1.0, 2.0], [3.0, 4.0, 5.0], 2.0);
        assert!(valid_capsule_geometry(baseline));
        assert!(geometry_matches(baseline, ([0.0001, 1.0, 2.0], baseline.1, 2.0)));
        assert!(!geometry_matches(baseline, (baseline.0, [3.0, 4.0, 6.0], 2.0)));
        assert!(!geometry_matches(baseline, (baseline.0, baseline.1, 3.0)));
        assert!(!valid_capsule_geometry((baseline.0, baseline.1, f32::NAN)));
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
