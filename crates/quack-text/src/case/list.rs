//! `/case list` and `/case history` (§12.12): 10 rows per page, an "Open a
//! case" select, and a pager.

use quack_core::ids::{CaseId, MemberId, RuleId};
use time::OffsetDateTime;

use crate::actions::ButtonAction;
use crate::fmt;
use crate::model::{Block, Msg, Person, Text, TimeStyle, Tone};
use crate::word::{self, CaseOption};

/// One case in a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseRow {
    /// The case.
    pub case: CaseId,
    /// Its number.
    pub number: u64,
    /// The member.
    pub member: Person,
    /// The rule's name.
    pub rule: String,
    /// When it was added.
    pub created_at: OffsetDateTime,
    /// Whether it is voided.
    pub voided: bool,
    /// Whether it was imported from Quack v4.
    pub imported: bool,
}

/// The filters of a `/case list`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListFilter {
    /// Only cases about this member.
    pub member: Option<(MemberId, Person)>,
    /// Only cases under this rule.
    pub rule: Option<(RuleId, String)>,
}

/// One page of a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListPage {
    /// The rows, newest first (at most 10).
    pub rows: Vec<CaseRow>,
    /// This page, 1-based.
    pub page: u32,
    /// How many pages there are.
    pub pages: u32,
}

fn row(r: &CaseRow) -> Block {
    let mut t = Text::new()
        .bold(format!("#{}", r.number))
        .s(" · ")
        .person(&r.member)
        .s(" · ")
        .user(fmt::one_line(&r.rule))
        .s(" · ")
        .time(r.created_at, TimeStyle::Date);
    if r.voided {
        t = t.s(" · ").s(word::VOIDED);
    }
    if r.imported {
        t = t.s(" · ").s(word::IMPORTED);
    }
    Block::Text(t)
}

fn rows_and_controls(mut msg: Msg, page: &ListPage, to: impl Fn(u32) -> ButtonAction) -> Msg {
    for r in &page.rows {
        msg = msg.block(row(r));
    }
    if page.pages > 1 {
        msg = msg.block(Block::Subtext(Text::fixed(word::page(
            page.page, page.pages,
        ))));
    }
    let options: Vec<CaseOption> = page
        .rows
        .iter()
        .map(|r| CaseOption {
            case: r.case,
            number: r.number,
            rule: r.rule.clone(),
        })
        .collect();
    if let Some(select) = word::open_case_select(&options) {
        msg = msg.select(select);
    }
    msg.buttons(word::pager(page.page, page.pages, to))
}

/// `case.list.*`.
pub fn list(filter: &ListFilter, page: &ListPage) -> Msg {
    let heading = match (&filter.member, &filter.rule) {
        (None, None) => Text::fixed("Recent cases"),
        (Some((_, m)), None) => Text::fixed("Recent cases about ").person(m),
        (None, Some((_, r))) => Text::fixed("Recent cases under ").bold_user(r),
        (Some((_, m)), Some((_, r))) => Text::fixed("Recent cases about ")
            .person(m)
            .s(" under ")
            .bold_user(r),
    };
    let mut msg = Msg::new(Tone::Neutral).block(Block::Heading(heading));
    if page.rows.is_empty() {
        msg = msg.para(Text::fixed(
            "No cases match these filters. Try choosing a different member or rule.",
        ));
    }
    let member = filter.member.as_ref().map(|(id, _)| *id);
    let rule = filter.rule.as_ref().map(|(id, _)| *id);
    rows_and_controls(msg, page, |page| ButtonAction::CaseListPage {
        page,
        member,
        rule,
    })
}

/// A member's case totals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistoryTotals {
    /// Every case about the member.
    pub total: u64,
    /// Cases that count.
    pub valid: u64,
    /// Voided cases.
    pub voided: u64,
}

/// `case.history.*`.
pub fn history(
    member_id: MemberId,
    member: &Person,
    totals: HistoryTotals,
    page: &ListPage,
) -> Msg {
    let mut msg = Msg::new(Tone::Neutral).block(Block::Heading(
        Text::fixed("Case history for ").person(member),
    ));
    if totals.total == 0 {
        msg = msg.para(
            Text::new()
                .person(member)
                .s(" has no cases in this server."),
        );
    } else {
        msg = msg.para(Text::fixed(format!(
            "{} · {} valid · {} voided",
            fmt::plural(totals.total, "case", "cases"),
            fmt::count(totals.valid),
            fmt::count(totals.voided)
        )));
    }
    rows_and_controls(msg, page, |page| ButtonAction::History {
        member: member_id,
        page,
    })
}
