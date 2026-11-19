//! Guided `/setup` for the core settings (§11.8): the home panel and the
//! log channel, appeals, and evidence areas. Every click re-checks
//! `manage_settings` + 2FA (inside `settings::patch`), saves at once
//! through the same engine call as the dashboard, and re-renders the
//! panel with a result line. The module areas' controls are in
//! [`super::setup_modules`].

use quack_core::ids::{ChannelId, GuildId};
use quack_core::limits;
use quack_discord::interaction::{FieldValue, SelectChoice, Source};
use quack_engine::Principal;
use quack_engine::settings::{
    FieldProblem, SettingsError, SettingsPatch, SettingsView, get, patch,
};
use quack_text::actions::{InputId, SetupArea, SetupOp};
use quack_text::case::PickerRule;
use quack_text::setup::{
    ActivityRoutes, ChannelKind, SetupResult, SetupState, channel_not_staff_only, channel_problem,
    invite_modal, result, settings_problem, warning_modal,
};
use quack_text::{ChannelRef, Msg, Text};

use super::op_key;
use super::setup_channels;
use super::setup_modules::{self, Chosen};
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::router::{expired, text_of};
use crate::{auth, errors};

const fn channel(id: ChannelId) -> ChannelRef {
    ChannelRef { id, name: None }
}

/// The panel's state from the saved settings and the cached module
/// settings.
fn state(a: &Acked<'_>, guild: GuildId, view: &SettingsView) -> SetupState {
    let engine = a.engine();
    let config = engine.config_cache().cached(guild);
    let tickets = config.as_ref().and_then(|c| c.tickets.clone());
    let honeypot = config.as_ref().and_then(|c| c.honeypot.clone());
    let activity = config.as_ref().and_then(|c| c.activity_log.clone());
    let honeypot_rule = honeypot.as_ref().and_then(|h| h.rule).and_then(|rule| {
        engine
            .rules_cache()
            .cached(guild)
            .and_then(|r| r.active(rule).map(|c| c.definition.name.clone()))
    });
    let routes = activity.as_ref().map_or_default(|a| a.routes.clone());
    let activity_routes = match routes.first() {
        None => ActivityRoutes::None,
        Some((_, first)) if routes.iter().all(|(_, c)| c == first) => {
            ActivityRoutes::One(channel(*first))
        }
        Some(_) => ActivityRoutes::Several,
    };
    SetupState {
        log: view.log_channel_id.map(channel),
        appeals: view.appeal_channel_id.map(channel),
        invite_set: view.appeal_invite_url.is_some(),
        evidence: view.evidence_channel_id.map(channel),
        tickets_on: view.modules.tickets.enabled,
        ticket_entry: tickets.as_ref().and_then(|t| t.entry_channel).map(channel),
        ticket_queue: tickets.as_ref().and_then(|t| t.queue_channel).map(channel),
        honeypot_on: view.modules.honeypot.enabled,
        honeypot_channel: honeypot.as_ref().and_then(|h| h.channel).map(channel),
        honeypot_rule,
        activity_on: view.modules.activity_log.enabled,
        activity: activity_routes,
        settings_url: engine.links().settings(guild),
    }
}

/// The current settings, read as `manage_settings` + 2FA.
async fn read(a: &Acked<'_>, principal: &Principal) -> Result<SettingsView, Failure> {
    get(a.engine(), principal)
        .await
        .map_err(|e| errors::settings(&e))
}

/// `area`'s panel with an optional result line.
async fn panel(
    a: &Acked<'_>,
    principal: &Principal,
    area: SetupArea,
    line: Option<Text>,
) -> Result<Msg, Failure> {
    let view = read(a, principal).await?;
    let s = state(a, principal.guild, &view);
    let rules = if area == SetupArea::Honeypot {
        honeypot_rules(a, principal.guild)
    } else {
        Vec::new()
    };
    Ok(quack_text::setup::area(area, &s, line, &rules))
}

/// The rules the honeypot can use: active, without required fields
/// (§3.8), by name.
fn honeypot_rules(a: &Acked<'_>, guild: GuildId) -> Vec<PickerRule> {
    let Some(rules) = a.engine().rules_cache().cached(guild) else {
        return Vec::new();
    };
    let mut out: Vec<PickerRule> = rules
        .active
        .iter()
        .filter(|r| !r.definition.has_required_fields())
        .map(|r| PickerRule {
            id: r.id,
            name: r.definition.name.clone(),
            description: r.definition.description.clone(),
        })
        .collect();
    out.sort_by_key(|r| r.name.to_lowercase());
    out
}

