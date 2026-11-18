//! Rules (§3): create, edit (a new immutable version), archive, restore,
//! import and export, and reading them. Every write notifies the rules
//! cache (§17.6).

mod read;

use quack_core::DomainError;
use quack_core::ids::{GuildId, RuleId};
use quack_core::limits::MAX_ACTIVE_RULES;
use quack_core::messages::MessageKind;
use quack_core::modlog::{ModLogEntry, ModLogLinks, RuleNameDetail, RuleVersionDetail};
use quack_core::permissions::Capability;
use quack_core::rule::{RuleDefinition, RuleFile, RuleFileError, RuleProblem, Strictness};
use quack_db::messages::{self, Owner};
use quack_db::{Conflict, DbError, Tx, honeypot, notify, rules, settings};
use serde::Serialize;

use crate::modlog::{self, By};
use crate::opkey::{KeyError, Keyed, begin_keyed, complete, request_hash, stored_id};
use crate::{Engine, OpKey, Principal, obs};

pub use read::{RuleSummary, RuleView, VersionView, export, get, list, version};

/// Why a rule write was refused.
#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    /// A refusal with a `quack-text` message (not found, changed, name
    /// taken, too many, used by the honeypot, capability, 2FA).
    #[error("refused: {0:?}")]
    Refused(DomainError),
    /// The definition has problems (every one is listed).
    #[error("invalid rule")]
    Invalid(Vec<RuleProblem>),
    /// The uploaded file is not a Quack rule file this version reads.
    #[error("invalid rule file")]
    File(RuleFileError),
    /// The operation key was used for a different request.
    #[error("operation key reused")]
    OperationKeyReused,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<DomainError> for RuleError {
    fn from(error: DomainError) -> Self {
        Self::Refused(error)
    }
}

impl From<KeyError> for RuleError {
    fn from(error: KeyError) -> Self {
        match error {
            KeyError::Reused => Self::OperationKeyReused,
            KeyError::Db(db) => Self::Db(db),
        }
    }
}

/// A rule write's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuleWritten {
    /// The rule.
    pub rule: RuleId,
    /// Its current version.
    pub version: u32,
    /// The same request already ran.
    pub replayed: bool,
}

