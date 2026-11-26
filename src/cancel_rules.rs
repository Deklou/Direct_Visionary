//! Per-move custom cancel rules. No custom cancel is enabled by default.

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashMap};
use crate::roster::{RosterBacking, RosterKey};
use crate::roster::edit_scope::{self, EditScope};
use crate::roster::index::RosterIndex;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    Jump,
    Walk,
    Dash,
    Crouch,
    Shield,
}

impl CancelTarget {
    pub const GRABS: [Self; 1] = [Self::Grab];
    pub const TILTS: [Self; 3] = [Self::TiltF, Self::TiltU, Self::TiltD];
    pub const SMASHES: [Self; 3] = [Self::SmashF, Self::SmashU, Self::SmashD];
    pub const SPECIALS: [Self; 4] =
        [Self::SpecialN, Self::SpecialS, Self::SpecialHi, Self::SpecialLw];
    pub const MOVEMENT: [Self; 4] = [Self::Jump, Self::Walk, Self::Dash, Self::Crouch];
    pub const DEFENSE: [Self; 1] = [Self::Shield];

    pub fn label(self) -> &'static str {
        match self {
            Self::Grab => "Standing Grab",
            Self::TiltF => "Forward Tilt",
            Self::TiltU => "Up Tilt",
            Self::TiltD => "Down Tilt",
            Self::SmashF => "Forward Smash",
            Self::SmashU => "Up Smash",
            Self::SmashD => "Down Smash",
            Self::SpecialN => "Neutral Special",
            Self::SpecialS => "Side Special",
            Self::SpecialHi => "Up Special",
            Self::SpecialLw => "Down Special",
            Self::Jump => "Jump",
            Self::Walk => "Walk",
            Self::Dash => "Dash",
            Self::Crouch => "Crouch",
            Self::Shield => "Shield",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelCondition {
    #[default]
    Anytime,
    OnHit,
    OnShield,
    OnHitOrShield,
    OnWhiff,
}

impl CancelCondition {
    pub const SUPPORTED: [Self; 4] = [
        Self::Anytime,
        Self::OnHit,
        Self::OnShield,
        Self::OnHitOrShield,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Anytime => "Anytime",
            Self::OnHit => "On Hit",
            Self::OnShield => "On Shield",
            Self::OnHitOrShield => "On Hit or Shield",
            Self::OnWhiff => "On Whiff (not supported yet)",
        }
    }
}

/// Saved owner and exact costume scope. A slot clone also retains its authored CSS identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelBinding {
    pub scope: EditScope,
    pub authored_name_id: Option<String>,
}

pub fn resolve_binding(
    roster: &RosterIndex,
    fighter: &str,
    key: &RosterKey,
    base_slots: &BTreeMap<String, Vec<u8>>,
) -> Option<CancelBinding> {
    let scope = edit_scope::resolve(roster, key, base_slots).ok()?;
    if !scope.fighter.eq_ignore_ascii_case(fighter) {
        return None;
    }
    let entry = roster.entries.iter().find(|entry| &entry.key == key)?;
    let authored_name_id = match &entry.backing {
        RosterBacking::Fighter => None,
        RosterBacking::SlotClone { .. } => Some(
            entry.name_id.as_ref().filter(|name| !name.is_empty())?.clone()
        ),
    };
    Some(CancelBinding { scope, authored_name_id })
}

/// One window and one condition, authorizing any of the selected targets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CancelRule {
    pub enabled: bool,
    pub targets: Vec<CancelTarget>,
    pub condition: CancelCondition,
    pub start_frame: u8,
    pub end_frame: u8,
    pub sync_combo: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<CancelBinding>,
}

impl Default for CancelRule {
    fn default() -> Self {
        Self {
            enabled: false,
            targets: vec![CancelTarget::Grab],
            condition: CancelCondition::Anytime,
            start_frame: 4,
            end_frame: 10,
            sync_combo: false,
            binding: None,
        }
    }
}

impl CancelRule {
    pub fn valid(&self) -> bool {
        !self.targets.is_empty()
            && (1..=60).contains(&self.start_frame)
            && self.start_frame <= self.end_frame
            && self.end_frame <= 60
            && self.condition != CancelCondition::OnWhiff
    }
}

