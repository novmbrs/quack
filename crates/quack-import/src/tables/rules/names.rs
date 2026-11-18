//! Active rule names are unique per server, case-insensitively (§19.5):
//! the later of two clashing rules gets " (2)", " (3)" ….

use std::collections::HashSet;

use super::RuleRow;
use crate::report::RenamedRule;

/// Renames clashing non-archived rules in place: the earliest by
/// `(created_at, id)` keeps its name; each new name is checked against
/// every name in the server, archived ones included.
pub(super) fn dedupe(rules: &mut [RuleRow]) -> Vec<RenamedRule> {
    let mut taken: HashSet<String> = rules.iter().map(|r| r.name.to_lowercase()).collect();
    let mut order: Vec<usize> = (0..rules.len())
        .filter(|i| rules[*i].archived_at.is_none())
        .collect();
    order.sort_by(|a, b| {
        (rules[*a].created_at, rules[*a].id).cmp(&(rules[*b].created_at, rules[*b].id))
    });
    let mut active: HashSet<String> = HashSet::new();
    let mut renamed = Vec::new();
    for i in order {
        let lower = rules[i].name.to_lowercase();
        if active.insert(lower) {
            continue;
        }
        let base = rules[i].name.clone();
        let mut n = 2;
        let name = loop {
            let candidate = format!("{base} ({n})");
            if !taken.contains(&candidate.to_lowercase()) {
                break candidate;
            }
            n += 1;
        };
        taken.insert(name.to_lowercase());
        active.insert(name.to_lowercase());
        renamed.push(RenamedRule {
            rule_id: rules[i].id.to_string(),
            old_name: base,
            new_name: name.clone(),
        });
        rules[i].name = name;
    }
    renamed
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn rule(n: u128, name: &str, archived: bool) -> RuleRow {
        RuleRow {
            id: Uuid::from_u128(n),
            key: n.to_string(),
            name: name.to_owned(),
            current_version: 1,
            archived_at: archived.then_some(OffsetDateTime::UNIX_EPOCH),
            created_at: OffsetDateTime::UNIX_EPOCH
                + time::Duration::seconds(i64::try_from(n).unwrap()),
            created_by: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            updated_by: None,
            rebuilt: false,
            versions: BTreeMap::new(),
        }
    }

    #[test]
    fn later_clashes_are_numbered_around_existing_names() {
        let mut rules = vec![
            rule(1, "Spam", false),
            rule(2, "spam", false),
            rule(3, "Spam (2)", true),
            rule(4, "SPAM", false),
        ];
        let renamed = dedupe(&mut rules);
        assert_eq!(rules[0].name, "Spam");
        assert_eq!(rules[1].name, "spam (3)");
        assert_eq!(rules[2].name, "Spam (2)");
        assert_eq!(rules[3].name, "SPAM (4)");
        assert_eq!(renamed.len(), 2);
    }

    proptest! {
        #[test]
        fn active_names_end_up_unique(names in proptest::collection::vec(("[aA]{1,2}( \\(2\\))?", any::<bool>()), 1..12)) {
            let mut rules: Vec<RuleRow> = names
                .iter()
                .enumerate()
                .map(|(i, (n, archived))| rule(i as u128 + 1, n, *archived))
                .collect();
            let renamed = dedupe(&mut rules);
            let active: Vec<String> = rules.iter().filter(|r| r.archived_at.is_none()).map(|r| r.name.to_lowercase()).collect();
            let unique: HashSet<&String> = active.iter().collect();
            prop_assert_eq!(unique.len(), active.len());
            for r in rules.iter().filter(|r| r.archived_at.is_some()) {
                prop_assert!(!renamed.iter().any(|x| x.rule_id == r.id.to_string()));
            }
        }
    }
}
