//! Live overrides of the shared fighter_param table (Smash 13.0.5 only).
//! Addresses and row layout confirmed against main 13.0.5 in Ghidra.
//! The game-thread driver owns all writes; no hook or PRC file is replaced.

use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

#[derive(Clone, Debug, Deserialize)]
pub struct TraitRule {
    pub fighter: String,
    pub weight: Option<f32>,
    pub air_speed_x_stable: Option<f32>,
}

const FIGHTER_TABLE_SINGLETON_13_0_5: usize = 0x52bc3b0;
const ROW_COUNT: usize = 94;
const ROW_SIZE: usize = 0x628;
const WEIGHT_OFFSET: usize = 0x80;
const AIR_SPEED_OFFSET: usize = 0x60;
const HASH40_MASK: u64 = 0xffffffffff;

static ENABLED: AtomicBool = AtomicBool::new(false);
static RULES: RwLock<Vec<TraitRule>> = RwLock::new(Vec::new());
static STATE: LazyLock<Mutex<LiveState>> = LazyLock::new(|| Mutex::new(LiveState::default()));

#[derive(Default)]
struct LiveState {
    table: Option<(usize, usize)>,
    applied: Vec<AppliedValue>,
    dirty: bool,
    frames: u64,
}

#[derive(Clone, Copy)]
struct AppliedValue {
    offset: usize,
    original: f32,
    written: f32,
}

#[derive(Clone, Copy)]
struct Table {
    start: usize,
    end: usize,
}

impl TraitRule {
    fn valid(&self) -> bool {
        !self.fighter.is_empty()
            && self.fighter.len() <= 40
            && self
                .fighter
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            && self.weight.is_none_or(|v| v.is_finite() && v > 0.0 && v <= 1000.0)
            && self
                .air_speed_x_stable
                .is_none_or(|v| v.is_finite() && (0.0..=20.0).contains(&v))
            && (self.weight.is_some() || self.air_speed_x_stable.is_some())
    }
}

/// Full replacement from the editor. Empty rules restore the captured values.
pub fn set_rules(rules: Vec<TraitRule>) {
    let mut accepted = Vec::new();
    for rule in rules {
        if rule.valid()
            && accepted.len() < ROW_COUNT
            && !accepted.iter().any(|r: &TraitRule| r.fighter == rule.fighter)
        {
            accepted.push(rule);
        }
    }

    let details = accepted
        .iter()
        .take(3)
        .map(|r| format!("{} weight={:?} air={:?}", r.fighter, r.weight, r.air_speed_x_stable))
        .collect::<Vec<_>>()
        .join("; ");
    let count = accepted.len();
    *RULES.write() = accepted;
    STATE.lock().dirty = true;
    crate::slight::diag::note(format!("trait rules applied: {count} {details}"));
}

/// Resolve only the version-verified global vector; never scan arbitrary heap memory.
unsafe fn read_table() -> Option<Table> {
    if !crate::slight::ver::is_13_0_5() {
        return None;
    }

    let text = skyline::hooks::getRegionAddress(skyline::hooks::Region::Text) as usize;
    let slot = text.checked_add(FIGHTER_TABLE_SINGLETON_13_0_5)?;
    if text == 0 {
        return None;
    }
    let owner = (slot as *const usize).read_volatile();
    if owner < 0x10000 || owner & 7 != 0 {
        return None;
    }
    let vector = (owner as *const usize).read_volatile();
    if vector < 0x10000 || vector & 7 != 0 {
        return None;
    }

    // Ghidra: singleton -> param object -> vector [begin, end] at +8/+16.
    let start = ((vector + 8) as *const usize).read_volatile();
    let end = ((vector + 16) as *const usize).read_volatile();
    if start < 0x10000 || start & 7 != 0 || end <= start {
        return None;
    }
    let bytes = end.checked_sub(start)?;
    if bytes != ROW_COUNT * ROW_SIZE {
        return None;
    }

    // Ghidra and the game's fighter_param layout agree that Mario is row zero.
    if ((start as *const u64).read_volatile() & HASH40_MASK)
        != (smash::hash40("fighter_kind_mario") & HASH40_MASK)
    {
        return None;
    }
    Some(Table { start, end })
}

