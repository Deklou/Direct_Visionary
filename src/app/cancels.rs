//! Custom cancel editor: independently timed rules, each with several target actions.

use super::VisionaryApp;
use crate::cancel_rules::{self, CancelCondition, CancelRule, CancelTarget};
use egui::Ui;

impl VisionaryApp {
    pub(super) fn publish_cancel_rules(&self) {
        self.game_link
            .send_cancel_rules(&cancel_rules::enabled_wires(&self.cancel_rules_store));
    }

    pub(super) fn draw_cancel_rules_section(&mut self, ui: &mut Ui) {
        let fighter = self.state.selected_fighter
            .and_then(|index| self.state.fighters.get(index))
            .map(|entry| entry.name.clone());
        let motion = self.state.selected_move.as_ref().map(|entry| entry.name.clone());
        let (Some(fighter), Some(motion)) = (fighter, motion) else {
            return;
        };
        let has_native_combo = matches!(motion.as_str(), "attack_11" | "attack_12" | "attack_13");
        let mut rules = self.cancel_rules_store
            .get(&fighter)
            .and_then(|moves| moves.get(&motion))
            .cloned()
            .unwrap_or_default();
        let original = rules.clone();

        ui.label(egui::RichText::new("Custom cancels").strong());
        ui.weak("Each rule has its own window and condition. Select several target actions in one rule.");
        if rules.is_empty() {
            ui.weak("No custom cancels on this move. Vanilla transitions are unchanged.");
        }

        let mut remove_index = None;
        for (index, rule) in rules.iter_mut().enumerate() {
            let targets = if rule.targets.is_empty() {
                "No target selected".to_owned()
            } else {
                rule.targets.iter().map(|t| t.label()).collect::<Vec<_>>().join(", ")
            };
            let window = if rule.sync_combo {
                "Native combo".to_owned()
            } else {
                format!("f{}–{}", rule.start_frame, rule.end_frame)
            };
            let heading = format!(
                "Cancel {} · {} · {}{}",
                index + 1, window, targets, if rule.enabled { "" } else { " (off)" },
            );
            egui::CollapsingHeader::new(heading)
                .id_salt(("cancel_rule", fighter.as_str(), motion.as_str(), index))
                .default_open(true)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut rule.enabled, "Enable");
                        if ui.button("Remove").clicked() {
                            remove_index = Some(index);
                        }
                    });

                    ui.add_enabled_ui(rule.enabled, |ui| {
                        for (family, choices) in [
                            ("Grabs", &CancelTarget::GRABS[..]),
                            ("Tilts", &CancelTarget::TILTS[..]),
                            ("Smashes", &CancelTarget::SMASHES[..]),
                            ("Specials", &CancelTarget::SPECIALS[..]),
                            ("Movement", &CancelTarget::MOVEMENT[..]),
                            ("Defense", &CancelTarget::DEFENSE[..]),
                        ] {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(egui::RichText::new(family).strong());
                                for &target in choices {
                                    let mut selected = rule.targets.contains(&target);
                                    if ui.checkbox(&mut selected, target.label()).changed() {
                                        if selected {
                                            rule.targets.push(target);
                                        } else {
                                            rule.targets.retain(|entry| *entry != target);
                                        }
                                    }
                                }
                            });
                        }

                        ui.add_enabled(
                            false,
                            egui::Label::new(
                                "Defense / Dodge, Roll · Character-specific (not yet available)",
                            ),
                        );
                        ui.horizontal(|ui| {
                            ui.label("Condition");
                            egui::ComboBox::from_id_salt(("cancel_condition", index))
                                .selected_text(rule.condition.label())
                                .show_ui(ui, |ui| {
                                    for condition in CancelCondition::SUPPORTED {
                                        ui.selectable_value(
                                            &mut rule.condition, condition, condition.label(),
                                        );
                                    }
                                    ui.add_enabled(
                                        false,
                                        egui::Label::new(CancelCondition::OnWhiff.label()),
                                    );
                                });
                        });

                        if has_native_combo {
                            ui.checkbox(&mut rule.sync_combo, "Follow native jab combo flag")
                                .on_hover_text(
                                    concat!(
                                        "Uses ENABLE_COMBO at runtime. ",
                                        "The exact jab-2 input window has not been proven."
                                    ),
                                );
                            if rule.sync_combo {
                                let events = self.state.script.to_work_flag_events();
                                for event in events.iter().filter(|e| e.call.flag.contains("ENABLE_COMBO")) {
                                    ui.weak(format!("ACMD f{}: {}", event.frame, event.call.flag));
                                }
                                ui.weak("Native combo flag synchronization is experimental.");
                            }
                        }

                        if !rule.sync_combo {
                            ui.horizontal(|ui| {
                                ui.label("Frames");
                                ui.add(egui::DragValue::new(&mut rule.start_frame).range(1..=60));
                                ui.label("to");
                                ui.add(egui::DragValue::new(&mut rule.end_frame).range(1..=60));
                            });
                            if rule.end_frame < rule.start_frame {
                                rule.end_frame = rule.start_frame;
                            }
                        }

                        if rule.targets.is_empty() {
                            ui.weak("Select at least one action to apply this rule.");
                        }
                    });
                });
        }

        if let Some(index) = remove_index {
            rules.remove(index);
        }
        if ui.button("＋ Add cancel").clicked() {
            rules.push(CancelRule::default());
        }
        ui.separator();
        ui.weak("Ground-only; the game's native transition checks choose priority when inputs overlap.");
        ui.weak(concat!(
            "Movement and Shield are experimental ground cancels. ",
            "Dodges, character-specific moves and On Whiff remain unavailable."
        ));
        ui.weak("Live plugin only; standalone Smashline export is not implemented.");

        if rules != original {
            if rules.is_empty() {
                let empty_fighter = self.cancel_rules_store
                    .get_mut(&fighter)
                    .map(|moves| {
                        moves.remove(&motion);
                        moves.is_empty()
                    })
                    .unwrap_or(false);
                if empty_fighter {
                    self.cancel_rules_store.remove(&fighter);
                }
            } else {
                self.cancel_rules_store
                    .entry(fighter)
                    .or_default()
                    .insert(motion, rules);
            }
            self.publish_cancel_rules();
        }
    }
}
