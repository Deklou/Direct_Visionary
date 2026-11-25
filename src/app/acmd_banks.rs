//! A verified logical character has its own ACMD editing workspace.
//! The existing editor still uses fighter/move keys within the currently open workspace.

use super::VisionaryApp;
use crate::cancel_rules::CancelBinding;
use crate::mod_project::{AcmdEdits, ModProjectFile, ScopedAcmdEdits, take_acmd_bank};
use crate::roster::RosterKey;
use std::collections::HashMap;
use crate::game_link::{HitboxRuleWire, SpawnRuleWire, EffectControlRuleWire};

/// Generated rules are session data. Keep them with their exact owner while its
/// editing workspace is parked, rather than merging two fighter/move namespaces.
pub(super) struct ScopedLiveAcmd {
    pub(super) binding: CancelBinding,
    pub(super) hitboxes: HashMap<String, Vec<HitboxRuleWire>>,
    pub(super) spawns: HashMap<String, Vec<SpawnRuleWire>>,
    pub(super) controls: HashMap<String, Vec<EffectControlRuleWire>>,
}

fn collect_scoped_rules<'a, T>(
    active: Option<(&'a CancelBinding, &'a HashMap<String, Vec<T>>)>,
    parked: impl Iterator<Item = (&'a CancelBinding, &'a HashMap<String, Vec<T>>)>,
    valid: impl Fn(&CancelBinding) -> bool,
) -> Vec<(i32, &'a [u8], &'a T)> {
    let owners: Vec<_> = active.into_iter().map(|(binding, rules)| (binding, rules, true))
        .chain(parked.map(|(binding, rules)| (binding, rules, false)))
        .filter(|(binding, _, _)| valid(binding) && !binding.scope.slots.is_empty())
        .collect();
    let mut result = Vec::new();
    for &(binding, rules, is_active) in &owners {
        // Duplicate or overlapping verified owners must not both reach the runtime.
        if owners.iter().filter(|(other, _, _)| {
            other.scope.fighter.eq_ignore_ascii_case(&binding.scope.fighter)
                && other.scope.slots.iter().any(|slot| binding.scope.slots.contains(slot))
        }).count() != 1 {
            continue;
        }
        let Some(kind) = super::fighter_kind_id(&binding.scope.fighter) else { continue; };
        let mut moves: Vec<_> = rules.iter().filter(|(key, _)|
            !is_active || super::fighter_key_matches(key, &binding.scope.fighter)).collect();
        moves.sort_by(|a, b| a.0.cmp(b.0));
        for (_, rules) in moves {
            result.extend(rules.iter().map(|rule| (kind, binding.scope.slots.as_slice(), rule)));
        }
    }
    result
}

fn take_fighter_moves<V>(map: &mut HashMap<String, V>, fighter: &str) -> HashMap<String, V> {
    let keys: Vec<_> = map.keys().filter(|key| {
        key.split_once('/').is_some_and(|(name, _)| name.eq_ignore_ascii_case(fighter))
    }).cloned().collect();
    let mut moves = HashMap::new();
    for key in keys {
        if let Some(value) = map.remove(&key) {
            if let Some((_, motion)) = key.split_once('/') {
                moves.insert(motion.to_string(), value);
            }
        }
    }
    moves
}

fn put_fighter_moves<V>(map: &mut HashMap<String, V>, fighter: &str, moves: HashMap<String, V>) {
    map.extend(moves.into_iter().map(|(motion, value)| (format!("{fighter}/{motion}"), value)));
}

fn clone_fighter_moves<V: Clone>(map: &HashMap<String, V>, fighter: &str) -> HashMap<String, V> {
    map.iter().filter_map(|(key, value)| {
        let (name, motion) = key.split_once('/')?;
        name.eq_ignore_ascii_case(fighter).then(|| (motion.to_string(), value.clone()))
    }).collect()
}

impl VisionaryApp {
    pub(super) fn verified_acmd_binding(&self, binding: &CancelBinding) -> bool {
        self.current_cancel_binding(&binding.scope.fighter, &binding.scope.key).as_ref() == Some(binding)
    }

