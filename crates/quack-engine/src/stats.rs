//! Server statistics (§14.3 route 8): counts by bucket over `[from, to)`,
//! computed in one read-only snapshot. No per-moderator ranking.

use quack_core::DomainError;
use quack_core::limits::{MAX_STATS_RANGE, MIN_STATS_RANGE};
use quack_core::permissions::Capability;
use quack_db::DbError;
use quack_db::stats::{self, Statistics};
use time::OffsetDateTime;

use crate::{Engine, Principal, obs};

/// Why statistics could not be computed.
#[derive(Debug, thiserror::Error)]
pub enum StatsError {
    /// A refusal with a `quack-text` message (capability, 2FA).
    #[error("refused: {0:?}")]
    Refused(DomainError),
    /// The range is not 1 second to 366 days (`validation_failed`).
    #[error("range invalid")]
    RangeInvalid,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<DomainError> for StatsError {
    fn from(error: DomainError) -> Self {
        Self::Refused(error)
    }
}

/// The range a request asks for: `to` defaults to now and `from` to one
/// calendar month before `to`.
pub fn range(
    from: Option<OffsetDateTime>,
    to: Option<OffsetDateTime>,
) -> Result<(OffsetDateTime, OffsetDateTime), StatsError> {
    let to = to.unwrap_or_else(OffsetDateTime::now_utc);
    let from = from.unwrap_or_else(|| month_before(to));
    let span = to - from;
    if span < MIN_STATS_RANGE || span > MAX_STATS_RANGE {
        return Err(StatsError::RangeInvalid);
    }
    Ok((from, to))
}

fn month_before(at: OffsetDateTime) -> OffsetDateTime {
    let (year, month) = match at.month() {
        time::Month::January => (at.year() - 1, time::Month::December),
        m => (at.year(), m.previous()),
    };
    let day = at.day().min(time::util::days_in_month(month, year));
    time::Date::from_calendar_date(year, month, day)
        .map_or(at - time::Duration::days(30), |d| at.replace_date(d))
}

/// The server's statistics for `[from, to)`, for staff with `moderate`.
pub async fn compute(
    engine: &Engine,
    principal: &Principal,
    from: Option<OffsetDateTime>,
    to: Option<OffsetDateTime>,
) -> Result<Statistics, StatsError> {
    obs::use_case("statistics", Some(principal.guild), async {
        {
            let mut conn = engine.db().conn().await?;
            principal.require(&mut conn, Capability::Moderate).await??;
        }
        let (from, to) = range(from, to)?;
        Ok(stats::compute(engine.db(), principal.guild, from, to).await?)
    })
    .await
}
