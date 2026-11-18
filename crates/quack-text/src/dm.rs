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

/// `dm.failure.*`: the "because …" fragment (§6.5).
pub const fn because(failure: DmFailure) -> &'static str {
    match failure {
        DmFailure::DmsClosed => {
            "because Discord didn't let Quack message them (their DMs may be closed, or they may have blocked Quack)"
        }
        DmFailure::NotInSharedServer => "because they no longer share a server with Quack",
        DmFailure::DiscordUnavailable => "because Discord wasn't available",
        DmFailure::DiscordRefused => "because Discord refused the message",
    }
}

/// `dm.state.*`: the staff sentence for a case DM; `None` means the level
/// sends no DM.
pub fn sentence(dm: Option<&DmView>, member: &Person) -> Text {
    let Some(dm) = dm else {
        return Text::fixed("This level doesn't send a DM.");
    };
    let see = " They can still see the case by signing in to the dashboard.";
    match dm.state {
        DmState::Pending | DmState::Sending => {
            Text::fixed("Quack is sending ").person(member).s(" a DM.")
        }
        DmState::Sent => Text::fixed("Quack sent ").person(member).s(" a DM."),
        DmState::Failed => Text::fixed("Quack couldn't send ")
            .person(member)
            .s(" a DM ")
            .s(because(dm.failure.unwrap_or(DmFailure::DiscordRefused)))
            .s(".")
            .s(see),
        DmState::Unknown => {
            Text::fixed("Quack couldn't confirm whether Discord accepted the DM to ")
                .person(member)
                .s(", and won't send it again so it can't arrive twice.")
                .s(see)
        }
        DmState::NotNeeded => {
            Text::fixed("Quack didn't send a DM because the case was voided first.")
        }
    }
}

/// `member.dm.*`: the member's sentence for their case DM.
pub fn member_sentence(dm: Option<&DmView>) -> Text {
    Text::fixed(match dm.map(|d| d.state) {
        None | Some(DmState::NotNeeded) => "This case didn't need a DM.",
        Some(DmState::Pending | DmState::Sending) => "Quack is sending you a DM about this case.",
        Some(DmState::Sent) => "Quack sent you a DM about this case.",
        Some(DmState::Failed) => {
            "Quack couldn't send you a DM about this case. You can see your case here."
        }
        Some(DmState::Unknown) => {
            "Quack couldn't confirm whether you got a DM about this case. You can see your case here."
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

fn what_happens(dm: &CaseDm) -> Text {
    match dm.action {
        None => Text::fixed("This is a warning. No other action is taken."),
        Some(LevelAction::Timeout { .. }) => {
            let t = Text::fixed("The moderators decided to time you out");
            match dm.timeout_ends_at {
                Some(at) => t.s(" until ").time(at, TimeStyle::FullRelative).s("."),
                None => t.s("."),
            }
        }
        Some(LevelAction::Kick) => {
            Text::fixed("The moderators decided to remove you from the server.")
        }
        Some(LevelAction::Ban { .. }) => {
            Text::fixed("The moderators decided to ban you from the server.")
        }
    }
}

/// `dm.case`: the one DM about a case.
pub fn case(dm: &CaseDm) -> Msg {
    let mut msg = Msg::new(Tone::Neutral)
        .block(Block::Heading(
            Text::fixed("You have a new case in ").bold_user(&dm.server),
        ))
        .block(Block::Text(Text::fixed("Rule: ").bold_user(&dm.rule)))
        .block(Block::Quote(Text::new().user(&dm.reason)));
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
    msg = msg.block(Block::Field {
        label: "What happens".to_owned(),
        value: what_happens(dm),
    });
    msg = msg.para(Text::fixed(if dm.appealable {
        "If you think this case is wrong, you can appeal it once."
    } else {
        "This case can't be appealed."
    }));
    if let Some(footer) = dm.footer.as_deref().filter(|s| !s.trim().is_empty()) {
        msg = msg.para(Text::new().push(Inline::ServerMarkdown(footer.to_owned())));
    }
    msg = msg.block(Block::Subtext(
        Text::fixed(format!("Case #{} · ", dm.number)).time(dm.created_at, TimeStyle::Full),
    ));
    let mut buttons = Vec::new();
    if dm.appealable {
        buttons.push(Button::new(
            "Appeal",
            ButtonStyle::Primary,
            ButtonAction::Appeal(dm.case),
        ));
    }
    buttons.push(Button::link("Open in dashboard", &dm.dashboard_url));
    msg.buttons(buttons)
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

/// `dm.appeal.*`: the DM about one appeal event.
pub fn appeal(dm: &AppealDm) -> Msg {
    let n = dm.number;
    let your_appeal = |what: &str| {
        Text::fixed(format!("Your appeal for case #{n} in "))
            .bold_user(&dm.server)
            .s(format!(" was {what}"))
    };
    let reason = Block::Quote(Text::new().user(&dm.reason));
    let reply = format!("Reply on the dashboard to continue your appeal for case #{n}.");
    let (heading, body, asks): (Text, Vec<Block>, bool) = match &dm.kind {
        AppealDmKind::InformationRequested => (
            Text::fixed("The moderators of ")
                .bold_user(&dm.server)
                .s(" need more information about your appeal"),
            vec![reason, Block::Text(Text::fixed(reply))],
            true,
        ),
        AppealDmKind::Reopened => (
            Text::fixed("The moderators of ")
                .bold_user(&dm.server)
                .s(" reopened your appeal"),
            vec![
                reason,
                Block::Text(Text::fixed(format!("They need more information. {reply}"))),
            ],
            true,
        ),
        AppealDmKind::Accepted(rejoin) => {
            let mut body = vec![
                reason,
                Block::Text(Text::fixed(
                    "The case no longer counts. Open your case to see what happened to any timeout or ban.",
                )),
            ];
            if let Some((after, invite)) = rejoin {
                let lead = match after {
                    RejoinAfter::Ban => "If your ban is removed, you can rejoin with this invite: ",
                    RejoinAfter::Kick => "You can rejoin the server with this invite: ",
                };
                body.push(Block::Text(Text::fixed(lead).link(invite, invite)));
            }
            (your_appeal("accepted"), body, false)
        }
        AppealDmKind::Rejected => (
            your_appeal("rejected"),
            vec![reason, Block::Text(Text::fixed("The case still counts."))],
            false,
        ),
        AppealDmKind::Closed => (your_appeal("closed"), vec![reason], false),
        AppealDmKind::ClosedCaseVoided => (
            your_appeal("closed"),
            vec![Block::Text(Text::fixed(
                "The case was voided, so there's nothing left to appeal. It no longer counts against you.",
            ))],
            false,
        ),
    };
    let mut msg = Msg::new(Tone::Neutral).block(Block::Heading(heading));
    msg.blocks.extend(body);
    msg.block(Block::Subtext(
        Text::fixed(format!("Case #{n} · "))
            .user(&dm.server)
            .s(" · ")
            .time(dm.at, TimeStyle::Full),
    ))
    .buttons(vec![Button::link(
        if asks {
            "Reply on the dashboard"
        } else {
            "View appeal"
        },
        &dm.dashboard_url,
    )])
}
