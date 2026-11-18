//! Server statistics (route 8, §14.3): one `count(*) … GROUP BY` per
//! bucket over `[from, to)`, all in one `REPEATABLE READ` read-only
//! transaction so the buckets agree.

use quack_core::action::{ActionKind, ActionState};
use quack_core::appeal::AppealState;
use quack_core::case::CaseSource;
use quack_core::ids::{GuildId, RuleId};
use quack_core::modlog::ModLogKind;
use time::{Date, OffsetDateTime};

use crate::obs::{self, Select};
use crate::pg::count;
use crate::{Db, DbError};

/// Counts by bucket; buckets without rows are left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Statistics {
    /// Cases created in the range.
    pub cases_total: u64,
    /// Cases per UTC day.
    pub cases_by_day: Vec<(Date, u64)>,
    /// Cases per rule (`None`: imported cases without a rule), with the
    /// rule's current name.
    pub cases_by_rule: Vec<(Option<RuleId>, Option<String>, u64)>,
    /// Cases valid (`false`) and voided (`true`).
    pub cases_by_voided: Vec<(bool, u64)>,
    /// Cases per source.
    pub cases_by_source: Vec<(CaseSource, u64)>,
    /// Actions (originals and undos) per kind.
    pub actions_by_kind: Vec<(ActionKind, u64)>,
    /// Actions per current state.
    pub actions_by_state: Vec<(ActionState, u64)>,
    /// Appeals per current state.
    pub appeals_by_state: Vec<(AppealState, u64)>,
    /// Moderation log entries per kind.
    pub mod_log_by_kind: Vec<(ModLogKind, u64)>,
}

/// Computes the statistics of `guild` for rows created in `[from, to)`.
pub async fn compute(
    db: &Db,
    guild: GuildId,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Result<Statistics, DbError> {
    let g = i64::from(guild);
    let mut tx = db.begin().await?;
    obs::run(
        Select,
        "transaction",
        sqlx::query!("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx),
    )
    .await?;
    let mut s = Statistics::default();

    let rows = obs::run(
        Select,
        "cases",
        sqlx::query!(
            r#"SELECT (created_at AT TIME ZONE 'UTC')::date AS "day!", count(*) AS "n!" FROM cases
               WHERE guild_id = $1 AND created_at >= $2 AND created_at < $3 GROUP BY 1 ORDER BY 1"#,
            g,
            from,
            to
        )
        .fetch_all(&mut *tx),
    )
    .await?;
    for r in rows {
        let n = count(r.n)?;
        s.cases_total += n;
        s.cases_by_day.push((r.day, n));
    }

    let rows = obs::run(
        Select,
        "cases",
        sqlx::query!(
            r#"SELECT c.rule_id, max(r.name) AS rule_name, count(*) AS "n!" FROM cases c
               LEFT JOIN rules r ON r.id = c.rule_id
               WHERE c.guild_id = $1 AND c.created_at >= $2 AND c.created_at < $3
               GROUP BY c.rule_id ORDER BY 3 DESC"#,
            g,
            from,
            to
        )
        .fetch_all(&mut *tx),
    )
    .await?;
    for r in rows {
        s.cases_by_rule
            .push((r.rule_id.map(RuleId::from_uuid), r.rule_name, count(r.n)?));
    }

    let rows = obs::run(
        Select,
        "cases",
        sqlx::query!(
            r#"SELECT (voided_at IS NOT NULL) AS "voided!", source, count(*) AS "n!" FROM cases
               WHERE guild_id = $1 AND created_at >= $2 AND created_at < $3 GROUP BY 1, 2"#,
            g,
            from,
            to
        )
        .fetch_all(&mut *tx),
    )
    .await?;
    for r in rows {
        let n = count(r.n)?;
        add(&mut s.cases_by_voided, r.voided, n);
        add(&mut s.cases_by_source, r.source.0, n);
    }

    other_buckets(&mut tx, g, from, to, &mut s).await?;
    tx.commit().await?;
    Ok(s)
}

/// Action, appeal, and moderation log buckets.
async fn other_buckets(
    tx: &mut crate::Tx,
    g: i64,
    from: OffsetDateTime,
    to: OffsetDateTime,
    s: &mut Statistics,
) -> Result<(), DbError> {
    let rows = obs::run(
        Select,
        "actions",
        sqlx::query!(
            r#"SELECT kind, state, count(*) AS "n!" FROM actions
               WHERE guild_id = $1 AND created_at >= $2 AND created_at < $3 GROUP BY 1, 2"#,
            g,
            from,
            to
        )
        .fetch_all(&mut **tx),
    )
    .await?;
    for r in rows {
        let n = count(r.n)?;
        add(&mut s.actions_by_kind, r.kind.0, n);
        add(&mut s.actions_by_state, r.state.0, n);
    }

    let rows = obs::run(
        Select,
        "appeals",
        sqlx::query!(
            r#"SELECT state, count(*) AS "n!" FROM appeals
               WHERE guild_id = $1 AND created_at >= $2 AND created_at < $3 GROUP BY 1"#,
            g,
            from,
            to
        )
        .fetch_all(&mut **tx),
    )
    .await?;
    for r in rows {
        s.appeals_by_state.push((r.state.0, count(r.n)?));
    }

    let rows = obs::run(
        Select,
        "mod_log",
        sqlx::query!(
            r#"SELECT kind, count(*) AS "n!" FROM mod_log
               WHERE guild_id = $1 AND created_at >= $2 AND created_at < $3 GROUP BY 1"#,
            g,
            from,
            to
        )
        .fetch_all(&mut **tx),
    )
    .await?;
    for r in rows {
        s.mod_log_by_kind.push((r.kind.0, count(r.n)?));
    }

    Ok(())
}

fn add<K: PartialEq>(buckets: &mut Vec<(K, u64)>, key: K, n: u64) {
    match buckets.iter_mut().find(|(k, _)| *k == key) {
        Some((_, total)) => *total += n,
        None => buckets.push((key, n)),
    }
}
