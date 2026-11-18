//! Rendering a DM from its own record (§6.4 step 4, §6.6, §8.4): the case
//! row for a case DM, the appeal event for an appeal update, the ticket and
//! its transcript for a closed ticket.

use quack_core::action::ActionKind;
use quack_core::appeal::AppealEventKind;
use quack_core::case::ContextValues;
use quack_core::rule::RuleDefinition;
use quack_db::cases::Case;
use quack_db::dms::Dm;
use quack_db::{DbError, PgConnection, actions, appeals, cases, guilds, rules, settings, tickets};
use quack_text::Msg;
use quack_text::case::ContextLine;
use quack_text::dm::{AppealDm, AppealDmKind, CaseDm, RejoinAfter};

use crate::Engine;
use crate::port::OutgoingFile;

/// A DM ready to send; `None` when its records are gone.
pub(super) struct Rendered {
    pub msg: Msg,
    pub files: Vec<OutgoingFile>,
}

pub(super) async fn render(engine: &Engine, dm: &Dm) -> Result<Option<Rendered>, DbError> {
    let mut conn = engine.db().conn().await?;
    let server = guilds::get(&mut conn, dm.guild)
        .await?
        .map_or_default(|g| g.name);
    if let Some(event) = dm.appeal_event {
        return appeal(engine, &mut conn, dm, event, server).await;
    }
    if let Some(ticket) = dm.ticket {
        let Some(transcript) = tickets::transcript(&mut conn, dm.guild, ticket).await? else {
            return Ok(None);
        };
        let name = format!("transcript-{}.txt", ticket.to_u22());
        return Ok(Some(Rendered {
            msg: quack_text::ticket::closed_dm(&server, &name),
            files: vec![OutgoingFile {
                name,
                content_type: Some("text/plain; charset=utf-8".to_owned()),
                bytes: transcript.content.into_bytes(),
            }],
        }));
    }
    let Some(case) = dm.case else {
        return Ok(None);
    };
    let Some(case) = cases::get(&mut conn, dm.guild, case).await? else {
        return Ok(None);
    };
    case_dm(engine, &mut conn, &case, server).await
}

async fn case_dm(
    engine: &Engine,
    conn: &mut PgConnection,
    case: &Case,
    server: String,
) -> Result<Option<Rendered>, DbError> {
    let (Some((rule, version)), Some(decision)) = (case.rule, case.decision) else {
        return Ok(None);
    };
    let Some(version) = rules::version(conn, case.guild, rule, version).await? else {
        return Ok(None);
    };
    let def = version.definition;
    let settings = settings::get(conn, case.guild).await?;
    let timeout_ends_at = actions::for_case(conn, case.guild, case.id)
        .await?
        .into_iter()
        .find(|a| a.kind == ActionKind::Timeout)
        .and_then(|a| a.timeout_ends_at);
    let msg = quack_text::dm::case(&CaseDm {
        case: case.id,
        number: case.number,
        created_at: case.created_at,
        server,
        rule: def.name.clone(),
        reason: def.reason.clone(),
        intro: settings.as_ref().and_then(|s| s.dm_intro.clone()),
        context: context_lines(&def, &case.context)
            .into_iter()
            .filter(|l| !l.is_link())
            .collect(),
        action: decision.action,
        timeout_ends_at,
        appealable: def.appealable,
        footer: settings.and_then(|s| s.dm_footer),
        dashboard_url: engine.links().member_case(case.guild, case.id),
    });
    Ok(Some(Rendered {
        msg,
        files: Vec::new(),
    }))
}

/// A case's answered context, in its rule version's field order.
#[allow(dead_code, reason = "the staff and member case views use it next")]
pub(crate) fn context_lines(def: &RuleDefinition, values: &ContextValues) -> Vec<ContextLine> {
    let fields = def.context_fields();
    let values = values.clone().with_kinds(&fields);
    fields
        .iter()
        .filter_map(|f| {
            values.get(&f.key).map(|value| ContextLine {
                label: f.label.clone(),
                kind: f.kind,
                value: value.clone(),
            })
        })
        .collect()
}

async fn appeal(
    engine: &Engine,
    conn: &mut PgConnection,
    dm: &Dm,
    event: quack_core::ids::AppealEventId,
    server: String,
) -> Result<Option<Rendered>, DbError> {
    let Some(event) = appeals::event(conn, dm.guild, event).await? else {
        return Ok(None);
    };
    let Some(appeal) = appeals::get(conn, dm.guild, event.appeal).await? else {
        return Ok(None);
    };
    let Some(case) = cases::get(conn, dm.guild, appeal.case).await? else {
        return Ok(None);
    };
    let kind = match event.kind {
        AppealEventKind::InformationRequested => AppealDmKind::InformationRequested,
        AppealEventKind::Reopened => AppealDmKind::Reopened,
        AppealEventKind::Accepted => {
            let invite = settings::get(conn, dm.guild)
                .await?
                .and_then(|s| s.appeal_invite_url);
            let after = match case.decision.and_then(|d| d.action_kind()) {
                Some(ActionKind::Ban) => Some(RejoinAfter::Ban),
                Some(ActionKind::Kick) => Some(RejoinAfter::Kick),
                _ => None,
            };
            AppealDmKind::Accepted(after.zip(invite))
        }
        AppealEventKind::Rejected => AppealDmKind::Rejected,
        AppealEventKind::Closed if event.closed_by_void => AppealDmKind::ClosedCaseVoided,
        AppealEventKind::Closed => AppealDmKind::Closed,
        AppealEventKind::Submitted | AppealEventKind::InformationProvided => return Ok(None),
    };
    let msg = quack_text::dm::appeal(&AppealDm {
        kind,
        server,
        number: case.number,
        reason: event.text,
        at: event.created_at,
        dashboard_url: engine.links().member_case(case.guild, case.id),
    });
    Ok(Some(Rendered {
        msg,
        files: Vec::new(),
    }))
}
