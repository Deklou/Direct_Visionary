//! In-game collision markers. Gameplay reads only; effects are owned by handle.
use parking_lot::Mutex;
use serde::Deserialize;
use smash::app::{
    self, lua_bind::{AttackModule, EffectModule, HitModule}, BattleObjectModuleAccessor,
};
use smash::lib::lua_const::*;
use smash::phx::{Hash40, Vector3f};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;

const MAX_MARKERS: usize = 96;
const MAX_PER_CAPSULE: usize = 12;
const RED: [f32; 3] = [1.0, 0.05, 0.05];
const GRAY: [f32; 3] = [0.75, 0.75, 0.75];
const BLUE: [f32; 3] = [0.1, 0.45, 1.0];
const GREEN: [f32; 3] = [0.15, 1.0, 0.25];

#[derive(Clone, Debug, Deserialize)]
struct ReferenceVolume {
    index: u32,
    bone: u64,
    endpoint1: [f32; 3],
    endpoint2: [f32; 3],
    radius: f32,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Config {
    attacks: bool,
    hurtboxes: bool,
    intangible: bool,
    invincible: bool,
    fighter_kind: Option<i32>,
    volumes: Vec<ReferenceVolume>,
}

impl Config {
    fn enabled(&self) -> bool {
        self.attacks || self.hurtboxes || self.intangible || self.invincible
    }
}

static CONFIG: Mutex<Option<(u64, Config)>> = Mutex::new(None);
static MARKERS: LazyLock<Mutex<HashMap<u32, ObjectMarkers>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static SPAWNING: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sphere {
    bone: u64,
    pos: [f32; 3],
    radius: f32,
    color: [f32; 3],
}

struct Marker {
    sphere: Sphere,
    handle: u32,
}

struct ObjectMarkers {
    accessor: usize,
    markers: Vec<Marker>,
}

fn valid_geometry(bone: u64, a: [f32; 3], b: [f32; 3], radius: f32) -> bool {
    bone != 0 && bone <= 0xff_ffff_ffff
        && radius.is_finite() && radius > 0.0 && radius <= 100.0
        && a.iter().chain(b.iter()).all(|v| v.is_finite() && v.abs() <= 1000.0)
}

pub fn set_config(mut config: Config) {
    config.volumes.truncate(128);
    config.volumes.retain(|v| v.index < 128
        && valid_geometry(v.bone, v.endpoint1, v.endpoint2, v.radius));
    let client = crate::rust_extender::net::simple_server::client_id();
    *CONFIG.lock() = Some((client, config));
}

/// Bypass Visionary's aliases, carrier redirects and capture for our own spawn.
pub fn is_marker_request(boma: *mut BattleObjectModuleAccessor, hash: u64) -> bool {
    !boma.is_null() && SPAWNING.load(Ordering::Acquire) == boma as usize
        && hash == Hash40::new("sys_shield").hash
}

fn vec3(v: [f32; 3]) -> Vector3f {
    Vector3f { x: v[0], y: v[1], z: v[2] }
}

fn append_capsule(out: &mut Vec<Sphere>, bone: u64, a: [f32; 3], b: [f32; 3],
                  radius: f32, color: [f32; 3]) {
    if !valid_geometry(bone, a, b, radius) {
        return;
    }
    let delta = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let length = delta.iter().map(|v| v * v).sum::<f32>().sqrt();
    let count = if length <= 0.0001 { 1 } else {
        ((length / (radius * 1.75)).ceil() as usize).saturating_add(1)
            .clamp(2, MAX_PER_CAPSULE)
    };
    for i in 0..count {
        if out.len() >= MAX_MARKERS {
            break;
        }
        let t = if count == 1 { 0.0 } else { i as f32 / (count - 1) as f32 };
        out.push(Sphere {
            bone, radius, color,
            pos: [a[0] + t * delta[0], a[1] + t * delta[1], a[2] + t * delta[2]],
        });
    }
}

unsafe fn desired(boma: *mut BattleObjectModuleAccessor, config: &Config) -> Vec<Sphere> {
    let mut out = Vec::new();
    if config.attacks {
        for id in 0..8 {
            if !AttackModule::is_attack(boma, id, false) {
                continue;
            }
            let ptr = AttackModule::attack_data(boma, id, false);
            if ptr.is_null() {
                continue;
            }
            let data = &*ptr;
            let a = [data.x, data.y, data.z];
            let second = [data.x2, data.y2, data.z2];
            // The public AttackData binding has no verified shape discriminator.
            let b = if second == [0.0; 3] { a } else { second };
            append_capsule(&mut out, data.node, a, b, data.size, RED);
        }
    }
    if !(config.hurtboxes || config.intangible || config.invincible)
        || config.fighter_kind != Some(app::utility::get_kind(&mut *boma))
    {
        return out;
    }
    // PRC shapes are references, not native geometry readback. The part count and
    // status selector still need a controlled Ryujinx test before trusting the result.
    let parts = HitModule::get_part_size(boma, 0);
    if parts == 0 || parts > 128 {
        return out;
    }
    for v in &config.volumes {
        if u64::from(v.index) >= parts {
            continue;
        }
        let status = HitModule::get_status(boma, v.index as i32, 0);
        let color = if status == *HIT_STATUS_NORMAL && config.hurtboxes {
            GRAY
        } else if status == *HIT_STATUS_XLU && config.intangible {
            BLUE
        } else if status == *HIT_STATUS_INVINCIBLE && config.invincible {
            GREEN
        } else {
            continue;
        };
        append_capsule(&mut out, v.bone, v.endpoint1, v.endpoint2, v.radius, color);
    }
    out
}

struct SpawnGuard;
impl Drop for SpawnGuard {
    fn drop(&mut self) {
        SPAWNING.store(0, Ordering::Release);
    }
}

unsafe fn spawn(boma: *mut BattleObjectModuleAccessor, sphere: Sphere) -> Option<Marker> {
    if SPAWNING.compare_exchange(0, boma as usize, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return None;
    }
    let _guard = SpawnGuard;
    // Approximate sphere size, based on the training mod's sys_shield marker calibration.
    let handle = EffectModule::req_follow(
        boma, Hash40::new("sys_shield"), Hash40::new_raw(sphere.bone),
        &vec3(sphere.pos), &vec3([0.0; 3]), sphere.radius * (19.0 / 200.0),
        true,
        *EFFECT_SUB_ATTRIBUTE_NO_JOINT_SCALE as u32
            | *EFFECT_SUB_ATTRIBUTE_FOLLOW as u32
            | *EFFECT_SUB_ATTRIBUTE_CONCLUDE_STATUS as u32,
        0, 0, 0, 0, true, true,
    ) as u32;
    if handle == 0 || !EffectModule::is_exist_effect(boma, handle) {
        return None;
    }
    EffectModule::set_rgb(boma, handle, sphere.color[0], sphere.color[1], sphere.color[2]);
    EffectModule::set_alpha(boma, handle, 0.55);
    EffectModule::set_rate(boma, handle, 8.0);
    Some(Marker { sphere, handle })
}

unsafe fn kill(boma: *mut BattleObjectModuleAccessor, marker: Marker) {
    if EffectModule::is_exist_effect(boma, marker.handle) {
        EffectModule::kill(boma, marker.handle, false, true);
    }
}

/// Called only from the existing fighter Main callback, never from the network thread.
pub unsafe fn tick(boma: *mut BattleObjectModuleAccessor) {
    if boma.is_null() || !crate::slight::ver::is_13_0_5()
        || app::utility::get_category(&mut *boma) != *BATTLE_OBJECT_CATEGORY_FIGHTER
    {
        return;
    }
    let Some(config) = CONFIG.try_lock() else { return; };
    let server = crate::rust_extender::net::simple_server::has_client();
    let active = config.as_ref().filter(|(client, c)| {
        server && *client == crate::rust_extender::net::simple_server::client_id()
            && c.enabled() && app::smashball::is_training_mode()
    });
    let id = (*boma).battle_object_id;
    let Some(mut objects) = MARKERS.try_lock() else { return; };
    if active.is_none() && !objects.contains_key(&id) {
        return;
    }
    let want = active.map(|(_, c)| desired(boma, c)).unwrap_or_default();
    drop(config);
    let previous = objects.remove(&id);
    let mut old = previous.filter(|o| o.accessor == boma as usize)
        .map(|o| o.markers).unwrap_or_default();
    // Reuse matching handles rather than respawning the whole overlay every frame.
    let mut next = Vec::with_capacity(want.len());
    for sphere in want {
        if let Some(i) = old.iter().position(|m| m.sphere == sphere) {
            let marker = old.swap_remove(i);
            if EffectModule::is_exist_effect(boma, marker.handle) {
                next.push(marker);
                continue;
            }
        }
        if let Some(marker) = spawn(boma, sphere) {
            next.push(marker);
        }
    }
    for marker in old {
        kill(boma, marker);
    }
    if !next.is_empty() {
        objects.retain(|boid, _| app::sv_battle_object::is_active(*boid));
        objects.insert(id, ObjectMarkers { accessor: boma as usize, markers: next });
    }
}

pub fn forget_match() {
    if let Some(mut markers) = MARKERS.try_lock() {
        markers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capsule_keeps_both_ends() {
        let mut out = Vec::new();
        append_capsule(&mut out, 1, [0.0; 3], [10.0, 0.0, 0.0], 2.0, RED);
        assert_eq!(out.first().unwrap().pos, [0.0; 3]);
        assert_eq!(out.last().unwrap().pos, [10.0, 0.0, 0.0]);
        assert!(out.len() <= MAX_PER_CAPSULE);
    }

    #[test]
    fn invalid_geometry_and_total_budget() {
        let mut out = Vec::new();
        append_capsule(&mut out, 1, [0.0; 3], [0.0; 3], f32::NAN, RED);
        assert!(out.is_empty());
        for _ in 0..MAX_MARKERS + 1 {
            append_capsule(&mut out, 1, [0.0; 3], [0.0; 3], 1.0, RED);
        }
        assert_eq!(out.len(), MAX_MARKERS);
    }
}
