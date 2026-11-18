//! Actions read on their own (the failed-actions page, a control's
//! answer): each item is built from its case's records, the same way the
//! case view builds it, without reading the moderation log.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use quack_core::DomainError;
use quack_core::action::ActionState;
use quack_core::ids::{ActionId, CaseId, MemberId};
use quack_core::permissions::Capability;
use quack_db::{Page, TimeCursor};
use time::OffsetDateTime;

use super::CaseError;
use super::records::CaseRecords;
use super::view_items::{ActionItem, action_item};
use crate::{Engine, Principal, obs};

/// One action with its case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedAction {
    /// The action, as the case view shows it.
    pub item: ActionItem,
    /// Its case.
    pub case: CaseId,
    /// The case's number.
    pub case_number: u64,
    /// The member.
    pub member: MemberId,
    /// The name of the rule version the case used (`word::IMPORTED`
    /// without one).
    pub rule_name: String,
}

/// The server's actions in `state`, newest first (for `failed`, the
/// dismissed or the undismissed ones), for `moderate` + 2FA: one page of
/// at most `limit`.
pub async fn actions(
    engine: &Engine,
    principal: &Principal,
    state: ActionState,
    dismissed: bool,
    cursor: Option<TimeCursor>,
    limit: u32,
) -> Result<Page<ListedAction>, CaseError> {
    let guild = principal.guild;
    obs::use_case("list_actions", Some(guild), async {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        let rows =
            quack_db::actions::list(&mut conn, guild, state, dismissed, cursor, limit).await?;
        let now = OffsetDateTime::now_utc();
        let mut records: HashMap<CaseId, CaseRecords> = HashMap::new();
        let mut out = Vec::with_capacity(rows.rows.len());
        for row in rows.rows {
            let case = row.action.case;
            let r = match records.entry(case) {
                Entry::Occupied(e) => e.into_mut(),
                Entry::Vacant(e) => {
                    let Some(found) = quack_db::cases::get(&mut conn, guild, case).await? else {
                        continue;
                    };
                    e.insert(CaseRecords::of(&mut conn, found).await?)
                }
            };
            let Some(action) = r.actions.iter().find(|a| a.id == row.action.id) else {
                continue;
            };
            let member = quack_text::Person::unnamed(r.case.member.account());
            out.push(ListedAction {
                item: action_item(r, action, &member, now),
                case,
                case_number: row.case_number,
                member: row.member,
                rule_name: r.rule_name(),
            });
        }
        Ok(Page {
            rows: out,
            more: rows.more,
        })
    })
    .await
}

/// One action of a case of the server, for `moderate` + 2FA.
pub async fn action(
    engine: &Engine,
    principal: &Principal,
    case: CaseId,
    action: ActionId,
) -> Result<ListedAction, CaseError> {
    let guild = principal.guild;
    let mut conn = engine.db().conn().await?;
    principal.require(&mut conn, Capability::Moderate).await??;
    let found = quack_db::cases::get(&mut conn, guild, case)
        .await?
        .ok_or(DomainError::CaseNotFound)?;
    let r = CaseRecords::of(&mut conn, found).await?;
    let a = r
        .actions
        .iter()
        .find(|a| a.id == action)
        .ok_or(DomainError::CaseNotFound)?;
    let member = quack_text::Person::unnamed(r.case.member.account());
    Ok(ListedAction {
        item: action_item(&r, a, &member, OffsetDateTime::now_utc()),
        case,
        case_number: r.case.number,
        member: r.case.member,
        rule_name: r.rule_name(),
    })
}
