//! Encoding and decoding current (`1:`) ids.

use quack_core::ids::{
    ActionId, AppealId, CaseId, ChannelId, EvidenceId, JobId, MemberId, MessageId, RuleId, TicketId,
};
use quack_text::actions::DraftPage;
use quack_text::ops::ServerSort;

use super::CustomId;
use super::setup::{area, area_code, op, op_code};

/// Strict decimal: no sign, no leading zeros (so every number has one
/// spelling and decoding round-trips exactly).
fn number<T: std::str::FromStr>(s: &str) -> Option<T> {
    let canonical = !s.is_empty()
        && s.len() <= 20
        && s.bytes().all(|b| b.is_ascii_digit())
        && (s == "0" || !s.starts_with('0'));
    canonical.then(|| s.parse().ok()).flatten()
}

fn member(s: &str) -> Option<MemberId> {
    number::<u64>(s).and_then(MemberId::new)
}

/// A message's location: `<channel>:<message>`.
fn located(channel: &str, message: &str) -> Option<(ChannelId, MessageId)> {
    Some((
        number::<u64>(channel).and_then(ChannelId::new)?,
        number::<u64>(message).and_then(MessageId::new)?,
    ))
}

fn draft_page(args: &[&str]) -> Option<DraftPage> {
    match args {
        [draft, rule, version, page] => Some(DraftPage {
            draft: CaseId::from_u22(draft).ok()?,
            rule: RuleId::from_u22(rule).ok()?,
            version: number(version)?,
            page: number(page)?,
        }),
        _ => None,
    }
}

fn draft_args(p: &DraftPage) -> String {
    format!(
        "{}:{}:{}:{}",
        p.draft.to_u22(),
        p.rule.to_u22(),
        p.version,
        p.page
    )
}

/// `args`, then `:n` when the answer goes in a new message.
fn with_new(args: String, new_message: bool) -> String {
    if new_message {
        format!("{args}:n")
    } else {
        args
    }
}

/// Writes `1:<code>[:<arg>]…`.
pub(super) fn encode(id: &CustomId) -> String {
    use CustomId as C;
    let (code, args): (&str, String) = match id {
        C::OpenCase(c) => ("cv", c.to_u22()),
        C::CasePicker => ("cvs", String::new()),
        C::EditContext(c) => ("ce", c.to_u22()),
        C::EditContextModal { case, revision } => ("cem", format!("{}:{revision}", case.to_u22())),
        C::AddEvidence { case, new_message } => ("ea", with_new(case.to_u22(), *new_message)),
        C::AddEvidenceModal { case, new_message } => ("eam", with_new(case.to_u22(), *new_message)),
        C::ViewEvidence {
            case,
            item,
            new_message,
        } => (
            "ev",
            with_new(format!("{}:{item}", case.to_u22()), *new_message),
        ),
        C::BackToEvidence(e) => ("evb", e.to_u22()),
        C::EvidenceSurrounding(e) => ("evs", e.to_u22()),
        C::EvidenceVisibility { evidence, visible } => (
            "evv",
            format!("{}:{}", evidence.to_u22(), u8::from(*visible)),
        ),
        C::EditEvidenceNote(e) => ("evn", e.to_u22()),
        C::EvidenceNoteModal { evidence, revision } => {
            ("evnm", format!("{}:{revision}", evidence.to_u22()))
        }
        C::RemoveEvidence(e) => ("evr", e.to_u22()),
        C::ConfirmRemoveEvidence(e) => ("evrc", e.to_u22()),
        C::AddAsEvidence { channel, message } => ("exs", format!("{channel}:{message}")),
        C::AddAsEvidenceByNumber { channel, message } => ("exn", format!("{channel}:{message}")),
        C::AddAsEvidenceModal { channel, message } => ("exm", format!("{channel}:{message}")),
        C::VoidCase(c) => ("vd", c.to_u22()),
        C::VoidCaseModal(c) => ("vdm", c.to_u22()),
        C::History { member, page } => ("hi", format!("{member}:{page}")),
        C::CaseList { page, member, rule } => (
            "cl",
            format!(
                "{page}:{}:{}",
                member.map_or_else(|| "0".to_owned(), |m| m.to_string()),
                rule.map_or_else(|| "0".to_owned(), RuleId::to_u22)
            ),
        ),
        C::RetryAction(a) => ("ar", a.to_u22()),
        C::DismissAction(a) => ("ad", a.to_u22()),
        C::UndoAction(a) => ("au", a.to_u22()),
        C::ConfirmUndo(a) => ("auc", a.to_u22()),
        C::RulePicker(d) => ("rp", d.to_u22()),
        C::RulePickerPage { draft, page } => ("rpp", format!("{}:{page}", draft.to_u22())),
        C::DraftPageModal(p) => ("cd", draft_args(p)),
        C::DraftContinue(p) => ("cn", draft_args(p)),
        C::DraftRetryPage(p) => ("cr", draft_args(p)),
        C::DraftSaveAgain(d) => ("cs", d.to_u22()),
        C::Appeal(c) => ("ap", c.to_u22()),
        C::AppealModal(c) => ("apm", c.to_u22()),
        C::AcceptAppeal { appeal, version } => ("aa", format!("{}:{version}", appeal.to_u22())),
        C::AcceptAppealModal { appeal, version } => {
            ("aam", format!("{}:{version}", appeal.to_u22()))
        }
        C::RejectAppeal { appeal, version } => ("aj", format!("{}:{version}", appeal.to_u22())),
        C::RejectAppealModal { appeal, version } => {
            ("ajm", format!("{}:{version}", appeal.to_u22()))
        }
        C::OpenAppeal(a) => ("ao", a.to_u22()),
        C::AppealsPage(p) => ("al", p.to_string()),
        C::RulesPage(p) => ("rl", p.to_string()),
        C::Setup(a, o) => ("su", format!("{}:{}", area_code(*a), op_code(*o))),
        C::SetupInviteModal => ("su", "ap:invm".to_owned()),
        C::SetupWarningModal => ("su", "hp:warnm".to_owned()),
        C::OpenTicket => ("to", String::new()),
        C::CloseTicket(t) => ("tc", t.to_u22()),
        C::FixTicketAccess(t) => ("tf", t.to_u22()),
        C::OpsRefresh => ("op", "st".to_owned()),
        C::OpsRequeueJob(j) => ("op", format!("rq:{}", j.to_u22())),
        C::OpsDismissJob(j) => ("op", format!("dm:{}", j.to_u22())),
        C::OpsJobsPage(p) => ("op", format!("jp:{p}")),
        C::OpsServersPage(sort, p) => ("op", format!("sv:{}:{p}", server_sort_code(*sort))),
        C::OpsServersSort(sort) => ("op", format!("so:{}", server_sort_code(*sort))),
    };
    if args.is_empty() {
        format!("1:{code}")
    } else {
        format!("1:{code}:{args}")
    }
}

