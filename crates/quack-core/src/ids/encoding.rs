//! Text forms of UUIDs: `u22` (unpadded base64url) and legacy ULIDs.

use uuid::Uuid;

use super::IdError;

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Encodes a UUID as 22 characters of unpadded base64url (§11.5).
///
/// Used in custom ids and as Discord message nonces.
pub fn to_u22(id: Uuid) -> String {
    let n = id.as_u128();
    // 22 digits of 6 bits cover 132 bits; the value sits in the top 128.
    (0..22_u32)
        .map(|i| {
            let digit = match 122_u32.checked_sub(6 * i) {
                Some(shift) => n >> shift,
                None => n << 4, // the last digit: 2 data bits, 4 zero bits
            };
            char::from(BASE64URL[(digit & 0x3f) as usize])
        })
        .collect()
}

/// Decodes the form written by [`to_u22`]. Only the canonical spelling is
/// accepted: exactly 22 characters, with the 4 unused low bits zero.
pub fn from_u22(s: &str) -> Result<Uuid, IdError> {
    let bytes = s.as_bytes();
    if bytes.len() != 22 {
        return Err(IdError::NotU22);
    }
    let mut acc: u128 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let digit = u128::from(base64url_value(b).ok_or(IdError::NotU22)?);
        if i < 21 {
            acc = (acc << 6) | digit;
        } else {
            if digit & 0x0f != 0 {
                return Err(IdError::NotU22);
            }
            acc = (acc << 2) | (digit >> 4);
        }
    }
    Ok(Uuid::from_u128(acc))
}

fn base64url_value(b: u8) -> Option<u8> {
    BASE64URL
        .iter()
        .position(|&c| c == b)
        .and_then(|p| u8::try_from(p).ok())
}

/// Maps a legacy ULID (26 characters of Crockford base32, case-insensitive)
/// to the `UUIDv7` the importer gave its row (§19.3).
///
/// The UUID is `T (48 bits) ‖ 0111 ‖ R[0..12) ‖ 10 ‖ R[12..74)`, where `T` is
/// the ULID's timestamp and `R` its 80 random bits; the last 6 random bits
/// are dropped. Deterministic and order-preserving.
pub fn ulid_to_uuid(ulid: &str) -> Result<Uuid, IdError> {
    let bytes = ulid.as_bytes();
    if bytes.len() != 26 {
        return Err(IdError::NotUlid);
    }
    let mut value: u128 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let digit = crockford_value(b).ok_or(IdError::NotUlid)?;
        // The first character carries only 3 bits (26 × 5 = 130 > 128).
        if i == 0 && digit > 7 {
            return Err(IdError::NotUlid);
        }
        value = (value << 5) | u128::from(digit);
    }
    let time = value >> 80;
    let random = value & ((1u128 << 80) - 1);
    let rand_a = random >> 68; // R[0..12)
    let rand_b = (random >> 6) & ((1u128 << 62) - 1); // R[12..74)
    Ok(Uuid::from_u128(
        (time << 80) | (0x7 << 76) | (rand_a << 64) | (0b10 << 62) | rand_b,
    ))
}

fn crockford_value(b: u8) -> Option<u8> {
    let upper = b.to_ascii_uppercase();
    CROCKFORD
        .iter()
        .position(|&c| c == upper)
        .and_then(|p| u8::try_from(p).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn u22_known_values() {
        assert_eq!(to_u22(Uuid::nil()), "AAAAAAAAAAAAAAAAAAAAAA");
        assert_eq!(to_u22(Uuid::max()), "_____________________w");
        assert_eq!(from_u22("_____________________w"), Ok(Uuid::max()));
    }

    #[test]
    fn u22_rejects_non_canonical_and_bad_input() {
        for bad in [
            "",
            "AAAAAAAAAAAAAAAAAAAAA",
            "AAAAAAAAAAAAAAAAAAAAAAA",
            "AAAAAAAAAAAAAAAAAAAAA=",
            "AAAAAAAAAAAAAAAAAAAAA+",
            "AAAAAAAAAAAAAAAAAAAAAB",
        ] {
            assert_eq!(from_u22(bad), Err(IdError::NotU22), "{bad:?}");
        }
    }

    #[test]
    fn ulid_maps_to_a_version_7_variant_rfc_uuid() {
        // The ULID spec's example: ulid(1469918176385) = 01ARYZ6S41….
        let uuid = ulid_to_uuid("01ARYZ6S41TSV4RRFFQ69G5FAV").unwrap();
        assert_eq!(uuid.get_version_num(), 7);
        assert_eq!(uuid.get_variant(), uuid::Variant::RFC4122);
        // The 48-bit timestamp survives unchanged.
        assert_eq!(uuid.as_u128() >> 80, 1_469_918_176_385);
        assert_eq!(ulid_to_uuid("01aryz6s41tsv4rrffq69g5fav"), Ok(uuid));
    }

    #[test]
    fn ulid_rejects_bad_input() {
        for bad in [
            "",
            "01ARZ3NDEKTSV4RRFFQ69G5FA",
            "81ARZ3NDEKTSV4RRFFQ69G5FAV",
            "01ARZ3NDEKTSV4RRFFQ69G5FAU",
        ] {
            assert_eq!(ulid_to_uuid(bad), Err(IdError::NotUlid), "{bad:?}");
        }
    }

    fn ulid_string(value: u128) -> String {
        (0..26)
            .map(|i| char::from(CROCKFORD[((value >> (125 - 5 * i)) & 0x1f) as usize]))
            .collect()
    }

    proptest! {
        #[test]
        fn u22_round_trip(n in any::<u128>()) {
            let id = Uuid::from_u128(n);
            let s = to_u22(id);
            prop_assert_eq!(s.len(), 22);
            prop_assert_eq!(from_u22(&s), Ok(id));
        }

        #[test]
        fn ulid_mapping_preserves_order(a in any::<u128>(), b in any::<u128>()) {
            let (ua, ub) = (ulid_to_uuid(&ulid_string(a)).unwrap(), ulid_to_uuid(&ulid_string(b)).unwrap());
            // Order is preserved on the bits that survive (all but the last 6).
            prop_assert_eq!((a >> 6).cmp(&(b >> 6)), ua.cmp(&ub));
        }
    }
}
