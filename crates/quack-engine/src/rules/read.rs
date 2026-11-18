//! Reading rules (§14.3 `RuleSummary`, `Rule`, `RuleVersion`) and the
//! export file (§3.6). Moderators and rules managers may read; export is
//! for rules managers.

use quack_core::DomainError;
use quack_core::ids::RuleId;
use quack_core::permissions::{Capabilities, Capability};
use quack_core::rule::RuleFile;
use quack_db::rules::{Rule, RuleVersion, VersionInfo};
use quack_db::{honeypot, rules, settings};
use time::OffsetDateTime;

use super::RuleError;
use crate::{Engine, Principal, obs};

/// The server's rules with the two settings that point at rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleList {
    /// The rules, by name.
    pub rules: Vec<Rule>,
    /// The server's starter rule.
    pub starter: Option<RuleId>,
    /// The rule the honeypot applies.
    pub honeypot_rule: Option<RuleId>,
}

/// Moderators and rules managers may read rules (§14.2 routes 12, 14, 59).
async fn require_reader(engine: &Engine, principal: &Principal) -> Result<(), RuleError> {
    let mut conn = engine.db().conn().await?;
    let caps: Capabilities = principal.current_capabilities(&mut conn).await?;
    if !caps.holds(Capability::Moderate) && !caps.holds(Capability::ManageRules) {
        return Err(RuleError::Refused(DomainError::MissingCapability(
            Capability::Moderate,
        )));
    }
    if caps.mfa_ok {
        Ok(())
    } else {
        Err(RuleError::Refused(DomainError::MfaRequired))
    }
}

/// The server's active (or archived) rules, by name.
pub async fn list(
    engine: &Engine,
    principal: &Principal,
    archived: bool,
) -> Result<RuleList, RuleError> {
    obs::use_case("list_rules", Some(principal.guild), async {
        require_reader(engine, principal).await?;
        let guild = principal.guild;
        let mut conn = engine.db().conn().await?;
        let starter = settings::get(&mut conn, guild)
            .await?
            .and_then(|s| s.starter_rule);
        let honeypot_rule = honeypot::settings(&mut conn, guild)
            .await?
            .and_then(|s| s.rule);
        Ok(RuleList {
            rules: rules::list(&mut conn, guild, archived).await?,
            starter,
            honeypot_rule,
        })
    })
    .await
}

/// One rule with its version history.
pub async fn get(
    engine: &Engine,
    principal: &Principal,
    rule: RuleId,
) -> Result<(Rule, Vec<VersionInfo>), RuleError> {
    obs::use_case("get_rule", Some(principal.guild), async {
        require_reader(engine, principal).await?;
        let mut conn = engine.db().conn().await?;
        let r = rules::get(&mut conn, principal.guild, rule)
            .await?
            .ok_or(DomainError::RuleNotFound)?;
        let versions = rules::versions(&mut conn, principal.guild, rule).await?;
        Ok((r, versions))
    })
    .await
}

/// One version of a rule, with its definition.
pub async fn version(
    engine: &Engine,
    principal: &Principal,
    rule: RuleId,
    number: u32,
) -> Result<RuleVersion, RuleError> {
    obs::use_case("get_rule_version", Some(principal.guild), async {
        require_reader(engine, principal).await?;
        let mut conn = engine.db().conn().await?;
        Ok(rules::version(&mut conn, principal.guild, rule, number)
            .await?
            .ok_or(DomainError::RuleNotFound)?)
    })
    .await
}

/// The portable rule file (§3.6): no server ids, cases, or settings.
/// A read, not logged.
pub async fn export(
    engine: &Engine,
    principal: &Principal,
    rule: RuleId,
) -> Result<RuleFile, RuleError> {
    obs::use_case("export_rule", Some(principal.guild), async {
        let mut conn = engine.db().conn().await?;
        principal
            .require(&mut conn, Capability::ManageRules)
            .await??;
        let r = rules::get(&mut conn, principal.guild, rule)
            .await?
            .ok_or(DomainError::RuleNotFound)?;
        Ok(RuleFile::new(r.definition, OffsetDateTime::now_utc()))
    })
    .await
}
