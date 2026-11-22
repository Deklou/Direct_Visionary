//! Opt-in, per-fighter/motion cancels using native ground transition checks.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use smash::app::lua_bind::{AttackModule, ControlModule, MotionModule, StatusModule, WorkModule};
use smash::app::BattleObjectModuleAccessor;
use smash::lib::lua_const::{
    BATTLE_OBJECT_CATEGORY_FIGHTER, COLLISION_KIND_MASK_HIT, COLLISION_KIND_MASK_SHIELD,
    FIGHTER_PAD_CMD_CAT1_FLAG_CATCH,
    FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_S3, FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_HI3,
    FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_LW3, FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_S4,
    FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_HI4, FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_LW4,
    FIGHTER_PAD_CMD_CAT1_FLAG_SPECIAL_N, FIGHTER_PAD_CMD_CAT1_FLAG_SPECIAL_S,
    FIGHTER_PAD_CMD_CAT1_FLAG_SPECIAL_HI, FIGHTER_PAD_CMD_CAT1_FLAG_SPECIAL_LW,
    FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO, FIGHTER_STATUS_KIND_ATTACK,
    FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_CATCH,
    FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_S3,
    FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_HI3,
    FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_LW3,
    FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_S4_START,
    FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_HI4_START,
    FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_LW4_START,
    FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_N,
    FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_S,
    FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_HI,
    FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_LW,
    SITUATION_KIND_GROUND,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelTarget {
    #[default]
    Grab,
    TiltF,
    TiltU,
    TiltD,
    SmashF,
    SmashU,
    SmashD,
    SpecialN,
    SpecialS,
    SpecialHi,
    SpecialLw,
}

impl CancelTarget {
    /// Native CAT1 commands, not motion hashes or forced statuses.
    unsafe fn command_flag(self) -> u32 {
        (match self {
            Self::Grab => *FIGHTER_PAD_CMD_CAT1_FLAG_CATCH,
            Self::TiltF => *FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_S3,
            Self::TiltU => *FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_HI3,
            Self::TiltD => *FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_LW3,
            Self::SmashF => *FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_S4,
            Self::SmashU => *FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_HI4,
            Self::SmashD => *FIGHTER_PAD_CMD_CAT1_FLAG_ATTACK_LW4,
            Self::SpecialN => *FIGHTER_PAD_CMD_CAT1_FLAG_SPECIAL_N,
            Self::SpecialS => *FIGHTER_PAD_CMD_CAT1_FLAG_SPECIAL_S,
            Self::SpecialHi => *FIGHTER_PAD_CMD_CAT1_FLAG_SPECIAL_HI,
            Self::SpecialLw => *FIGHTER_PAD_CMD_CAT1_FLAG_SPECIAL_LW,
        }) as u32
    }

    /// Only unblock this target's specific native transition term.
    unsafe fn matches_transition_term(self, term: i32) -> bool {
        term == match self {
            Self::Grab => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_CATCH,
            Self::TiltF => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_S3,
            Self::TiltU => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_HI3,
            Self::TiltD => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_LW3,
            Self::SmashF => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_S4_START,
            Self::SmashU => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_HI4_START,
            Self::SmashD => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_ATTACK_LW4_START,
            Self::SpecialN => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_N,
            Self::SpecialS => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_S,
            Self::SpecialHi => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_HI,
            Self::SpecialLw => *FIGHTER_STATUS_TRANSITION_TERM_ID_CONT_SPECIAL_LW,
        }
    }
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

/// Match every authoring rule for this fighter/motion. Game calls never run under RULES.
unsafe fn matching_rule(
    boma: *mut BattleObjectModuleAccessor,
    transition_term: Option<i32>,
) -> bool {
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

    // Avoid native contact / combo queries unless the selected move actually uses them.
    let (needs_combo, needs_contact) = {
        let Some(rules) = RULES.try_read() else {
            return false;
        };
        let mut relevant = false;
        let mut combo = false;
        let mut contact = false;
        for rule in rules.iter().filter(|r| r.motion == motion && r.fighter == fighter) {
            if transition_term.map_or(false, |term| !rule.target.matches_transition_term(term)) {
                continue;
            }
            relevant = true;
            combo |= rule.sync_combo;
            contact |= rule.condition != CancelCondition::Anytime;
        }
        if !relevant {
            return false;
        }
        (combo, contact)
    };

    let commands = ControlModule::get_command_flag_cat(boma, 0) as u32;
    let frame = MotionModule::frame(boma);
    let combo_allowed = needs_combo
        && StatusModule::status_kind(boma) == *FIGHTER_STATUS_KIND_ATTACK
        && WorkModule::is_flag(boma, *FIGHTER_STATUS_ATTACK_FLAG_ENABLE_COMBO);
    let (hit, shield) = if needs_contact {
        (
            AttackModule::is_infliction_status(boma, *COLLISION_KIND_MASK_HIT),
            AttackModule::is_infliction_status(boma, *COLLISION_KIND_MASK_SHIELD),
        )
    } else {
        (false, false)
    };

    let Some(rules) = RULES.try_read() else {
        return false;
    };
    rules.iter().any(|rule| {
        if rule.motion != motion
            || rule.fighter != fighter
            || transition_term.map_or(false, |term| !rule.target.matches_transition_term(term))
            || commands & rule.target.command_flag() == 0
        {
            return false;
        }
        let in_window = if rule.sync_combo {
            combo_allowed
        } else {
            frame >= f32::from(rule.start_frame) && frame <= f32::from(rule.end_frame)
        };
        if !in_window {
            return false;
        }
        match rule.condition {
            CancelCondition::Anytime => true,
            CancelCondition::OnHit => hit,
            CancelCondition::OnShield => shield,
            CancelCondition::OnHitOrShield => hit || shield,
            CancelCondition::OnWhiff => false,
        }
    })
}

#[skyline::hook(replace = smash::app::lua_bind::CancelModule::is_enable_cancel)]
unsafe fn custom_cancel(boma: *mut BattleObjectModuleAccessor) -> bool {
    let vanilla = original!()(boma);
    vanilla || matching_rule(boma, None)
}

#[skyline::hook(replace = smash::app::lua_bind::WorkModule::is_enable_transition_term)]
unsafe fn custom_transition(
    boma: *mut BattleObjectModuleAccessor,
    transition_term: i32,
) -> bool {
    let vanilla = original!()(boma, transition_term);
    vanilla || matching_rule(boma, Some(transition_term))
}

pub fn install() {
    skyline::install_hooks!(custom_cancel, custom_transition);
}
