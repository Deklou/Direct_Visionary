//! Opt-in, per-fighter and per-motion Grab cancels. Mario Jab 1 is the reference test.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use smash::app::lua_bind::{AttackModule, ControlModule, MotionModule, StatusModule, WorkModule};
use smash::app::BattleObjectModuleAccessor;
use smash::lib::lua_const::{
    BATTLE_OBJECT_CATEGORY_FIGHTER, COLLISION_KIND_MASK_HIT, COLLISION_KIND_MASK_SHIELD,
    FIGHTER_PAD_CMD_CAT1_FLAG_CATCH, FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO,
    FIGHTER_STATUS_KIND_ATTACK, FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_CATCH,
    SITUATION_KIND_GROUND,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelTarget {
    #[default]
    Grab,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelCondition {
    #[default]
    Anytime,
    OnHit,
    OnShield,
    OnHitOrShield,
    OnWhiff,
}

/// Wire representation. The fighter is the game kind-name (e.g. mario); motion is Hash40.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct CancelRule {
    pub fighter: String,
    pub motion: u64,
    pub enabled: bool,
    pub target: CancelTarget,
    pub condition: CancelCondition,
    pub start_frame: u8,
    pub end_frame: u8,
    pub sync_combo: bool,
}

impl Default for CancelRule {
    fn default() -> Self {
        Self {
            fighter: String::new(),
            motion: 0,
            enabled: false,
            target: CancelTarget::Grab,
            condition: CancelCondition::Anytime,
            start_frame: 4,
            end_frame: 10,
            sync_combo: false,
        }
    }
}

impl CancelRule {
    fn valid(&self) -> bool {
        self.enabled
            && !self.fighter.is_empty()
            && self.fighter.len() <= 40
            && self.fighter.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            && self.motion != 0
            && self.target == CancelTarget::Grab
            && self.condition != CancelCondition::OnWhiff
            && (1..=60).contains(&self.start_frame)
            && self.start_frame <= self.end_frame
            && self.end_frame <= 60
    }
}

const MAX_RULES: usize = 512;
static RULES: parking_lot::RwLock<Vec<CancelRule>> = parking_lot::RwLock::new(Vec::new());
static HAS_RULES: AtomicBool = AtomicBool::new(false);

/// Called by the existing TCP handler on the game thread, never in the hook.
pub fn set_rules(rules: Vec<CancelRule>) {
    let accepted: Vec<_> = rules.into_iter().filter(CancelRule::valid).take(MAX_RULES).collect();
    let count = accepted.len();
    *RULES.write() = accepted;
    HAS_RULES.store(count != 0, Ordering::Release);
    crate::slight::diag::note(format!("cancel rules applied: {count}"));
}

/// Copies tiny rule fields under try_read; native calls happen after the guard is dropped.
unsafe fn matching_rule(boma: *mut BattleObjectModuleAccessor) -> bool {
    if !HAS_RULES.load(Ordering::Acquire)
        || boma.is_null()
        || smash::app::utility::get_category(&mut *boma) != *BATTLE_OBJECT_CATEGORY_FIGHTER
        || StatusModule::situation_kind(boma) != *SITUATION_KIND_GROUND
    {
        return false;
    }
    let kind = smash::app::utility::get_kind(&mut *boma);
    let Some(fighter) = crate::slight::slight_consts::fighters::game_kind_name(kind) else {
        return false;
    };
    let motion = MotionModule::motion_kind(boma);
    let selected = {
        let Some(rules) = RULES.try_read() else {
            return false;
        };
        rules.iter()
            .find(|rule| rule.motion == motion && rule.fighter == fighter)
            .map(|rule| (rule.condition, rule.start_frame, rule.end_frame, rule.sync_combo))
    };
    let Some((condition, start, end, sync_combo)) = selected else {
        return false;
    };
    let frame = MotionModule::frame(boma);
    let in_window = if sync_combo {
        StatusModule::status_kind(boma) == *FIGHTER_STATUS_KIND_ATTACK
            && WorkModule::is_flag(boma, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO)
    } else {
        frame >= f32::from(start) && frame <= f32::from(end)
    };
    if !in_window {
        return false;
    }
    let commands = ControlModule::get_command_flag_cat(boma, 0) as u32;
    if commands & (*FIGHTER_PAD_CMD_CAT1_FLAG_CATCH as u32) == 0 {
        return false;
    }
    if condition == CancelCondition::Anytime {
        return true;
    }
    let hit = AttackModule::is_infliction_status(boma, *COLLISION_KIND_MASK_HIT);
    let shield = AttackModule::is_infliction_status(boma, *COLLISION_KIND_MASK_SHIELD);
    match condition {
        CancelCondition::Anytime => true,
        CancelCondition::OnHit => hit,
        CancelCondition::OnShield => shield,
        CancelCondition::OnHitOrShield => hit || shield,
        CancelCondition::OnWhiff => false,
    }
}

#[skyline::hook(replace = smash::app::lua_bind::CancelModule::is_enable_cancel)]
unsafe fn custom_grab_cancel(boma: *mut BattleObjectModuleAccessor) -> bool {
    let vanilla = original!()(boma);
    vanilla || matching_rule(boma)
}

#[skyline::hook(replace = smash::app::lua_bind::WorkModule::is_enable_transition_term)]
unsafe fn custom_grab_transition(
    boma: *mut BattleObjectModuleAccessor,
    transition_term: i32,
) -> bool {
    let vanilla = original!()(boma, transition_term);
    vanilla || (transition_term == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_CATCH
        && matching_rule(boma))
}

pub fn install() {
    skyline::install_hooks!(custom_grab_cancel, custom_grab_transition);
}
