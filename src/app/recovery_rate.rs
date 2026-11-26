use egui::Ui;

use super::VisionaryApp;
use crate::game_link::{
    HitboxRuleWire, InjectRuleWire, LuaArgWire, CAT_MOTION_MODULE_SET_RATE,
};

const MIN_SPEED: f32 = 0.1;
const MAX_SPEED: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct SpeedRange {
    start: u32,
    end: u32,
    speed: f32,
}

fn rate_from_rule(rule: &HitboxRuleWire) -> Option<(u32, f32)> {
    if rule.category != CAT_MOTION_MODULE_SET_RATE {
        return None;
    }
    let injection = rule.inject.as_ref()?;
    if injection.command.as_deref() != Some("MotionModule::set_rate") {
        return None;
    }
    let rate = injection.args.first()?.as_f32()?;
    (injection.frame.is_finite()
        && injection.frame >= 1.0
        && rate.is_finite()
        && rate >= MIN_SPEED
        && rate <= MAX_SPEED)
        .then_some((injection.frame.round() as u32, rate))
}

fn read_speed_ranges(rules: Option<&Vec<HitboxRuleWire>>, max_frame: u32) -> Vec<SpeedRange> {
    let Some(rules) = rules else {
        return Vec::new();
    };
    // The previous editor stored a single "from frame" rule with no end frame.
    if rules.len() == 1 {
        return rate_from_rule(&rules[0])
            .filter(|(start, _)| *start <= max_frame)
            .map(|(start, speed)| {
                vec![SpeedRange {
                    start,
                    end: max_frame,
                    speed,
                }]
            })
            .unwrap_or_default();
    }
    let mut ranges = Vec::new();
    for pair in rules.chunks_exact(2) {
        let (Some((start, speed)), Some((reset, reset_rate))) =
            (rate_from_rule(&pair[0]), rate_from_rule(&pair[1]))
        else {
            continue;
        };
        if start > max_frame || reset <= start || (reset_rate - 1.0).abs() > 0.0001 {
            continue;
        }
        ranges.push(SpeedRange {
            start,
            end: (reset - 1).min(max_frame),
            speed,
        });
    }
    ranges
}

fn live_rate_rule(motion: u64, frame: u32, speed: f32) -> HitboxRuleWire {
    HitboxRuleWire {
        motion,
        category: CAT_MOTION_MODULE_SET_RATE,
        hitbox_id: None,
        suppress: false,
        frame_start: None,
        frame_end: None,
        overrides: None,
        inject: Some(InjectRuleWire {
            frame: frame as f32,
            args: vec![LuaArgWire::Num(speed)],
            command: Some("MotionModule::set_rate".into()),
        }),
        func: None,
    }
}

fn speed_range_rules(motion: u64, ranges: &[SpeedRange]) -> Vec<HitboxRuleWire> {
    let mut rules = Vec::with_capacity(ranges.len() * 2);
    for range in ranges {
        let mut start = live_rate_rule(motion, range.start, range.speed);
        // Do not reapply a modified start rate if this interval has already ended.
        start.frame_end = Some(range.end as f32);
        rules.push(start);
        // End is inclusive: the native playback rate returns to 1x on the next frame.
        rules.push(live_rate_rule(motion, range.end + 1, 1.0));
    }
    rules
}

impl VisionaryApp {
    /// Live-only timing rules; no source ACMD call is invented or exported.
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
        let max_frame = self.state.total_frames - 1;
        let suggested = self
            .state
            .hitboxes
            .iter()
            .map(|hitbox| hitbox.active_end)
            .max()
            .unwrap_or(self.state.current_frame)
            .saturating_add(1)
            .clamp(1, max_frame);
        let mut ranges = read_speed_ranges(self.hitbox_rules_store.get(&key), max_frame);
        let mut enabled = !ranges.is_empty();

