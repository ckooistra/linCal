//! DATE, DATE-TIME and DURATION values, and TZID resolution.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use chrono_tz::Tz;

use crate::IcalError;
use crate::content::{Component, Property};

/// A DATE or DATE-TIME value as written, before zone resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IcalTime {
    Date(NaiveDate),
    Utc(DateTime<Utc>),
    /// Local time with an optional TZID (none = floating).
    Local(NaiveDateTime, Option<String>),
}

fn bad(what: &str, value: &str) -> IcalError {
    IcalError::Value(format!("{what}: {value}"))
}

fn parse_date(s: &str) -> Result<NaiveDate, IcalError> {
    NaiveDate::parse_from_str(s, "%Y%m%d").map_err(|_| bad("date", s))
}

fn parse_date_time(s: &str) -> Result<NaiveDateTime, IcalError> {
    NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%S").map_err(|_| bad("date-time", s))
}

/// Parses a (possibly comma-separated) DATE / DATE-TIME property value.
/// PERIOD values (RDATE only) are reduced to their start.
pub fn parse_times(p: &Property) -> Result<Vec<IcalTime>, IcalError> {
    let is_date = p
        .param("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE"));
    let tzid = p.param("TZID").map(str::to_owned);
    p.value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|raw| {
            let s = raw.split('/').next().unwrap_or(raw);
            if is_date || (s.len() == 8 && !s.contains('T')) {
                Ok(IcalTime::Date(parse_date(s)?))
            } else if let Some(utc) = s.strip_suffix('Z') {
                Ok(IcalTime::Utc(parse_date_time(utc)?.and_utc()))
            } else {
                Ok(IcalTime::Local(parse_date_time(s)?, tzid.clone()))
            }
        })
        .collect()
}

pub fn parse_time(p: &Property) -> Result<IcalTime, IcalError> {
    parse_times(p)?
        .into_iter()
        .next()
        .ok_or_else(|| bad("empty time value", &p.name))
}

/// A DURATION split into nominal days (calendar days, DST-aware) and
/// exact seconds, as RFC 5545 §3.3.6 distinguishes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IcalDuration {
    pub days: i64,
    pub seconds: i64,
}

/// Longest accepted DURATION. Values come from servers and other clients;
/// anything this large is malformed, and bounding it keeps every later
/// date calculation in range.
const MAX_DURATION_DAYS: i64 = 100 * 366;

pub fn parse_duration(s: &str) -> Result<IcalDuration, IcalError> {
    let err = || bad("duration", s);
    // digits × unit, refusing overflow.
    let scaled = |digits: &str, unit: i64| -> Result<i64, IcalError> {
        digits
            .parse::<i64>()
            .ok()
            .and_then(|n| n.checked_mul(unit))
            .ok_or_else(err)
    };
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'-') => (-1, &s[1..]),
        Some(b'+') => (1, &s[1..]),
        _ => (1, s),
    };
    let rest = rest.strip_prefix('P').ok_or_else(err)?;
    let (date_part, time_part) = match rest.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (rest, None),
    };
    let mut days = 0i64;
    let mut seconds = 0i64;
    let mut digits = String::new();
    for c in date_part.chars() {
        let unit = match c {
            '0'..='9' => {
                digits.push(c);
                continue;
            }
            'W' => 7,
            'D' => 1,
            _ => return Err(err()),
        };
        days = days.checked_add(scaled(&digits, unit)?).ok_or_else(err)?;
        digits.clear();
    }
    if !digits.is_empty() {
        return Err(err());
    }
    if let Some(t) = time_part {
        for c in t.chars() {
            let unit = match c {
                '0'..='9' => {
                    digits.push(c);
                    continue;
                }
                'H' => 3600,
                'M' => 60,
                'S' => 1,
                _ => return Err(err()),
            };
            seconds = seconds
                .checked_add(scaled(&digits, unit)?)
                .ok_or_else(err)?;
            digits.clear();
        }
        if !digits.is_empty() {
            return Err(err());
        }
    }
    if days > MAX_DURATION_DAYS || seconds / 86_400 > MAX_DURATION_DAYS {
        return Err(err());
    }
    Ok(IcalDuration {
        days: sign * days,
        seconds: sign * seconds,
    })
}

impl IcalDuration {
    pub fn exact(self) -> Duration {
        Duration::days(self.days) + Duration::seconds(self.seconds)
    }
}

