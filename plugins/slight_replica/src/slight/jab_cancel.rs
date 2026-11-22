//! Prototype Mario jab 1 → grab, avec une trace temporaire bornée pour le diagnostic.

use std::sync::atomic::{AtomicU32, Ordering};

use smash::app::lua_bind::{ControlModule, MotionModule, StatusModule};
use smash::app::BattleObjectModuleAccessor;
use smash::lib::lua_const::{
    BATTLE_OBJECT_CATEGORY_FIGHTER, CONTROL_PAD_BUTTON_CATCH, FIGHTER_KIND_MARIO,
    FIGHTER_PAD_CMD_CAT1_FLAG_CATCH, FIGHTER_STATUS_KIND_ATTACK,
};
use smash::phx::Hash40;

const START_FRAME: f32 = 4.0;
const END_FRAME: f32 = 10.0;
static TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

#[skyline::hook(replace = smash::app::lua_bind::CancelModule::is_enable_cancel)]
unsafe fn mario_jab_grab_cancel(boma: *mut BattleObjectModuleAccessor) -> bool {
    let vanilla = original!()(boma);
    if boma.is_null()
        || smash::app::utility::get_category(&mut *boma) != *BATTLE_OBJECT_CATEGORY_FIGHTER
        || smash::app::utility::get_kind(&mut *boma) != *FIGHTER_KIND_MARIO
    {
        return vanilla;
    }

    let status = StatusModule::status_kind(boma);
    if status != *FIGHTER_STATUS_KIND_ATTACK {
        return vanilla;
    }
    let motion = MotionModule::motion_kind(boma);
    let frame = MotionModule::frame(boma);
    let is_jab1 = motion == Hash40::new("attack_11").hash;
    let in_window = is_jab1 && (START_FRAME..=END_FRAME).contains(&frame);
    let commands = ControlModule::get_command_flag_cat(boma, 0) as u32;
    let cmd_catch = commands & (*FIGHTER_PAD_CMD_CAT1_FLAG_CATCH as u32) != 0;
    let button_catch = ControlModule::check_button_on(boma, *CONTROL_PAD_BUTTON_CATCH);
    let granted = in_window && cmd_catch;

    if (2.0..=14.0).contains(&frame) && TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 64 {
        skyline::println!(
            "[jab-cancel] frame={:.1} motion={:x} jab1={} vanilla={} cmd_catch={} button_catch={} cat0={:x} granted={}",
            frame, motion, is_jab1, vanilla, cmd_catch, button_catch, commands, granted
        );
    }

    // Les cancels vanilla restent inchangés ; le prototype ne force aucun statut.
    vanilla || granted
}

pub fn install() {
    skyline::install_hooks!(mario_jab_grab_cancel);
}
