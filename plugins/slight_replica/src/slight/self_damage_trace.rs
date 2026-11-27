//! Opt-in observation of damage-add routes. No rules, injections or damage writes.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use parking_lot::Mutex;
use smash::app::{self, lua_bind::{DamageModule, MotionModule, StatusModule, WorkModule}, BattleObjectModuleAccessor};
use smash::lib::lua_const::*;
use super::{diag, hitbox_viewer::{read_args_exact, LuaArg}, smash_utils};

const MARKER: &str = "sd:/slight/debug/self_damage.txt";
const ACMD: u8 = 1;
const NATIVE: u8 = 2;
const PARAMS: u8 = 4;
const WATCH: u8 = 8;
const EVENT_LIMIT: usize = 512;
const PARAM_LIMIT: usize = 256;
static ACMD_COUNT: AtomicUsize = AtomicUsize::new(0);
static NATIVE_COUNT: AtomicUsize = AtomicUsize::new(0);
static WATCH_COUNT: AtomicUsize = AtomicUsize::new(0);
static WATCH_ENABLED: AtomicBool = AtomicBool::new(false);

fn parse_modes(text: &str) -> u8 {
    text.split(|c: char| c.is_whitespace() || c == ',').fold(0, |modes, name| {
        modes | match name {
            "acmd" => ACMD,
            "native" => NATIVE,
            "params" => PARAMS,
            "watch" => WATCH,
            _ => 0,
        }
    })
}

fn reserve(counter: &AtomicUsize, source: &str) -> Option<usize> {
    let n = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed,
        |n| (n < EVENT_LIMIT).then_some(n + 1)).ok()? + 1;
    if n == EVENT_LIMIT {
        diag::note(format!("SELF_DAMAGE budget reached source={source} limit={EVENT_LIMIT}"));
    }
    Some(n)
}

#[derive(Clone, Copy)]
struct Context {
    boid: u32,
    kind: i32,
    costume: i32,
    status: i32,
    motion: u64,
    frame: f32,
}

impl Context {
    unsafe fn read(boma: *mut BattleObjectModuleAccessor) -> Option<Self> {
        if boma.is_null() || app::utility::get_category(&mut *boma) != *BATTLE_OBJECT_CATEGORY_FIGHTER {
            return None;
        }
        let boid = (*boma).battle_object_id;
        if app::sv_battle_object::module_accessor(boid) != boma {
            return None;
        }
        Some(Self {
            boid,
            kind: app::utility::get_kind(&mut *boma),
            costume: WorkModule::get_int(boma, *FIGHTER_INSTANCE_WORK_ID_INT_COLOR),
            status: StatusModule::status_kind(boma),
            motion: MotionModule::motion_kind(boma),
            frame: MotionModule::frame(boma),
        })
    }

    fn label(self) -> String {
        format!("boid={:#x} kind={} costume={} status={:#x} motion={:#x} frame={:.3}",
            self.boid, self.kind, self.costume, self.status, self.motion, self.frame)
    }
}

unsafe fn damage(boma: *mut BattleObjectModuleAccessor) -> Option<f32> {
    let value = DamageModule::damage(boma, 0);
    value.is_finite().then_some(value)
}

struct Observation {
    seq: usize,
    context: Context,
    before: Option<f32>,
}

unsafe fn observe(boma: *mut BattleObjectModuleAccessor, counter: &AtomicUsize, source: &str)
    -> Option<Observation>
{
    if counter.load(Ordering::Relaxed) >= EVENT_LIMIT {
        return None;
    }
    let context = Context::read(boma)?;
    let seq = reserve(counter, source)?;
    Some(Observation { seq, context, before: damage(boma) })
}

unsafe fn report(source: &str, observation: Observation, boma: *mut BattleObjectModuleAccessor,
    amount: Option<f32>, arg3: Option<i32>)
{
    // Do not dereference an accessor that disappeared while the original ran.
    let after = if app::sv_battle_object::module_accessor(observation.context.boid) == boma {
        damage(boma)
    } else {
        None
    };
    let delta = observation.before.zip(after).map(|(before, after)| after - before);
    // This is a measurement, not a read of FighterManager's ratio. Clamps, stamina,
    // invincibility, rounding and other effects can change it.
    let observed_ratio = delta.zip(amount).and_then(|(d, amount)| {
        if amount > 0.0 && amount.is_finite() {
            let ratio = d / amount;
            ratio.is_finite().then_some(ratio)
        } else {
            None
        }
    });
    diag::note(format!(
        "SELF_DAMAGE source={source} seq={} {} amount={amount:?} arg3={arg3:?} before={:?} after={after:?} delta={delta:?} observed_ratio={observed_ratio:?}",
        observation.seq, observation.context.label(), observation.before
    ));
}