/// Common Windows zone names (as used by Outlook/Exchange) → IANA.
const WINDOWS_ZONES: &[(&str, &str)] = &[
    ("UTC", "UTC"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("FLE Standard Time", "Europe/Kiev"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("Eastern Standard Time", "America/New_York"),
    ("Central Standard Time", "America/Chicago"),
    ("Mountain Standard Time", "America/Denver"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("India Standard Time", "Asia/Kolkata"),
    ("China Standard Time", "Asia/Shanghai"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("Korea Standard Time", "Asia/Seoul"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
];

/// The Windows name of an IANA zone, for servers that prefer those
/// (Exchange/Outlook), when it is one of the common zones known here.
pub fn windows_zone_name(tz: Tz) -> Option<&'static str> {
    WINDOWS_ZONES
        .iter()
        .find(|(_, iana)| *iana == tz.name())
        .map(|(windows, _)| *windows)
}

/// Maps a TZID to an IANA zone: IANA names directly, Mozilla-style path
/// prefixes (`/mozilla.org/.../Europe/Berlin`), common Windows names, and
/// the `X-LIC-LOCATION` of a matching VTIMEZONE.
pub fn resolve_tzid(tzid: &str, calendar: &Component) -> Option<Tz> {
    let tzid = tzid.trim();
    if let Ok(tz) = tzid.parse::<Tz>() {
        return Some(tz);
    }
    let segments: Vec<&str> = tzid.split('/').filter(|s| !s.is_empty()).collect();
    for n in [3, 2] {
        if segments.len() >= n
            && let Ok(tz) = segments[segments.len() - n..].join("/").parse::<Tz>()
        {
            return Some(tz);
        }
    }
    if let Some((_, iana)) = WINDOWS_ZONES
        .iter()
        .find(|(w, _)| w.eq_ignore_ascii_case(tzid))
    {
        return iana.parse().ok();
    }
    calendar
        .children("VTIMEZONE")
        .find(|vt| vt.property("TZID").is_some_and(|p| p.value.trim() == tzid))
        .and_then(|vt| vt.property("X-LIC-LOCATION"))
        .and_then(|p| p.value.trim().parse().ok())
}

pub fn format_date(d: NaiveDate) -> String {
    d.format("%Y%m%d").to_string()
}

pub fn format_local(t: NaiveDateTime) -> String {
    t.format("%Y%m%dT%H%M%S").to_string()
}

pub fn format_utc(t: DateTime<Utc>) -> String {
    t.format("%Y%m%dT%H%M%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_time_values() {
        let p = Property::new("DTSTART", "20261029T093000").with_param("TZID", "Europe/Berlin");
        assert!(
            matches!(parse_time(&p).unwrap(), IcalTime::Local(_, Some(z)) if z == "Europe/Berlin")
        );
        let p = Property::new("DTSTART", "20261029T083000Z");
        assert!(matches!(parse_time(&p).unwrap(), IcalTime::Utc(_)));
        let p = Property::new("DTSTART", "20261029").with_param("VALUE", "DATE");
        assert!(matches!(parse_time(&p).unwrap(), IcalTime::Date(_)));
        let p = Property::new("EXDATE", "20261029T093000Z,20261030T093000Z");
        assert_eq!(parse_times(&p).unwrap().len(), 2);
        assert!(parse_time(&Property::new("DTSTART", "yesterday")).is_err());
    }

    #[test]
    fn parses_durations() {
        assert_eq!(
            parse_duration("PT1H30M").unwrap(),
            IcalDuration {
                days: 0,
                seconds: 5400
            }
        );
        assert_eq!(
            parse_duration("P1D").unwrap(),
            IcalDuration {
                days: 1,
                seconds: 0
            }
        );
        assert_eq!(
            parse_duration("P2W").unwrap(),
            IcalDuration {
                days: 14,
                seconds: 0
            }
        );
        assert_eq!(
            parse_duration("-PT15M").unwrap(),
            IcalDuration {
                days: 0,
                seconds: -900
            }
        );
        assert_eq!(
            parse_duration("P1DT12H").unwrap(),
            IcalDuration {
                days: 1,
                seconds: 43_200
            }
        );
        for bad in [
            "",
            "1H",
            "PT1",
            "P1X",
            "PTH",
            "P999999999999D",
            "P9223372036854775807W",
            "PT99999999999999999999S",
            "P36601D",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn resolves_time_zone_ids() {
        let empty = Component::new("VCALENDAR");
        assert_eq!(
            resolve_tzid("Europe/Berlin", &empty),
            Some(chrono_tz::Europe::Berlin)
        );
        assert_eq!(
            resolve_tzid("/mozilla.org/20070129_1/Europe/Berlin", &empty),
            Some(chrono_tz::Europe::Berlin)
        );
        assert_eq!(
            resolve_tzid("W. Europe Standard Time", &empty),
            Some(chrono_tz::Europe::Berlin)
        );
        assert_eq!(resolve_tzid("Custom Zone", &empty), None);
        let mut cal = Component::new("VCALENDAR");
        let mut vt = Component::new("VTIMEZONE");
        vt.properties.push(Property::new("TZID", "Custom Zone"));
        vt.properties
            .push(Property::new("X-LIC-LOCATION", "America/Chicago"));
        cal.components.push(vt);
        assert_eq!(
            resolve_tzid("Custom Zone", &cal),
            Some(chrono_tz::America::Chicago)
        );
    }
}