/// Each target is expanded to the old plugin wire shape: one transition per entry.
#[derive(Clone, Debug, Serialize)]
pub struct CancelRuleWire {
    pub fighter: String,
    pub motion: u64,
    pub target: CancelTarget,
    pub enabled: bool,
    pub condition: CancelCondition,
    pub start_frame: u8,
    pub end_frame: u8,
    pub sync_combo: bool,
    /// Absent for legacy fighter-wide rules. Explicit slots require a capable plugin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub costume_slots: Option<Vec<u8>>,
}

pub fn enabled_wires(
    store: &HashMap<String, HashMap<String, Vec<CancelRule>>>,
) -> Vec<CancelRuleWire> {
    let mut wires = Vec::new();
    for (fighter, moves) in store {
        for (motion, rules) in moves {
            let motion_hash = hash40::hash40(motion).0;
            for rule in rules.iter().filter(|rule| rule.enabled && rule.valid()) {
                for (target_index, &target) in rule.targets.iter().enumerate() {
                    // One authoring rule must not send the same action twice.
                    if rule.targets[..target_index].contains(&target) {
                        continue;
                    }
                    wires.push(CancelRuleWire {
                        fighter: fighter.to_ascii_lowercase(),
                        motion: motion_hash,
                        target,
                        enabled: true,
                        condition: rule.condition,
                        start_frame: rule.start_frame,
                        end_frame: rule.end_frame,
                        sync_combo: rule.sync_combo,
                        costume_slots: None,
                    });
                }
            }
        }
    }
    // Stable order between publication calls; order within one move stays authored.
    wires.sort_by(|a, b| (&a.fighter, a.motion).cmp(&(&b.fighter, b.motion)));
    wires
}

/// Build a scoped cancel payload for one explicitly resolved character/move.
/// The caller must verify the saved binding and plugin capability before publishing.
pub fn scoped_wires(
    fighter: &str,
    motion: &str,
    rules: &[CancelRule],
    scope: &crate::roster::edit_scope::EditScope,
) -> Result<Vec<CancelRuleWire>, &'static str> {
    if !scope.fighter.eq_ignore_ascii_case(fighter) || scope.slots.is_empty() {
        return Err("cancel target does not match the resolved fighter/costumes");
    }
    let store = HashMap::from([(
        fighter.to_string(),
        HashMap::from([(motion.to_string(), rules.to_vec())]),
    )]);
    let mut wires = enabled_wires(&store);
    for wire in &mut wires {
        wire.costume_slots = Some(scope.slots.clone());
    }
    Ok(wires)
}

/// Editing one logical character must not remove another character's rules on the same move.
pub fn replace_bound_rules(
    all: &mut Vec<CancelRule>,
    binding: &CancelBinding,
    edited: Vec<CancelRule>,
) {
    all.retain(|rule| rule.binding.as_ref() != Some(binding));
    all.extend(edited.into_iter().map(|mut rule| {
        rule.binding = Some(binding.clone());
        rule
    }));
}

/// Only publish rules whose saved identity still resolves to the same costume set.
pub fn resolved_wires(
    store: &HashMap<String, HashMap<String, Vec<CancelRule>>>,
    mut resolve: impl FnMut(&str, &RosterKey) -> Option<CancelBinding>,
) -> Vec<CancelRuleWire> {
    let mut cache = BTreeMap::new();
    let mut wires = Vec::new();
    for (fighter, moves) in store {
        for (motion, rules) in moves {
            for rule in rules.iter().filter(|rule| rule.enabled && rule.valid()) {
                let Some(saved) = rule.binding.as_ref() else {
                    continue;
                };
                let current = cache
                    .entry((fighter.clone(), saved.scope.key.clone()))
                    .or_insert_with(|| resolve(fighter, &saved.scope.key));
                if current.as_ref() != Some(saved) {
                    continue;
                }
                if let Ok(scoped) = scoped_wires(
                    fighter, motion, std::slice::from_ref(rule), &saved.scope,
                ) {
                    wires.extend(scoped);
                }
            }
        }
    }
    wires.sort_by(|a, b| {
        (&a.fighter, a.motion, &a.costume_slots)
            .cmp(&(&b.fighter, b.motion, &b.costume_slots))
    });
    wires
}