    pub(super) fn verified_acmd_donors(
        &self, fighter: &str, motion: u64, captures: &[crate::game_link::CaptureLine],
    ) -> Vec<crate::game_link::CaptureLine> {
        let Some(binding) = self.active_acmd_binding.as_ref() else { return Vec::new(); };
        if !binding.scope.fighter.eq_ignore_ascii_case(fighter) || !self.verified_acmd_binding(binding) {
            return Vec::new();
        }
        let Some(kind) = super::fighter_kind_id(fighter) else { return Vec::new(); };
        crate::game_link::verified_saved_captures(captures, kind, motion, &binding.scope.slots)
    }

    // The active rule maps still have fighter/move keys; isolate them before flattening,
    // including all point-family suffixes. Parked maps contain only that owner's moves.
    pub(super) fn send_scoped_acmd_union<'a, T>(
        &'a self, active: &'a HashMap<String, Vec<T>>,
        parked: impl Fn(&'a ScopedLiveAcmd) -> &'a HashMap<String, Vec<T>>,
        send: impl FnOnce(&[(i32, &[u8], &T)]),
    ) {
        if self.game_link.sends_suppressed() { return; }
        let active = self.active_acmd_binding.as_ref().map(|binding| (binding, active));
        let rules = collect_scoped_rules(active,
            self.parked_live_acmd.iter().map(|bank| (&bank.binding, parked(bank))),
            |binding| self.verified_acmd_binding(binding));
        send(&rules);
    }

    fn park_live_acmd(&mut self, binding: &CancelBinding) {
        let fighter = &binding.scope.fighter;
        self.parked_live_acmd.retain(|bank| &bank.binding != binding);
        let bank = ScopedLiveAcmd {
            binding: binding.clone(),
            hitboxes: take_fighter_moves(&mut self.hitbox_rules_store, fighter),
            spawns: take_fighter_moves(&mut self.effect_rules_store, fighter),
            controls: take_fighter_moves(&mut self.effect_control_rules_store, fighter),
        };
        if !bank.hitboxes.is_empty() || !bank.spawns.is_empty() || !bank.controls.is_empty() {
            self.parked_live_acmd.push(bank);
        }
    }

    fn restore_live_acmd(&mut self, binding: &CancelBinding) {
        let Some(index) = self.parked_live_acmd.iter().position(|bank| &bank.binding == binding) else { return; };
        let bank = self.parked_live_acmd.remove(index);
        let fighter = &binding.scope.fighter;
        put_fighter_moves(&mut self.hitbox_rules_store, fighter, bank.hitboxes);
        put_fighter_moves(&mut self.effect_rules_store, fighter, bank.spawns);
        put_fighter_moves(&mut self.effect_control_rules_store, fighter, bank.controls);
    }

    /// The previewed slot must belong to exactly one fully verified logical character.
    /// Selecting c08 or c09 of one authored character opens the same bank; an unknown
    /// imported slot, or a conflict, never inherits another character's bank.
    pub(super) fn acmd_binding_for_slot(&self, fighter: &str, slot: u8) -> Option<CancelBinding> {
        let base = RosterKey::fighter(fighter);
        let mut keys = vec![base];
        keys.extend(self.roster.project.authored.iter()
            .filter(|entry| entry.donor.eq_ignore_ascii_case(fighter))
            .map(|entry| entry.key.clone()));
        let mut matches = keys.iter().filter_map(|key| self.current_cancel_binding(fighter, key))
            .filter(|binding| binding.scope.matches(fighter, Some(slot)));
        let binding = matches.next()?;
        if matches.next().is_some() { return None; }
        Some(binding)
    }

    fn take_acmd_workspace(&mut self, fighter: &str) -> AcmdEdits {
        AcmdEdits {
            acmd: self.state.edit_log.entries.remove(fighter).unwrap_or_default(),
            effect_calls: take_fighter_moves(&mut self.state.effect_call_edits, fighter),
            effect_calls_full: take_fighter_moves(&mut self.state.effect_call_full, fighter),
            effect_dropped_lines: take_fighter_moves(&mut self.state.effect_dropped_lines, fighter),
            effect_frame_residue: take_fighter_moves(&mut self.state.effect_frame_residue, fighter),
            sound_scripts: take_fighter_moves(&mut self.state.sound_script_edits, fighter),
            expression_scripts: take_fighter_moves(&mut self.state.expression_script_edits, fighter),
            capture_branch_warnings: take_fighter_moves(&mut self.state.capture_branch_warnings, fighter),
            move_sources: take_fighter_moves(&mut self.move_source_cache, fighter),
        }
    }