#[skyline::hook(replace = app::sv_animcmd::FT_ADD_DAMAGE)]
unsafe fn hook_ft_add_damage(lua_state: u64) {
    let boma = if lua_state != 0 && ACMD_COUNT.load(Ordering::Relaxed) < EVENT_LIMIT {
        app::sv_system::battle_object_module_accessor(lua_state) as *mut BattleObjectModuleAccessor
    } else {
        std::ptr::null_mut()
    };
    let observation = observe(boma, &ACMD_COUNT, "acmd");
    let amount = if observation.is_some() {
        // The same non-destructive stack reader used by the existing numeric ACMD hooks.
        match read_args_exact(lua_state, 1).first() {
            Some(LuaArg::Num(value)) => Some(*value),
            Some(LuaArg::Int(value)) => Some(*value as f32),
            _ => None,
        }
    } else {
        None
    };
    original!()(lua_state);
    if let Some(observation) = observation {
        report("acmd", observation, boma, amount, None);
    }
}

#[skyline::hook(replace = DamageModule::add_damage)]
unsafe fn hook_add_damage(boma: *mut BattleObjectModuleAccessor, amount: f32, arg3: i32) {
    let observation = observe(boma, &NATIVE_COUNT, "native_wrapper");
    original!()(boma, amount, arg3);
    if let Some(observation) = observation {
        // Not every positive add_damage call is recoil. Retain the native argument as-is.
        report("native_wrapper", observation, boma, Some(amount), Some(arg3));
    }
}

// These are exact parent/child pairs seen in the dumped fighter statuses.
fn recoil_param(parent: u64, child: u64, integer: bool) -> Option<&'static str> {
    match (parent, child, integer) {
        (0x0f899192aa, 0x1ba36e915b, false) => Some("special_n_charge_max_damage"),
        (0x0f899192aa, 0x1b1108ebc5, false) => Some("special_n_charge_mdl_damage"),
        (0x0fea97fe73, 0x0cbabd4249, false) => Some("rush_damage_"),
        (0x0fea97fe73, 0x15bbf63010, false) => Some("special_s_fail_damage"),
        (0x1086bc4a93, 0x2129bdee8d, false) => Some("special_hi_explosion_damage_value"),
        (0x0fea97fe73, 0x11db8f2b03, false) => Some("explosion_damage_"),
        (0x0fea97fe73, 0x206edc2d9e, true) => Some("explosion_damage_interval_frame_"),
        (0x0fea97fe73 | 0x1018dfb2f4, 0x0fb15550ff, false) => Some("hold_max_damage"),
        (0x0fea97fe73 | 0x1018dfb2f4, 0x13190e3939, false) => Some("hold_max_damage_max"),
        _ => None,
    }
}

#[derive(PartialEq, Eq)]
struct ParamKey {
    boid: u32,
    kind: i32,
    costume: i32,
    status: i32,
    motion: u64,
    parent: u64,
    child: u64,
    integer: bool,
    bits: u32,
}

static PARAM_SEEN: Mutex<Vec<ParamKey>> = Mutex::new(Vec::new());

unsafe fn report_param(boma: *mut BattleObjectModuleAccessor, parent: u64, child: u64,
    integer: bool, bits: u32)
{
    let Some(name) = recoil_param(parent, child, integer) else { return; };
    let Some(context) = Context::read(boma) else { return; };
    let key = ParamKey {
        boid: context.boid, kind: context.kind, costume: context.costume,
        status: context.status, motion: context.motion, parent, child, integer, bits,
    };
    let full = {
        let Some(mut seen) = PARAM_SEEN.try_lock() else { return; };
        if seen.len() >= PARAM_LIMIT || seen.contains(&key) { return; }
        seen.push(key);
        seen.len() == PARAM_LIMIT
    };
    let value = if integer { (bits as i32).to_string() } else { f32::from_bits(bits).to_string() };
    diag::note(format!("SELF_DAMAGE_PARAM {} parent={parent:#x} child={child:#x} name={name} integer={integer} value={value}", context.label()));
    if full {
        diag::note("SELF_DAMAGE_PARAM budget reached");
    }
}

#[skyline::hook(replace = WorkModule::get_param_float)]
unsafe fn hook_param_float(boma: *mut BattleObjectModuleAccessor, parent: u64, child: u64) -> f32 {
    let value = original!()(boma, parent, child);
    report_param(boma, parent, child, false, value.to_bits());
    value
}

