//! Appeals (§12.9): the member's form and replies, the staff appeal post
//! and card, decision modals, and `/appeals`.

use quack_core::appeal::{AppealEventKind, AppealState};
use quack_core::ids::{AppealId, CaseId};
use quack_core::limits;
use time::OffsetDateTime;

use crate::actions::{ButtonAction, InputId, ModalAction};
use crate::budget;
use crate::fmt;
use crate::model::{
    Actor, Block, Button, ButtonStyle, ChannelRef, Input, InputKind, InputStyle, Modal, Msg,
    Person, Text, TimeStyle, Tone,
};
use crate::word;

fn input_max(chars: usize) -> u16 {
    u16::try_from(chars.min(budget::INPUT_MAX_LENGTH)).unwrap_or(u16::MAX)
}

/// `appeal.form.*`: the member's appeal modal.
pub fn form(case: CaseId) -> Modal {
    Modal {
        action: ModalAction::Appeal(case),
        title: "Appeal this case".to_owned(),
        inputs: vec![Input {
            id: InputId::Statement,
            label: "Why should moderators reconsider this case?".to_owned(),
            description: Some(
                "Explain what you want the moderators to review. You can appeal this case once."
                    .to_owned(),
            ),
            required: true,
            kind: InputKind::Text {
                style: InputStyle::Paragraph,
                placeholder: None,
                min_length: 1,
                max_length: input_max(limits::APPEAL_TEXT.max),
                value: None,
            },
        }],
    }
}

/// `appeal.submitted`.
pub fn submitted(number: u64) -> Text {
    Text::fixed(format!(
        "Your appeal for case #{number} is saved. It's waiting for a decision. Open your appeal on the dashboard to see updates."
    ))
}

/// `appeal.submit.exists`.
pub fn submit_exists(number: u64) -> Text {
    Text::fixed(format!(
        "You already appealed case #{number}. Open your appeal on the dashboard to see the latest update."
    ))
}

/// `appeal.submit.not_allowed`.
pub fn submit_not_allowed(number: u64) -> Text {
    Text::fixed(format!(
        "Case #{number} can't be appealed. You can still review it on the dashboard."
    ))
}

/// `appeal.submit.voided`.
pub fn submit_voided(number: u64) -> Text {
    Text::fixed(format!(
        "Case #{number} was voided and no longer counts. You can still review it on the dashboard."
    ))
}

/// `appeal.submit.not_yours`.
pub fn submit_not_yours() -> Text {
    Text::fixed(
        "This button isn't for your account. Sign in to the dashboard to see your own cases.",
    )
}

/// `appeal.information.saved`.
pub fn information_saved(number: u64) -> Text {
    Text::fixed(format!(
        "Your reply was added to the appeal for case #{number}. It's waiting for a decision."
    ))
}

/// `appeal.information.not_requested`.
pub fn information_not_requested() -> Text {
    Text::fixed(
        "There's no question waiting for you to answer. Open your appeal to see the latest update.",
    )
}

/// `appeal.information.outdated`.
pub fn information_outdated() -> Text {
    Text::fixed(
        "The appeal changed while you were writing. Reload it and read the latest question before replying.",
    )
}

/// Everything the staff appeal post (and the appeal card) shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppealPost {
    /// The appeal.
    pub appeal: AppealId,
    /// Its version, carried by the decision buttons.
    pub version: u32,
    /// The case's number.
    pub number: u64,
    /// The rule's name.
    pub rule: String,
    /// The member.
    pub member: Person,
    /// When they appealed.
    pub submitted_at: OffsetDateTime,
    /// Where it stands.
    pub state: AppealState,
    /// Who made the latest decision, and when (Quack when closed by a void).
    pub decided: Option<(Actor, OffsetDateTime)>,
    /// Quack closed it because the case was voided.
    pub closed_by_void: bool,
    /// The member's statement.
    pub statement: String,
    /// The member's latest added information.
    pub information: Option<String>,
    /// The reason given to the member with the latest decision.
    pub decision_reason: Option<String>,
    /// The appeal on the dashboard.
    pub dashboard_url: String,
}

