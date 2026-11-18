//! One case and everything about it, loaded once and projected three
//! ways: the staff card (Discord), the staff view (dashboard), and the
//! member view (§4.10).

use std::collections::HashMap;

use quack_core::action::{ActionKind, ActionState};
use quack_core::dm::DmKind;
use quack_core::ids::{CaseId, GuildId};
use quack_core::rule::RuleDefinition;
use quack_db::actions::{Action, Attempt};
use quack_db::appeals::{Appeal, AppealEvent};
use quack_db::cases::{Case, CaseRef as DbCaseRef};
use quack_db::dms::Dm;
use quack_db::evidence::{Evidence, EvidenceFile};
use quack_db::{DbError, PgConnection, actions, appeals, cases, dms, evidence, rules};
use quack_text::action::ActionView;
use quack_text::dm::DmView;
use time::OffsetDateTime;

/// A case with its decision's rule version, actions, DM, evidence, appeal,
/// and replacement links.
#[derive(Clone, Debug)]
pub(crate) struct CaseRecords {
    pub case: Case,
    /// The rule version the case used (`None` for imported cases).
    pub definition: Option<RuleDefinition>,
    /// The original action first, then its undo.
    pub actions: Vec<Action>,
    /// Every attempt of the case's actions, oldest first.
    pub attempts: Vec<Attempt>,
    /// The case DM (`None` when the level sends none).
    pub dm: Option<Dm>,
    pub evidence: Vec<Evidence>,
    pub files: Vec<EvidenceFile>,
    pub appeal: Option<Appeal>,
    pub appeal_events: Vec<AppealEvent>,
    pub replaces: Option<DbCaseRef>,
    pub replaced_by: Option<DbCaseRef>,
    /// Numbers of the cases named by skipped undos (`other_case_applies`).
    pub still_applies: HashMap<CaseId, u64>,
}

impl CaseRecords {
    /// Loads a case of `guild` and its records.
    pub(crate) async fn load(
        conn: &mut PgConnection,
        guild: GuildId,
        case: CaseId,
    ) -> Result<Option<Self>, DbError> {
        let Some(case) = cases::get(conn, guild, case).await? else {
            return Ok(None);
        };
        Self::of(conn, case).await.map(Some)
    }

    /// Loads the records of an already-loaded case.
    pub(crate) async fn of(conn: &mut PgConnection, case: Case) -> Result<Self, DbError> {
        let guild = case.guild;
        let definition = match case.rule {
            Some((rule, version)) => rules::version(conn, guild, rule, version)
                .await?
                .map(|v| v.definition),
            None => None,
        };
        let actions = actions::for_case(conn, guild, case.id).await?;
        let attempts = actions::attempts_for_case(conn, guild, case.id).await?;
        let dm = dms::for_case(conn, guild, case.id)
            .await?
            .into_iter()
            .find(|d| d.kind == DmKind::Case);
        let evidence = evidence::for_case(conn, guild, case.id).await?;
        let files = evidence::files_for_case(conn, guild, case.id).await?;
        let appeal = appeals::for_case(conn, guild, case.id).await?;
        let appeal_events = match &appeal {
            Some(a) => appeals::events(conn, guild, a.id).await?,
            None => Vec::new(),
        };
        let replaces = match case.replaces {
            Some(id) => cases::by_id_ref(conn, guild, id).await?,
            None => None,
        };
        let replaced_by = cases::replaced_by(conn, guild, case.id).await?;
        let mut still_applies = HashMap::new();
        for id in actions.iter().filter_map(|a| a.skip.and_then(|(_, c)| c)) {
            if let Some(other) = cases::get(conn, guild, id).await? {
                still_applies.insert(id, other.number);
            }
        }
        Ok(Self {
            case,
            definition,
            actions,
            attempts,
            dm,
            evidence,
            files,
            appeal,
            appeal_events,
            replaces,
            replaced_by,
            still_applies,
        })
    }

    /// The original action.
    pub(crate) fn original(&self) -> Option<&Action> {
        self.actions.iter().find(|a| a.undo_of.is_none())
    }

    /// The undo of the original.
    pub(crate) fn undo(&self) -> Option<&Action> {
        self.actions.iter().find(|a| a.undo_of.is_some())
    }

    /// The rule's name in the case's version.
    pub(crate) fn rule_name(&self) -> String {
        self.definition
            .as_ref()
            .map_or_else(|| quack_text::word::IMPORTED.to_owned(), |d| d.name.clone())
    }

    /// The official reason.
    pub(crate) fn reason(&self) -> String {
        self.definition
            .as_ref()
            .map_or_default(|d| d.reason.clone())
    }

    /// The action as the action sentences see it.
    pub(crate) fn action_view(&self, action: &Action) -> ActionView {
        let timeout_ends_at = match action.kind {
            ActionKind::RemoveTimeout => self.original().and_then(|o| o.timeout_ends_at),
            _ => action.timeout_ends_at,
        };
        ActionView {
            kind: action.kind,
            state: action.state,
            failure: action.failure,
            skip: action.skip.map(|(reason, _)| reason),
            still_applies_case: action
                .skip
                .and_then(|(_, c)| c)
                .and_then(|c| self.still_applies.get(&c).copied()),
            no_change: action.no_change,
            attempts: action.attempts,
            next_attempt_at: (action.state == ActionState::Pending)
                .then_some(action.next_attempt_at)
                .flatten(),
            timeout_ends_at,
            ban_delete_message_seconds: action.delete_message_seconds,
            case_number: self.case.number,
            dismissed: action.dismissed.map(|(at, by)| {
                (
                    quack_text::Actor::Person(quack_text::Person::unnamed(by)),
                    at,
                )
            }),
        }
    }

    /// The case DM's outcome; `None` when the level sends none.
    pub(crate) fn dm_view(&self) -> Option<DmView> {
        self.dm.as_ref().map(|d| DmView {
            state: d.state,
            failure: d.failure,
        })
    }

    /// The staff controls one action allows now (§5.8).
    pub(crate) fn controls(
        &self,
        action: &Action,
        now: OffsetDateTime,
    ) -> quack_core::action::Controls {
        let has_undo = action.kind.is_original() && self.undo().is_some();
        quack_core::action::controls(
            &crate::actions::view(action, has_undo),
            self.case.is_valid(),
            now,
        )
    }
}