/// Reads everything after `1:`.
pub(super) fn decode(rest: &str) -> Option<CustomId> {
    use CustomId as C;
    let parts: Vec<&str> = rest.split(':').collect();
    let (code, args) = parts.split_first()?;
    let case = |s: &str| CaseId::from_u22(s).ok();
    let action = |s: &str| ActionId::from_u22(s).ok();
    let appeal = |s: &str| AppealId::from_u22(s).ok();
    let ticket = |s: &str| TicketId::from_u22(s).ok();
    Some(match (*code, args) {
        ("cv", [c]) => C::OpenCase(case(c)?),
        ("cvs", []) => C::CasePicker,
        ("ce", [c]) => C::EditContext(case(c)?),
        ("cem", [c, r]) => C::EditContextModal {
            case: case(c)?,
            revision: number(r)?,
        },
        ("ea", [c]) => C::AddEvidence {
            case: case(c)?,
            new_message: false,
        },
        ("ea", [c, "n"]) => C::AddEvidence {
            case: case(c)?,
            new_message: true,
        },
        ("eam", [c]) => C::AddEvidenceModal {
            case: case(c)?,
            new_message: false,
        },
        ("eam", [c, "n"]) => C::AddEvidenceModal {
            case: case(c)?,
            new_message: true,
        },
        ("ev", [c, i]) => C::ViewEvidence {
            case: case(c)?,
            item: number(i)?,
            new_message: false,
        },
        ("ev", [c, i, "n"]) => C::ViewEvidence {
            case: case(c)?,
            item: number(i)?,
            new_message: true,
        },
        ("vd", [c]) => C::VoidCase(case(c)?),
        ("vdm", [c]) => C::VoidCaseModal(case(c)?),
        ("hi", [m, p]) => C::History {
            member: member(m)?,
            page: number(p)?,
        },
        ("cl", [p, m, r]) => C::CaseList {
            page: number(p)?,
            member: if *m == "0" { None } else { Some(member(m)?) },
            rule: if *r == "0" {
                None
            } else {
                Some(RuleId::from_u22(r).ok()?)
            },
        },
        ("ar", [a]) => C::RetryAction(action(a)?),
        ("ad", [a]) => C::DismissAction(action(a)?),
        ("au", [a]) => C::UndoAction(action(a)?),
        ("auc", [a]) => C::ConfirmUndo(action(a)?),
        ("rp", [d]) => C::RulePicker(case(d)?),
        ("rpp", [d, p]) => C::RulePickerPage {
            draft: case(d)?,
            page: number(p)?,
        },
        ("cd", args) => C::DraftPageModal(draft_page(args)?),
        ("cn", args) => C::DraftContinue(draft_page(args)?),
        ("cr", args) => C::DraftRetryPage(draft_page(args)?),
        ("cs", [d]) => C::DraftSaveAgain(case(d)?),
        ("ap", [c]) => C::Appeal(case(c)?),
        ("apm", [c]) => C::AppealModal(case(c)?),
        ("aa", [a, v]) => C::AcceptAppeal {
            appeal: appeal(a)?,
            version: number(v)?,
        },
        ("aam", [a, v]) => C::AcceptAppealModal {
            appeal: appeal(a)?,
            version: number(v)?,
        },
        ("aj", [a, v]) => C::RejectAppeal {
            appeal: appeal(a)?,
            version: number(v)?,
        },
        ("ajm", [a, v]) => C::RejectAppealModal {
            appeal: appeal(a)?,
            version: number(v)?,
        },
        ("ao", [a]) => C::OpenAppeal(appeal(a)?),
        ("al", [p]) => C::AppealsPage(number(p)?),
        ("rl", [p]) => C::RulesPage(number(p)?),
        ("su", ["ap", "invm"]) => C::SetupInviteModal,
        ("su", ["hp", "warnm"]) => C::SetupWarningModal,
        ("su", [a, o]) => C::Setup(area(a)?, op(o)?),
        ("to", []) => C::OpenTicket,
        ("tc", [t]) => C::CloseTicket(ticket(t)?),
        ("tf", [t]) => C::FixTicketAccess(ticket(t)?),
        ("op", args) => return decode_ops(args),
        (code, args) => return decode_evidence(code, args),
    })
}