unsafe fn restore(table: Table, value: AppliedValue) -> bool {
    let Some(end) = value.offset.checked_add(std::mem::size_of::<f32>()) else {
        return false;
    };
    if value.offset < table.start || end > table.end {
        return false;
    }
    let field = value.offset as *mut f32;
    // Another plugin may have written to the same field in the meantime.
    if field.read_volatile().to_bits() != value.written.to_bits() {
        return false;
    }
    field.write_volatile(value.original);
    true
}

unsafe fn override_field(table: Table, row: usize, field_offset: usize, desired: f32) -> Option<AppliedValue> {
    let offset = row.checked_add(field_offset)?;
    if offset < table.start || offset.checked_add(4)? > table.end {
        return None;
    }
    let field = offset as *mut f32;
    let original = field.read_volatile();
    let plausible = if field_offset == WEIGHT_OFFSET {
        original.is_finite() && original > 0.0 && original <= 1000.0
    } else {
        original.is_finite() && (0.0..=20.0).contains(&original)
    };
    if !plausible || original.to_bits() == desired.to_bits() {
        return None;
    }
    field.write_volatile(desired);
    Some(AppliedValue { offset, original, written: desired })
}

/// Called from the existing global game-thread facade, not from a socket thread.
pub fn tick() {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let mut state = STATE.lock();
    state.frames = state.frames.wrapping_add(1);

    if !state.dirty && (state.applied.is_empty() || state.frames % 120 != 0) {
        return;
    }

    let rules = RULES.read().clone();
    if rules.is_empty() && state.applied.is_empty() {
        state.dirty = false;
        return;
    }

    let Some(table) = (unsafe { read_table() }) else {
        if state.dirty && state.frames % 240 == 0 {
            crate::slight::diag::note("TRAITS table not ready; no memory written");
        }
        return;
    };

    let identity = (table.start, table.end);
    if state.table != Some(identity) {
        // Old addresses may have been freed by a reload: never restore through them.
        state.applied.clear();
        state.table = Some(identity);
        state.dirty = true;
    }
    if !state.dirty {
        return;
    }
    state.dirty = false;

    let mut restored = 0;
    let mut conflicts = 0;
    for previous in std::mem::take(&mut state.applied) {
        if unsafe { restore(table, previous) } {
            restored += 1;
        } else {
            conflicts += 1;
        }
    }

    let mut matched = 0;
    let mut written = 0;
    for rule in &rules {
        let hash = smash::hash40(&format!("fighter_kind_{}", rule.fighter)) & HASH40_MASK;
        let row = (0..ROW_COUNT)
            .map(|i| table.start + i * ROW_SIZE)
            .find(|&addr| unsafe { (addr as *const u64).read_volatile() & HASH40_MASK } == hash);
        let Some(row) = row else {
            crate::slight::diag::note(format!("TRAITS fighter not found in table: {}", rule.fighter));
            continue;
        };
        matched += 1;
        for (field_offset, value) in [
            (WEIGHT_OFFSET, rule.weight),
            (AIR_SPEED_OFFSET, rule.air_speed_x_stable),
        ] {
            if let Some(desired) = value {
                if let Some(applied) = unsafe { override_field(table, row, field_offset, desired) } {
                    crate::slight::diag::note(format!(
                        "TRAITS {} +{field_offset:#x}: {} -> {}",
                        rule.fighter, applied.original, desired
                    ));
                    state.applied.push(applied);
                    written += 1;
                }
            }
        }
    }
    crate::slight::diag::note(format!(
        "TRAITS table snapshot: matched={matched}/{} written={written} restored={restored} changed_elsewhere={conflicts}",
        rules.len()
    ));
}

pub fn install() {
    if !crate::slight::ver::is_13_0_5() {
        skyline::println!("[SLight] Traits disabled: requires Smash 13.0.5");
        return;
    }
    ENABLED.store(true, Ordering::Release);
    skyline::println!("[SLight] Fighter trait table overrides enabled (experimental)");
}
