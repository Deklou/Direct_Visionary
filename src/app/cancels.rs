//! Per-move cancel UI. The only supported live target is Mario Jab 1 -> Grab.

use super::VisionaryApp;
use crate::cancel_rules::{CancelCondition, CancelRule, CancelTarget};
use egui::Ui;

const FIGHTER: &str = "mario";
const MOTION: &str = "attack_11";

impl VisionaryApp {
    pub(super) fn publish_cancel_rules(&self) {
        let rule = self.cancel_rules_store.get(FIGHTER)
            .and_then(|moves| moves.get(MOTION))
            .cloned().unwrap_or_default();
        if rule.enabled && rule.valid() {
            self.game_link.send_cancel_rules(&[rule]);
        } else {
            self.game_link.send_cancel_rules(&[]);
        }
    }

    pub(super) fn draw_cancel_rules_section(&mut self, ui: &mut Ui) {
        let fighter = self.state.selected_fighter
            .and_then(|index| self.state.fighters.get(index))
            .map(|f| f.name.clone());
        let motion = self.state.selected_move.as_ref().map(|m| m.name.clone());
        let (Some(fighter), Some(motion)) = (fighter, motion) else { return; };
        if fighter != FIGHTER || motion != MOTION { return; }

        ui.separator();
        ui.label(egui::RichText::new("Cancels").strong());
        ui.weak("Mario / Jab 1 - experimental Ryujinx prototype.");
        let mut rule = self.cancel_rules_store.get(&fighter)
            .and_then(|moves| moves.get(&motion))
            .cloned().unwrap_or_default();
        let original = rule.clone();

        ui.checkbox(&mut rule.enabled, "Enable Jab 1 -> Grab");
        ui.add_enabled_ui(rule.enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label("Target action");
                egui::ComboBox::from_id_salt("cancel_target")
                    .selected_text("Grab")
                    .show_ui(ui, |ui| {
                        ui.label("Grabs");
                        ui.selectable_value(&mut rule.target, CancelTarget::Grab, "Standing Grab");
                        ui.separator();
                        for family in [
                            "Normals / Tilts / Smashes", "Specials", "Movement",
                            "Defense", "Character-specific",
                        ] {
                            ui.add_enabled(false, egui::Label::new(family));
                        }
                        ui.weak("Other targets require native transition tests.");
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Cancel condition");
                egui::ComboBox::from_id_salt("cancel_condition")
                    .selected_text(rule.condition.label())
                    .show_ui(ui, |ui| {
                        for condition in CancelCondition::SUPPORTED {
                            ui.selectable_value(&mut rule.condition, condition, condition.label());
                        }
                        ui.add_enabled(false, egui::Label::new(CancelCondition::OnWhiff.label()));
                    });
            });
            ui.checkbox(&mut rule.sync_combo, "Follow native jab combo flag")
                .on_hover_text(
                    "Uses the game's ENABLE_COMBO flag at runtime instead of manual frames. Experimental: not a verified exact jab-2 window.",
                );
            if rule.sync_combo {
                let events: Vec<_> = self.state.script.to_work_flag_events().into_iter()
                    .filter(|event| event.call.flag.contains("ENABLE_COMBO"))
                    .collect();
                if events.is_empty() {
                    ui.weak("No ENABLE_COMBO event in the loaded script. Runtime uses the native flag.");
                } else {
                    for event in events {
                        ui.weak(format!("ACMD f{}: {}", event.frame, event.call.flag));
                    }
                    ui.weak("ACMD flags are not proof of the entire jab-2 input window.");
                }
            } else {
                ui.horizontal(|ui| {
                    ui.label("Start frame");
                    ui.add(egui::DragValue::new(&mut rule.start_frame).range(1..=60));
                    ui.label("End frame");
                    ui.add(egui::DragValue::new(&mut rule.end_frame).range(1..=60));
                });
                if rule.end_frame < rule.start_frame {
                    rule.end_frame = rule.start_frame;
                }
                ui.weak(format!("Grab active on frames {} to {} (inclusive)", rule.start_frame, rule.end_frame));
            }
        });
        ui.weak("Hit / Shield conditions and native combo sync still need in-game validation.");
        ui.weak("These are live plugin rules, not standalone Smashline exports.");

        if rule != original {
            self.cancel_rules_store.entry(fighter).or_default().insert(motion, rule);
            self.publish_cancel_rules();
        }
    }
}