/// `/setup`: the home panel.
pub(crate) async fn command(p: Pending<'_>) -> Finish {
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = async {
        let principal = auth::principal(a.engine(), a.ix).await?;
        panel(&a, &principal, SetupArea::Home, None).await
    }
    .await;
    a.finish(result).await
}

/// A `/setup` button or select of a core area (`su`).
pub(crate) async fn control(
    p: Pending<'_>,
    area: SetupArea,
    op: SetupOp,
    values: &[SelectChoice],
    source: Source,
) -> Finish {
    match (area, op) {
        (SetupArea::Appeals, SetupOp::SetInvite) => return open_invite(p).await,
        (SetupArea::Honeypot, SetupOp::EditWarning) => return open_warning(p).await,
        _ => {}
    }
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let chosen = values.iter().find_map(|v| match v {
        SelectChoice::Channel(c) => Some(*c),
        _ => None,
    });
    let result = if is_module_area(area) {
        let rule = values.iter().find_map(|v| match v {
            SelectChoice::Rule(r) => Some(*r),
            _ => None,
        });
        module_control(
            &a,
            area,
            op,
            Chosen {
                channel: chosen,
                rule,
            },
        )
        .await
    } else {
        apply(&a, area, op, chosen).await
    };
    a.finish(result).await
}

/// The areas that belong to an optional module.
const fn is_module_area(area: SetupArea) -> bool {
    matches!(
        area,
        SetupArea::Tickets | SetupArea::Honeypot | SetupArea::ActivityLog
    )
}

async fn module_control(
    a: &Acked<'_>,
    area: SetupArea,
    op: SetupOp,
    chosen: Chosen,
) -> Result<Msg, Failure> {
    let principal = auth::principal(a.engine(), a.ix).await?;
    // Reading the area first checks `manage_settings` + 2FA before any
    // module settings are shown or changed.
    read(a, &principal).await?;
    let line = setup_modules::apply(a, &principal, area, op, chosen).await?;
    panel(a, &principal, area, line).await
}

/// "Edit warning" (`warn`, §11.3's exception): the modal prefilled with
/// the server's warning.
async fn open_warning(p: Pending<'_>) -> Finish {
    let ix = p.ix;
    p.prefilled_modal(|engine| async move {
        let principal = auth::principal(engine, ix).await?;
        let view = quack_honeypot::settings(engine, &principal)
            .await
            .map_err(|e| errors::settings(&e))?;
        Ok(warning_modal(view.warning_text.as_deref()))
    })
    .await
}

/// The honeypot warning modal (`warnm`).
pub(crate) async fn save_warning(
    p: Pending<'_>,
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let text = text_of(fields, &InputId::Warning);
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = async {
        let principal = auth::principal(a.engine(), a.ix).await?;
        let line = setup_modules::save_warning(&a, &principal, text).await?;
        panel(&a, &principal, SetupArea::Honeypot, Some(line)).await
    }
    .await;
    a.finish(result).await
}

/// What a channel area saves, and the result words for it.
struct AreaChannel {
    kind: ChannelKind,
    saved: fn(ChannelRef) -> SetupResult,
    cleared: SetupResult,
    set: fn(&mut SettingsPatch, Option<ChannelId>),
}

fn area_channel(area: SetupArea) -> Option<AreaChannel> {
    Some(match area {
        SetupArea::Log => AreaChannel {
            kind: ChannelKind::Log,
            saved: SetupResult::LogSaved,
            cleared: SetupResult::LogCleared,
            set: |p, c| p.log_channel_id = Some(c),
        },
        SetupArea::Appeals => AreaChannel {
            kind: ChannelKind::Appeals,
            saved: SetupResult::AppealsSaved,
            cleared: SetupResult::AppealsCleared,
            set: |p, c| p.appeal_channel_id = Some(c),
        },
        SetupArea::Evidence => AreaChannel {
            kind: ChannelKind::Evidence,
            saved: SetupResult::EvidenceSaved,
            cleared: SetupResult::EvidenceCleared,
            set: |p, c| p.evidence_channel_id = Some(c),
        },
        _ => return None,
    })
}

