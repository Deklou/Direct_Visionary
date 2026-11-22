//! Live cancel settings for the selected move; runtime remains opt-in.

use super::VisionaryApp;
use crate::cancel_rules::{self, CancelCondition, CancelRule, CancelTarget};
use egui::Ui;

impl VisionaryApp {
    /// Full-list replacement across all fighters and motions, including the empty state.
    pub(super) fn publish_cancel_rules(&self) {
        let rules = cancel_rules::enabled_wires(&self.cancel_rules_store);
        self.game_link.send_cancel_rules(&rules);
    }

    pub(super) fn draw_cancel_rules_section(&mut self, ui: &mut Ui) {
        let fighter = self
            .state
            .selected_fighter
            .and_then(|index| self.state.fighters.get(index))
            .map(|f| f.name.clone());
        let motion = self.state.selected_move.as_ref().map(|m| m.name.clone());
        let (Some(fighter), Some(motion)) = (fighter, motion) else {
            return;
        };
        let is_reference = fighter == "mario" && motion == "attack_11";
        let has_native_combo = matches!(motion.as_str(), "attack_11" | "attack_12" | "attack_13");
        let mut rule = self.cancel_rules_store
            .get(&fighter)
            .and_then(|moves| moves.get(&motion))
            .cloned()
            .unwrap_or_default();
        let before = rule.clone();

        ui.separator();
        ui.label(egui::RichText::new("Cancels").strong());
        if is_reference {
            ui.weak("Mario / Jab 1: Grab has been observed working on Ryujinx.");
        } else {
            ui.weak("Experimental on this move; transitions depend on the fighter and status.");
        }
        ui.checkbox(&mut rule.enabled, "Enable custom cancel")
            .on_hover_text("Disabled by default. Existing native cancels stay unchanged.");

        ui.add_enabled_ui(rule.enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label("Target action");
                egui::ComboBox::from_id_salt("cancel_target")
                    .selected_text(rule.target.label())
                    .show_ui(ui, |ui| {
                        for (family, targets) in [
                            ("Grabs", &CancelTarget::GRABS[..]),
                            ("Tilts", &CancelTarget::TILTS[..]),
                            ("Smashes", &CancelTarget::SMASHES[..]),
                            ("Specials", &CancelTarget::SPECIALS[..]),
                        ] {
                            ui.label(egui::RichText::new(family).strong());
                            for &target in targets {
                                ui.selectable_value(&mut rule.target, target, target.label());
                            }
                            ui.separator();
                        }
                        ui.add_enabled(false, egui::Label::new("Movement / Jump"));
                        ui.add_enabled(false, egui::Label::new("Defense / Shield"));
                        ui.add_enabled(false, egui::Label::new("Character-specific"));
                        ui.weak("These categories need separate native transition tests.");
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

            if has_native_combo {
                ui.checkbox(&mut rule.sync_combo, "Follow native jab combo flag")
                    .on_hover_text(
                        "Follows ENABLE_COMBO at runtime. This is not yet verified to be the complete Jab 2 input window.",
                    );
                if rule.sync_combo {
                    let events: Vec<_> = self.state.script.to_work_flag_events().into_iter()
                        .filter(|event| event.call.flag.contains("ENABLE_COMBO"))
                        .collect();
                    if events.is_empty() {
                        ui.weak("No ENABLE_COMBO event found in this script. The plugin reads the native runtime flag.");
                    } else {
                        for event in events {
                            ui.weak(format!("ACMD f{}: {}", event.frame, event.call.flag));
                        }
                    }
                    ui.weak("Combo flag synchronization remains experimental.");
                }
            }
            if !rule.sync_combo {
                ui.horizontal(|ui| {
                    ui.label("Start frame");
                    ui.add(egui::DragValue::new(&mut rule.start_frame).range(1..=60));
                    ui.label("End frame");
                    ui.add(egui::DragValue::new(&mut rule.end_frame).range(1..=60));
                });
                if rule.end_frame < rule.start_frame {
                    rule.end_frame = rule.start_frame;
                }
                ui.weak(format!(
                    "Window: frames {} to {} (inclusive)",
                    rule.start_frame, rule.end_frame,
                ));
            }
        });
        ui.weak("All listed targets are ground-only. Tilts, Smashes, Specials and Hit / Shield conditions require Ryujinx tests.");
        ui.weak("Live plugin only: standalone Smashline export is not yet supported.");
        if rule != before {
            self.cancel_rules_store
                .entry(fighter)
                .or_default()
                .insert(motion, rule);
            self.publish_cancel_rules();
        }
    }
}