    fn restore_acmd_workspace(&mut self, fighter: &str, edits: AcmdEdits) {
        if !edits.acmd.is_empty() {
            self.state.edit_log.entries.insert(fighter.to_string(), edits.acmd);
        }
        put_fighter_moves(&mut self.state.effect_call_edits, fighter, edits.effect_calls);
        put_fighter_moves(&mut self.state.effect_call_full, fighter, edits.effect_calls_full);
        put_fighter_moves(&mut self.state.effect_dropped_lines, fighter, edits.effect_dropped_lines);
        put_fighter_moves(&mut self.state.effect_frame_residue, fighter, edits.effect_frame_residue);
        put_fighter_moves(&mut self.state.sound_script_edits, fighter, edits.sound_scripts);
        put_fighter_moves(&mut self.state.expression_script_edits, fighter, edits.expression_scripts);
        put_fighter_moves(&mut self.state.capture_branch_warnings, fighter, edits.capture_branch_warnings);
        put_fighter_moves(&mut self.move_source_cache, fighter, edits.move_sources);
    }

    fn snapshot_acmd_workspace(&self, fighter: &str) -> AcmdEdits {
        AcmdEdits {
            acmd: self.state.edit_log.entries.get(fighter).cloned().unwrap_or_default(),
            effect_calls: clone_fighter_moves(&self.state.effect_call_edits, fighter),
            effect_calls_full: clone_fighter_moves(&self.state.effect_call_full, fighter),
            effect_dropped_lines: clone_fighter_moves(&self.state.effect_dropped_lines, fighter),
            effect_frame_residue: clone_fighter_moves(&self.state.effect_frame_residue, fighter),
            sound_scripts: clone_fighter_moves(&self.state.sound_script_edits, fighter),
            expression_scripts: clone_fighter_moves(&self.state.expression_script_edits, fighter),
            capture_branch_warnings: clone_fighter_moves(&self.state.capture_branch_warnings, fighter),
            move_sources: clone_fighter_moves(&self.move_source_cache, fighter),
        }
    }

    /// A bank is parked with the identity it had while editing, never the newly selected slot.
    /// The old fighter/move maps are restored to their legacy/unassigned contents.
    pub(super) fn park_acmd_workspace(&mut self) {
        let Some(binding) = self.active_acmd_binding.take() else { return; };
        let fighter = binding.scope.fighter.clone();
        let edits = self.take_acmd_workspace(&fighter);
        self.park_live_acmd(&binding);
        if !edits.is_empty() {
            self.scoped_acmd_banks.entry(fighter.clone()).or_default()
                .push(ScopedAcmdEdits { binding, edits });
        }
        if let Some(legacy) = self.parked_legacy_acmd.take() {
            self.restore_acmd_workspace(&fighter, legacy);
        }
        // Undo and rule caches use fighter/move keys, not logical identities.
        self.clear_acmd_workspace_caches(&fighter);
    }

    fn clear_acmd_workspace_caches(&mut self, fighter: &str) {
        self.edit_history.retain(|key, _| !super::fighter_key_matches(key, fighter));
        self.hitbox_rules_store.retain(|key, _| !super::fighter_key_matches(key, fighter));
        self.effect_rules_store.retain(|key, _| !super::fighter_key_matches(key, fighter));
        self.effect_control_rules_store.retain(|key, _| !super::fighter_key_matches(key, fighter));
        self.pending_history_action = None;
        self.history_suppress_frame = true;
    }

    pub(super) fn open_acmd_workspace(&mut self, fighter: &str, slot: u8) {
        let Some(binding) = self.acmd_binding_for_slot(fighter, slot) else { return; };
        let Some(edits) = take_acmd_bank(self.scoped_acmd_banks.entry(fighter.to_string()).or_default(), &binding) else {
            self.state.status = "Duplicate ACMD owner: saved banks kept separate and inactive.".into();
            return;
        };
        self.parked_legacy_acmd = Some(self.take_acmd_workspace(fighter));
        self.restore_acmd_workspace(fighter, edits);
        self.clear_acmd_workspace_caches(fighter);
        self.restore_live_acmd(&binding);
        self.active_acmd_binding = Some(binding);
    }

