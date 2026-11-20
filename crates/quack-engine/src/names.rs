//! Display names for the dashboard's sentences (§12.0). Discord renders
//! people as mentions; plain text needs their names, read with one call to
//! the `Discord` port's `accounts` (which answers from Quack's account
//! cache, §11.10, before asking Discord).

use std::collections::HashMap;

use quack_core::ids::AccountId;
use quack_text::{Actor, Person};

use crate::Engine;

/// Display names by account. An account without one renders with the
/// unknown-name wording.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Names(HashMap<AccountId, String>);

impl Names {
    /// Reads the display names (global name, else username) of `ids` with
    /// one `accounts` call. If Discord can't answer, those people stay
    /// unnamed: a name never fails a read.
    pub async fn read(engine: &Engine, mut ids: Vec<AccountId>) -> Self {
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return Self::default();
        }
        let names = engine
            .discord()
            .accounts(&ids)
            .await
            .done()
            .into_iter()
            .flatten()
            .map(|a| (a.id, a.global_name.unwrap_or(a.username)))
            .collect();
        Self(names)
    }

    /// The account as a person, named when known.
    pub fn person(&self, id: AccountId) -> Person {
        self.0
            .get(&id)
            .map_or_else(|| Person::unnamed(id), |name| Person::new(id, name.clone()))
    }

    /// Who acted: a person, or Quack for `None`.
    pub fn actor(&self, id: Option<AccountId>) -> Actor {
        id.map_or(Actor::Quack, |id| Actor::Person(self.person(id)))
    }
}
