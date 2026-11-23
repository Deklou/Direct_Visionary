//! Opt-in live overrides for two fighter-wide floats. No version-dependent offsets.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::LazyLock;

use parking_lot::RwLock;
use serde::Deserialize;
use smash::app::lua_bind::WorkModule;
use smash::app::BattleObjectModuleAccessor;
use smash::lib::lua_const::BATTLE_OBJECT_CATEGORY_FIGHTER;

#[derive(Clone, Debug, Deserialize)]
pub struct TraitRule {
    pub fighter: String,
    pub weight: Option<f32>,
    pub air_speed_x_stable: Option<f32>,
}

const MAX_FIGHTERS: usize = 96;
static RULES: RwLock<Vec<TraitRule>> = RwLock::new(Vec::new());
static HAS_RULES: AtomicBool = AtomicBool::new(false);
static WEIGHT_HASH: LazyLock<u64> = LazyLock::new(|| smash::hash40("weight"));
static AIR_SPEED_HASH: LazyLock<u64> = LazyLock::new(|| smash::hash40("air_speed_x_stable"));

// Bounded read-only probe. Hot hooks update atomics; diag is emitted on the game-frame path.
static PROBE_FRAMES: AtomicU64 = AtomicU64::new(0);
static READ_CALLS: AtomicU64 = AtomicU64::new(0);
static ATTR_READS: AtomicU64 = AtomicU64::new(0);
static OTHER_SUBPARAM: AtomicU64 = AtomicU64::new(0);
static FIGHTER_READS: AtomicU64 = AtomicU64::new(0);
static RULE_MATCHES: AtomicU64 = AtomicU64::new(0);
static OVERRIDES: AtomicU64 = AtomicU64::new(0);
static LAST_KIND_MISS: AtomicI32 = AtomicI32::new(-1);

impl TraitRule {
    fn valid(&self) -> bool {
        !self.fighter.is_empty()
            && self.fighter.len() <= 40
            && self
                .fighter
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            && self.weight.is_none_or(|v| v.is_finite() && v > 0.0 && v <= 1000.0)
            && self
                .air_speed_x_stable
                .is_none_or(|v| v.is_finite() && (0.0..=20.0).contains(&v))
            && (self.weight.is_some() || self.air_speed_x_stable.is_some())
    }
}

/// Entire snapshot replaces the previous one; an empty snapshot restores vanilla.
pub fn set_rules(rules: Vec<TraitRule>) {
    let accepted: Vec<_> = rules
        .into_iter()
        .filter(TraitRule::valid)
        .take(MAX_FIGHTERS)
        .collect();
    let count = accepted.len();
    let detail = accepted
        .iter()
        .take(3)
        .map(|rule| {
            format!(
                "{} weight={:?} air={:?}",
                rule.fighter, rule.weight, rule.air_speed_x_stable
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    *RULES.write() = accepted;
    for counter in [
        &PROBE_FRAMES,
        &READ_CALLS,
        &ATTR_READS,
        &OTHER_SUBPARAM,
        &FIGHTER_READS,
        &RULE_MATCHES,
        &OVERRIDES,
    ] {
        counter.store(0, Ordering::Relaxed);
    }
    LAST_KIND_MISS.store(-1, Ordering::Relaxed);
    HAS_RULES.store(count != 0, Ordering::Release);
    crate::slight::diag::note(format!("trait rules applied: {count} {detail}"));
}

/// Emit a few summaries after each update, never from the hot WorkModule hook.
pub fn report() {
    if !HAS_RULES.load(Ordering::Acquire) {
        return;
    }
    let frame = PROBE_FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    if frame % 240 != 0 || frame > 960 {
        return;
    }
    let kind = LAST_KIND_MISS.load(Ordering::Relaxed);
    let missed_fighter = if kind >= 0 {
        crate::slight::slight_consts::fighters::game_kind_name(kind).unwrap_or("unknown")
    } else {
        "none"
    };
    crate::slight::diag::note(format!(
        "TRAITS probe frame={frame} float_calls={} attribute_reads={} other_subparam={} fighter_reads={} matching_rules={} overridden={} unmatched_kind={missed_fighter}",
        READ_CALLS.load(Ordering::Relaxed),
        ATTR_READS.load(Ordering::Relaxed),
        OTHER_SUBPARAM.load(Ordering::Relaxed),
        FIGHTER_READS.load(Ordering::Relaxed),
        RULE_MATCHES.load(Ordering::Relaxed),
        OVERRIDES.load(Ordering::Relaxed),
    ));
}

#[skyline::hook(replace = WorkModule::get_param_float)]
unsafe fn get_param_float_live(
    boma: *mut BattleObjectModuleAccessor,
    param_type: u64,
    param_hash: u64,
) -> f32 {
    let vanilla = original!()(boma, param_type, param_hash);
    if !HAS_RULES.load(Ordering::Acquire) {
        return vanilla;
    }
    READ_CALLS.fetch_add(1, Ordering::Relaxed);
    if boma.is_null() || (param_type != *WEIGHT_HASH && param_type != *AIR_SPEED_HASH) {
        return vanilla;
    }
    ATTR_READS.fetch_add(1, Ordering::Relaxed);
    if param_hash != 0 {
        OTHER_SUBPARAM.fetch_add(1, Ordering::Relaxed);
        return vanilla;
    }
    if smash::app::utility::get_category(&mut *boma) != *BATTLE_OBJECT_CATEGORY_FIGHTER {
        return vanilla;
    }
    FIGHTER_READS.fetch_add(1, Ordering::Relaxed);

    let kind = smash::app::utility::get_kind(&mut *boma);
    let Some(fighter) = crate::slight::slight_consts::fighters::game_kind_name(kind) else {
        return vanilla;
    };
    let Some(rules) = RULES.try_read() else {
        return vanilla;
    };
    let Some(rule) = rules.iter().find(|rule| rule.fighter == fighter) else {
        LAST_KIND_MISS.store(kind, Ordering::Relaxed);
        return vanilla;
    };
    RULE_MATCHES.fetch_add(1, Ordering::Relaxed);

    let value = if param_type == *WEIGHT_HASH {
        rule.weight
    } else {
        rule.air_speed_x_stable
    };
    if let Some(value) = value {
        OVERRIDES.fetch_add(1, Ordering::Relaxed);
        value
    } else {
        vanilla
    }
}

pub fn install() {
    // A symbol-based replacement avoids using 13.0.3 function offsets on 13.0.5.
    skyline::install_hook!(get_param_float_live);
    skyline::println!("[SLight] Fighter trait float hook installed (experimental)");
}
