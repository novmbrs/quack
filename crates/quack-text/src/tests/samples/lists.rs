//! §12.12 case lists and member history.

use super::{Out, Sample, covering, sample};
use crate::case::{self, CaseRow, HistoryTotals, ListFilter, ListPage};
use crate::tests::fixtures::*;

pub fn samples() -> Vec<Sample> {
    let row = CaseRow {
        case: case_id(),
        number: 12,
        member: sam(),
        rule: "Spam".to_owned(),
        created_at: t0(),
        voided: false,
        imported: false,
    };
    let rows = vec![
        row.clone(),
        CaseRow {
            number: 11,
            voided: true,
            ..row.clone()
        },
        CaseRow {
            number: 3,
            imported: true,
            voided: true,
            ..row.clone()
        },
    ];
    let maximal: Vec<CaseRow> = (0..10)
        .map(|i| CaseRow {
            number: 9_007_199_254_740_991 - i,
            rule: long(100),
            member: crate::model::Person::new(account(222), long(32)),
            voided: true,
            imported: true,
            ..row.clone()
        })
        .collect();
    let page = |rows: Vec<CaseRow>, page, pages| ListPage { rows, page, pages };
    let member = Some((member_id(), sam()));
    let rule = Some((rule_id(), "Spam".to_owned()));
    let mut v = vec![
        covering(
            "case.list",
            &["case.list.heading", "case.list.row"],
            "no filter",
            Out::Msg(case::list(
                &ListFilter::default(),
                &page(rows.clone(), 1, 1),
            )),
        ),
        covering(
            "case.list",
            &["case.list.heading_member"],
            "member",
            Out::Msg(case::list(
                &ListFilter {
                    member: member.clone(),
                    rule: None,
                },
                &page(rows.clone(), 1, 2),
            )),
        ),
        covering(
            "case.list",
            &["case.list.heading_rule"],
            "rule",
            Out::Msg(case::list(
                &ListFilter {
                    member: None,
                    rule: rule.clone(),
                },
                &page(rows.clone(), 2, 2),
            )),
        ),
        covering(
            "case.list",
            &["case.list.heading_both"],
            "both",
            Out::Msg(case::list(
                &ListFilter {
                    member: member.clone(),
                    rule,
                },
                &page(rows.clone(), 2, 3),
            )),
        ),
        covering(
            "case.list",
            &["case.list.empty"],
            "empty",
            Out::Msg(case::list(&ListFilter::default(), &page(vec![], 1, 1))),
        ),
        sample(
            "case.list",
            "maximal",
            Out::Msg(case::list(
                &ListFilter::default(),
                &page(maximal.clone(), 2, 3),
            )),
        ),
    ];
    let totals = HistoryTotals {
        total: 3,
        valid: 1,
        voided: 2,
    };
    v.push(covering(
        "case.history",
        &["case.history.heading", "case.history.totals"],
        "",
        Out::Msg(case::history(
            member_id(),
            &sam(),
            totals,
            &page(rows, 1, 1),
        )),
    ));
    v.push(sample(
        "case.history",
        "one case",
        Out::Msg(case::history(
            member_id(),
            &sam(),
            HistoryTotals {
                total: 1,
                valid: 1,
                voided: 0,
            },
            &page(vec![row], 1, 1),
        )),
    ));
    v.push(sample(
        "case.history",
        "maximal",
        Out::Msg(case::history(
            member_id(),
            &sam(),
            HistoryTotals {
                total: 1_234,
                valid: 1_000,
                voided: 234,
            },
            &page(maximal, 5, 124),
        )),
    ));
    v.push(covering(
        "case.history.empty",
        &[],
        "",
        Out::Msg(case::history(
            member_id(),
            &sam(),
            HistoryTotals::default(),
            &page(vec![], 1, 1),
        )),
    ));
    v
}
