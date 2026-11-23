//! Live scalar edits of fighter/common/param/fighter_param.prc (Smash 13.0.5 only).
//! All writes occur in the existing game-frame driver; no PRC reload or new hook.

#[path = "fighter_trait_fields.rs"]
mod fields;

use fields::{lookup, ValueKind};
use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Deserialize)]
pub enum TraitValue {
    Bool(bool),
    I8(i8),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    Float(f32),
    Hash(u64),
}

#[derive(Clone, Debug, Deserialize)]
pub struct TraitRule {
    pub fighter: String,
    pub fields: BTreeMap<String, TraitValue>,
}

const FIGHTER_TABLE_SINGLETON_13_0_5: usize = 0x52bc3b0;
const ROW_COUNT: usize = 94;
const ROW_SIZE: usize = 0x628;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bits {
    Float(u32),
    Integer(i32),
    Bool(u8),
    Hash(u64),
}

impl TraitValue {
    fn as_bits(self, kind: ValueKind) -> Option<Bits> {
        match (self, kind) {
            (Self::Float(v), ValueKind::Float) if v.is_finite() && v.abs() <= 1_000_000.0 =>
                Some(Bits::Float(v.to_bits())),
            (Self::Bool(v), ValueKind::Bool) => Some(Bits::Bool(u8::from(v))),
            (Self::I8(v), ValueKind::Integer) => Some(Bits::Integer(i32::from(v))),
            (Self::U8(v), ValueKind::Integer) => Some(Bits::Integer(i32::from(v))),
            (Self::I16(v), ValueKind::Integer) => Some(Bits::Integer(i32::from(v))),
            (Self::U16(v), ValueKind::Integer) => Some(Bits::Integer(i32::from(v))),
            (Self::I32(v), ValueKind::Integer) => Some(Bits::Integer(v)),
            (Self::U32(v), ValueKind::Integer) => i32::try_from(v).ok().map(Bits::Integer),
            (Self::Hash(v), ValueKind::Hash) if v <= HASH40_MASK => Some(Bits::Hash(v)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
struct AppliedValue {
    offset: usize,
    original: Bits,
    written: Bits,
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
            && self.fighter.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            && !self.fields.is_empty()
            && self.fields.len() <= 369
            && self.fields.keys().all(|key| !key.is_empty() && key.len() <= 96)
    }
}

/// Full project snapshot. Invalid fields are skipped individually, not coerced.
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
    let count = accepted.len();
    let field_count: usize = accepted.iter().map(|r| r.fields.len()).sum();
    *RULES.write() = accepted;
    STATE.lock().dirty = true;
    crate::slight::diag::note(format!("trait rules applied: {count} fighters, {field_count} fields"));
}

/// Ghidra 13.0.5: singleton -> object -> [begin,end] of 94 x 0x628-byte rows.
unsafe fn read_table() -> Option<Table> {
    if !crate::slight::ver::is_13_0_5() {
        return None;
    }
    let text = skyline::hooks::getRegionAddress(skyline::hooks::Region::Text) as usize;
    if text == 0 {
        return None;
    }
    let slot = text.checked_add(FIGHTER_TABLE_SINGLETON_13_0_5)?;
    let owner = (slot as *const usize).read_volatile();
    if owner < 0x10000 || owner & 7 != 0 {
        return None;
    }
    let vector = (owner as *const usize).read_volatile();
    if vector < 0x10000 || vector & 7 != 0 {
        return None;
    }
    let start = ((vector + 8) as *const usize).read_volatile();
    let end = ((vector + 16) as *const usize).read_volatile();
    if start < 0x10000 || start & 7 != 0 || end <= start
        || end.checked_sub(start)? != ROW_COUNT * ROW_SIZE
    {
        return None;
    }
    if ((start as *const u64).read_volatile() & HASH40_MASK)
        != (smash::hash40("fighter_kind_mario") & HASH40_MASK)
    {
        return None;
    }
    Some(Table { start, end })
}

unsafe fn read_bits(table: Table, offset: usize, kind: ValueKind) -> Option<Bits> {
    let width = match kind { ValueKind::Hash => 8, ValueKind::Bool => 1, _ => 4 };
    if offset < table.start || offset.checked_add(width)? > table.end {
        return None;
    }
    match kind {
        ValueKind::Float => {
            let bits = (offset as *const u32).read_volatile();
            f32::from_bits(bits).is_finite().then_some(Bits::Float(bits))
        }
        ValueKind::Integer => Some(Bits::Integer((offset as *const i32).read_volatile())),
        ValueKind::Bool => {
            let v = (offset as *const u8).read_volatile();
            (v <= 1).then_some(Bits::Bool(v))
        }
        ValueKind::Hash => {
            let v = (offset as *const u64).read_volatile();
            (v <= HASH40_MASK).then_some(Bits::Hash(v))
        }
    }
}

unsafe fn write_bits(table: Table, offset: usize, value: Bits) -> bool {
    let kind = match value {
        Bits::Float(_) => ValueKind::Float,
        Bits::Integer(_) => ValueKind::Integer,
        Bits::Bool(_) => ValueKind::Bool,
        Bits::Hash(_) => ValueKind::Hash,
    };
    if read_bits(table, offset, kind).is_none() {
        return false;
    }
    match value {
        Bits::Float(v) => (offset as *mut u32).write_volatile(v),
        Bits::Integer(v) => (offset as *mut i32).write_volatile(v),
        Bits::Bool(v) => (offset as *mut u8).write_volatile(v),
        Bits::Hash(v) => (offset as *mut u64).write_volatile(v),
    };
    true
}

unsafe fn restore(table: Table, previous: AppliedValue) -> bool {
    let kind = match previous.written {
        Bits::Float(_) => ValueKind::Float,
        Bits::Integer(_) => ValueKind::Integer,
        Bits::Bool(_) => ValueKind::Bool,
        Bits::Hash(_) => ValueKind::Hash,
    };
    if read_bits(table, previous.offset, kind) != Some(previous.written) {
        // Another plugin owns this value now: do not overwrite it.
        return false;
    }
    write_bits(table, previous.offset, previous.original)
}

/// Game-thread application. Rechecks the table identity periodically after a reload.
pub fn tick() {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    let mut state = STATE.lock();
    state.frames = state.frames.wrapping_add(1);
    if !state.dirty && state.frames % 120 != 0 {
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
        // A reallocated table invalidates all old field pointers.
        state.applied.clear();
        state.table = Some(identity);
        state.dirty = true;
    }
    if !state.dirty {
        return;
    }
    state.dirty = false;

    let mut restored = 0;
    let mut conflicts = BTreeSet::new();
    for previous in std::mem::take(&mut state.applied) {
        if unsafe { restore(table, previous) } {
            restored += 1;
        } else {
            conflicts.insert(previous.offset);
        }
    }

    let mut matched = 0usize;
    let mut written = 0usize;
    let mut skipped = 0usize;
    for rule in &rules {
        let hash = smash::hash40(&format!("fighter_kind_{}", rule.fighter)) & HASH40_MASK;
        let row = (0..ROW_COUNT)
            .map(|i| table.start + i * ROW_SIZE)
            .find(|&addr| unsafe { (addr as *const u64).read_volatile() & HASH40_MASK } == hash);
        let Some(row) = row else {
            crate::slight::diag::note(format!("TRAITS fighter not found in table: {}", rule.fighter));
            skipped += rule.fields.len();
            continue;
        };
        matched += 1;
        let mut per_fighter_offsets = BTreeSet::new();
        for (key, value) in &rule.fields {
            let Some(spec) = lookup(key) else {
                skipped += 1;
                continue;
            };
            let Some(wanted) = value.as_bits(spec.kind) else {
                skipped += 1;
                continue;
            };
            let Some(offset) = row.checked_add(spec.offset) else {
                skipped += 1;
                continue;
            };
            if !per_fighter_offsets.insert(offset) || conflicts.contains(&offset) {
                skipped += 1;
                continue;
            }
            let Some(original) = (unsafe { read_bits(table, offset, spec.kind) }) else {
                skipped += 1;
                continue;
            };
            if original == wanted {
                continue;
            }
            if !unsafe { write_bits(table, offset, wanted) } {
                skipped += 1;
                continue;
            }
            if written < 20 {
                crate::slight::diag::note(format!(
                    "TRAITS {} {} +{:#x}: {:?} -> {:?}",
                    rule.fighter, key, spec.offset, original, wanted
                ));
            }
            state.applied.push(AppliedValue { offset, original, written: wanted });
            written += 1;
        }
    }
    crate::slight::diag::note(format!(
        "TRAITS table snapshot: matched={matched}/{} written={written} restored={restored} changed_elsewhere={} skipped={skipped}",
        rules.len(), conflicts.len()
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matching_variant_is_required() {
        assert_eq!(TraitValue::Float(2.0).as_bits(ValueKind::Float), Some(Bits::Float(2.0_f32.to_bits())));
        assert_eq!(TraitValue::Bool(true).as_bits(ValueKind::Bool), Some(Bits::Bool(1)));
        assert_eq!(TraitValue::I16(7).as_bits(ValueKind::Integer), Some(Bits::Integer(7)));
        assert_eq!(TraitValue::Hash(0xff).as_bits(ValueKind::Hash), Some(Bits::Hash(0xff)));
        assert_eq!(TraitValue::Hash(4).as_bits(ValueKind::Float), None);
        assert_eq!(TraitValue::U32(u32::MAX).as_bits(ValueKind::Integer), None);
        assert_eq!(TraitValue::Float(f32::NAN).as_bits(ValueKind::Float), None);
    }
}
