use egui::Ui;

use super::VisionaryApp;

impl VisionaryApp {
    /// Live-only timing rule; no source ACMD call is invented or exported.
    pub(super) fn draw_recovery_rate_section(&mut self, ui: &mut Ui) {
        let (Some(move_key), Some(motion)) =
            (self.current_move_key(), self.current_motion_hash())
        else {
            return;
        };
        if self.state.total_frames < 2 {
            return;
        }

        let key = format!("{move_key}#recovery_rate");
        let stored = self
            .hitbox_rules_store
            .get(&key)
            .and_then(|rules| rules.first())
            .and_then(|rule| rule.inject.as_ref())
            .and_then(|injection| {
                let rate = injection.args.first()?.as_f32()?;
                (rate.is_finite() && rate > 0.0)
                    .then_some((injection.frame, 1.0 / rate))
            });

        let max_start = self.state.total_frames - 1;
        let suggested = self
            .state
            .hitboxes
            .iter()
            .map(|hitbox| hitbox.active_end)
            .max()
            .unwrap_or(self.state.current_frame)
            .saturating_add(1)
            .clamp(1, max_start);
        let mut enabled = stored.is_some();
        let mut start = stored
            .map(|(frame, _)| frame.round() as u32)
            .unwrap_or(suggested)
            .clamp(1, max_start);
        let mut speed = stored.map(|(_, speed)| speed).unwrap_or(2.0).clamp(1.0, 4.0);

        ui.separator();
        ui.label("Recovery speed (live)");
        ui.weak("Accelerates the animation from the chosen frame. Earlier frames stay unchanged.");
        let mut changed = ui.checkbox(&mut enabled, "Speed up recovery").changed();
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label("From frame");
                changed |= ui
                    .add(egui::DragValue::new(&mut start).range(1..=max_start))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Playback speed");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut speed)
                            .speed(0.1)
                            .range(1.0..=4.0)
                            .suffix("x"),
                    )
                    .changed();
            });
        });
        ui.weak("Live preview only; this timing rule is not saved or exported.");

        if !changed {
            return;
        }

        if enabled {
            self.hitbox_rules_store.insert(
                key,
                vec![crate::game_link::HitboxRuleWire {
                    motion,
                    category: crate::game_link::CAT_MOTION_RATE,
                    hitbox_id: None,
                    suppress: false,
                    frame_start: None,
                    frame_end: None,
                    overrides: None,
                    inject: Some(crate::game_link::InjectRuleWire {
                        frame: start as f32,
                        // FT_MOTION_RATE uses game frames per motion frame: 0.5 = 2x faster.
                        args: vec![crate::game_link::LuaArgWire::Num(1.0 / speed)],
                        command: Some("FT_MOTION_RATE".into()),
                    }),
                    func: None,
                }],
            );
        } else {
            self.hitbox_rules_store.remove(&key);
        }

        let rules: Vec<crate::game_link::HitboxRuleWire> = self
            .hitbox_rules_store
            .values()
            .flatten()
            .cloned()
            .collect();
        self.game_link.send_hitbox_rules(&rules);
        self.state.status = if enabled {
            format!("Recovery speed {speed:.2}x from frame {start} sent for live preview")
        } else {
            "Live recovery speed removed".into()
        };
    }
}