        ui.separator();
        ui.label("Animation speed (live)");
        ui.weak("Set playback speed for one or more frame ranges of this move.");
        let mut changed = ui.checkbox(&mut enabled, "Custom playback speed").changed();
        if enabled && ranges.is_empty() {
            ranges.push(SpeedRange {
                start: suggested,
                end: suggested.saturating_add(2).min(max_frame),
                speed: 2.0,
            });
            changed = true;
        }

        if enabled {
            let mut remove = None;
            let mut previous_end = 0;
            for idx in 0..ranges.len() {
                let next_start = ranges
                    .get(idx + 1)
                    .map(|range| range.start)
                    .unwrap_or(max_frame + 1);
                let range = &mut ranges[idx];
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("Range {}", idx + 1));
                    ui.label("From");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut range.start)
                                .range((previous_end + 1)..=next_start.saturating_sub(1)),
                        )
                        .changed();
                    if range.start > range.end {
                        range.end = range.start;
                        changed = true;
                    }
                    ui.label("To");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut range.end)
                                .range(range.start..=next_start.saturating_sub(1)),
                        )
                        .changed();
                    ui.label("Speed");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut range.speed)
                                .speed(0.1)
                                .range(MIN_SPEED..=MAX_SPEED)
                                .suffix("x"),
                        )
                        .changed();
                    if ui.small_button("Remove").clicked() {
                        remove = Some(idx);
                    }
                });
                previous_end = range.end;
            }
            if let Some(idx) = remove {
                ranges.remove(idx);
                changed = true;
            }
            let next_frame = ranges
                .last()
                .map(|range| range.end.saturating_add(1))
                .unwrap_or(suggested);
            if ui
                .add_enabled(next_frame <= max_frame, egui::Button::new("+ Add range"))
                .clicked()
            {
                ranges.push(SpeedRange {
                    start: next_frame,
                    end: next_frame.saturating_add(2).min(max_frame),
                    speed: 2.0,
                });
                changed = true;
            }
            if next_frame > max_frame {
                ui.weak("Shorten the last range to add another.");
            }
        }
        ui.weak("End frame is included. Speed returns to 1x on the following frame.");
        ui.weak("Live preview only; these ranges are not saved or exported.");

        if !changed {
            return;
        }

        if enabled && !ranges.is_empty() {
            self.hitbox_rules_store
                .insert(key, speed_range_rules(motion, &ranges));
        } else {
            self.hitbox_rules_store.remove(&key);
        }

        self.send_live_hitbox_rules();
        self.state.status = if enabled && !ranges.is_empty() {
            format!("{} animation speed range(s) sent for live preview", ranges.len())
        } else {
            "Live animation speed ranges removed".into()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiple_ranges_restore_normal_speed_between_them() {
        let ranges = [
            SpeedRange {
                start: 11,
                end: 13,
                speed: 0.1,
            },
            SpeedRange {
                start: 20,
                end: 25,
                speed: 10.0,
            },
        ];
        let rules = speed_range_rules(0x1234, &ranges);
        assert_eq!(rules.len(), 4);
        assert_eq!(rules[0].frame_end, Some(13.0));
        assert_eq!(rules[1].frame_end, None);
        assert_eq!(rules[2].frame_end, Some(25.0));
        let events: Vec<(u32, f32)> =
            rules.iter().map(|rule| rate_from_rule(rule).unwrap()).collect();
        assert_eq!(events, vec![(11, 0.1), (14, 1.0), (20, 10.0), (26, 1.0)]);
        assert_eq!(read_speed_ranges(Some(&rules), 50), ranges.to_vec());
    }

    #[test]
    fn reads_legacy_recovery_rule() {
        let old = vec![live_rate_rule(0x1234, 11, 2.0)];
        assert_eq!(
            read_speed_ranges(Some(&old), 40),
            vec![SpeedRange {
                start: 11,
                end: 40,
                speed: 2.0,
            }]
        );
    }
}
