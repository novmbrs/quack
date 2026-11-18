//! Case lists, a member's history, the case preview (§14.3), and a
//! member's own cases.

use quack_core::DomainError;
use quack_core::escalation::{counting_cutoff, select_level};
use quack_core::ids::{AccountId, GuildId, MemberId, RuleId};
use quack_core::permissions::Capability;
use quack_core::rule::Level;
use quack_db::cases::{self, CaseFilter, CaseSummary, MemberCaseSummary, RuleTally};
use quack_db::{TimeCursor, rules};
use serde::Serialize;
use time::OffsetDateTime;

use super::CaseError;
use crate::{Engine, Principal, obs};

/// What applying a rule to a member would choose now (§14.3 route 58).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CasePreview {
    /// The rule's current version (sent back when the case is added).
    pub rule_version: u32,
    /// Cases that count now.
    pub counted_cases: u32,
    /// The case count the new case would have.
    pub case_count: u32,
    /// The level it would choose (1-based).
    pub level_index: u16,
    /// That level.
    pub level: Level,
    /// The version's counting window.
    pub counting_window_days: Option<u16>,
    /// When this was computed.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// The server's cases, newest first, before `before_number`.
pub async fn list(
    engine: &Engine,
    principal: &Principal,
    filter: &CaseFilter,
    before_number: Option<u64>,
    limit: u32,
) -> Result<Vec<CaseSummary>, CaseError> {
    obs::use_case("list_cases", Some(principal.guild), async {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        Ok(cases::list(&mut conn, principal.guild, filter, before_number, limit).await?)
    })
    .await
}

/// A member's cases in the server, counted per rule (route 27).
pub async fn member_summary(
    engine: &Engine,
    principal: &Principal,
    member: MemberId,
) -> Result<Vec<RuleTally>, CaseError> {
    obs::use_case("member_summary", Some(principal.guild), async {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        Ok(cases::member_summary(&mut conn, principal.guild, member).await?)
    })
    .await
}

/// The counted cases, case count, and level the rule's current version
/// would choose for the member now (§3.4, route 58).
pub async fn preview(
    engine: &Engine,
    principal: &Principal,
    member: MemberId,
    rule: RuleId,
) -> Result<CasePreview, CaseError> {
    obs::use_case("case_preview", Some(principal.guild), async {
        let guild = principal.guild;
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        let r = rules::get(&mut conn, guild, rule)
            .await?
            .ok_or(DomainError::RuleNotFound)?;
        if r.archived_at.is_some() {
            return Err(DomainError::RuleArchived.into());
        }
        let at = OffsetDateTime::now_utc();
        let since = counting_cutoff(at, r.definition.counting_window_days);
        let counted = cases::count_counted(&mut conn, guild, member, rule, since).await?;
        let case_count = counted.saturating_add(1);
        let choice = select_level(&r.definition, case_count).ok_or(DomainError::RuleNotFound)?;
        let index = usize::from(choice.index.saturating_sub(1));
        let level = r
            .definition
            .levels
            .get(index)
            .cloned()
            .ok_or(DomainError::RuleNotFound)?;
        Ok(CasePreview {
            rule_version: r.current_version,
            counted_cases: counted,
            case_count,
            level_index: choice.index,
            level,
            counting_window_days: r.definition.counting_window_days,
            at,
        })
    })
    .await
}

/// A member's own cases across servers (or in one), newest first.
pub async fn member_cases(
    engine: &Engine,
    member: AccountId,
    guild: Option<GuildId>,
    cursor: Option<TimeCursor>,
    limit: u32,
) -> Result<Vec<MemberCaseSummary>, CaseError> {
    obs::use_case("member_cases", guild, async {
        let mut conn = engine.db().conn().await?;
        Ok(cases::member_cases(&mut conn, member.member(), guild, cursor, limit).await?)
    })
    .await
}