    /// Before saving, separate the active editor maps from the legacy maps. Parked banks
    /// are copied verbatim, even when their owner is no longer installed or has changed.
    pub(super) fn save_acmd_workspaces(&self, project: &mut ModProjectFile) {
        for (fighter, slots) in &self.legacy_acmd_costumes {
            project.fighters.entry(fighter.clone()).or_default().costume_slots = slots.clone();
        }
        for (fighter, banks) in &self.scoped_acmd_banks {
            if !banks.is_empty() {
                project.fighters.entry(fighter.clone()).or_default().scoped_acmd = banks.clone();
            }
        }
        if let Some(binding) = &self.active_acmd_binding {
            let fighter = &binding.scope.fighter;
            let active = self.snapshot_acmd_workspace(fighter);
            let fm = project.fighters.entry(fighter.clone()).or_default();
            // The existing build_project collected the active bank under the engine name.
            // Replace it with the untouched legacy bank instead of saving the active bank twice.
            self.parked_legacy_acmd.clone().unwrap_or_default().put_into_fighter(fm);
            if !active.is_empty() {
                fm.scoped_acmd.push(ScopedAcmdEdits { binding: binding.clone(), edits: active });
            }
        }
    }

    pub(super) fn reset_acmd_workspaces(&mut self) {
        self.active_acmd_binding = None;
        self.parked_legacy_acmd = None;
        self.scoped_acmd_banks.clear();
        self.parked_live_acmd.clear();
        self.legacy_acmd_costumes.clear();
    }

    /// Called before an index replacement, while the old selection still identifies its owner.
    pub(super) fn park_acmd_before_reindex(&mut self) {
        if self.active_acmd_binding.is_none() { return; }
        self.commit_current_edits();
        self.park_acmd_workspace();
        self.state.selected_fighter = None;
        self.state.selected_move = None;
        super::clear_move_state(&mut self.state);
        self.move_list.clear();
        self.move_list_receiver = None;
        self.acmd_receiver = None;
        self.pending_project_script = None;
        self.pending_project_move_selection = None;
        self.source_mismatch_prompt = None;
        self.fetching_acmd = false;
    }

