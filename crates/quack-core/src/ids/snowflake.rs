//! Discord snowflake newtypes.

use super::IdError;

macro_rules! snowflake {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(u64);

        impl $name {
            /// Wraps a snowflake; `None` unless it is in `1..=i64::MAX`.
            pub const fn new(value: u64) -> Option<Self> {
                if value == 0 || value > i64::MAX as u64 {
                    None
                } else {
                    Some(Self(value))
                }
            }

            /// The snowflake as an unsigned integer.
            pub const fn get(self) -> u64 {
                self.0
            }

            /// The snowflake as the `bigint` stored in PostgreSQL.
            #[allow(clippy::cast_possible_wrap, reason = "new() keeps the value at or below i64::MAX")]
            pub const fn to_i64(self) -> i64 {
                self.0 as i64
            }
        }

        impl TryFrom<i64> for $name {
            type Error = IdError;

            fn try_from(value: i64) -> Result<Self, IdError> {
                u64::try_from(value).ok().and_then(Self::new).ok_or(IdError::NotASnowflake)
            }
        }

        impl TryFrom<u64> for $name {
            type Error = IdError;

            fn try_from(value: u64) -> Result<Self, IdError> {
                Self::new(value).ok_or(IdError::NotASnowflake)
            }
        }

        impl From<$name> for i64 {
            fn from(id: $name) -> i64 {
                id.to_i64()
            }
        }

        impl ::core::str::FromStr for $name {
            type Err = IdError;

            /// Parses a decimal snowflake of 1–20 digits (surrounding
            /// whitespace allowed, as the importer receives it).
            fn from_str(s: &str) -> Result<Self, IdError> {
                let s = s.trim();
                if s.is_empty() || s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(IdError::NotASnowflake);
                }
                s.parse::<u64>().ok().and_then(Self::new).ok_or(IdError::NotASnowflake)
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <String as ::serde::Deserialize>::deserialize(d)?;
                s.parse().map_err(::serde::de::Error::custom)
            }
        }
    };
}

snowflake!(
    /// A Discord server. Code and the API say guild; people read "server".
    GuildId
);
snowflake!(
    /// A Discord account not discussed as a member of a server, such as
    /// whoever is signed in to the dashboard, a moderator, or Quack itself.
    AccountId
);
snowflake!(
    /// The account a case is about, as a member of one server.
    MemberId
);
snowflake!(
    /// A Discord channel or thread.
    ChannelId
);
snowflake!(
    /// A Discord role. A server's @everyone role has the server's id.
    RoleId
);
snowflake!(
    /// A Discord message.
    MessageId
);

impl MemberId {
    /// The same Discord account, viewed as an account.
    pub const fn account(self) -> AccountId {
        AccountId(self.0)
    }
}

impl AccountId {
    /// The same Discord account, viewed as the member a case is about.
    pub const fn member(self) -> MemberId {
        MemberId(self.0)
    }
}

impl GuildId {
    /// The server's @everyone role, whose id equals the server's.
    pub const fn everyone_role(self) -> RoleId {
        RoleId(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rejects_zero_and_values_above_i64_max() {
        assert_eq!(GuildId::new(0), None);
        assert_eq!(GuildId::new(i64::MAX as u64 + 1), None);
        assert!(GuildId::new(i64::MAX as u64).is_some());
        assert_eq!(GuildId::try_from(-1_i64), Err(IdError::NotASnowflake));
        assert_eq!(GuildId::try_from(0_i64), Err(IdError::NotASnowflake));
    }

    #[test]
    fn parses_decimal_strings_only() {
        assert_eq!("  42 ".parse::<RoleId>().map(RoleId::get), Ok(42));
        for bad in [
            "",
            "abc",
            "-5",
            "+5",
            "1.0",
            "0",
            "123456789012345678901",
            "9223372036854775808",
        ] {
            assert_eq!(
                bad.parse::<RoleId>(),
                Err(IdError::NotASnowflake),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn json_is_a_string() {
        let id = MessageId::new(1_234_567_890_123_456_789).unwrap();
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"1234567890123456789\""
        );
        assert_eq!(
            serde_json::from_str::<MessageId>("\"1234567890123456789\"").unwrap(),
            id
        );
        assert!(serde_json::from_str::<MessageId>("1234567890123456789").is_err());
        assert!(serde_json::from_str::<MessageId>("\"0\"").is_err());
    }

    #[test]
    fn account_and_member_views_agree() {
        let member = MemberId::new(7).unwrap();
        assert_eq!(member.account().member(), member);
        assert_eq!(GuildId::new(9).unwrap().everyone_role().get(), 9);
    }

    proptest! {
        #[test]
        fn i64_round_trip(v in 1_i64..=i64::MAX) {
            let id = ChannelId::try_from(v).unwrap();
            prop_assert_eq!(i64::from(id), v);
            prop_assert_eq!(id.get(), v.cast_unsigned());
        }

        #[test]
        fn string_and_json_round_trip(v in 1_u64..=i64::MAX as u64) {
            let id = GuildId::new(v).unwrap();
            prop_assert_eq!(id.to_string().parse::<GuildId>(), Ok(id));
            let json = serde_json::to_string(&id).unwrap();
            prop_assert_eq!(serde_json::from_str::<GuildId>(&json).unwrap(), id);
        }
    }
}
