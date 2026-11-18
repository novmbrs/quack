//! Reading rules (§14.3 `RuleSummary`, `Rule`, `RuleVersion`) and the
//! export file (§3.6). Moderators and rules managers may read; export is
//! for rules managers.

use quack_core::DomainError;
use quack_core::ids::{AccountId, RuleId};
use quack_core::permissions::{Capabilities, Capability};
use quack_core::rule::{RuleDefinition, RuleFile};
use quack_db::{honeypot, rules, settings};
use serde::Serialize;
use time::OffsetDateTime;

use super::RuleError;
use crate::{Engine, Principal, obs};

/// One rule in a list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RuleSummary {
    /// The rule.
    pub id: RuleId,
    /// Its current name.
    pub name: String,
    /// Its description.
    pub description: String,
    /// Its current version.
    pub current_version: u32,
    /// When it was archived.
    #[serde(with = "time::serde::rfc3339::option")]
    pub archived_at: Option<OffsetDateTime>,
    /// Its levels in words.
    pub levels_text: String,
    /// How many context fields it has.
    pub field_count: usize,
    /// It is the server's starter rule.
    pub is_starter: bool,
    /// The honeypot uses it.
    pub used_by_honeypot: bool,
}

/// One rule with its current definition and version history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RuleView {
    /// The rule.
    pub id: RuleId,
    /// Its current version.
    pub current_version: u32,
    /// When it was archived.
    #[serde(with = "time::serde::rfc3339::option")]
    pub archived_at: Option<OffsetDateTime>,
    /// When it was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Who created it.
    pub created_by: Option<AccountId>,
    /// When it last changed.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// Who changed it last.
    pub updated_by: Option<AccountId>,
    /// The current version's definition.
    pub definition: RuleDefinition,
    /// Every version, oldest first.
    pub versions: Vec<VersionView>,
}

/// One version's history entry, or one version with its definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VersionView {
    /// The version.
    pub version: u32,
    /// When it was saved.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Who saved it.
    pub created_by: Option<AccountId>,
    /// Rebuilt by the importer.
    pub reconstructed: bool,
    /// The definition (only when one version is read).
    pub definition: Option<RuleDefinition>,
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
) -> Result<Vec<RuleSummary>, RuleError> {
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
        Ok(rules::list(&mut conn, guild, archived)
            .await?
            .into_iter()
            .map(|r| RuleSummary {
                id: r.id,
                levels_text: quack_text::word::levels_summary(
                    &r.definition
                        .levels
                        .iter()
                        .map(|l| (l.from_case_count, l.action))
                        .collect::<Vec<_>>(),
                ),
                field_count: r.definition.fields.len(),
                is_starter: starter == Some(r.id),
                used_by_honeypot: honeypot_rule == Some(r.id),
                name: r.definition.name,
                description: r.definition.description,
                current_version: r.current_version,
                archived_at: r.archived_at,
            })
            .collect())
    })
    .await
}

/// One rule with its version history.
pub async fn get(
    engine: &Engine,
    principal: &Principal,
    rule: RuleId,
) -> Result<RuleView, RuleError> {
    obs::use_case("get_rule", Some(principal.guild), async {
        require_reader(engine, principal).await?;
        let mut conn = engine.db().conn().await?;
        let r = rules::get(&mut conn, principal.guild, rule)
            .await?
            .ok_or(DomainError::RuleNotFound)?;
        let versions = rules::versions(&mut conn, principal.guild, rule).await?;
        Ok(RuleView {
            id: r.id,
            current_version: r.current_version,
            archived_at: r.archived_at,
            created_at: r.created_at,
            created_by: r.created_by,
            updated_at: r.updated_at,
            updated_by: r.updated_by,
            definition: r.definition,
            versions: versions
                .into_iter()
                .map(|v| VersionView {
                    version: v.version,
                    created_at: v.created_at,
                    created_by: v.created_by,
                    reconstructed: v.reconstructed,
                    definition: None,
                })
                .collect(),
        })
    })
    .await
}

/// One version of a rule, with its definition.
pub async fn version(
    engine: &Engine,
    principal: &Principal,
    rule: RuleId,
    number: u32,
) -> Result<VersionView, RuleError> {
    obs::use_case("get_rule_version", Some(principal.guild), async {
        require_reader(engine, principal).await?;
        let mut conn = engine.db().conn().await?;
        let v = rules::version(&mut conn, principal.guild, rule, number)
            .await?
            .ok_or(DomainError::RuleNotFound)?;
        Ok(VersionView {
            version: v.version,
            created_at: v.created_at,
            created_by: v.created_by,
            reconstructed: v.reconstructed,
            definition: Some(v.definition),
        })
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
