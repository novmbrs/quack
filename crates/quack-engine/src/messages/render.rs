//! Rendering a tracked message from current data (§10.3 step 2), never
//! from the revision, so a later render always shows the latest state.

use quack_core::appeal::{AppealEventKind, AppealState};
use quack_core::ids::{AppealId, CaseId, GuildId, TicketId};
use quack_core::messages::MessageKind;
use quack_core::modules::TicketCloseReason;
use quack_db::messages::{Owner, TrackedMessage};
use quack_db::{DbError, PgConnection, appeals, cases, honeypot, rules, tickets};
use quack_text::appeal::AppealPost;
use quack_text::honeypot::Warning;
use quack_text::ticket::{TicketPost, TranscriptPlace};
use quack_text::{Actor, ChannelRef, Msg, Person};

use crate::Engine;
use crate::cases::{CaseRecords, card, receipt};
use crate::port::OutgoingFile;

/// A rendered message and, for a ticket post, the transcript to attach
/// once.
pub(super) struct Rendered {
    pub msg: Msg,
    pub attach: Option<OutgoingFile>,
}

impl Rendered {
    fn plain(msg: Msg) -> Self {
        Self { msg, attach: None }
    }
}

/// Renders `row`; `None` when its owner is gone.
pub(super) async fn render(
    engine: &Engine,
    row: &TrackedMessage,
) -> Result<Option<Rendered>, DbError> {
    let mut conn = engine.db().conn().await?;
    let guild = row.guild;
    match row.owner {
        Owner::Case(case) => case_receipt(engine, &mut conn, guild, case, row.private).await,
        Owner::Appeal(appeal) => Ok(appeal_post(engine, &mut conn, guild, appeal)
            .await?
            .map(|p| Rendered::plain(quack_text::appeal::post(&p)))),
        Owner::Ticket(ticket) => ticket_post(&mut conn, guild, ticket, row).await,
        Owner::Server(MessageKind::HoneypotWarning) => honeypot_warning(&mut conn, guild).await,
        Owner::Server(MessageKind::TicketPanel) => {
            let on = tickets::settings(&mut conn, guild)
                .await?
                .is_some_and(|s| s.enabled);
            Ok(Some(Rendered::plain(quack_text::ticket::panel(on))))
        }
        Owner::Server(_) => Ok(None),
    }
}

async fn case_receipt(
    engine: &Engine,
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
    private: bool,
) -> Result<Option<Rendered>, DbError> {
    let Some(records) = CaseRecords::load(conn, guild, case).await? else {
        return Ok(None);
    };
    let msg = if private {
        quack_text::case::card(&card(engine, &records))
    } else {
        quack_text::case::receipt(&receipt(engine, &records))
    };
    Ok(Some(Rendered::plain(msg)))
}

/// The appeal post's facts (§8.3), also the appeal card's.
pub(crate) async fn appeal_post(
    engine: &Engine,
    conn: &mut PgConnection,
    guild: GuildId,
    appeal: AppealId,
) -> Result<Option<AppealPost>, DbError> {
    let Some(appeal) = appeals::get(conn, guild, appeal).await? else {
        return Ok(None);
    };
    let Some(case) = cases::get(conn, guild, appeal.case).await? else {
        return Ok(None);
    };
    let rule = match case.rule {
        Some((rule, version)) => rules::version(conn, guild, rule, version)
            .await?
            .map_or_default(|v| v.definition.name),
        None => String::new(),
    };
    let events = appeals::events(conn, guild, appeal.id).await?;
    let closed_by_void = appeal.state == AppealState::Closed
        && events
            .iter()
            .rev()
            .find(|e| e.kind == AppealEventKind::Closed)
            .is_some_and(|e| e.closed_by_void);
    let decided = if closed_by_void {
        events.last().map(|e| (Actor::Quack, e.created_at))
    } else {
        appeal
            .decision
            .as_ref()
            .map(|(by, at, _)| (Actor::Person(Person::unnamed(*by)), *at))
    };
    let information = events
        .iter()
        .rev()
        .find(|e| e.kind == AppealEventKind::InformationProvided)
        .map(|e| e.text.clone());
    Ok(Some(AppealPost {
        appeal: appeal.id,
        version: appeal.version,
        number: case.number,
        rule,
        member: Person::unnamed(appeal.member.account()),
        submitted_at: appeal.created_at,
        state: appeal.state,
        decided,
        closed_by_void,
        statement: appeal.statement.clone(),
        information,
        decision_reason: appeal.decision.map(|(_, _, reason)| reason),
        dashboard_url: engine.links().appeal(guild, appeal.id),
    }))
}

async fn honeypot_warning(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Option<Rendered>, DbError> {
    let Some(settings) = honeypot::settings(conn, guild).await? else {
        return Ok(None);
    };
    let rule = match settings.rule {
        Some(rule) if settings.enabled => rules::get(conn, guild, rule).await?,
        _ => None,
    };
    let warning = match rule {
        Some(rule) => Warning::On {
            rule: rule.definition.name.clone(),
            levels: rule.definition.levels.iter().map(|l| l.action).collect(),
            custom: settings.warning_text,
            cases: cases::count_honeypot(conn, guild).await?,
        },
        None => Warning::Off,
    };
    Ok(Some(Rendered::plain(quack_text::honeypot::warning(
        &warning,
    ))))
}

async fn ticket_post(
    conn: &mut PgConnection,
    guild: GuildId,
    ticket: TicketId,
    row: &TrackedMessage,
) -> Result<Option<Rendered>, DbError> {
    let Some(t) = tickets::get(conn, guild, ticket).await? else {
        return Ok(None);
    };
    let transcript = tickets::transcript(conn, guild, ticket).await?;
    let mut attach = None;
    let name = quack_core::modules::ticket_transcript_name(t.number);
    // A kept attachment belongs to the posted message; a post being made
    // again (no message yet) uploads its own. Either way the post names
    // the file in a File component, or Discord drops it (Components V2).
    let kept = row.attachment_id.filter(|_| row.message.is_some());
    let place = match (&transcript, kept) {
        (_, Some(_)) => TranscriptPlace::Attached(name),
        (Some(saved), None) => {
            attach = Some(OutgoingFile {
                name: name.clone(),
                content_type: Some("text/plain; charset=utf-8".to_owned()),
                bytes: saved.content.clone().into_bytes(),
            });
            TranscriptPlace::Attached(name)
        }
        (None, None) => TranscriptPlace::NotYet,
    };
    let post = TicketPost {
        ticket,
        member: Person::unnamed(t.member.account()),
        state: t.state,
        open_failed: t.close_reason == Some(TicketCloseReason::OpenFailed),
        thread: t.thread.map(|id| ChannelRef { id, name: None }),
        thread_url: t
            .thread
            .map(|id| format!("https://discord.com/channels/{guild}/{id}")),
        opened_at: t.opened_at.unwrap_or(t.created_at),
        closed: t.closed_at.map(|at| {
            (
                t.closed_by
                    .map_or(Actor::Quack, |by| Actor::Person(Person::unnamed(by))),
                at,
            )
        }),
        transcript: place,
        partial: transcript.and_then(|s| s.partial),
        thread_not_deleted: t.thread_delete_failed,
    };
    Ok(Some(Rendered {
        msg: quack_text::ticket::post(&post),
        attach,
    }))
}
