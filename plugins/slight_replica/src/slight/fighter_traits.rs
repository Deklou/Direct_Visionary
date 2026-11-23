//! Opt-in live overrides for two fighter-wide floats. No version-dependent offsets.

use std::sync::atomic::{AtomicBool, Ordering};
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
    *RULES.write() = accepted;
    HAS_RULES.store(count != 0, Ordering::Release);
    crate::slight::diag::note(format!("trait rules applied: {count}"));
}

#[skyline::hook(replace = WorkModule::get_param_float)]
unsafe fn get_param_float_live(
    boma: *mut BattleObjectModuleAccessor,
    param_type: u64,
    param_hash: u64,
) -> f32 {
    let vanilla = original!()(boma, param_type, param_hash);
    if !HAS_RULES.load(Ordering::Acquire)
        || boma.is_null()
        || param_hash != 0
        || (param_type != *WEIGHT_HASH && param_type != *AIR_SPEED_HASH)
        || smash::app::utility::get_category(&mut *boma) != *BATTLE_OBJECT_CATEGORY_FIGHTER
    {
        return vanilla;
    }

    let kind = smash::app::utility::get_kind(&mut *boma);
    let Some(fighter) = crate::slight::slight_consts::fighters::game_kind_name(kind) else {
        return vanilla;
    };
    let Some(rules) = RULES.try_read() else {
        return vanilla;
    };
    let Some(rule) = rules.iter().find(|rule| rule.fighter == fighter) else {
        return vanilla;
    };

    if param_type == *WEIGHT_HASH {
        rule.weight.unwrap_or(vanilla)
    } else {
        rule.air_speed_x_stable.unwrap_or(vanilla)
    }
}

pub fn install() {
    // A symbol-based replacement avoids using 13.0.3 function offsets on 13.0.5.
    skyline::install_hook!(get_param_float_live);
    skyline::println!("[SLight] Fighter trait float hook installed (experimental)");
}
