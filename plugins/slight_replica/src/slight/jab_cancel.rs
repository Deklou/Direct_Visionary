//! Mario Jab 1 -> Grab: the first verified in-game transition prototype.

use std::sync::atomic::{AtomicBool, Ordering};
use serde::Deserialize;
use smash::app::lua_bind::{AttackModule, ControlModule, MotionModule, StatusModule, WorkModule};
use smash::app::BattleObjectModuleAccessor;
use smash::lib::lua_const::{
    BATTLE_OBJECT_CATEGORY_FIGHTER, COLLISION_KIND_MASK_HIT, COLLISION_KIND_MASK_SHIELD,
    FIGHTER_KIND_MARIO, FIGHTER_PAD_CMD_CAT1_FLAG_CATCH,
    FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO, FIGHTER_STATUS_KIND_ATTACK,
    FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_CATCH,
};
use smash::phx::Hash40;

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

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default)]
pub struct CancelRule {
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
            enabled: true, target: CancelTarget::Grab,
            condition: CancelCondition::Anytime,
            start_frame: 4, end_frame: 10, sync_combo: false,
        }
    }
}

impl CancelRule {
    fn valid(self) -> bool {
        self.enabled && self.target == CancelTarget::Grab
            && self.condition != CancelCondition::OnWhiff
            && (1..=60).contains(&self.start_frame)
            && self.start_frame <= self.end_frame && self.end_frame <= 60
    }
}

static RULES: parking_lot::RwLock<Vec<CancelRule>> = parking_lot::RwLock::new(Vec::new());
static CONFIGURED: AtomicBool = AtomicBool::new(false);

pub fn set_rules(rules: Vec<CancelRule>) {
    let accepted: Vec<CancelRule> = rules.into_iter().filter(|r| r.valid()).take(1).collect();
    let count = accepted.len();
    *RULES.write() = accepted;
    CONFIGURED.store(true, Ordering::Release);
    crate::slight::diag::note(format!("cancel rules applied: {count}"));
}

unsafe fn matching_rule(boma: *mut BattleObjectModuleAccessor) -> Option<CancelRule> {
    if boma.is_null()
        || smash::app::utility::get_category(&mut *boma) != *BATTLE_OBJECT_CATEGORY_FIGHTER
        || smash::app::utility::get_kind(&mut *boma) != *FIGHTER_KIND_MARIO
        || StatusModule::status_kind(boma) != *FIGHTER_STATUS_KIND_ATTACK
        || MotionModule::motion_kind(boma) != Hash40::new("attack_11").hash
    {
        return None;
    }
    let rule = if CONFIGURED.load(Ordering::Acquire) {
        RULES.try_read().and_then(|rules| rules.first().copied())
    } else {
        Some(CancelRule::default())
    }?;
    let frame = MotionModule::frame(boma);
    let in_window = if rule.sync_combo {
        WorkModule::is_flag(boma, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO)
    } else {
        frame >= f32::from(rule.start_frame) && frame <= f32::from(rule.end_frame)
    };
    if !in_window { return None; }
    let commands = ControlModule::get_command_flag_cat(boma, 0) as u32;
    if commands & (*FIGHTER_PAD_CMD_CAT1_FLAG_CATCH as u32) == 0 {
        return None;
    }
    // Keep the already-tested Anytime path free of extra native contact queries.
    if rule.condition == CancelCondition::Anytime {
        return Some(rule);
    }
    let hit = AttackModule::is_infliction_status(boma, *COLLISION_KIND_MASK_HIT);
    let shield = AttackModule::is_infliction_status(boma, *COLLISION_KIND_MASK_SHIELD);
    let allowed = match rule.condition {
        CancelCondition::Anytime => true,
        CancelCondition::OnHit => hit,
        CancelCondition::OnShield => shield,
        CancelCondition::OnHitOrShield => hit || shield,
        CancelCondition::OnWhiff => false,
    };
    allowed.then_some(rule)
}

#[skyline::hook(replace = smash::app::lua_bind::CancelModule::is_enable_cancel)]
unsafe fn mario_jab_grab_cancel(boma: *mut BattleObjectModuleAccessor) -> bool {
    let vanilla = original!()(boma);
    vanilla || matching_rule(boma).is_some()
}

#[skyline::hook(replace = smash::app::lua_bind::WorkModule::is_enable_transition_term)]
unsafe fn mario_jab_grab_transition(
    boma: *mut BattleObjectModuleAccessor,
    transition_term: i32,
) -> bool {
    let vanilla = original!()(boma, transition_term);
    vanilla || (transition_term == *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_CATCH
        && matching_rule(boma).is_some())
}

pub fn install() {
    skyline::install_hooks!(mario_jab_grab_cancel, mario_jab_grab_transition);
}
