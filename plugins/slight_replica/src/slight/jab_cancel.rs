//! Test ciblé : autoriser la vérification de cancel uniquement pour Mario jab 1 → grab.

use smash::app::lua_bind::{CancelModule, ControlModule, MotionModule, StatusModule};
use smash::app::BattleObjectModuleAccessor;
use smash::lib::lua_const::{
    BATTLE_OBJECT_CATEGORY_FIGHTER, FIGHTER_KIND_MARIO, FIGHTER_PAD_CMD_CAT1_FLAG_CATCH,
    FIGHTER_STATUS_KIND_ATTACK,
};
use smash::phx::Hash40;

const START_FRAME: f32 = 4.0;
const END_FRAME: f32 = 10.0;

#[skyline::hook(replace = smash::app::lua_bind::CancelModule::is_enable_cancel)]
unsafe fn mario_jab_grab_cancel(boma: *mut BattleObjectModuleAccessor) -> bool {
    // Ne modifie jamais un cancel déjà autorisé par le jeu.
    if original!()(boma) {
        return true;
    }
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

    // Le moteur reste responsable de la transition vers Catch ; on ne force pas de statut.
    let commands = ControlModule::get_command_flag_cat(boma, 0) as u32;
    commands & (*FIGHTER_PAD_CMD_CAT1_FLAG_CATCH as u32) != 0
}

pub fn install() {
    skyline::install_hooks!(mario_jab_grab_cancel);
}
