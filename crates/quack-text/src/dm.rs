//! DM outcome sentences (§12.7) and the DMs Quack sends members (§12.8).

use quack_core::dm::{DmFailure, DmState};
use quack_core::ids::CaseId;
use quack_core::rule::LevelAction;
use time::OffsetDateTime;

use crate::actions::ButtonAction;
use crate::case::{ContextLine, context_value};
use crate::model::{Block, Button, ButtonStyle, Inline, Msg, Person, Text, TimeStyle, Tone};
use crate::word::Outcome;

/// A DM's stored outcome. A case whose level sends no DM has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmView {
    /// Where it stands.
    pub state: DmState,
    /// Why it failed, when `failed`.
    pub failure: Option<DmFailure>,
}

/// The outcome badge for a case DM; `None` means the level sends no DM.
pub fn outcome(dm: Option<&DmView>) -> Outcome {
    match dm.map(|d| d.state) {
        None | Some(DmState::NotNeeded) => Outcome::NotNeeded,
        Some(DmState::Pending | DmState::Sending) => Outcome::InProgress,
        Some(DmState::Sent) => Outcome::Done,
        Some(DmState::Failed) => Outcome::Failed,
        Some(DmState::Unknown) => Outcome::CouldntConfirm,
    }
}

/// What a DM is about: the words after "a DM" in its outcome sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmAbout {
    /// The case DM for this case number.
    Case(Option<u64>),
    /// An appeal DM, about the appeal for this case number.
    Appeal(Option<u64>),
    /// The DM with a closed ticket's transcript.
    Transcript,
}

/// `dm.state.*`: the one staff sentence for a DM's outcome (§12.7). The
/// card, the receipt, the log channel, and the dashboard all use it, so
/// the same outcome reads the same everywhere. `member` is `None` when
/// Quack can't name them ("the member").
pub fn outcome_sentence(
    state: DmState,
    failure: Option<DmFailure>,
    member: Option<&Person>,
    about: DmAbout,
) -> Text {
    outcome_with(state, failure, member, about, &|t, n| {
        t.s(crate::fmt::case_ref(n))
    })
}

/// [`outcome_sentence`] with the case number built by `case` (the log
/// channel links it).
pub(crate) fn outcome_with(
    state: DmState,
    failure: Option<DmFailure>,
    member: Option<&Person>,
    about: DmAbout,
    case: &dyn Fn(Text, u64) -> Text,
) -> Text {
    let who = |t: Text, start: bool| match member {
        Some(m) => t.person(m),
        None if start => t.s("The member"),
        None => t.s("the member"),
    };
    let subject = |t: Text| match about {
        DmAbout::Case(Some(n)) => case(t.s(" about "), n),
        DmAbout::Case(None) => t.s(" about their case"),
        DmAbout::Appeal(Some(n)) => case(t.s(" about the appeal for "), n),
        DmAbout::Appeal(None) => t.s(" about their appeal"),
        DmAbout::Transcript => t.s(" with the ticket transcript"),
    };
    let recovery = |t: Text| match about {
        DmAbout::Case(_) => {
            who(t.s(" "), true).s(" can sign in to the Quack dashboard to see the case.")
        }
        DmAbout::Appeal(_) => {
            who(t.s(" "), true).s(" can sign in to the Quack dashboard to see the appeal.")
        }
        DmAbout::Transcript => {
            t.s(" Staff can open the ticket on the dashboard to check for the transcript.")
        }
    };
    match state {
        DmState::Pending => {
            subject(who(Text::fixed("Quack will try to send "), false).s(" a DM")).s(".")
        }
        DmState::Sending => subject(who(Text::fixed("Quack is sending "), false).s(" a DM")).s("."),
        DmState::Sent => subject(who(Text::fixed("Quack sent "), false).s(" a DM")).s("."),
        DmState::Failed => {
            let lead = subject(who(Text::fixed("Quack couldn't send "), false).s(" a DM"));
            let lead = match failure.unwrap_or(DmFailure::DiscordRefused) {
                DmFailure::DmsClosed | DmFailure::DiscordRefused => {
                    lead.s(". Discord didn't allow the DM.")
                }
                DmFailure::NotInSharedServer => {
                    who(lead.s(". "), true).s(" and Quack don't share a server.")
                }
                DmFailure::DiscordUnavailable => lead.s(" in time."),
            };
            recovery(lead)
        }
        DmState::Unknown => recovery(
            subject(who(
                Text::fixed("Quack couldn't confirm whether Discord accepted the DM to "),
                false,
            ))
            .s(". Quack won't send it again."),
        ),
        DmState::NotNeeded => {
            Text::fixed("Quack didn't send a DM because the case was voided first.")
        }
    }
}