async fn apply(
    a: &Acked<'_>,
    area: SetupArea,
    op: SetupOp,
    chosen: Option<ChannelId>,
) -> Result<Msg, Failure> {
    let principal = auth::principal(a.engine(), a.ix).await?;
    let line = match (area_channel(area), op) {
        (_, SetupOp::Open) => None,
        (Some(ch), SetupOp::Channel) => {
            let c = chosen.ok_or_else(expired)?;
            let mut changes = SettingsPatch::default();
            (ch.set)(&mut changes, Some(c));
            Some(save(a, &principal, &changes, Some(c), (ch.saved)(channel(c))).await?)
        }
        (Some(ch), SetupOp::Clear) => {
            let mut changes = SettingsPatch::default();
            (ch.set)(&mut changes, None);
            Some(save(a, &principal, &changes, None, ch.cleared).await?)
        }
        (Some(ch), SetupOp::Create) => Some(create(a, &principal, &ch).await?),
        (Some(_), SetupOp::ClearInvite) if area == SetupArea::Appeals => {
            let changes = SettingsPatch {
                appeal_invite_url: Some(String::new()),
                ..SettingsPatch::default()
            };
            Some(save(a, &principal, &changes, None, SetupResult::InviteCleared).await?)
        }
        _ => return Err(expired()),
    };
    panel(a, &principal, area, line).await
}

/// "Create #…": make or adopt the channel, save it, then post its intro.
async fn create(a: &Acked<'_>, principal: &Principal, ch: &AreaChannel) -> Result<Text, Failure> {
    let (id, done, new) = setup_modules::make(a, principal, ch.kind).await?;
    let mut changes = SettingsPatch::default();
    (ch.set)(&mut changes, Some(id));
    let line = save(a, principal, &changes, Some(id), done).await?;
    if new {
        setup_channels::introduce(a.engine(), ch.kind, id).await;
    }
    Ok(line)
}

/// Saves `changes`; field problems become the result line.
async fn save(
    a: &Acked<'_>,
    principal: &Principal,
    changes: &SettingsPatch,
    chosen: Option<ChannelId>,
    done: SetupResult,
) -> Result<Text, Failure> {
    let saved = patch(a.engine(), principal, changes, &op_key(a.ix, "setup")).await;
    line(saved, chosen, &done)
}

/// A settings result as the line shown: the result, the field problems,
/// or the refusal.
pub(crate) fn line<T>(
    saved: Result<T, SettingsError>,
    chosen: Option<ChannelId>,
    done: &SetupResult,
) -> Result<Text, Failure> {
    match saved {
        Ok(_) => Ok(result(done)),
        Err(SettingsError::Invalid(problems)) => Ok(problems_line(&problems, chosen)),
        Err(e) => Err(errors::settings(&e)),
    }
}

/// Field problems as the result line (`chosen` names the channel a
/// channel problem is about).
pub(crate) fn problems_line(
    problems: &[(&'static str, FieldProblem)],
    chosen: Option<ChannelId>,
) -> Text {
    let mut out = Text::new();
    for (i, (_, problem)) in problems.iter().enumerate() {
        let line = match problem {
            FieldProblem::Channel(list) => {
                let c = chosen.map_or_else(unknown_channel, channel);
                let lines: Vec<Text> = list
                    .iter()
                    .map(|p| channel_problem(p, &c, &[], &[]))
                    .collect();
                channel_not_staff_only(&c, &lines)
            }
            FieldProblem::Value(p) => settings_problem(*p, limits::DM_INTRO.max),
        };
        out = if i == 0 { line } else { out.s("\n").then(line) };
    }
    out
}

/// A channel problem's channel when the caller didn't name one.
fn unknown_channel() -> ChannelRef {
    ChannelRef {
        id: ChannelId::new(1).unwrap_or_else(|| unreachable!("1 is a snowflake")),
        name: None,
    }
}

/// Set rejoin invite (`inv`, §11.3's exception): the modal prefilled with
/// the current invite.
async fn open_invite(p: Pending<'_>) -> Finish {
    let ix = p.ix;
    p.prefilled_modal(|engine| async move {
        let principal = auth::principal(engine, ix).await?;
        let view = get(engine, &principal)
            .await
            .map_err(|e| errors::settings(&e))?;
        Ok(invite_modal(view.appeal_invite_url.as_deref()))
    })
    .await
}

/// The rejoin invite modal (`invm`).
pub(crate) async fn save_invite(
    p: Pending<'_>,
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let invite = text_of(fields, &InputId::Invite);
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = async {
        let principal = auth::principal(a.engine(), a.ix).await?;
        let changes = SettingsPatch {
            appeal_invite_url: Some(invite.clone()),
            ..SettingsPatch::default()
        };
        let line = match patch(a.engine(), &principal, &changes, &op_key(a.ix, "setup")).await {
            Ok(view) => result(&SetupResult::InviteSaved(
                view.appeal_invite_url.unwrap_or(invite),
            )),
            Err(SettingsError::Invalid(problems)) => problems_line(&problems, None),
            Err(e) => return Err(errors::settings(&e)),
        };
        panel(&a, &principal, SetupArea::Appeals, Some(line)).await
    }
    .await;
    a.finish(result).await
}
