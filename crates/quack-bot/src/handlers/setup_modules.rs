//! The module areas of guided `/setup` (§11.8): tickets, the honeypot, and
//! the server activity log. Each change calls the module crate's settings
//! function (the same one the dashboard uses) or `set_enabled` with the
//! module's hooks, and returns the result line; the caller re-renders the
//! area from the reloaded config cache.

use quack_core::ids::{ChannelId, RuleId};
use quack_core::modules::{ModuleName, ModuleProblem};
use quack_engine::Principal;
use quack_engine::settings::{ModuleHooks, SettingsError, set_enabled};
use quack_honeypot::HoneypotSettingsInput;
use quack_text::actions::{SetupArea, SetupOp};
use quack_text::setup::{ChannelKind, SetupResult, module_not_ready, module_problem, result};
use quack_text::{ChannelRef, Text};
use quack_tickets::TicketSettingsInput;

use super::op_key;
use super::setup::problems_line;
use super::setup_channels::{self, Made};
use crate::ack::{Acked, Failure};
use crate::errors;
use crate::router::expired;

const fn channel(id: ChannelId) -> ChannelRef {
    ChannelRef { id, name: None }
}

/// What the caller chose: a channel or a rule from a select.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Chosen {
    pub(crate) channel: Option<ChannelId>,
    pub(crate) rule: Option<RuleId>,
}

/// One control of a module area; `None` when it only shows the area.
pub(crate) async fn apply(
    a: &Acked<'_>,
    principal: &Principal,
    area: SetupArea,
    op: SetupOp,
    chosen: Chosen,
) -> Result<Option<Text>, Failure> {
    if op == SetupOp::Open {
        return Ok(None);
    }
    let line = match area {
        SetupArea::Tickets => tickets(a, principal, op, chosen.channel).await?,
        SetupArea::Honeypot => honeypot(a, principal, op, chosen).await?,
        SetupArea::ActivityLog => activity(a, principal, op, chosen.channel).await?,
        _ => return Err(expired()),
    };
    reload(a, principal).await?;
    Ok(Some(line))
}

/// The honeypot warning modal's save (`warnm`).
pub(crate) async fn save_warning(
    a: &Acked<'_>,
    principal: &Principal,
    text: String,
) -> Result<Text, Failure> {
    let line = update_honeypot(a, principal, None, SetupResult::HoneypotWarningSaved, |i| {
        i.warning_text = Some(text);
    })
    .await?;
    reload(a, principal).await?;
    Ok(line)
}

/// Reloads the server's config cache so the re-rendered panel shows the
/// change at once (the notification reloads it again, harmlessly).
async fn reload(a: &Acked<'_>, principal: &Principal) -> Result<(), Failure> {
    let engine = a.engine();
    engine
        .config_cache()
        .reload(engine.db(), principal.guild)
        .await
        .map(|_| ())
        .map_err(|e| errors::db(&e))
}