/// `dm.state.*` for a case DM; `None` means the level sends no DM.
pub fn sentence(dm: Option<&DmView>, member: &Person, case_number: u64) -> Text {
    match dm {
        None => Text::fixed("This level doesn't send a DM."),
        Some(dm) => outcome_sentence(
            dm.state,
            dm.failure,
            Some(member),
            DmAbout::Case(Some(case_number)),
        ),
    }
}

/// `member.dm.*`: the member's sentence for their case DM, shown on their
/// case page (so it doesn't point back to it).
pub fn member_sentence(dm: Option<&DmView>) -> Text {
    Text::fixed(match dm.map(|d| d.state) {
        None => "Quack doesn't send a DM for this case.",
        Some(DmState::NotNeeded) => "Quack didn't send a DM because the case was voided first.",
        Some(DmState::Pending) => "Quack will try to send you a DM about this case.",
        Some(DmState::Sending) => "Quack is sending you a DM about this case.",
        Some(DmState::Sent) => "Quack sent you a DM about this case.",
        Some(DmState::Failed) => "Quack couldn't send you a DM about this case.",
        Some(DmState::Unknown) => {
            "Quack couldn't confirm whether Discord accepted the DM about this case. Quack won't send it again."
        }
    })
}

/// Everything the case DM shows (§6.6). It never names the moderator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseDm {
    /// The case.
    pub case: CaseId,
    /// Its number.
    pub number: u64,
    /// When it was added.
    pub created_at: OffsetDateTime,
    /// The server's name.
    pub server: String,
    /// The rule's name in the case's version.
    pub rule: String,
    /// The official reason.
    pub reason: String,
    /// The server's DM introduction.
    pub intro: Option<String>,
    /// The case context; `message_link` answers are left out here.
    pub context: Vec<ContextLine>,
    /// The decided action; `None` is a warning.
    pub action: Option<LevelAction>,
    /// A timeout's fixed end.
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// Whether the case can be appealed.
    pub appealable: bool,
    /// The server's DM footer.
    pub footer: Option<String>,
    /// The member's appeal page on the dashboard.
    pub dashboard_url: String,
}

/// `dm.case.outcome.*`: the configured action, never a claim that Discord
/// applied it (the DM may arrive first, §6.3).
fn what_happens(dm: &CaseDm) -> Text {
    let check = " Open your case on the dashboard to check its outcome.";
    match dm.action {
        None => Text::fixed("This case is a warning."),
        Some(LevelAction::Timeout { .. }) => {
            let t = Text::fixed("The rule calls for a timeout");
            match dm.timeout_ends_at {
                Some(at) => t.s(" until ").time(at, TimeStyle::FullRelative).s("."),
                None => t.s("."),
            }
            .s(check)
        }
        Some(LevelAction::Kick) => {
            Text::fixed("The rule calls for removing you from the server.").s(check)
        }
        Some(LevelAction::Ban { .. }) => {
            Text::fixed("The rule calls for banning you from the server.").s(check)
        }
    }
}

/// `dm.case`: the one DM about a case. The case number, date, action, and
/// appeal line come first so a long reason or context can't push them out.
pub fn case(dm: &CaseDm) -> Msg {
    let mut msg = Msg::new(Tone::Neutral)
        .block(Block::Heading(
            Text::fixed(format!("Case #{} in ", dm.number)).bold_user(&dm.server),
        ))
        .block(Block::Text(
            Text::fixed("Rule: ")
                .bold_user(&dm.rule)
                .s(" · ")
                .time(dm.created_at, TimeStyle::Full),
        ))
        .block(Block::Quote(Text::new().user(&dm.reason)))
        .block(Block::Field {
            label: "Action for this case".to_owned(),
            value: what_happens(dm),
        })
        .para(Text::fixed(if dm.appealable {
            "To ask the moderators to review this case, select Appeal case. You can appeal once."
        } else {
            "This case can't be appealed."
        }));
    if let Some(intro) = dm.intro.as_deref().filter(|s| !s.trim().is_empty()) {
        msg = msg.para(Text::new().push(Inline::ServerMarkdown(intro.to_owned())));
    }
    let details: Vec<&ContextLine> = dm.context.iter().filter(|l| !l.is_link()).collect();
    if !details.is_empty() {
        msg = msg.para(Text::new().bold("Details"));
        for line in details {
            msg = msg.block(Block::Field {
                label: crate::fmt::one_line(&line.label),
                value: context_value(line, crate::case::CONTEXT_CUT),
            });
        }
    }
    if let Some(footer) = dm.footer.as_deref().filter(|s| !s.trim().is_empty()) {
        msg = msg.para(Text::new().push(Inline::ServerMarkdown(footer.to_owned())));
    }
    let mut buttons = Vec::new();
    if dm.appealable {
        buttons.push(Button::new(
            "Appeal case",
            ButtonStyle::Primary,
            ButtonAction::Appeal(dm.case),
        ));
    }
    buttons.push(Button::link("Open your case", &dm.dashboard_url));
    msg.when_trimmed(Text::fixed(
        "Open your case on the dashboard for the full details.",
    ))
    .buttons(buttons)
}

