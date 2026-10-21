//! Bounded validation and identity-only normalization. Wire payloads never change.
use crate::{Cell, Error, PgType, Result, valid_decimal};

/// PG numeric limits: 131072 integer digits and 16383 fractional digits. An input
/// cap also bounds leading zeros/exponent spelling work; no expanded decimal is built.
const MAX_NUMERIC_INPUT_BYTES: usize = 160_000;
const MAX_NUMERIC_EXPONENT: i64 = 147_455;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum IdentityValue {
    Text(String),
    Bool(bool),
    Integer(i64),
    Numeric {
        negative: bool,
        coefficient: String,
        exponent: i64,
    },
    Binary(Vec<u8>),
    Uuid([u8; 16]),
    Date(String),
}

pub(crate) fn identity_value(value: &Cell, pg_type: PgType) -> Result<IdentityValue> {
    match pg_type {
        PgType::Text => match value {
            Cell::Text(value) => Ok(IdentityValue::Text(value.clone())),
            _ => invalid(),
        },
        PgType::Boolean => Ok(IdentityValue::Bool(boolean(value)?)),
        PgType::SmallInt | PgType::Integer | PgType::BigInt => {
            Ok(IdentityValue::Integer(integer(value, pg_type)?))
        }
        PgType::Numeric => numeric(&numeric_text(value)?),
        PgType::Uuid => match value {
            Cell::Text(value) => Ok(IdentityValue::Uuid(uuid(value)?)),
            _ => invalid(),
        },
        PgType::Bytea => match value {
            Cell::Binary(value) => Ok(IdentityValue::Binary(value.clone())),
            _ => invalid(),
        },
        PgType::Date => match value {
            Cell::Text(value) => {
                date(value)?;
                Ok(IdentityValue::Date(value.clone()))
            }
            _ => invalid(),
        },
        _ => Err(Error::Invalid(
            "read-only: unsupported identity equivalence".into(),
        )),
    }
}

