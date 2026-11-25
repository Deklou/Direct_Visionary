//! Runtime identity shared by all scoped live ACMD rule families.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeTarget {
    pub fighter_kind: i32,
    pub costume: i32,
}

impl RuntimeTarget {
    pub const UNKNOWN: Self = Self { fighter_kind: -1, costume: -1 };

    pub fn matches(self, fighter_kind: Option<i32>, slots: Option<&[u8]>) -> bool {
        self.fighter_kind >= 0
            && fighter_kind == Some(self.fighter_kind)
            && u8::try_from(self.costume).ok().is_some_and(|costume| {
                slots.is_some_and(|slots| slots.contains(&costume))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_acmd_target_requires_fighter_and_explicit_costume() {
        let target = RuntimeTarget { fighter_kind: 0, costume: 9 };
        assert!(target.matches(Some(0), Some(&[8, 9, 10])));
        assert!(!target.matches(Some(0), Some(&[0, 1, 2, 3, 4, 5, 6, 7])));
        assert!(!target.matches(Some(1), Some(&[8, 9, 10])));
        assert!(!target.matches(None, Some(&[9])));
        assert!(!target.matches(Some(0), None));
        assert!(!target.matches(Some(0), Some(&[])));
        assert!(!RuntimeTarget::UNKNOWN.matches(Some(-1), Some(&[255])));
        assert!(!RuntimeTarget { fighter_kind: 0, costume: 256 }.matches(Some(0), Some(&[0])));
        assert!(RuntimeTarget { fighter_kind: 0, costume: 255 }.matches(Some(0), Some(&[255])));
    }
}