fn state_line(p: &AppealPost) -> Text {
    let by = |what: &str| match &p.decided {
        Some((actor, at)) => Text::fixed(format!("{what} by "))
            .actor(actor)
            .s(" ")
            .time(*at, TimeStyle::Relative)
            .s("."),
        None => Text::fixed(format!("{what}.")),
    };
    match p.state {
        AppealState::Pending => Text::fixed("Waiting for a decision."),
        AppealState::NeedsInformation => Text::fixed("Waiting for ")
            .person(&p.member)
            .s(" to reply."),
        AppealState::Accepted => by("Accepted"),
        AppealState::Rejected => by("Rejected"),
        AppealState::Closed if p.closed_by_void => {
            let t = Text::fixed("Closed ");
            match &p.decided {
                Some((_, at)) => t
                    .time(*at, TimeStyle::Relative)
                    .s(" because the case was voided."),
                None => t.s("because the case was voided."),
            }
        }
        AppealState::Closed => by("Closed"),
    }
}

fn tone(state: AppealState) -> Tone {
    match state {
        AppealState::Pending => Tone::Warning,
        AppealState::NeedsInformation | AppealState::Closed => Tone::Neutral,
        AppealState::Accepted => Tone::Success,
        AppealState::Rejected => Tone::Danger,
    }
}

/// `appeal.post.*`: the staff post in the appeal channel, and the appeal
/// card `/appeals` opens. Accept and Reject only while pending.
pub fn post(p: &AppealPost) -> Msg {
    let statement_cut = p.statement.chars().count() > limits::APPEAL_POST_STATEMENT_CHARS;
    let mut msg = Msg::new(tone(p.state))
        .block(Block::Heading(
            Text::fixed(format!("Appeal for case #{} · ", p.number)).bold_user(&p.rule),
        ))
        .para(
            Text::fixed("From ")
                .person(&p.member)
                .s(" · submitted ")
                .time(p.submitted_at, TimeStyle::Relative),
        )
        .para(state_line(p))
        .para(Text::new().bold("Appeal statement"))
        .block(Block::Quote(Text::new().user(fmt::cut(
            &p.statement,
            limits::APPEAL_POST_STATEMENT_CHARS,
        ))));
    if statement_cut {
        msg = msg.block(Block::Subtext(Text::fixed(
            "Open the appeal on the dashboard for the full statement.",
        )));
    }
    if let Some(info) = &p.information {
        msg = msg
            .para(Text::new().bold("Reply from the member"))
            .block(Block::Quote(
                Text::new().user(fmt::cut(info, limits::APPEAL_POST_STATEMENT_CHARS)),
            ));
    }
    if let Some(reason) = p.decision_reason.as_ref().filter(|_| !p.closed_by_void) {
        msg = msg
            .para(Text::new().bold("Reason shared with the member"))
            .block(Block::Quote(Text::new().user(reason)));
    }
    let mut buttons = Vec::new();
    if p.state == AppealState::Pending {
        let (appeal, version) = (p.appeal, p.version);
        buttons.push(Button::new(
            "Accept",
            ButtonStyle::Success,
            ButtonAction::AcceptAppeal { appeal, version },
        ));
        buttons.push(Button::new(
            "Reject",
            ButtonStyle::Danger,
            ButtonAction::RejectAppeal { appeal, version },
        ));
    }
    buttons.push(Button::link("Open appeal", &p.dashboard_url));
    msg.when_trimmed(Text::fixed(
        "Open the appeal on the dashboard for the full statement and decision.",
    ))
    .buttons(buttons)
}

/// `appeal.post.moved`: what the old post says after the appeal channel
/// changed.
pub fn post_moved(channel: &ChannelRef) -> Msg {
    Msg::new(Tone::Neutral).block(Block::Subtext(
        Text::fixed("Appeals now go to ")
            .channel(channel)
            .s(". Review this appeal there or on the dashboard."),
    ))
}

/// A staff decision made with a modal in Discord.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalDecision {
    /// Accept: void the case.
    Accept,
    /// Reject: the case stands.
    Reject,
}