#[derive(Serialize)]
enum Request<'a> {
    Create(&'a RuleDefinition),
    Import(&'a RuleDefinition),
    Edit(RuleId, u32, &'a RuleDefinition),
    Archive(RuleId),
    Restore(RuleId),
}

fn name_taken(error: DbError) -> RuleError {
    match error.conflict() {
        Some(Conflict::RuleNameTaken) => RuleError::Refused(DomainError::RuleNameTaken),
        _ => error.into(),
    }
}

/// Validates a definition written through Quack (`Strictness::New`).
fn validated(def: RuleDefinition) -> Result<RuleDefinition, RuleError> {
    let def = def.trimmed();
    def.validate(Strictness::New).map_err(RuleError::Invalid)?;
    Ok(def)
}

/// Opens the keyed transaction of a rule write, with the capability
/// checked again inside it.
async fn begin(
    engine: &Engine,
    principal: &Principal,
    op_key: &OpKey,
    request: &Request<'_>,
) -> Result<Result<Tx, RuleWritten>, RuleError> {
    let key = op_key.scoped(Some(principal.guild), Some(principal.account));
    match begin_keyed(engine.db(), &key, &request_hash(request)).await? {
        Keyed::Replay(response) => Ok(Err(RuleWritten {
            rule: stored_id(&response, "rule_id")?,
            version: stored_id(&response, "version")?,
            replayed: true,
        })),
        Keyed::Fresh(mut tx) => {
            principal
                .require(&mut tx, Capability::ManageRules)
                .await??;
            Ok(Ok(tx))
        }
    }
}

/// Logs, notifies the cache, stores the key's result, commits, and
/// reloads this process's cache.
async fn finish(
    engine: &Engine,
    principal: &Principal,
    op_key: &OpKey,
    mut tx: Tx,
    entry: ModLogEntry,
    written: RuleWritten,
) -> Result<RuleWritten, RuleError> {
    let guild = principal.guild;
    modlog::write(
        &mut tx,
        guild,
        entry,
        By {
            actor: Some(principal.account),
            source: principal.origin.mod_log_source(),
        },
        ModLogLinks {
            rule_id: Some(written.rule),
            ..ModLogLinks::default()
        },
    )
    .await?;
    notify::rules_changed(&mut tx, guild).await?;
    let key = op_key.scoped(Some(guild), Some(principal.account));
    let stored = serde_json::json!({ "rule_id": written.rule, "version": written.version });
    complete(&mut tx, &key, 200, stored).await?;
    tx.commit().await?;
    engine.rules_cache().reload(engine.db(), guild).await?;
    Ok(written)
}

/// Creates a rule at version 1 (§3.1), checking the active-rule limit
/// under the `guild_settings` lock.
pub async fn create(
    engine: &Engine,
    principal: &Principal,
    definition: RuleDefinition,
    op_key: &OpKey,
) -> Result<RuleWritten, RuleError> {
    obs::use_case("create_rule", Some(principal.guild), async {
        let def = validated(definition)?;
        insert(engine, principal, def, op_key, false).await
    })
    .await
}

/// Imports a rule file as a new rule at version 1 (§3.6); `name` replaces
/// the file's name (the dashboard asks for one after a name clash).
pub async fn import(
    engine: &Engine,
    principal: &Principal,
    file: RuleFile,
    name: Option<String>,
    op_key: &OpKey,
) -> Result<RuleWritten, RuleError> {
    obs::use_case("import_rule", Some(principal.guild), async {
        let mut def = file.into_rule().map_err(|e| match e {
            RuleFileError::Invalid(problems) => RuleError::Invalid(problems),
            other => RuleError::File(other),
        })?;
        if let Some(name) = name {
            def.name = name;
        }
        let def = validated(def)?;
        insert(engine, principal, def, op_key, true).await
    })
    .await
}

async fn insert(
    engine: &Engine,
    principal: &Principal,
    def: RuleDefinition,
    op_key: &OpKey,
    imported: bool,
) -> Result<RuleWritten, RuleError> {
    let guild = principal.guild;
    let request = if imported {
        Request::Import(&def)
    } else {
        Request::Create(&def)
    };
    let mut conn = engine.db().conn().await?;
    principal
        .require(&mut conn, Capability::ManageRules)
        .await??;
    drop(conn);
    let mut tx = match begin(engine, principal, op_key, &request).await? {
        Ok(tx) => tx,
        Err(replayed) => return Ok(replayed),
    };
    settings::lock(&mut tx, guild).await?;
    room_for_one_more(&mut tx, guild).await?;
    let id = RuleId::from_uuid(uuid::Uuid::now_v7());
    rules::create(
        &mut tx,
        &rules::NewRule {
            id,
            guild,
            definition: def.clone(),
            created_by: Some(principal.account),
            reconstructed: false,
        },
    )
    .await
    .map_err(name_taken)?;
    let entry = if imported {
        ModLogEntry::RuleImported(RuleNameDetail { name: def.name })
    } else {
        ModLogEntry::RuleCreated(RuleVersionDetail {
            name: def.name,
            version: 1,
        })
    };
    let written = RuleWritten {
        rule: id,
        version: 1,
        replayed: false,
    };
    finish(engine, principal, op_key, tx, entry, written).await
}

async fn room_for_one_more(tx: &mut Tx, guild: GuildId) -> Result<(), RuleError> {
    if rules::count_active(tx, guild).await? >= MAX_ACTIVE_RULES {
        return Err(DomainError::TooManyRules.into());
    }
    Ok(())
}

/// Saves a new version of a rule if it is still at `expected_version`
/// (else `rule_changed`). The honeypot's rule may not gain a required
/// field; editing it refreshes the honeypot warning.
pub async fn edit(
    engine: &Engine,
    principal: &Principal,
    rule: RuleId,
    expected_version: u32,
    definition: RuleDefinition,
    op_key: &OpKey,
) -> Result<RuleWritten, RuleError> {
    let guild = principal.guild;
    obs::use_case("edit_rule", Some(guild), async {
        let def = validated(definition)?;
        let request = Request::Edit(rule, expected_version, &def);
        let mut tx = match begin(engine, principal, op_key, &request).await? {
            Ok(tx) => tx,
            Err(replayed) => return Ok(replayed),
        };
        let honeypot_rule = honeypot::lock_settings(&mut tx, guild, false)
            .await?
            .and_then(|s| s.rule);
        let lock = rules::lock_for_update(&mut tx, guild, rule)
            .await?
            .ok_or(DomainError::RuleNotFound)?;
        if lock.current_version != expected_version {
            return Err(DomainError::RuleChanged.into());
        }
        if honeypot_rule == Some(rule) {
            def.check_honeypot_compatible()?;
            messages::bump(&mut tx, guild, Owner::Server(MessageKind::HoneypotWarning)).await?;
        }
        let version = rules::add_version(
            &mut tx,
            guild,
            rule,
            expected_version,
            &def,
            Some(principal.account),
        )
        .await
        .map_err(name_taken)?
        .ok_or(DomainError::RuleChanged)?;
        let entry = ModLogEntry::RuleEdited(RuleVersionDetail {
            name: def.name.clone(),
            version,
        });
        let written = RuleWritten {
            rule,
            version,
            replayed: false,
        };
        finish(engine, principal, op_key, tx, entry, written).await
    })
    .await
}

/// Archives a rule (§3.5): it leaves pickers; its cases and versions stay.
/// Refused while the honeypot's settings point at it.
pub async fn archive(
    engine: &Engine,
    principal: &Principal,
    rule: RuleId,
    op_key: &OpKey,
) -> Result<RuleWritten, RuleError> {
    let guild = principal.guild;
    obs::use_case("archive_rule", Some(guild), async {
        let mut tx = match begin(engine, principal, op_key, &Request::Archive(rule)).await? {
            Ok(tx) => tx,
            Err(replayed) => return Ok(replayed),
        };
        let honeypot_rule = honeypot::lock_settings(&mut tx, guild, false)
            .await?
            .and_then(|s| s.rule);
        if honeypot_rule == Some(rule) {
            return Err(DomainError::RuleUsedByHoneypot.into());
        }
        let lock = rules::lock_for_update(&mut tx, guild, rule)
            .await?
            .ok_or(DomainError::RuleNotFound)?;
        rules::archive(&mut tx, guild, rule, Some(principal.account)).await?;
        let name = current_name(&mut tx, guild, rule).await?;
        let written = RuleWritten {
            rule,
            version: lock.current_version,
            replayed: false,
        };
        let entry = ModLogEntry::RuleArchived(RuleNameDetail { name });
        finish(engine, principal, op_key, tx, entry, written).await
    })
    .await
}

/// Restores an archived rule (§3.5), re-checking its name and the
/// active-rule limit.
pub async fn restore(
    engine: &Engine,
    principal: &Principal,
    rule: RuleId,
    op_key: &OpKey,
) -> Result<RuleWritten, RuleError> {
    let guild = principal.guild;
    obs::use_case("restore_rule", Some(guild), async {
        let mut tx = match begin(engine, principal, op_key, &Request::Restore(rule)).await? {
            Ok(tx) => tx,
            Err(replayed) => return Ok(replayed),
        };
        settings::lock(&mut tx, guild).await?;
        let lock = rules::lock_for_update(&mut tx, guild, rule)
            .await?
            .ok_or(DomainError::RuleNotFound)?;
        if lock.archived {
            room_for_one_more(&mut tx, guild).await?;
        }
        rules::restore(&mut tx, guild, rule, Some(principal.account))
            .await
            .map_err(name_taken)?;
        let name = current_name(&mut tx, guild, rule).await?;
        let written = RuleWritten {
            rule,
            version: lock.current_version,
            replayed: false,
        };
        let entry = ModLogEntry::RuleRestored(RuleNameDetail { name });
        finish(engine, principal, op_key, tx, entry, written).await
    })
    .await
}

async fn current_name(tx: &mut Tx, guild: GuildId, rule: RuleId) -> Result<String, DbError> {
    Ok(rules::get(tx, guild, rule)
        .await?
        .map_or_default(|r| r.definition.name))
}