#[skyline::hook(replace = WorkModule::get_param_int)]
unsafe fn hook_param_int(boma: *mut BattleObjectModuleAccessor, parent: u64, child: u64) -> i32 {
    let value = original!()(boma, parent, child);
    report_param(boma, parent, child, true, value as u32);
    value
}

struct WatchEntry {
    boid: u32,
    accessor: usize,
    damage: f32,
}

static WATCH_STATE: Mutex<Vec<WatchEntry>> = Mutex::new(Vec::new());

/// Optional passive observation of changes not explained by either hooked route.
/// No collision hooks: a delta here is unclassified, not proof of self damage.
pub unsafe fn tick(boma: *mut BattleObjectModuleAccessor) {
    if !WATCH_ENABLED.load(Ordering::Relaxed) || WATCH_COUNT.load(Ordering::Relaxed) >= EVENT_LIMIT
        || boma.is_null()
    {
        return;
    }
    if app::utility::get_category(&mut *boma) != *BATTLE_OBJECT_CATEGORY_FIGHTER { return; }
    let boid = (*boma).battle_object_id;
    if app::sv_battle_object::module_accessor(boid) != boma { return; }
    let Some(current) = damage(boma) else { return; };
    let previous = {
        let Some(mut entries) = WATCH_STATE.try_lock() else { return; };
        if let Some(entry) = entries.iter_mut().find(|entry| entry.boid == boid && entry.accessor == boma as usize) {
            let previous = entry.damage;
            entry.damage = current;
            previous
        } else {
            if entries.len() >= 32 { entries.remove(0); }
            entries.push(WatchEntry { boid, accessor: boma as usize, damage: current });
            return;
        }
    };
    if previous == current { return; }
    let Some(context) = Context::read(boma) else { return; };
    let Some(seq) = reserve(&WATCH_COUNT, "watch_unclassified") else { return; };
    diag::note(format!("SELF_DAMAGE source=watch_unclassified seq={seq} {} before={previous} after={current} delta={}", context.label(), current - previous));
}

pub fn install() {
    if smash_utils::subsystem_disabled("selfdamage") { return; }
    let Ok(config) = std::fs::read_to_string(MARKER) else { return; };
    if !super::ver::is_13_0_5() {
        diag::note("SELF_DAMAGE skipped: only Smash 13.0.5 is supported");
        return;
    }
    let modes = parse_modes(&config);
    if modes == 0 {
        diag::note("SELF_DAMAGE skipped: marker has no recognized modes");
        return;
    }
    // Each detour can be tested independently. The default installation is unchanged.
    if modes & ACMD != 0 {
        skyline::println!("[SLight] SELF_DAMAGE installing acmd");
        skyline::install_hook!(hook_ft_add_damage);
        diag::note("SELF_DAMAGE installed acmd");
    }
    if modes & NATIVE != 0 {
        skyline::println!("[SLight] SELF_DAMAGE installing native_wrapper");
        skyline::install_hook!(hook_add_damage);
        diag::note("SELF_DAMAGE installed native_wrapper");
    }
    if modes & PARAMS != 0 {
        skyline::println!("[SLight] SELF_DAMAGE installing params");
        skyline::install_hooks!(hook_param_float, hook_param_int);
        diag::note("SELF_DAMAGE installed params");
    }
    WATCH_ENABLED.store(modes & WATCH != 0, Ordering::Relaxed);
    if modes & WATCH != 0 { diag::note("SELF_DAMAGE enabled watch_unclassified"); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modes_are_explicit_and_independent() {
        assert_eq!(parse_modes(""), 0);
        assert_eq!(parse_modes("acmd, native\nwatch"), ACMD | NATIVE | WATCH);
        assert_eq!(parse_modes("params"), PARAMS);
        assert_eq!(parse_modes("acmdx nativeoff"), 0);
    }
    #[test]
    fn parameter_trace_is_exact_and_typed() {
        assert_eq!(recoil_param(0x0f899192aa, 0x1ba36e915b, false), Some("special_n_charge_max_damage"));
        assert_eq!(recoil_param(0x0f899192aa, 0x1ba36e915b, true), None);
        assert_eq!(recoil_param(0, 0x1ba36e915b, false), None);
        assert_eq!(recoil_param(0x1018dfb2f4, 0x0fb15550ff, false), Some("hold_max_damage"));
        assert_eq!(recoil_param(0x0fea97fe73, 0x11db8f2b03, false), Some("explosion_damage_"));
        assert_eq!(recoil_param(0x0fea97fe73, 0x206edc2d9e, true), Some("explosion_damage_interval_frame_"));
    }
}
