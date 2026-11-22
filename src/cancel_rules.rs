//! Editable, per-move cancel configuration. No custom transition is enabled by default.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelTarget {
    #[default]
    Grab,
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
        Self::Anytime, Self::OnHit, Self::OnShield, Self::OnHitOrShield,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    pub fn valid(&self) -> bool {
        (1..=60).contains(&self.start_frame)
            && self.start_frame <= self.end_frame
            && self.end_frame <= 60
            && self.condition != CancelCondition::OnWhiff
    }
}


#[derive(Clone, Debug, Serialize)]
pub struct CancelRuleWire {
    pub fighter: String,
    pub motion: u64,
    #[serde(flatten)]
    pub rule: CancelRule,
}

/// Build a stable full replacement. Disabled rules remain in the project, never on the wire.
pub fn enabled_wires(
    store: &HashMap<String, HashMap<String, CancelRule>>,
) -> Vec<CancelRuleWire> {
    let mut rules: Vec<_> = store
        .iter()
        .flat_map(|(fighter, moves)| {
            moves.iter().filter_map(move |(motion, rule)| {
                (rule.enabled && rule.valid()).then(|| CancelRuleWire {
                    fighter: fighter.to_ascii_lowercase(),
                    motion: hash40::hash40(motion).0,
                    rule: rule.clone(),
                })
            })
        })
        .collect();
    rules.sort_by(|a, b| (&a.fighter, a.motion).cmp(&(&b.fighter, b.motion)));
    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let rule = CancelRule::default();
        assert!(rule.valid());
        assert!(!rule.enabled);
        let json = serde_json::to_string(&rule).unwrap();
        assert_eq!(serde_json::from_str::<CancelRule>(&json).unwrap(), rule);
    }

    #[test]
    fn only_explicitly_enabled_moves_are_sent() {
        let mut mario = HashMap::new();
        mario.insert("attack_11".to_string(), CancelRule::default());
        let mut project = HashMap::new();
        project.insert("mario".to_string(), mario);
        assert!(enabled_wires(&project).is_empty());

        project.get_mut("mario").unwrap().get_mut("attack_11").unwrap().enabled = true;
        let wires = enabled_wires(&project);
        assert_eq!(wires.len(), 1);
        assert_eq!(wires[0].motion, hash40::hash40("attack_11").0);
        let json = serde_json::to_value(&wires).unwrap();
        assert_eq!(json[0]["fighter"], "mario");
        assert_eq!(json[0]["enabled"], true);
        assert_eq!(json[0]["start_frame"], 4);
    }

    #[test]
    fn invalid_rules_are_rejected() {
        let mut rule = CancelRule::default();
        rule.condition = CancelCondition::OnWhiff;
        assert!(!rule.valid());
        rule.condition = CancelCondition::Anytime;
        rule.start_frame = 12;
        rule.end_frame = 10;
        assert!(!rule.valid());
    }
}
