//! Choosing a rule's level from the member's case count (§3.4).

use time::{Duration, OffsetDateTime};

use crate::rule::{LevelAction, RuleDefinition};

/// The level a new case gets, as recorded in the case's decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelChoice {
    /// 1-based position of the level in the rule version.
    pub index: u8,
    /// The level's `from_case_count`.
    pub from_case_count: u32,
    /// The level's action; `None` makes the case a warning.
    pub action: Option<LevelAction>,
    /// Whether the member gets a case DM.
    pub send_dm: bool,
}

/// Picks the level for a new case: the level with the highest
/// `from_case_count` at or below `case_count`.
///
/// `case_count` is the number of counted cases plus one for the new case.
/// Every version a case can use starts at 1, so there is always a level;
/// below every level (only a reconstructed imported version, never applied
/// to a new case) the first level applies. `None` only for a definition
/// with no levels, which validation never lets through.
pub fn select_level(def: &RuleDefinition, case_count: u32) -> Option<LevelChoice> {
    let chosen = def
        .levels
        .iter()
        .enumerate()
        .rev()
        .find(|(_, level)| level.from_case_count <= case_count)
        .or_else(|| def.levels.iter().enumerate().next())?;
    let (position, level) = chosen;
    Some(LevelChoice {
        index: u8::try_from(position + 1).ok()?,
        from_case_count: level.from_case_count,
        action: level.action,
        send_dm: level.send_dm,
    })
}

/// The earliest `created_at` a past case may have and still count, or
/// `None` when the rule counts every past case.
///
/// The window always comes from the version being applied.
pub fn counting_cutoff(
    now: OffsetDateTime,
    counting_window_days: Option<u16>,
) -> Option<OffsetDateTime> {
    counting_window_days.map(|days| now - Duration::seconds(i64::from(days) * 86_400))
}

/// Whether a past case created at `created_at` falls inside the window.
/// The cutoff itself counts ("created at or after").
pub fn in_window(created_at: OffsetDateTime, cutoff: Option<OffsetDateTime>) -> bool {
    cutoff.is_none_or(|cutoff| created_at >= cutoff)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use time::macros::datetime;

    use super::*;
    use crate::rule::{Level, starter_rule};

    fn rule_with(froms: &[u32]) -> RuleDefinition {
        let mut def = starter_rule();
        def.levels = froms
            .iter()
            .map(|&from| Level {
                from_case_count: from,
                action: None,
                send_dm: from % 2 == 0,
            })
            .collect();
        def
    }

    #[test]
    fn starter_rule_levels() {
        let def = starter_rule();
        let pick = |n| select_level(&def, n).unwrap();
        assert_eq!(pick(1).index, 1);
        assert_eq!(pick(1).action, None);
        assert_eq!(pick(2).index, 1);
        assert_eq!(pick(3).index, 2);
        assert_eq!(
            pick(3).action,
            Some(LevelAction::Timeout {
                duration_seconds: 86_400
            })
        );
        assert_eq!(pick(4).index, 2);
        assert_eq!(pick(5).index, 3);
        assert_eq!(pick(5).from_case_count, 5);
        assert_eq!(pick(u32::MAX).index, 3);
    }

    #[test]
    fn below_every_level_uses_the_first() {
        let def = rule_with(&[3, 5]);
        assert_eq!(select_level(&def, 1).unwrap().index, 1);
        assert_eq!(select_level(&def, 0).unwrap().from_case_count, 3);
    }

    #[test]
    fn no_levels_means_no_choice() {
        assert_eq!(select_level(&rule_with(&[]), 1), None);
    }

    #[test]
    fn window_boundaries() {
        let now = datetime!(2026-11-17 12:00 UTC);
        assert_eq!(counting_cutoff(now, None), None);
        let cutoff = counting_cutoff(now, Some(30));
        assert_eq!(cutoff, Some(datetime!(2026-10-18 12:00 UTC)));
        assert!(in_window(datetime!(2026-10-18 12:00 UTC), cutoff));
        assert!(!in_window(datetime!(2026-10-18 11:59:59.999 UTC), cutoff));
        assert!(in_window(datetime!(2000-01-01 0:00 UTC), None));
    }

    fn levels() -> impl Strategy<Value = Vec<u32>> {
        prop::collection::btree_set(2_u32..=1_000_000, 0..10).prop_map(|rest| {
            let mut v = vec![1];
            v.extend(rest);
            v
        })
    }

    proptest! {
        #[test]
        fn monotonic_in_case_count(froms in levels(), a in 1_u32..2_000_000, b in 1_u32..2_000_000) {
            let def = rule_with(&froms);
            let (lo, hi) = (a.min(b), a.max(b));
            let (l, h) = (select_level(&def, lo).unwrap(), select_level(&def, hi).unwrap());
            prop_assert!(l.index <= h.index);
            prop_assert!(l.from_case_count <= h.from_case_count);
        }

        #[test]
        fn chosen_level_is_the_highest_reached(froms in levels(), n in 1_u32..2_000_000) {
            let def = rule_with(&froms);
            let choice = select_level(&def, n).unwrap();
            let level = &def.levels[usize::from(choice.index) - 1];
            prop_assert_eq!(level.from_case_count, choice.from_case_count);
            prop_assert_eq!(level.send_dm, choice.send_dm);
            prop_assert!(choice.from_case_count <= n);
            if let Some(next) = def.levels.get(usize::from(choice.index)) {
                prop_assert!(next.from_case_count > n);
            }
        }

        #[test]
        fn first_level_for_one_and_last_for_large(froms in levels()) {
            let def = rule_with(&froms);
            prop_assert_eq!(select_level(&def, 1).unwrap().index, 1);
            let last = u8::try_from(froms.len()).unwrap();
            prop_assert_eq!(select_level(&def, 1_000_000).unwrap().index, last);
            prop_assert_eq!(select_level(&def, u32::MAX).unwrap().index, last);
        }

        #[test]
        fn default_level_below_all_thresholds(first in 2_u32..=1_000_000, n in 0_u32..2) {
            let def = rule_with(&[first]);
            prop_assert_eq!(select_level(&def, n).unwrap().index, 1);
        }

        #[test]
        fn window_includes_exactly_cases_at_or_after_the_cutoff(
            days in 1_u16..=36_500,
            offset in -10_000_i64..10_000,
        ) {
            let now = datetime!(2026-11-17 12:00 UTC);
            let cutoff = counting_cutoff(now, Some(days)).unwrap();
            prop_assert_eq!(now - cutoff, Duration::days(i64::from(days)));
            let created = cutoff + Duration::seconds(offset);
            prop_assert_eq!(in_window(created, Some(cutoff)), offset >= 0);
        }
    }
}