pub(crate) fn validate_scalar(value: &Cell, pg_type: PgType) -> Result<()> {
    if *value == Cell::Null {
        return Ok(());
    }
    match pg_type {
        PgType::Text | PgType::Bytea => Ok(()),
        PgType::Boolean => boolean(value).map(|_| ()),
        PgType::SmallInt | PgType::Integer | PgType::BigInt => integer(value, pg_type).map(|_| ()),
        PgType::Numeric => numeric(&numeric_text(value)?).map(|_| ()),
        PgType::Uuid => match value {
            Cell::Text(value) => uuid(value).map(|_| ()),
            _ => invalid(),
        },
        PgType::Date => match value {
            Cell::Text(value) => date(value),
            _ => invalid(),
        },
        PgType::Timestamp | PgType::TimestampTz => match value {
            Cell::Text(value) => timestamp(value, pg_type == PgType::TimestampTz),
            _ => invalid(),
        },
        PgType::Json | PgType::Jsonb => match value {
            Cell::Text(value) => {
                let _: serde_json::Value = serde_json::from_str(value)?;
                Ok(())
            }
            _ => invalid(),
        },
        PgType::DoublePrecision => match value {
            Cell::Text(value) if value.len() <= 128 => {
                if matches!(value.as_str(), "NaN" | "Infinity" | "-Infinity") {
                    return Ok(());
                }
                let parsed: f64 = value
                    .parse()
                    .map_err(|_| Error::Invalid("invalid double precision text".into()))?;
                if parsed.is_finite() {
                    Ok(())
                } else {
                    Err(Error::Invalid(
                        "double precision overflow; use explicit Infinity if intended".into(),
                    ))
                }
            }
            _ => invalid(),
        },
    }
}
fn invalid<T>() -> Result<T> {
    Err(Error::Invalid("invalid PostgreSQL scalar value".into()))
}
fn boolean(value: &Cell) -> Result<bool> {
    match value {
        Cell::Bool(value) => Ok(*value),
        Cell::Text(value)
            if value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("t") =>
        {
            Ok(true)
        }
        Cell::Text(value)
            if value.eq_ignore_ascii_case("false") || value.eq_ignore_ascii_case("f") =>
        {
            Ok(false)
        }
        _ => invalid(),
    }
}
fn integer(value: &Cell, pg_type: PgType) -> Result<i64> {
    let integer = match value {
        Cell::Integer(value) => *value,
        Cell::Text(value) if value.len() <= 21 => value
            .parse()
            .map_err(|_| Error::Invalid("invalid integer text".into()))?,
        _ => return invalid(),
    };
    let valid = match pg_type {
        PgType::SmallInt => i16::try_from(integer).is_ok(),
        PgType::Integer => i32::try_from(integer).is_ok(),
        PgType::BigInt => true,
        _ => false,
    };
    if valid {
        Ok(integer)
    } else {
        Err(Error::Invalid("integer out of destination range".into()))
    }
}
fn numeric_text(value: &Cell) -> Result<std::borrow::Cow<'_, str>> {
    match value {
        Cell::Text(value) | Cell::Decimal(value) => Ok(std::borrow::Cow::Borrowed(value)),
        Cell::Integer(value) => Ok(std::borrow::Cow::Owned(value.to_string())),
        _ => invalid(),
    }
}
fn numeric(value: &str) -> Result<IdentityValue> {
    if value.len() > MAX_NUMERIC_INPUT_BYTES {
        return Err(Error::Limit("numeric lexeme exceeds 160000 bytes".into()));
    }
    let unsigned_plus = value.strip_prefix('+').unwrap_or(value);
    if value.starts_with("+-") || !valid_decimal(unsigned_plus) {
        return Err(Error::Invalid("invalid exact PostgreSQL numeric".into()));
    }
    let negative = unsigned_plus.starts_with('-');
    let unsigned = unsigned_plus.strip_prefix('-').unwrap_or(unsigned_plus);
    let (mantissa, exponent_text) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
    let exponent = exponent_text
        .parse::<i64>()
        .map_err(|_| Error::Invalid("numeric exponent out of bounded range".into()))?;
    if !(-MAX_NUMERIC_EXPONENT..=MAX_NUMERIC_EXPONENT).contains(&exponent) {
        return Err(Error::Invalid(
            "numeric exponent out of bounded range".into(),
        ));
    }
    let fractional = mantissa
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len()) as i64;
    let scale = fractional
        .checked_sub(exponent)
        .ok_or_else(|| Error::Invalid("numeric scale overflow".into()))?;
    if scale > 16_383 {
        return Err(Error::Invalid(
            "numeric exceeds PostgreSQL fractional digit limit".into(),
        ));
    }
    let digits: String = mantissa
        .chars()
        .filter(|&character| character != '.')
        .collect();
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Ok(IdentityValue::Numeric {
            negative: false,
            coefficient: "0".into(),
            exponent: 0,
        });
    }
    let integer_digits = (significant.len() as i64)
        .checked_sub(fractional)
        .and_then(|v| v.checked_add(exponent))
        .ok_or_else(|| Error::Invalid("numeric precision overflow".into()))?;
    if integer_digits > 131_072 {
        return Err(Error::Invalid(
            "numeric exceeds PostgreSQL integer digit limit".into(),
        ));
    }
    let coefficient = significant.trim_end_matches('0');
    let trailing = significant.len() - coefficient.len();
    let exponent = exponent
        .checked_sub(fractional)
        .and_then(|v| v.checked_add(trailing as i64))
        .ok_or_else(|| Error::Invalid("numeric exponent overflow".into()))?;
    Ok(IdentityValue::Numeric {
        negative,
        coefficient: coefficient.into(),
        exponent,
    })
}
fn uuid(value: &str) -> Result<[u8; 16]> {
    if !matches!(value.len(), 32 | 36) || !value.is_ascii() {
        return invalid();
    }
    let mut bytes = [0_u8; 16];
    let mut nibble = 0;
    for (index, character) in value.bytes().enumerate() {
        if value.len() == 36 && matches!(index, 8 | 13 | 18 | 23) {
            if character != b'-' {
                return invalid();
            }
            continue;
        }
        let digit = match character {
            b'0'..=b'9' => character - b'0',
            b'a'..=b'f' => character - b'a' + 10,
            b'A'..=b'F' => character - b'A' + 10,
            _ => return invalid(),
        };
        if nibble >= 32 {
            return invalid();
        }
        bytes[nibble / 2] = (bytes[nibble / 2] << 4) | digit;
        nibble += 1;
    }
    if nibble == 32 { Ok(bytes) } else { invalid() }
}
fn digits(bytes: &[u8]) -> Result<u32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return invalid();
    }
    Ok(bytes
        .iter()
        .fold(0, |number, byte| number * 10 + u32::from(byte - b'0')))
}
fn date(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return invalid();
    }
    let year = digits(&bytes[..4])?;
    let month = digits(&bytes[5..7])?;
    let day = digits(&bytes[8..])?;
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return invalid(),
    };
    if year > 0 && day > 0 && day <= days {
        Ok(())
    } else {
        invalid()
    }
}
fn timestamp(value: &str, with_zone: bool) -> Result<()> {
    let bytes = value.as_bytes();
    if !value.is_ascii() || !(19..=35).contains(&bytes.len()) {
        return invalid();
    }
    date(&value[..10])?;
    if !matches!(bytes[10], b'T' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
        || digits(&bytes[11..13])? > 23
        || digits(&bytes[14..16])? > 59
        || digits(&bytes[17..19])? > 59
    {
        return invalid();
    }
    let mut index = 19;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if !(1..=6).contains(&(index - start)) {
            return invalid();
        }
    }
    if !with_zone {
        return if index == bytes.len() {
            Ok(())
        } else {
            invalid()
        };
    }
    let zone = &bytes[index..];
    if zone == b"Z" || zone == b"z" {
        return Ok(());
    }
    if !matches!(zone.len(), 3 | 6) || !matches!(zone[0], b'+' | b'-') {
        return invalid();
    }
    if digits(&zone[1..3])? > 15 {
        return invalid();
    }
    if zone.len() == 6 && (zone[3] != b':' || digits(&zone[4..6])? > 59) {
        return invalid();
    }
    Ok(())
}
