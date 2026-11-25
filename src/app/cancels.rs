//! Custom cancel editor: independently timed rules, each with several target actions.

use super::VisionaryApp;
use crate::cancel_rules::{self, CancelBinding, CancelCondition, CancelRule, CancelTarget};
use crate::roster::{RosterKey, index::RosterIndex};
use egui::Ui;

impl VisionaryApp {
    fn current_cancel_binding(&self, fighter: &str, key: &RosterKey) -> Option<CancelBinding> {
        let entry = self.state.fighters.iter().find(|entry| entry.name.eq_ignore_ascii_case(fighter))?;
        let index = RosterIndex::build(
            std::slice::from_ref(entry), &self.roster.library, &self.roster.project, None,
        );
        let binding = cancel_rules::resolve_binding(&index, fighter, key, &self.cancel_base_slots)?;
        if !binding.scope.slots.iter().all(|slot| entry.slots.contains(slot)) {
            return None;
        }
        if binding.authored_name_id.is_some() {
            let owner = self.roster.project.authored.iter().find(|owner| &owner.key == key)?;
            let root = owner.files_root.as_ref()?;
            let owns_provider = |provider: &crate::roster::library::ImportedMod| provider.root.starts_with(root);
            if !self.roster.library.enabled().any(owns_provider) {
                return None;
            }
            // A different enabled provider claiming the same modded slots is ambiguous.
            if self.roster.library.enabled().any(|provider| {
                !owns_provider(provider) && provider.manifest.fighters.get(&binding.scope.fighter)
                    .is_some_and(|provision| binding.scope.slots.iter().any(|slot| provision.slots.contains(slot)))
            }) {
                return None;
            }
        }
        Some(binding)
    }

    pub(super) fn publish_cancel_rules(&self) {
        if self.game_link.supports_scoped_cancels() {
            let wires = cancel_rules::resolved_wires(&self.cancel_rules_store, |fighter, key| {
                self.current_cancel_binding(fighter, key)
            });
            if self.game_link.send_scoped_cancel_rules(&wires) {
                return;
            }
        }
        // An old or disconnected NRO must never receive a non-empty unscoped fallback.
        self.game_link.send_cancel_rules(&[]);
    }

    pub(super) fn draw_cancel_rules_section(&mut self, ui: &mut Ui) {
        let fighter = self.state.selected_fighter
            .and_then(|index| self.state.fighters.get(index))
            .map(|entry| entry.name.clone());
        let motion = self.state.selected_move.as_ref().map(|entry| entry.name.clone());
        let (Some(fighter), Some(motion)) = (fighter, motion) else {
            return;
        };
        let key = self.roster.cancel_edit_target().cloned()
            .unwrap_or_else(|| RosterKey::fighter(&fighter));
        ui.label(egui::RichText::new("Custom cancels").strong());
        let Some(binding) = self.current_cancel_binding(&fighter, &key) else {
            ui.weak("No verified character/costume target. Check the base game folder and Roster edit target.");
            return;
        };
        if !binding.scope.matches(&fighter, Some(self.selected_costume_slot)) {
            ui.weak("Preview a costume owned by the selected Roster edit target before editing cancels.");
            return;
        }
        ui.label(format!("Target: {} — {}", key, binding.scope.slots.iter()
            .map(|slot| format!("c{slot:02}")).collect::<Vec<_>>().join(", ")));
        if !self.game_link.supports_scoped_cancels() {
            ui.weak("Live cancels require a compatible Skyline plugin. Edits can be saved but are not sent.");
        }
        let has_native_combo = matches!(motion.as_str(), "attack_11" | "attack_12" | "attack_13");
        let mut all_rules = self.cancel_rules_store
            .get(&fighter)
            .and_then(|moves| moves.get(&motion))
            .cloned()
            .unwrap_or_default();
        let unbound = all_rules.iter().filter(|rule| rule.binding.is_none()).count();
        let stale = all_rules.iter().filter(|rule| rule.binding.as_ref().is_some_and(|saved| {
            saved.scope.key == binding.scope.key && saved != &binding
        })).count();
        let mut reassigned = false;
        if unbound > 0 {
            ui.weak(format!("{unbound} legacy cancel(s) have no confirmed target and are inactive."));
            if ui.button("Assign legacy cancels to this target").clicked() {
                for rule in all_rules.iter_mut().filter(|rule| rule.binding.is_none()) {
                    rule.binding = Some(binding.clone());
                }
                reassigned = true;
            }
        }
        if stale > 0 {
            ui.weak(format!("{stale} saved cancel(s) have a changed target and are inactive."));
            if ui.button("Confirm changed target for these cancels").clicked() {
                for rule in all_rules.iter_mut().filter(|rule| rule.binding.as_ref().is_some_and(|saved| {
                    saved.scope.key == binding.scope.key && saved != &binding
                })) {
                    rule.binding = Some(binding.clone());
                }
                reassigned = true;
            }
        }
        let mut rules: Vec<_> = all_rules.iter()
            .filter(|rule| rule.binding.as_ref() == Some(&binding))
            .cloned().collect();
        let original = rules.clone();
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
                .id_salt(("cancel_rule", key.as_str(), motion.as_str(), index))
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
            rules.push(CancelRule { binding: Some(binding.clone()), ..Default::default() });
        }
        ui.separator();
        ui.weak("Ground-only; the game's native transition checks choose priority when inputs overlap.");
        ui.weak(concat!(
            "Movement and Shield are experimental ground cancels. ",
            "Dodges, character-specific moves and On Whiff remain unavailable."
        ));
        ui.weak("Live plugin only; standalone Smashline export is not implemented.");

        if rules != original || reassigned {
            // Do not overwrite other logical characters sharing this fighter and motion.
            all_rules.retain(|rule| rule.binding.as_ref() != Some(&binding));
            all_rules.extend(rules);
            if all_rules.is_empty() {
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
                    .insert(motion, all_rules);
            }
            self.publish_cancel_rules();
        }
    }
}