/// The ops ids (`op:…`).
fn decode_ops(args: &[&str]) -> Option<CustomId> {
    use CustomId as C;
    Some(match args {
        ["st"] => C::OpsRefresh,
        ["rq", j] => C::OpsRequeueJob(JobId::from_u22(j).ok()?),
        ["dm", j] => C::OpsDismissJob(JobId::from_u22(j).ok()?),
        ["jp", p] => C::OpsJobsPage(number(p)?),
        ["sv", s, p] => C::OpsServersPage(server_sort(s)?, number(p)?),
        ["so", s] => C::OpsServersSort(server_sort(s)?),
        _ => return None,
    })
}

/// A server order's one-letter code in `op:sv`.
const fn server_sort_code(sort: ServerSort) -> &'static str {
    match sort {
        ServerSort::Members => "m",
        ServerSort::Cases => "c",
        ServerSort::Newest => "n",
    }
}

fn server_sort(code: &str) -> Option<ServerSort> {
    ServerSort::ALL
        .into_iter()
        .find(|s| server_sort_code(*s) == code)
}

/// The evidence ids (`evs` … `exm`).
fn decode_evidence(code: &str, args: &[&str]) -> Option<CustomId> {
    use CustomId as C;
    let evidence = |s: &str| EvidenceId::from_u22(s).ok();
    Some(match (code, args) {
        ("evs", [e]) => C::EvidenceSurrounding(evidence(e)?),
        ("evb", [e]) => C::BackToEvidence(evidence(e)?),
        ("evv", [e, v]) => C::EvidenceVisibility {
            evidence: evidence(e)?,
            visible: match *v {
                "1" => true,
                "0" => false,
                _ => return None,
            },
        },
        ("evn", [e]) => C::EditEvidenceNote(evidence(e)?),
        ("evnm", [e, r]) => C::EvidenceNoteModal {
            evidence: evidence(e)?,
            revision: number(r)?,
        },
        ("evr", [e]) => C::RemoveEvidence(evidence(e)?),
        ("evrc", [e]) => C::ConfirmRemoveEvidence(evidence(e)?),
        ("exs", [c, m]) => {
            let (channel, message) = located(c, m)?;
            C::AddAsEvidence { channel, message }
        }
        ("exn", [c, m]) => {
            let (channel, message) = located(c, m)?;
            C::AddAsEvidenceByNumber { channel, message }
        }
        ("exm", [c, m]) => {
            let (channel, message) = located(c, m)?;
            C::AddAsEvidenceModal { channel, message }
        }
        _ => return None,
    })
}
