//! Prototype Mario jab 1 → grab : autorisations temporaires sans mutation des transitions.

use std::sync::atomic::{AtomicU32, Ordering};

use smash::app::lua_bind::{ControlModule, MotionModule, StatusModule};
use smash::app::BattleObjectModuleAccessor;
use smash::lib::lua_const::{
    BATTLE_OBJECT_CATEGORY_FIGHTER, FIGHTER_KIND_MARIO, FIGHTER_PAD_CMD_CAT1_FLAG_CATCH,
    FIGHTER_STATUS_KIND_ATTACK, FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_CATCH,
};
use smash::phx::Hash40;

const START_FRAME: f32 = 4.0;
const END_FRAME: f32 = 10.0;
static CANCEL_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static CATCH_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

unsafe fn is_mario_jab_grab_window(boma: *mut BattleObjectModuleAccessor) -> bool {
    if boma.is_null()
        || smash::app::utility::get_category(&mut *boma) != *BATTLE_OBJECT_CATEGORY_FIGHTER
        || smash::app::utility::get_kind(&mut *boma) != *FIGHTER_KIND_MARIO
        || StatusModule::status_kind(boma) != *FIGHTER_STATUS_KIND_ATTACK
        || MotionModule::motion_kind(boma) != Hash40::new("attack_11").hash
    {
        return false;
    }

    let frame = MotionModule::frame(boma);
    if !(START_FRAME..=END_FRAME).contains(&frame) {
        return false;
    }

    let commands = ControlModule::get_command_flag_cat(boma, 0) as u32;
    commands & (*FIGHTER_PAD_CMD_CAT1_FLAG_CATCH as u32) != 0
}

#[skyline::hook(replace = smash::app::lua_bind::CancelModule::is_enable_cancel)]
unsafe fn mario_jab_grab_cancel(boma: *mut BattleObjectModuleAccessor) -> bool {
    let vanilla = original!()(boma);
    if vanilla || !is_mario_jab_grab_window(boma) {
        return vanilla;
    }

    if CANCEL_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 16 {
        skyline::println!(
            "[jab-cancel] cancel_override frame={:.1}",
            MotionModule::frame(boma)
        );
    }
    true
}

#[skyline::hook(replace = smash::app::lua_bind::WorkModule::is_enable_transition_term)]
unsafe fn mario_jab_grab_transition(
    boma: *mut BattleObjectModuleAccessor,
    transition_term: i32,
) -> bool {
    let vanilla = original!()(boma, transition_term);
    if vanilla
        || transition_term != *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_CATCH
        || !is_mario_jab_grab_window(boma)
    {
        return vanilla;
    }

    if CATCH_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 16 {
        skyline::println!(
            "[jab-cancel] catch_term_override frame={:.1}",
            MotionModule::frame(boma)
        );
    }
    true
}

pub fn install() {
    skyline::install_hooks!(mario_jab_grab_cancel, mario_jab_grab_transition);
}
