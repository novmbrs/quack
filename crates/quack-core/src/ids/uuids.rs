//! UUID newtypes for Quack's own records.

use uuid::Uuid;

use super::{IdError, from_u22, to_u22};

macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::serde::Serialize,
            ::serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Wraps a UUID (normally a fresh `Uuid::now_v7()` from the caller).
            pub const fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            /// The underlying UUID.
            pub const fn get(self) -> Uuid {
                self.0
            }

            /// The id as 22 characters of unpadded base64url, for custom ids
            /// and nonces.
            pub fn to_u22(self) -> String {
                to_u22(self.0)
            }

            /// Parses the 22-character form written by [`Self::to_u22`].
            pub fn from_u22(s: &str) -> Result<Self, IdError> {
                from_u22(s).map(Self)
            }
        }

        impl From<Uuid> for $name {
            fn from(id: Uuid) -> Self {
                Self(id)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Uuid {
                id.0
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

uuid_id!(
    /// A case.
    CaseId
);
uuid_id!(
    /// A rule (its identity; versions are numbered within it).
    RuleId
);
uuid_id!(
    /// An original or undo action.
    ActionId
);
uuid_id!(
    /// An appeal.
    AppealId
);
uuid_id!(
    /// One event on an appeal's timeline.
    AppealEventId
);
uuid_id!(
    /// One DM row.
    DmId
);
uuid_id!(
    /// One saved message (evidence item).
    EvidenceId
);
uuid_id!(
    /// One file of an evidence item.
    FileId
);
uuid_id!(
    /// A ticket.
    TicketId
);
uuid_id!(
    /// A tracked message row (`messages.id`), not a Discord message id.
    MessageRowId
);
uuid_id!(
    /// A job queue row.
    JobId
);
uuid_id!(
    /// A moderation log entry.
    ModLogId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_the_hyphenated_uuid() {
        let id = CaseId::from_uuid(Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0001));
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"01930000-0000-7000-8000-000000000001\"");
        assert_eq!(serde_json::from_str::<CaseId>(&json).unwrap(), id);
    }

    #[test]
    fn u22_round_trip() {
        let id = DmId::from_uuid(Uuid::from_u128(u128::MAX - 12345));
        let s = id.to_u22();
        assert_eq!(s.len(), 22);
        assert_eq!(DmId::from_u22(&s), Ok(id));
    }
}
