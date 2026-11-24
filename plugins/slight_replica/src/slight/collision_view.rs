//! In-game collision markers. Gameplay reads only; effects are owned by handle.
use parking_lot::Mutex;
use serde::Deserialize;
use smash::app::{
    self, lua_bind::{AttackModule, EffectModule, HitModule}, BattleObjectModuleAccessor,
};
use smash::lib::lua_const::*;
use smash::phx::{Hash40, Vector3f};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    client_id: u64,
    revision: u64,
    attacks: bool,
    hurtboxes: bool,
    intangible: bool,
    invincible: bool,
    #[serde(skip)]
    roster: HashMap<i32, Vec<ReferenceVolume>>,
    #[serde(skip)]
    requests: HashMap<i32, u16>,
}

#[derive(Deserialize)]
pub struct RosterReply {
    client_id: u64,
    revision: u64,
    kind: i32,
    volumes: Vec<(u32, u64, [f32; 3], [f32; 3], f32)>,
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
static HIT_VTABLE_REPORTED: AtomicBool = AtomicBool::new(false);

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
    let client = crate::rust_extender::net::simple_server::client_id();
    if config.client_id != client {
        return;
    }
    let mut slot = CONFIG.lock();
    if let Some((previous_client, previous)) = slot.as_mut() {
        if *previous_client == client && previous.revision == config.revision {
            config.roster = std::mem::take(&mut previous.roster);
            config.requests = std::mem::take(&mut previous.requests);
        }
    }
    crate::slight::diag::note(format!(
        "COLLISION flags client={client} revision={} attacks={} hurtboxes={} intangible={} invincible={} cached={}",
        config.revision, config.attacks, config.hurtboxes, config.intangible, config.invincible,
        config.roster.len(),
    ));
    *slot = Some((client, config));
}

pub fn set_roster(reply: RosterReply) {
    let client = crate::rust_extender::net::simple_server::client_id();
    if reply.client_id != client || !(0..128).contains(&reply.kind) {
        return;
    }
    let mut slot = CONFIG.lock();
    let Some((configured_client, config)) = slot.as_mut() else { return; };
    if *configured_client != client || config.revision != reply.revision {
        return;
    }
    let volumes: Vec<_> = reply.volumes.into_iter().take(128)
        .filter(|(index, bone, a, b, radius)| *index < 128 && valid_geometry(*bone, *a, *b, *radius))
        .map(|(index, bone, endpoint1, endpoint2, radius)| ReferenceVolume {
            index, bone, endpoint1, endpoint2, radius,
        }).collect();
    crate::slight::diag::note(format!(
        "COLLISION roster client={client} revision={} kind={} volumes={}",
        reply.revision, reply.kind, volumes.len(),
    ));
    config.roster.insert(reply.kind, volumes);
    config.requests.remove(&reply.kind);
}

fn request_roster(config: &mut Config, kind: i32) {
    if !(0..128).contains(&kind) {
        return;
    }
    let remaining = config.requests.entry(kind).or_insert(0);
    if *remaining != 0 {
        *remaining -= 1;
        return;
    }
    // Retry if the request or response was dropped; empty replies are cached too.
    *remaining = 240;
    let client_id = config.client_id;
    let revision = config.revision;
    let message = serde_json::json!({
        "header": "CollisionRosterRequest",
        "body": { "client_id": client_id, "revision": revision, "kind": kind }
    });
    crate::rust_extender::net::simple_server::queue(message.to_string());
    crate::slight::diag::note(format!(
        "COLLISION request client={client_id} revision={revision} kind={kind}"
    ));
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

// All offsets here are the verified HitModule dispatch slots in main 13.0.5.
// Log their resolved targets once, to locate the concrete native layout in Ghidra.
unsafe fn report_hit_vtable(boma: *mut BattleObjectModuleAccessor) {
    if HIT_VTABLE_REPORTED.load(Ordering::Relaxed) {
        return;
    }
    let module = *boma.cast::<u8>().add(0xb0).cast::<*const usize>();
    if module.is_null() {
        return;
    }
    let vtable = *module as *const usize;
    if vtable.is_null() {
        return;
    }
    let text = skyline::hooks::getRegionAddress(skyline::hooks::Region::Text) as usize;
    let rodata = skyline::hooks::getRegionAddress(skyline::hooks::Region::Rodata) as usize;
    let mut targets = Vec::new();
    for slot in [0x90, 0x98, 0xd8, 0x148, 0x150, 0x170, 0x200] {
        let addr = *vtable.add(slot / core::mem::size_of::<usize>());
        if !(text..rodata).contains(&addr) {
            return;
        }
        targets.push(format!("{slot:#x}=main+{:#x}", addr - text));
    }
    if !HIT_VTABLE_REPORTED.swap(true, Ordering::Relaxed) {
        crate::slight::diag::note(format!("COLLISION HitModule dispatch: {}", targets.join(" ")));
    }
}

fn effective_status(local: i32, whole: i32, off: i32, xlu: i32, invincible: i32) -> i32 {
    if local == off || whole == off {
        off
    } else if local == xlu || whole == xlu {
        xlu
    } else if local == invincible || whole == invincible {
        invincible
    } else {
        local
    }
}

unsafe fn desired(boma: *mut BattleObjectModuleAccessor, config: &mut Config) -> Vec<Sphere> {
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
    if !(config.hurtboxes || config.intangible || config.invincible) {
        return out;
    }
    report_hit_vtable(boma);
    let kind = app::utility::get_kind(&mut *boma);
    if !config.roster.contains_key(&kind) {
        request_roster(config, kind);
        return out;
    }
    let Some(volumes) = config.roster.get(&kind) else { return out; };
    // PRC shapes are references, not native geometry readback.
    let parts = HitModule::get_part_size(boma, 0) as i32;
    if !(1..=128).contains(&parts) {
        return out;
    }
    let whole = HitModule::get_whole(boma, 0);
    for v in volumes {
        if v.index as i32 >= parts {
            continue;
        }
        let local = HitModule::get_status(boma, v.index as i32, 0);
        let status = effective_status(
            local, whole, *HIT_STATUS_OFF, *HIT_STATUS_XLU, *HIT_STATUS_INVINCIBLE,
        );
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
    let Some(mut config) = CONFIG.try_lock() else { return; };
    let server = crate::rust_extender::net::simple_server::has_client();
    let active = config.as_mut().filter(|(client, c)| {
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
    fn flags_and_geometry_are_separate_wire_messages() {
        let flags: Config = serde_json::from_str(
            r#"{"client_id":2,"revision":1,"attacks":true,"hurtboxes":false,"intangible":false,"invincible":false}"#,
        ).unwrap();
        assert!(flags.attacks);
        assert!(flags.roster.is_empty());
        let reply: RosterReply = serde_json::from_str(
            r#"{"kind":3,"client_id":2,"revision":1,"volumes":[[0,123,[1,2,3],[4,5,6],2.5]]}"#,
        ).unwrap();
        assert_eq!(reply.kind, 3);
        assert_eq!(reply.volumes[0], (0, 123, [1.0, 2.0, 3.0], [4.0, 5.0, 6.0], 2.5));
    }

    #[test]
    fn global_and_local_status_are_combined() {
        let (normal, invincible, xlu, off) = (0, 1, 2, 3);
        assert_eq!(effective_status(normal, invincible, off, xlu, invincible), invincible);
        assert_eq!(effective_status(normal, xlu, off, xlu, invincible), xlu);
        assert_eq!(effective_status(xlu, normal, off, xlu, invincible), xlu);
        assert_eq!(effective_status(off, invincible, off, xlu, invincible), off);
    }

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