/// `appeal.modal.*`: the decision modal, prefilled with the default reason
/// (`appeal.reason.default.*`).
pub fn decision_modal(decision: ModalDecision, appeal: AppealId, version: u32) -> Modal {
    let (action, title, default) = match decision {
        ModalDecision::Accept => (
            ModalAction::AcceptAppeal { appeal, version },
            "Accept appeal",
            "We reviewed your appeal. The case no longer counts.",
        ),
        ModalDecision::Reject => (
            ModalAction::RejectAppeal { appeal, version },
            "Reject appeal",
            "We reviewed your appeal. The case still counts.",
        ),
    };
    Modal {
        action,
        title: title.to_owned(),
        inputs: vec![Input {
            id: InputId::Reason,
            label: "Reason the member will see".to_owned(),
            description: Some("Leave out staff names and private details.".to_owned()),
            required: true,
            kind: InputKind::Text {
                style: InputStyle::Paragraph,
                placeholder: None,
                min_length: 1,
                max_length: input_max(limits::APPEAL_DECISION_REASON.max),
                value: Some(default.to_owned()),
            },
        }],
    }
}

/// `appeal.decided.*`: the moderator's confirmation.
pub fn decided(decision: ModalDecision, number: u64, member: &Person) -> Text {
    match decision {
        ModalDecision::Accept => Text::fixed(format!(
            "Appeal accepted. Case #{number} is voided and no longer counts. Quack will try to send "
        ))
        .person(member)
        .s(" a DM. Open the case to check the outcome of any timeout or ban."),
        ModalDecision::Reject => Text::fixed(format!(
            "Appeal rejected. Case #{number} still counts. Quack will try to send "
        ))
        .person(member)
        .s(" a DM."),
    }
}

/// `appeal.closed_by_void`: the text of the event Quack records when a void
/// closes an appeal.
pub fn closed_by_void() -> Text {
    Text::fixed("The appeal was closed because the case was voided.")
}

/// One appeal event on a timeline. Staff see who acted; members see "you"
/// and never a staff name.
pub fn event(
    kind: AppealEventKind,
    actor: Option<&Actor>,
    member: &Person,
    for_member: bool,
) -> Text {
    let staff = |what: &str| match (for_member, actor) {
        (false, Some(a)) => Text::new().actor(a).s(format!(" {what}")),
        _ => Text::fixed(format!("The moderators {what}")),
    };
    let them = |t: Text| {
        if for_member {
            t.s("You")
        } else {
            t.person(member)
        }
    };
    match kind {
        AppealEventKind::Submitted => them(Text::new()).s(" appealed the case."),
        AppealEventKind::InformationRequested => staff("asked for more information."),
        AppealEventKind::InformationProvided => them(Text::new()).s(" replied to the appeal."),
        AppealEventKind::Accepted => {
            staff("accepted the appeal. The case was voided and no longer counts.")
        }
        AppealEventKind::Rejected => staff("rejected the appeal. The case still counts."),
        AppealEventKind::Closed if actor == Some(&Actor::Quack) => closed_by_void(),
        AppealEventKind::Closed => staff("closed the appeal."),
        AppealEventKind::Reopened => staff("reopened the appeal and asked for more information."),
    }
}

/// One row of `/appeals`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppealRow {
    /// The appeal.
    pub appeal: AppealId,
    /// The case's number.
    pub number: u64,
    /// The member.
    pub member: Person,
    /// The rule's name.
    pub rule: String,
    /// When they appealed.
    pub submitted_at: OffsetDateTime,
}

/// `appeals.*`: pending appeals, oldest first, each with **Open**.
pub fn list(rows: &[AppealRow], page: u32, pages: u32) -> Msg {
    let mut msg = Msg::new(Tone::Neutral).block(Block::Heading(Text::fixed(
        "Appeals waiting for a decision",
    )));
    if rows.is_empty() {
        msg = msg.para(Text::fixed(
            "No appeals are waiting for a decision. Open Appeals on the dashboard to see replies and past decisions.",
        ));
    }
    for r in rows {
        msg = msg.para(
            Text::fixed(format!("Case #{} · ", r.number))
                .person(&r.member)
                .s(" · ")
                .user(fmt::one_line(&r.rule))
                .s(" · submitted ")
                .time(r.submitted_at, TimeStyle::Relative),
        );
    }
    if pages > 1 {
        msg = msg.block(Block::Subtext(Text::fixed(word::page(page, pages))));
    }
    let open = rows
        .iter()
        .map(|r| {
            Button::new(
                format!("Open appeal for #{}", r.number),
                ButtonStyle::Secondary,
                ButtonAction::OpenAppeal(r.appeal),
            )
        })
        .collect();
    msg.buttons(open)
        .buttons(word::pager(page, pages, ButtonAction::AppealsPage))
}