    pub(super) fn draw_acmd_workspace_target(&mut self, ui: &mut egui::Ui) {
        let Some(index) = self.state.selected_fighter else { return; };
        let Some(fighter) = self.state.fighters.get(index).map(|entry| entry.name.clone()) else { return; };
        let Some(binding) = self.active_acmd_binding.clone() else {
            ui.weak("Unassigned ACMD workspace. Choose a verified costume to open moves; saved edits are preserved.");
            return;
        };
        if self.acmd_binding_for_slot(&fighter, self.selected_costume_slot).as_ref() != Some(&binding) {
            ui.weak("ACMD target changed — saved edits are kept under their old identity.");
            return;
        }
        ui.label(format!("ACMD target: {} — {}", binding.scope.key,
            binding.scope.slots.iter().map(|slot| format!("c{slot:02}")).collect::<Vec<_>>().join(", ")));
        let stale = self.scoped_acmd_banks.get(&fighter).into_iter().flatten().filter(|bank| {
            bank.binding.scope.key == binding.scope.key && bank.binding != binding && bank.edits.has_edits()
        }).count();
        if stale > 0 {
            ui.weak(format!("{stale} saved ACMD bank(s) have a changed identity. They remain separate."));
        }
        let Some(legacy) = self.parked_legacy_acmd.as_ref() else { return; };
        if !legacy.has_edits() { return; }
        ui.weak("Unassigned ACMD edits are preserved but are not shown in this character's bank.");
        let compatible = self.legacy_acmd_costumes.get(&fighter)
            .is_none_or(|slots| slots.is_empty() || slots == &binding.scope.slots)
            && legacy.acmd.values().all(|record| {
                let slots = record.effective_scopes();
                slots.is_empty() || slots == binding.scope.slots
            });
        let active_empty = !self.snapshot_acmd_workspace(&fighter).has_edits();
        if !compatible {
            ui.weak("Legacy edits contain different costume scopes. Automatic assignment is refused.");
        } else if !active_empty {
            ui.weak("Assignment requires an empty target bank so no existing edit can be overwritten.");
        } else if ui.button("Assign all unassigned ACMD edits to this character")
            .on_hover_text("Explicitly assigns every unassigned move of this engine fighter to the displayed owner. Other character banks are unchanged.")
            .clicked()
        {
            self.commit_current_edits();
            if self.snapshot_acmd_workspace(&fighter).has_edits() { return; }
            let Some(mut legacy) = self.parked_legacy_acmd.take() else { return; };
            for record in legacy.acmd.values_mut() {
                record.slot_scope = binding.scope.slots.first().copied();
                record.slot_scopes = binding.scope.slots.iter().copied().skip(1).collect();
            }
            self.parked_legacy_acmd = Some(AcmdEdits::default());
            self.legacy_acmd_costumes.remove(&fighter);
            let _ = self.take_acmd_workspace(&fighter);
            self.restore_acmd_workspace(&fighter, legacy);
            // The displayed move still has the previous bank's state. Drop it before reloading
            // the fighter, otherwise navigation could commit it over the newly assigned data.
            let resume = self.state.selected_move.as_ref().map(|entry| entry.name.clone());
            self.state.selected_move = None;
            super::clear_move_state(&mut self.state);
            self.source_mismatch_prompt = None;
            self.select_fighter_slot(index, self.selected_costume_slot);
            if let Some(motion) = resume {
                self.pending_project_move_selection = Some((fighter, motion));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(slot: u8) -> CancelBinding {
        CancelBinding {
            scope: crate::roster::edit_scope::EditScope {
                key: if slot == 0 { RosterKey::fighter("mario") } else { RosterKey::slot("mario", slot) },
                fighter: "mario".into(),
                slots: vec![slot, slot + 1],
            },
            authored_name_id: (slot != 0).then(|| format!("mod_{slot}")),
        }
    }

    #[test]
    fn scoped_acmd_live_union_preserves_parked_owners_and_removals() {
        let base = binding(0);
        let first = binding(8);
        let second = binding(16);
        let a = HashMap::from([("attack_11".to_string(), vec![1])]);
        let mut b = HashMap::from([("mario/attack_11".to_string(), vec![2]), ("luigi/attack_11".to_string(), vec![99])]);
        let c = HashMap::from([("attack_11".to_string(), vec![3])]);
        let union = collect_scoped_rules(Some((&first, &b)), [(&base, &a), (&second, &c)].into_iter(), |_| true);
        assert_eq!(union.iter().map(|(_, slots, rule)| (slots[0], **rule)).collect::<Vec<_>>(),
            vec![(8, 2), (0, 1), (16, 3)]);
        b.clear();
        let union = collect_scoped_rules(Some((&first, &b)), [(&base, &a), (&second, &c)].into_iter(), |_| true);
        assert_eq!(union.iter().map(|(_, slots, rule)| (slots[0], **rule)).collect::<Vec<_>>(),
            vec![(0, 1), (16, 3)]);
    }

    #[test]
    fn scoped_acmd_live_union_rejects_stale_and_duplicate_owners() {
        let first = binding(8);
        let second = binding(16);
        let rules = HashMap::from([("mario/attack_11".to_string(), vec![1])]);
        let union = collect_scoped_rules(Some((&first, &rules)), [(&second, &rules)].into_iter(), |b| b == &second);
        assert_eq!(union.len(), 1);
        assert_eq!(union[0].1, &[16, 17]);
        let union = collect_scoped_rules(Some((&first, &rules)), [(&first, &rules), (&second, &rules)].into_iter(), |_| true);
        assert_eq!(union.len(), 1);
        assert_eq!(union[0].1, &[16, 17]);
    }

    #[test]
    fn scoped_acmd_workspace_switch_does_not_take_other_fighters() {
        let mut map = HashMap::from([
            ("mario/attack_11".to_owned(), "original"),
            ("luigi/attack_11".to_owned(), "other fighter"),
        ]);
        let original = take_fighter_moves(&mut map, "mario");
        assert_eq!(map.len(), 1);
        put_fighter_moves(&mut map, "mario", HashMap::from([("attack_11".into(), "mod")]));
        assert_eq!(clone_fighter_moves(&map, "mario")["attack_11"], "mod");
        let modified = take_fighter_moves(&mut map, "mario");
        put_fighter_moves(&mut map, "mario", original);
        assert_eq!(modified["attack_11"], "mod");
        assert_eq!(map["mario/attack_11"], "original");
        assert_eq!(map["luigi/attack_11"], "other fighter");
    }
}
