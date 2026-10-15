//! Conversion between nu values and Stardust documents.
//!
//! The structural types map one to one: nothing, bool, int, float, string,
//! list and record. Stardust's tagged objects map to nu's native types where
//! nu has one: `{#utc text}` is a date and `{#dur text}` is a duration.
//! `{#link id}` stays a record, so it round-trips unchanged.

use chrono::{DateTime, FixedOffset, Timelike, Utc};
use nu_protocol::{LabeledError, Record, Span, Value};
use stardust::{Document, Value as Sd};

/// Converts a Stardust error into a nu error labeled at `span`.
pub fn sd_err(error: stardust::Error, span: Span) -> LabeledError {
    LabeledError::new(format!("stardust: {}", error.status.name())).with_label(error.to_string(), span)
}

pub fn to_document(value: &Value) -> Result<Document, LabeledError> {
    let span = value.span();
    stardust::from_value(&to_sd(value)?).map_err(|e| sd_err(e, span))
}

pub fn from_document(document: &Document, span: Span) -> Result<Value, LabeledError> {
    let value = document.to_value().map_err(|e| sd_err(e, span))?;
    Ok(from_sd(value, span))
}

pub fn to_sd(value: &Value) -> Result<Sd, LabeledError> {
    Ok(match value {
        Value::Nothing { .. } => Sd::Null,
        Value::Bool { val, .. } => Sd::Boolean(*val),
        Value::Int { val, .. } => Sd::I64(*val),
        Value::Float { val, .. } => Sd::F64(*val),
        Value::String { val, .. } => Sd::String(val.clone()),
        Value::Glob { val, .. } => Sd::String(val.clone()),
        Value::Filesize { val, .. } => Sd::I64(val.get()),
        Value::Date { val, .. } => tagged("#utc", utc_text(val)),
        Value::Duration { val, .. } => tagged("#dur", dur_text(*val)),
        Value::List { vals, .. } => Sd::Array(vals.iter().map(to_sd).collect::<Result<_, _>>()?),
        Value::Record { val, .. } => Sd::Object(
            val.iter()
                .map(|(k, v)| Ok((k.clone(), to_sd(v)?)))
                .collect::<Result<_, LabeledError>>()?,
        ),
        other => {
            return Err(LabeledError::new("unsupported value")
                .with_label(format!("a {} has no Stardust form", other.get_type()), other.span()));
        }
    })
}

pub fn from_sd(value: Sd, span: Span) -> Value {
    match value {
        Sd::Null => Value::nothing(span),
        Sd::Boolean(b) => Value::bool(b, span),
        Sd::I64(n) => Value::int(n, span),
        Sd::F64(f) => Value::float(f, span),
        Sd::String(s) => Value::string(s, span),
        Sd::Array(items) => Value::list(items.into_iter().map(|v| from_sd(v, span)).collect(), span),
        Sd::Object(members) => {
            if let [(key, Sd::String(text))] = members.as_slice() {
                let native = match key.as_str() {
                    "#utc" => DateTime::parse_from_rfc3339(text).ok().map(|d| Value::date(d, span)),
                    "#dur" => parse_dur(text).map(|ns| Value::duration(ns, span)),
                    _ => None,
                };
                if let Some(native) = native {
                    return native;
                }
            }
            let mut record = Record::with_capacity(members.len());
            for (key, value) in members {
                record.push(key, from_sd(value, span));
            }
            Value::record(record, span)
        }
    }
}

fn tagged(tag: &str, text: String) -> Sd {
    Sd::Object(vec![(tag.into(), Sd::String(text))])
}

/// RFC 3339 in UTC, truncated to Stardust's 100 ns tick.
fn utc_text(date: &DateTime<FixedOffset>) -> String {
    let utc = date.with_timezone(&Utc);
    let base = utc.format("%Y-%m-%dT%H:%M:%S").to_string();
    format!("{base}{}Z", fraction(utc.nanosecond() as u64 % 1_000_000_000))
}

/// ISO 8601 duration in seconds, truncated to the 100 ns tick.
fn dur_text(nanos: i64) -> String {
    let sign = if nanos < 0 { "-" } else { "" };
    let abs = nanos.unsigned_abs();
    format!("{sign}PT{}{}S", abs / 1_000_000_000, fraction(abs % 1_000_000_000))
}

fn fraction(nanos: u64) -> String {
    let ticks = nanos / 100;
    if ticks == 0 {
        return String::new();
    }
    format!(".{ticks:07}").trim_end_matches('0').to_string()
}

/// Parses the day-and-time subset of ISO 8601 durations: `[-]P[nD][T[nH][nM][n.nS]]`.
/// Years, months and weeks have no fixed length, so those stay records.
fn parse_dur(text: &str) -> Option<i64> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let rest = rest.strip_prefix('P')?;
    let (date, time) = match rest.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (rest, None),
    };
    let mut total: i128 = 0;
    if !date.is_empty() {
        let days = date.strip_suffix('D')?;
        total += days.parse::<i128>().ok()? * 86_400 * 1_000_000_000;
    }
    if let Some(mut time) = time {
        if time.is_empty() {
            return None;
        }
        for (unit, scale) in [('H', 3_600_i128), ('M', 60), ('S', 1)] {
            if let Some(at) = time.find(unit) {
                total += decimal_nanos(&time[..at])? * scale;
                time = &time[at + 1..];
            }
        }
        if !time.is_empty() {
            return None;
        }
    }
    let total = if negative { -total } else { total };
    i64::try_from(total).ok()
}

/// `12.5` -> 12_500_000_000 nanoseconds.
fn decimal_nanos(text: &str) -> Option<i128> {
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) || frac.len() > 9 {
        return None;
    }
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let frac_nanos: i128 = if frac.is_empty() { 0 } else { format!("{frac:0<9}").parse().ok()? };
    Some(whole.parse::<i128>().ok()? * 1_000_000_000 + frac_nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_round_trip() {
        for ns in [0, 1_500_000_000, -2_000_000_000, 86_400_000_000_000 + 100, 250_000_000] {
            assert_eq!(parse_dur(&dur_text(ns)), Some(ns), "{}", dur_text(ns));
        }
        assert_eq!(dur_text(250_000_000), "PT0.25S");
        assert_eq!(parse_dur("P30D"), Some(30 * 86_400 * 1_000_000_000));
        assert_eq!(parse_dur("PT1M"), Some(60_000_000_000));
        assert_eq!(parse_dur("P1Y"), None);
        assert_eq!(parse_dur("PT"), None);
    }

    #[test]
    fn utc_truncates_to_ticks() {
        let d = DateTime::parse_from_rfc3339("2026-10-07T14:34:56.123456789+02:00").unwrap();
        assert_eq!(utc_text(&d), "2026-10-07T12:34:56.1234567Z");
        let d = DateTime::parse_from_rfc3339("2026-10-07T12:34:56Z").unwrap();
        assert_eq!(utc_text(&d), "2026-10-07T12:34:56Z");
    }
}