/// Whether a kick or a ban is what a rejoin invite is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejoinAfter {
    /// The case banned the member; the ban may still be removed.
    Ban,
    /// The case removed the member from the server.
    Kick,
}

/// Which appeal event a DM is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppealDmKind {
    /// Staff asked for more information.
    InformationRequested,
    /// Staff reopened a decided appeal and asked for information.
    Reopened,
    /// Staff accepted, with the rejoin invite when the case kicked or banned.
    Accepted(Option<(RejoinAfter, String)>),
    /// Staff rejected.
    Rejected,
    /// Staff closed it.
    Closed,
    /// Quack closed it because the case was voided.
    ClosedCaseVoided,
}

/// Everything an appeal DM shows; rendered from one appeal event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppealDm {
    /// What happened.
    pub kind: AppealDmKind,
    /// The server's name.
    pub server: String,
    /// The case number.
    pub number: u64,
    /// The reason staff gave (unused when Quack closed it).
    pub reason: String,
    /// When the event happened.
    pub at: OffsetDateTime,
    /// The member's appeal page on the dashboard.
    pub dashboard_url: String,
}

/// `dm.appeal.*`: the DM about one appeal event. Every one has the same
/// heading, says what the moderators did and what it means for the case,
/// then quotes their words.
pub fn appeal(dm: &AppealDm) -> Msg {
    let n = dm.number;
    let reason = Block::Quote(Text::new().user(&dm.reason));
    let reply = "Select Reply to appeal to answer on the dashboard.";
    let (lead, body, button): (String, Vec<Block>, &str) = match &dm.kind {
        AppealDmKind::InformationRequested => (
            "The moderators asked for more information:".to_owned(),
            vec![reason, Block::Text(Text::fixed(reply))],
            "Reply to appeal",
        ),
        AppealDmKind::Reopened => (
            "The moderators reopened your appeal and asked for more information:".to_owned(),
            vec![reason, Block::Text(Text::fixed(reply))],
            "Reply to appeal",
        ),
        AppealDmKind::Accepted(rejoin) => {
            let mut body = vec![
                reason,
                Block::Text(Text::fixed(
                    "Open your case to check whether any timeout or ban was removed.",
                )),
            ];
            if let Some((after, invite)) = rejoin {
                let lead = match after {
                    RejoinAfter::Ban => {
                        "Once your ban is removed, you can try this invite to rejoin the server: "
                    }
                    RejoinAfter::Kick => "You can try this invite to rejoin the server: ",
                };
                body.push(Block::Text(Text::fixed(lead).link(invite, invite)));
            }
            (
                format!("The moderators accepted your appeal. Case #{n} no longer counts."),
                body,
                "Open your case",
            )
        }
        AppealDmKind::Rejected => (
            format!("The moderators rejected your appeal. Case #{n} still counts."),
            vec![reason],
            "Open your appeal",
        ),
        AppealDmKind::Closed => (
            format!("The moderators closed your appeal. Case #{n} still counts."),
            vec![reason],
            "Open your appeal",
        ),
        AppealDmKind::ClosedCaseVoided => (
            format!(
                "Your appeal was closed because case #{n} was voided. The case no longer counts."
            ),
            vec![],
            "Open your case",
        ),
    };
    let mut msg = Msg::new(Tone::Neutral)
        .block(Block::Heading(
            Text::fixed(format!("Your appeal for case #{n} in ")).bold_user(&dm.server),
        ))
        .para(Text::fixed(lead));
    msg.blocks.extend(body);
    msg.block(Block::Subtext(Text::new().time(dm.at, TimeStyle::Full)))
        .when_trimmed(Text::fixed(
            "Open your appeal on the dashboard for the full details.",
        ))
        .buttons(vec![Button::link(button, &dm.dashboard_url)])
}
