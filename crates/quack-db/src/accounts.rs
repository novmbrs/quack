//! Discord accounts that signed in to the dashboard, with their 2FA state.

use quack_core::ids::AccountId;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select};

/// The 2FA confirmation Discord last reported for an account (§2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Account {
    /// The account.
    pub id: AccountId,
    /// Whether Discord reported 2FA on.
    pub mfa_enabled: bool,
    /// When Discord reported it.
    pub mfa_checked_at: OffsetDateTime,
}

/// Records what Discord reported at sign-in (`mfa_checked_at = now()`).
pub async fn upsert(
    conn: &mut PgConnection,
    account: AccountId,
    mfa_enabled: bool,
) -> Result<(), DbError> {
    obs::run(
        Insert,
        "discord_accounts",
        sqlx::query!(
            "INSERT INTO discord_accounts (account_id, mfa_enabled, mfa_checked_at) VALUES ($1, $2, now())
             ON CONFLICT (account_id) DO UPDATE SET
               mfa_enabled = EXCLUDED.mfa_enabled, mfa_checked_at = now(), updated_at = now()",
            i64::from(account),
            mfa_enabled
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// An account's recorded 2FA state, if it ever signed in.
pub async fn get(conn: &mut PgConnection, account: AccountId) -> Result<Option<Account>, DbError> {
    let row = obs::run(
        Select,
        "discord_accounts",
        sqlx::query!(
            "SELECT mfa_enabled, mfa_checked_at FROM discord_accounts WHERE account_id = $1",
            i64::from(account)
        )
        .fetch_optional(conn),
    )
    .await?;
    Ok(row.map(|r| Account {
        id: account,
        mfa_enabled: r.mfa_enabled,
        mfa_checked_at: r.mfa_checked_at,
    }))
}