#[derive(Deserialize)]
#[serde(default)]
struct LegacyRule {
    enabled: bool,
    target: CancelTarget,
    condition: CancelCondition,
    start_frame: u8,
    end_frame: u8,
    sync_combo: bool,
}

impl Default for LegacyRule {
    fn default() -> Self {
        let rule = CancelRule::default();
        Self {
            enabled: rule.enabled,
            target: CancelTarget::Grab,
            condition: rule.condition,
            start_frame: rule.start_frame,
            end_frame: rule.end_frame,
            sync_combo: rule.sync_combo,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SavedRules {
    Multiple(Vec<CancelRule>),
    Legacy(LegacyRule),
}

/// Old projects stored one rule object per motion. Convert it to a one-item list.
pub fn deserialize_rule_map<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, Vec<CancelRule>>, D::Error>
where
    D: Deserializer<'de>,
{
    let old_or_new = HashMap::<String, SavedRules>::deserialize(deserializer)?;
    Ok(old_or_new
        .into_iter()
        .map(|(motion, rules)| {
            let list = match rules {
                SavedRules::Multiple(rules) => rules,
                SavedRules::Legacy(rule) => vec![CancelRule {
                    enabled: rule.enabled,
                    targets: vec![rule.target],
                    condition: rule.condition,
                    start_frame: rule.start_frame,
                    end_frame: rule.end_frame,
                    sync_combo: rule.sync_combo,
                    binding: None,
                }],
            };
            (motion, list)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(slot: Option<u8>, slots: Vec<u8>, name: Option<&str>) -> CancelBinding {
        CancelBinding {
            scope: EditScope {
                key: slot.map_or_else(|| RosterKey::fighter("mario"), |slot| RosterKey::slot("mario", slot)),
                fighter: "mario".into(),
                slots,
            },
            authored_name_id: name.map(str::to_owned),
        }
    }

    fn bound_rule(binding: Option<CancelBinding>, target: CancelTarget) -> CancelRule {
        CancelRule { enabled: true, binding, targets: vec![target], ..Default::default() }
    }

    #[test]
    fn scoped_cancel_publication_keeps_three_logical_owners_separate() {
        let vanilla = binding(None, (0..8).collect(), None);
        let first = binding(Some(8), vec![8, 9], Some("first"));
        let second = binding(Some(12), vec![12, 13], Some("second"));
        let store = HashMap::from([("mario".into(), HashMap::from([(
            "attack_11".into(), vec![
                bound_rule(Some(vanilla.clone()), CancelTarget::Grab),
                bound_rule(Some(first.clone()), CancelTarget::Jump),
                bound_rule(Some(second.clone()), CancelTarget::Shield),
                bound_rule(None, CancelTarget::TiltU),
            ],
        )]))]);
        let wires = resolved_wires(&store, |_, key| {
            [&vanilla, &first, &second].into_iter()
                .find(|binding| &binding.scope.key == key).cloned()
        });
        assert_eq!(wires.len(), 3);
        assert_eq!(wires[0].costume_slots, Some((0..8).collect()));
        assert_eq!(wires[0].target, CancelTarget::Grab);
        assert_eq!(wires[1].costume_slots, Some(vec![8, 9]));
        assert_eq!(wires[1].target, CancelTarget::Jump);
        assert_eq!(wires[2].costume_slots, Some(vec![12, 13]));
        assert_eq!(wires[2].target, CancelTarget::Shield);
    }

    #[test]
    fn scoped_cancel_publication_refuses_changed_or_missing_identity() {
        let saved = binding(Some(8), vec![8, 9], Some("first"));
        let store = HashMap::from([("mario".into(), HashMap::from([(
            "attack_11".into(), vec![bound_rule(Some(saved.clone()), CancelTarget::Jump)],
        )]))]);
        assert!(resolved_wires(&store, |_, _| None).is_empty());
        let changed_slots = binding(Some(8), vec![8, 10], Some("first"));
        assert!(resolved_wires(&store, |_, _| Some(changed_slots.clone())).is_empty());
        let changed_owner = binding(Some(8), vec![8, 9], Some("replacement"));
        assert!(resolved_wires(&store, |_, _| Some(changed_owner.clone())).is_empty());
        assert_eq!(resolved_wires(&store, |_, _| Some(saved.clone())).len(), 1);
    }

    #[test]
    fn editing_or_removing_one_scoped_cancel_keeps_the_other_owners() {
        let vanilla = binding(None, (0..8).collect(), None);
        let first = binding(Some(8), vec![8, 9], Some("first"));
        let second = binding(Some(12), vec![12, 13], Some("second"));
        let original = bound_rule(Some(vanilla), CancelTarget::Grab);
        let other = bound_rule(Some(second), CancelTarget::Shield);
        let legacy = bound_rule(None, CancelTarget::TiltU);
        let mut rules = vec![
            original.clone(),
            bound_rule(Some(first.clone()), CancelTarget::Jump),
            other.clone(),
            legacy.clone(),
        ];
        replace_bound_rules(&mut rules, &first, vec![bound_rule(None, CancelTarget::Dash)]);
        assert_eq!(rules.len(), 4);
        assert_eq!(rules[3].binding.as_ref(), Some(&first));
        assert_eq!(rules[3].targets, vec![CancelTarget::Dash]);
        replace_bound_rules(&mut rules, &first, Vec::new());
        assert_eq!(rules, vec![original, other, legacy]);
    }

    #[test]
    fn scoped_cancel_binding_survives_save_and_reload() {
        let saved = binding(Some(8), vec![8, 9], Some("first"));
        let original = bound_rule(Some(saved), CancelTarget::Jump);
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(serde_json::from_str::<CancelRule>(&json).unwrap(), original);
        let legacy: CancelRule = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(legacy.enabled && legacy.binding.is_none());
    }

    #[derive(Deserialize)]
    struct Stored {
        #[serde(default, deserialize_with = "deserialize_rule_map")]
        cancel_rules: HashMap<String, Vec<CancelRule>>,
    }

    #[test]
    fn old_projects_keep_their_target_and_enabled_state() {
        let json = r#"{
            "cancel_rules": {
                "attack_11": {
                    "enabled": true,
                    "target": "tilt_f",
                    "condition": "on_hit",
                    "start_frame": 6,
                    "end_frame": 11,
                    "sync_combo": false
                }
            }
        }"#;
        let project: Stored = serde_json::from_str(json).unwrap();
        let rule = &project.cancel_rules["attack_11"][0];
        assert_eq!(rule.targets, vec![CancelTarget::TiltF]);
        assert!(rule.enabled);
        assert_eq!(rule.start_frame, 6);
        assert_eq!(rule.condition, CancelCondition::OnHit);
    }

    #[test]
    fn distinct_windows_and_multiple_actions_expand_into_wires() {
        let mut a = CancelRule::default();
        a.enabled = true;
        a.targets = vec![CancelTarget::Grab, CancelTarget::TiltF, CancelTarget::TiltU];
        let mut b = CancelRule::default();
        b.enabled = true;
        b.targets = vec![CancelTarget::SpecialN];
        b.start_frame = 6;
        b.end_frame = 12;
        b.condition = CancelCondition::OnHitOrShield;
        let store = HashMap::from([(
            "mario".to_string(),
            HashMap::from([("attack_11".to_string(), vec![a, b])]),
        )]);
        let wires = enabled_wires(&store);
        assert_eq!(wires.len(), 4);
        assert_eq!(wires[0].target, CancelTarget::Grab);
        assert_eq!(wires[3].target, CancelTarget::SpecialN);
        assert_eq!(wires[3].condition, CancelCondition::OnHitOrShield);
        assert_eq!(wires[3].start_frame, 6);
        assert_eq!(wires[3].end_frame, 12);
    }

    #[test]
    fn two_windows_for_the_same_action_are_not_merged() {
        let mut a = CancelRule::default();
        a.enabled = true;
        let mut b = a.clone();
        b.start_frame = 8;
        b.end_frame = 16;
        let store = HashMap::from([(
            "mario".to_string(),
            HashMap::from([("attack_11".to_string(), vec![a, b])]),
        )]);
        let wires = enabled_wires(&store);
        assert_eq!(wires.len(), 2);
        assert_eq!(wires[0].target, CancelTarget::Grab);
        assert_eq!(wires[1].target, CancelTarget::Grab);
        assert_eq!(wires[1].start_frame, 8);
    }

    #[test]
    fn movement_and_shield_preserve_wire_names() {
        for (action, expected) in [
            (CancelTarget::Jump, "\"jump\""),
            (CancelTarget::Walk, "\"walk\""),
            (CancelTarget::Dash, "\"dash\""),
            (CancelTarget::Crouch, "\"crouch\""),
            (CancelTarget::Shield, "\"shield\""),
        ] {
            assert_eq!(serde_json::to_string(&action).unwrap(), expected);
            assert_eq!(serde_json::from_str::<CancelTarget>(expected).unwrap(), action);
        }
    }

    #[test]
    fn movement_targets_expand_once_per_action() {
        let mut rule = CancelRule::default();
        rule.enabled = true;
        rule.targets = vec![
            CancelTarget::Walk,
            CancelTarget::Dash,
            CancelTarget::Crouch,
            CancelTarget::Walk,
        ];
        let store = HashMap::from([(
            "mario".to_string(),
            HashMap::from([("attack_11".to_string(), vec![rule])]),
        )]);
        let wires = enabled_wires(&store);
        assert_eq!(wires.len(), 3);
        assert_eq!(
            wires.iter().map(|wire| wire.target).collect::<Vec<_>>(),
            vec![CancelTarget::Walk, CancelTarget::Dash, CancelTarget::Crouch]
        );
    }

    #[test]
    fn cancel_wire_can_limit_one_logical_character_to_multiple_costumes() {
        let mut rule = CancelRule::default();
        rule.enabled = true;
        let scope = crate::roster::edit_scope::EditScope {
            key: crate::roster::RosterKey::slot("mario", 8),
            fighter: "mario".into(),
            slots: vec![8, 9, 10, 11],
        };
        let wires = scoped_wires("Mario", "attack_11", &[rule], &scope).unwrap();
        assert_eq!(wires.len(), 1);
        assert_eq!(wires[0].costume_slots, Some(vec![8, 9, 10, 11]));
        let json = serde_json::to_value(&wires[0]).unwrap();
        assert_eq!(json["costume_slots"], serde_json::json!([8, 9, 10, 11]));
        assert_eq!(json["fighter"], "mario");
    }

    #[test]
    fn cancel_wire_does_not_guess_a_mismatched_or_missing_scope() {
        let scope = crate::roster::edit_scope::EditScope {
            key: crate::roster::RosterKey::slot("mario", 8),
            fighter: "mario".into(),
            slots: vec![8],
        };
        assert!(scoped_wires("luigi", "attack_11", &[], &scope).is_err());
        let empty = crate::roster::edit_scope::EditScope {
            slots: Vec::new(),
            ..scope
        };
        assert!(scoped_wires("mario", "attack_11", &[], &empty).is_err());
    }

    #[test]
    fn old_cancel_wire_stays_without_a_costume_field() {
        let mut rule = CancelRule::default();
        rule.enabled = true;
        let store = HashMap::from([(
            "mario".to_string(),
            HashMap::from([("attack_11".to_string(), vec![rule])]),
        )]);
        let wires = enabled_wires(&store);
        let json = serde_json::to_value(&wires[0]).unwrap();
        assert!(json.get("costume_slots").is_none());
    }

    #[test]
    fn new_rule_is_disabled_and_does_not_publish() {
        let rule = CancelRule::default();
        assert!(!rule.enabled);
        let store = HashMap::from([(
            "mario".to_string(),
            HashMap::from([("attack_11".to_string(), vec![rule])]),
        )]);
        assert!(enabled_wires(&store).is_empty());
    }

    #[test]
    fn project_round_trip_new_format() {
        #[derive(Serialize, Deserialize)]
        struct Project {
            #[serde(default, deserialize_with = "deserialize_rule_map")]
            cancel_rules: HashMap<String, Vec<CancelRule>>,
        }
        let mut rule = CancelRule::default();
        rule.enabled = true;
        rule.targets = vec![CancelTarget::Grab, CancelTarget::TiltF];
        let original = Project {
            cancel_rules: HashMap::from([("attack_11".to_string(), vec![rule])]),
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(original.cancel_rules, parsed.cancel_rules);
    }
}
