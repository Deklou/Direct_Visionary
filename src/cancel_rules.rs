//! Editable cancel configuration. Mario Jab 1 -> Grab is the first supported target.

use serde::{Deserialize, Serialize};

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
            enabled: true,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let rule = CancelRule::default();
        assert!(rule.valid());
        let json = serde_json::to_string(&rule).unwrap();
        assert_eq!(serde_json::from_str::<CancelRule>(&json).unwrap(), rule);
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