/// A settings result as the line shown: the result, the field problems,
/// or the refusal.
fn line<T>(
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

/// Turns a module on or off; problems become the line.
async fn toggle(
    a: &Acked<'_>,
    principal: &Principal,
    hooks: &ModuleHooks,
    on: bool,
    done: SetupResult,
    facts: (Option<&str>, Option<&ChannelRef>),
) -> Result<Text, Failure> {
    let op = op_key(a.ix, if on { "setup_on" } else { "setup_off" });
    match set_enabled(a.engine(), principal, hooks, on, &op).await {
        Ok(_) => Ok(result(&done)),
        Err(SettingsError::ModuleNotReady(problems)) => Ok(not_ready(hooks.name, &problems, facts)),
        Err(e) => Err(errors::settings(&e)),
    }
}

fn not_ready(
    module: ModuleName,
    problems: &[ModuleProblem],
    (rule, channel): (Option<&str>, Option<&ChannelRef>),
) -> Text {
    let lines: Vec<Text> = problems
        .iter()
        .map(|p| module_problem(p, rule, channel))
        .collect();
    module_not_ready(module, &lines)
}

/// "Create …": makes or adopts the channel and says which.
async fn make(
    a: &Acked<'_>,
    principal: &Principal,
    kind: ChannelKind,
) -> Result<(ChannelId, SetupResult, bool), Failure> {
    let engine = a.engine();
    let view = quack_engine::settings::get(engine, principal)
        .await
        .map_err(|e| errors::settings(&e))?;
    let made = {
        let _one_at_a_time = a.shared().creating.lock().await;
        setup_channels::create(engine, principal.guild, kind, &view.moderator_role_ids).await?
    };
    Ok(match made {
        Made::Created(c) if setup_channels::is_public(kind) => {
            (c.id, SetupResult::CreatedPublic(c), true)
        }
        Made::Created(c) => (c.id, SetupResult::Created(c), true),
        Made::Adopted(c) => (c.id, SetupResult::Adopted(c), false),
    })
}

async fn tickets(
    a: &Acked<'_>,
    principal: &Principal,
    op: SetupOp,
    chosen: Option<ChannelId>,
) -> Result<Text, Failure> {
    let engine = a.engine();
    let current = || async {
        quack_tickets::settings(engine, principal)
            .await
            .map_err(|e| errors::settings(&e))
    };
    let input = |s: &quack_tickets::TicketSettingsView| TicketSettingsInput {
        expected_version: s.version,
        entry_channel_id: s.entry_channel_id,
        queue_channel_id: s.queue_channel_id,
        transcript_retention_days: s.transcript_retention_days,
    };
    match op {
        SetupOp::TicketEntry | SetupOp::TicketQueue => {
            let c = chosen.ok_or_else(expired)?;
            let mut change = input(&current().await?);
            let done = if op == SetupOp::TicketEntry {
                change.entry_channel_id = Some(c);
                SetupResult::TicketEntrySaved(channel(c))
            } else {
                change.queue_channel_id = Some(c);
                SetupResult::TicketQueueSaved(channel(c))
            };
            let saved =
                quack_tickets::update_settings(engine, principal, &change, &op_key(a.ix, "setup"))
                    .await;
            line(saved, Some(c), &done)
        }
        SetupOp::Create => {
            let (entry, entry_done, _) = make(a, principal, ChannelKind::TicketEntry).await?;
            let (queue, queue_done, queue_new) =
                make(a, principal, ChannelKind::TicketQueue).await?;
            let mut change = input(&current().await?);
            change.entry_channel_id = Some(entry);
            change.queue_channel_id = Some(queue);
            let saved =
                quack_tickets::update_settings(engine, principal, &change, &op_key(a.ix, "setup"))
                    .await;
            let first = line(saved, None, &entry_done)?;
            if queue_new {
                setup_channels::introduce(engine, ChannelKind::TicketQueue, queue).await;
            }
            Ok(first.s("\n").then(result(&queue_done)))
        }
        SetupOp::TurnOn => {
            let entry = current().await?.entry_channel_id.map(channel);
            let done = match &entry {
                Some(c) => SetupResult::TicketsOn(c.clone()),
                None => SetupResult::TicketsOff,
            };
            toggle(
                a,
                principal,
                &quack_tickets::hooks(),
                true,
                done,
                (None, entry.as_ref()),
            )
            .await
        }
        SetupOp::TurnOff => {
            let hooks = quack_tickets::hooks();
            toggle(
                a,
                principal,
                &hooks,
                false,
                SetupResult::TicketsOff,
                (None, None),
            )
            .await
        }
        _ => Err(expired()),
    }
}

/// Saves the honeypot settings with one change applied.
async fn update_honeypot(
    a: &Acked<'_>,
    principal: &Principal,
    chosen: Option<ChannelId>,
    done: SetupResult,
    change: impl FnOnce(&mut HoneypotSettingsInput),
) -> Result<Text, Failure> {
    let engine = a.engine();
    let s = quack_honeypot::settings(engine, principal)
        .await
        .map_err(|e| errors::settings(&e))?;
    let mut input = HoneypotSettingsInput {
        expected_version: s.version,
        channel_id: s.channel_id,
        rule_id: s.rule_id,
        warning_text: s.warning_text,
    };
    change(&mut input);
    let saved =
        quack_honeypot::update_settings(engine, principal, &input, &op_key(a.ix, "setup")).await;
    line(saved, chosen, &done)
}

fn rule_name(a: &Acked<'_>, principal: &Principal, rule: Option<RuleId>) -> Option<String> {
    let rules = a.engine().rules_cache().cached(principal.guild)?;
    rules.active(rule?).map(|r| r.definition.name.clone())
}

async fn honeypot(
    a: &Acked<'_>,
    principal: &Principal,
    op: SetupOp,
    chosen: Chosen,
) -> Result<Text, Failure> {
    let engine = a.engine();
    match op {
        SetupOp::Channel => {
            let c = chosen.channel.ok_or_else(expired)?;
            let done = SetupResult::HoneypotChannelSaved(channel(c));
            update_honeypot(a, principal, Some(c), done, |i| i.channel_id = Some(c)).await
        }
        SetupOp::Create => {
            let (c, done, _) = make(a, principal, ChannelKind::Honeypot).await?;
            update_honeypot(a, principal, Some(c), done, |i| i.channel_id = Some(c)).await
        }
        SetupOp::Rule => {
            let rule = chosen.rule.ok_or_else(expired)?;
            let name = rule_name(a, principal, Some(rule)).unwrap_or_default();
            let done = SetupResult::HoneypotRuleSaved(name);
            update_honeypot(a, principal, None, done, |i| i.rule_id = Some(rule)).await
        }
        SetupOp::ClearRule => {
            let done = SetupResult::HoneypotRuleCleared;
            update_honeypot(a, principal, None, done, |i| i.rule_id = None).await
        }
        SetupOp::TurnOn => {
            let s = quack_honeypot::settings(engine, principal)
                .await
                .map_err(|e| errors::settings(&e))?;
            let mut lines = Vec::new();
            let mut rule = s.rule_id;
            if rule.is_none() {
                match quack_honeypot::use_default_rule(engine, principal).await {
                    Ok(view) => {
                        rule = view.rule_id;
                        let name = rule_name(a, principal, rule).unwrap_or_default();
                        lines.push(result(&SetupResult::HoneypotRuleCreated(name)));
                    }
                    Err(SettingsError::ModuleNotReady(problems)) => {
                        return Ok(not_ready(ModuleName::Honeypot, &problems, (None, None)));
                    }
                    Err(e) => return Err(errors::settings(&e)),
                }
            }
            let ch = s.channel_id.map(channel);
            let done = match &ch {
                Some(c) => SetupResult::HoneypotOn(c.clone()),
                None => SetupResult::HoneypotOff,
            };
            let name = rule_name(a, principal, rule);
            let hooks = quack_honeypot::hooks();
            let on = toggle(
                a,
                principal,
                &hooks,
                true,
                done,
                (name.as_deref(), ch.as_ref()),
            );
            lines.push(on.await?);
            Ok(join(lines))
        }
        SetupOp::TurnOff => {
            let hooks = quack_honeypot::hooks();
            toggle(
                a,
                principal,
                &hooks,
                false,
                SetupResult::HoneypotOff,
                (None, None),
            )
            .await
        }
        _ => Err(expired()),
    }
}

fn join(lines: Vec<Text>) -> Text {
    let mut out: Option<Text> = None;
    for l in lines {
        out = Some(match out {
            None => l,
            Some(o) => o.s("\n").then(l),
        });
    }
    out.unwrap_or_default()
}

async fn activity(
    a: &Acked<'_>,
    principal: &Principal,
    op: SetupOp,
    chosen: Option<ChannelId>,
) -> Result<Text, Failure> {
    let engine = a.engine();
    let route = |c: ChannelId| async move {
        quack_activity_log::route_all(engine, principal, c, &op_key(a.ix, "setup")).await
    };
    match op {
        SetupOp::Channel => {
            let c = chosen.ok_or_else(expired)?;
            line(
                route(c).await,
                Some(c),
                &SetupResult::ActivitySaved(channel(c)),
            )
        }
        SetupOp::Create => {
            let (c, done, new) = make(a, principal, ChannelKind::ActivityLog).await?;
            let saved = line(route(c).await, Some(c), &done)?;
            if new {
                setup_channels::introduce(engine, ChannelKind::ActivityLog, c).await;
            }
            Ok(saved)
        }
        SetupOp::TurnOn => {
            let hooks = quack_activity_log::hooks();
            toggle(
                a,
                principal,
                &hooks,
                true,
                SetupResult::ActivityOn,
                (None, None),
            )
            .await
        }
        SetupOp::TurnOff => {
            let hooks = quack_activity_log::hooks();
            toggle(
                a,
                principal,
                &hooks,
                false,
                SetupResult::ActivityOff,
                (None, None),
            )
            .await
        }
        _ => Err(expired()),
    }
}
