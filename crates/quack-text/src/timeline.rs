//! The member timeline (§4.11): the case's history in member-facing words.
//! Action, DM, and appeal items reuse [`crate::action::member_sentence`],
//! [`crate::dm::member_sentence`], and [`crate::appeal::event`].

use crate::model::Text;

/// The case was added.
pub fn case_added(number: u64) -> Text {
    Text::fixed(format!("Case #{number} was added."))
}

/// The case was voided; its reason follows as a quote.
pub fn case_voided(reason: &str) -> Text {
    Text::fixed("The case was voided, so it no longer counts:\n> ").user(reason)
}
