//! Outlook recurrence (`patternedRecurrence`) ⇄ RFC 5545 RRULE.
//!
//! Outlook can express fewer rules than RRULE: daily, weekly on given
//! weekdays, monthly/yearly on a day of the month or on the first–fourth
//! or last given weekday, ending never, on a date or after a count. [`check`]
//! refuses everything else before it is stored, so writes never have to
//! approximate. Rules read from Outlook are normalised so that the editor's
//! presets (`FREQ=WEEKLY`, `FREQ=MONTHLY`, …) round-trip unchanged.

use std::collections::BTreeMap;

use calendar_core::timing::resolve_local;
use calendar_core::{EventTiming, Recurrence};
use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;
use serde_json::{Value, json};

const DAYS: [(&str, &str, Weekday); 7] = [
    ("MO", "monday", Weekday::Mon),
    ("TU", "tuesday", Weekday::Tue),
    ("WE", "wednesday", Weekday::Wed),
    ("TH", "thursday", Weekday::Thu),
    ("FR", "friday", Weekday::Fri),
    ("SA", "saturday", Weekday::Sat),
    ("SU", "sunday", Weekday::Sun),
];

const INDEXES: [(i32, &str); 5] = [
    (1, "first"),
    (2, "second"),
    (3, "third"),
    (4, "fourth"),
    (-1, "last"),
];

fn graph_day(ical: &str) -> Option<&'static str> {
    DAYS.iter().find(|d| d.0 == ical).map(|d| d.1)
}

fn ical_day(graph: &str) -> Option<&'static str> {
    DAYS.iter()
        .find(|d| d.1.eq_ignore_ascii_case(graph))
        .map(|d| d.0)
}

fn weekday_code(w: Weekday) -> &'static str {
    DAYS.iter().find(|d| d.2 == w).map_or("MO", |d| d.0)
}

/// The zone, first date and wall-clock start of a series.
fn anchor(timing: &EventTiming) -> (Option<Tz>, NaiveDate, NaiveTime) {
    match timing {
        EventTiming::Zoned {
            start, time_zone, ..
        } => {
            let local = start.with_timezone(time_zone).naive_local();
            (Some(*time_zone), local.date(), local.time())
        }
        EventTiming::Floating { start, .. } => (None, start.date(), start.time()),
        EventTiming::AllDay { start_date, .. } => (None, *start_date, NaiveTime::MIN),
    }
}

/// The zone name to send to Graph: Windows names where known.
pub fn zone_name(tz: Tz) -> &'static str {
    calendar_ical::values::windows_zone_name(tz).unwrap_or(tz.name())
}

struct Rule {
    freq: String,
    interval: u32,
    /// (ordinal, two-letter day)
    by_day: Vec<(Option<i32>, String)>,
    by_month_day: Option<u32>,
    by_month: Option<u32>,
    by_set_pos: Option<i32>,
    count: Option<u32>,
    until: Option<String>,
    wkst: Option<String>,
}

fn unsupported(what: &str) -> String {
    format!("Outlook calendars cannot repeat {what}")
}

fn single<T: std::str::FromStr>(key: &str, value: &str) -> Result<T, String> {
    if value.contains(',') {
        return Err(unsupported(&format!("with several {key} values")));
    }
    value
        .parse()
        .map_err(|_| format!("invalid {key} in the repeat rule"))
}

fn parse(rrule: &str) -> Result<Rule, String> {
    let parts: BTreeMap<String, String> = rrule
        .split(';')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (k.trim().to_ascii_uppercase(), v.trim().to_ascii_uppercase())
        })
        .collect();
    let mut rule = Rule {
        freq: parts.get("FREQ").cloned().unwrap_or_default(),
        interval: 1,
        by_day: Vec::new(),
        by_month_day: None,
        by_month: None,
        by_set_pos: None,
        count: None,
        until: None,
        wkst: None,
    };
    for (key, value) in &parts {
        match key.as_str() {
            "FREQ" => {}
            "INTERVAL" => {
                rule.interval = single(key, value)?;
                if rule.interval == 0 {
                    return Err("invalid INTERVAL in the repeat rule".into());
                }
            }
            "BYDAY" => {
                for day in value.split(',') {
                    let split = day.len().saturating_sub(2);
                    let (ordinal, code) = day.split_at(split);
                    if graph_day(code).is_none() {
                        return Err(format!("invalid day {day} in the repeat rule"));
                    }
                    let ordinal = if ordinal.is_empty() {
                        None
                    } else {
                        Some(
                            ordinal
                                .trim_start_matches('+')
                                .parse::<i32>()
                                .map_err(|_| format!("invalid day {day} in the repeat rule"))?,
                        )
                    };
                    rule.by_day.push((ordinal, code.to_owned()));
                }
            }
            "BYMONTHDAY" => {
                let day: i32 = single(key, value)?;
                if !(1..=31).contains(&day) {
                    return Err(unsupported("on days counted from the end of the month"));
                }
                rule.by_month_day = Some(day.unsigned_abs());
            }
            "BYMONTH" => rule.by_month = Some(single(key, value)?),
            "BYSETPOS" => rule.by_set_pos = Some(single(key, value)?),
            "COUNT" => rule.count = Some(single(key, value)?),
            "UNTIL" => rule.until = Some(value.clone()),
            "WKST" => rule.wkst = Some(value.clone()),
            other => return Err(unsupported(&format!("by {other}"))),
        }
    }
    Ok(rule)
}

/// The weekdays and Outlook `index` of a relative monthly/yearly rule.
fn relative(rule: &Rule) -> Result<(Vec<&'static str>, &'static str), String> {
    let ordinals: Vec<Option<i32>> = rule.by_day.iter().map(|(o, _)| *o).collect();
    let n = match (ordinals[0], rule.by_set_pos) {
        (Some(n), None) if ordinals.iter().all(|o| *o == Some(n)) => n,
        (None, Some(n)) if ordinals.iter().all(Option::is_none) => n,
        _ => return Err(unsupported("on every given weekday of a month")),
    };
    let index = INDEXES
        .iter()
        .find(|(i, _)| *i == n)
        .map(|(_, name)| *name)
        .ok_or_else(|| unsupported("on that week of the month"))?;
    let days = rule
        .by_day
        .iter()
        .filter_map(|(_, d)| graph_day(d))
        .collect();
    Ok((days, index))
}

/// The end date of an UNTIL value, as a date in the series' zone.
fn until_date(until: &str, tz: Option<Tz>) -> Result<NaiveDate, String> {
    let bad = || format!("invalid UNTIL {until} in the repeat rule");
    if until.len() == 8 {
        return NaiveDate::parse_from_str(until, "%Y%m%d").map_err(|_| bad());
    }
    let (text, utc) = match until.strip_suffix('Z') {
        Some(t) => (t, true),
        None => (until, false),
    };
    let t = NaiveDateTime::parse_from_str(text, "%Y%m%dT%H%M%S").map_err(|_| bad())?;
    Ok(match (utc, tz) {
        (true, Some(tz)) => t.and_utc().with_timezone(&tz).date_naive(),
        _ => t.date(),
    })
}

/// The Graph `patternedRecurrence` for `rrule` starting with `timing`.
pub fn to_graph(rrule: &str, timing: &EventTiming) -> Result<Value, String> {
    let rule = parse(rrule)?;
    let (tz, start, _) = anchor(timing);
    let mut pattern = json!({ "interval": rule.interval });
    let start_day = weekday_code(start.weekday());
    let plain_days = || -> Result<Vec<&'static str>, String> {
        if rule.by_day.iter().any(|(o, _)| o.is_some()) {
            return Err(unsupported("on numbered weekdays in a week"));
        }
        let days: Vec<_> = rule
            .by_day
            .iter()
            .filter_map(|(_, d)| graph_day(d))
            .collect();
        Ok(if days.is_empty() {
            vec![graph_day(start_day).expect("known day")]
        } else {
            days
        })
    };
    let no_month_parts = rule.by_month_day.is_none() && rule.by_month.is_none();
    match rule.freq.as_str() {
        "DAILY" if rule.by_day.is_empty() && no_month_parts && rule.by_set_pos.is_none() => {
            pattern["type"] = json!("daily");
        }
        // "Every weekday" is often written as a daily rule.
        "DAILY" | "WEEKLY"
            if no_month_parts
                && rule.by_set_pos.is_none()
                && (rule.freq == "WEEKLY" || rule.interval == 1) =>
        {
            pattern["type"] = json!("weekly");
            pattern["daysOfWeek"] = json!(plain_days()?);
            let first = rule.wkst.as_deref().and_then(graph_day).unwrap_or("monday");
            pattern["firstDayOfWeek"] = json!(first);
        }
        "MONTHLY" | "YEARLY" => {
            let yearly = rule.freq == "YEARLY";
            if !yearly && rule.by_month.is_some() {
                return Err(unsupported("in only some months"));
            }
            if rule.by_day.is_empty() {
                if rule.by_set_pos.is_some() {
                    return Err(unsupported("by position without weekdays"));
                }
                let day = rule.by_month_day.unwrap_or(start.day());
                pattern["type"] = json!(if yearly {
                    "absoluteYearly"
                } else {
                    "absoluteMonthly"
                });
                pattern["dayOfMonth"] = json!(day);
            } else {
                if rule.by_month_day.is_some() {
                    return Err(unsupported("on a weekday that is also a given day"));
                }
                let (days, index) = relative(&rule)?;
                pattern["type"] = json!(if yearly {
                    "relativeYearly"
                } else {
                    "relativeMonthly"
                });
                pattern["daysOfWeek"] = json!(days);
                pattern["index"] = json!(index);
            }
            if yearly {
                pattern["month"] = json!(rule.by_month.unwrap_or(start.month()));
            }
        }
        "" => return Err("the repeat rule has no FREQ".into()),
        "DAILY" | "WEEKLY" => return Err(unsupported("with that combination of rule parts")),
        other => return Err(unsupported(&other.to_ascii_lowercase())),
    }
    let mut range = json!({ "startDate": start.format("%Y-%m-%d").to_string() });
    match (rule.count, &rule.until) {
        (Some(_), Some(_)) => return Err("a repeat rule cannot have both COUNT and UNTIL".into()),
        (Some(n), None) => {
            range["type"] = json!("numbered");
            range["numberOfOccurrences"] = json!(n);
        }
        (None, Some(until)) => {
            range["type"] = json!("endDate");
            range["endDate"] = json!(until_date(until, tz)?.format("%Y-%m-%d").to_string());
        }
        (None, None) => range["type"] = json!("noEnd"),
    }
    if let Some(tz) = tz {
        range["recurrenceTimeZone"] = json!(zone_name(tz));
    }
    Ok(json!({ "pattern": pattern, "range": range }))
}

/// Whether Outlook can hold this series (registered as the storage check
/// for Microsoft calendars).
pub fn check(timing: &EventTiming, recurrence: &Recurrence) -> Result<(), String> {
    if !recurrence.rdates.is_empty() || recurrence.rrule.trim().is_empty() {
        return Err(unsupported("on extra dates outside a rule"));
    }
    to_graph(&recurrence.rrule, timing).map(|_| ())
}

fn int(v: &Value, key: &str) -> Option<u32> {
    v.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
}

/// The RRULE of a Graph `patternedRecurrence` for a series starting with
/// `timing`, leaving out parts implied by the start.
pub fn to_rrule(recurrence: &Value, timing: &EventTiming) -> Result<String, String> {
    let pattern = &recurrence["pattern"];
    let range = &recurrence["range"];
    let (tz, start, _) = anchor(timing);
    let kind = pattern["type"]
        .as_str()
        .ok_or("recurrence without a type")?;
    let interval = int(pattern, "interval").unwrap_or(1).max(1);
    let mut days: Vec<&str> = pattern["daysOfWeek"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(ical_day)
        .collect();
    days.sort_by_key(|d| DAYS.iter().position(|x| x.0 == *d));
    days.dedup();
    let index = pattern["index"].as_str().unwrap_or("first");
    let ordinal = INDEXES
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case(index))
        .map_or(1, |(i, _)| *i);
    let mut parts: Vec<String> = Vec::new();
    let relative_days = |parts: &mut Vec<String>| {
        if days.len() == 1 {
            parts.push(format!("BYDAY={ordinal}{}", days[0]));
        } else {
            parts.push(format!("BYDAY={}", days.join(",")));
            parts.push(format!("BYSETPOS={ordinal}"));
        }
    };
    let day_of_month = int(pattern, "dayOfMonth").unwrap_or(start.day());
    let month = int(pattern, "month").unwrap_or(start.month());
    match kind {
        "daily" => parts.push("FREQ=DAILY".into()),
        "weekly" => {
            parts.push("FREQ=WEEKLY".into());
            if !(days.is_empty() || days == [weekday_code(start.weekday())]) {
                parts.push(format!("BYDAY={}", days.join(",")));
            }
            let first = pattern["firstDayOfWeek"].as_str().and_then(ical_day);
            if interval > 1
                && let Some(first) = first.filter(|f| *f != "MO")
            {
                parts.push(format!("WKST={first}"));
            }
        }
        "absoluteMonthly" => {
            parts.push("FREQ=MONTHLY".into());
            if day_of_month != start.day() {
                parts.push(format!("BYMONTHDAY={day_of_month}"));
            }
        }
        "relativeMonthly" if !days.is_empty() => {
            parts.push("FREQ=MONTHLY".into());
            relative_days(&mut parts);
        }
        "absoluteYearly" => {
            parts.push("FREQ=YEARLY".into());
            if month != start.month() || day_of_month != start.day() {
                parts.push(format!("BYMONTH={month}"));
                parts.push(format!("BYMONTHDAY={day_of_month}"));
            }
        }
        "relativeYearly" if !days.is_empty() => {
            parts.push("FREQ=YEARLY".into());
            parts.push(format!("BYMONTH={month}"));
            relative_days(&mut parts);
        }
        other => return Err(format!("unknown Outlook recurrence type {other:?}")),
    }
    if interval > 1 {
        parts.insert(1, format!("INTERVAL={interval}"));
    }
    match range["type"].as_str().unwrap_or("noEnd") {
        "numbered" => {
            let n = int(range, "numberOfOccurrences").ok_or("numbered range without a count")?;
            parts.push(format!("COUNT={n}"));
        }
        "endDate" => {
            let end = range["endDate"]
                .as_str()
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .ok_or("end-date range without a date")?;
            parts.push(format!("UNTIL={}", until_value(end, timing, tz)));
        }
        _ => {}
    }
    Ok(parts.join(";"))
}

/// UNTIL for the last day `end` of a series (inclusive).
fn until_value(end: NaiveDate, timing: &EventTiming, tz: Option<Tz>) -> String {
    let last = end.and_hms_opt(23, 59, 59).expect("valid time");
    match (timing, tz) {
        (EventTiming::AllDay { .. }, _) => end.format("%Y%m%d").to_string(),
        (_, Some(tz)) => resolve_local(&tz, last)
            .with_timezone(&Utc)
            .format("%Y%m%dT%H%M%SZ")
            .to_string(),
        _ => last.format("%Y%m%dT%H%M%S").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn berlin(y: i32, m: u32, d: u32, h: u32) -> EventTiming {
        let tz = chrono_tz::Europe::Berlin;
        let start = tz
            .with_ymd_and_hms(y, m, d, h, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        EventTiming::Zoned {
            start,
            end: start + chrono::Duration::hours(1),
            time_zone: tz,
        }
    }

    #[test]
    fn editor_presets_round_trip() {
        // Thursday 2026-06-04.
        let t = berlin(2026, 6, 4, 9);
        for rule in [
            "FREQ=DAILY",
            "FREQ=WEEKLY",
            "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
            "FREQ=MONTHLY",
            "FREQ=YEARLY",
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,TH",
            "FREQ=MONTHLY;BYDAY=-1FR",
            "FREQ=MONTHLY;BYDAY=SA,SU;BYSETPOS=1",
            "FREQ=YEARLY;BYMONTH=11;BYDAY=4TH",
            "FREQ=DAILY;COUNT=5",
            "FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=15",
        ] {
            let graph = to_graph(rule, &t).unwrap();
            assert_eq!(to_rrule(&graph, &t).unwrap(), rule, "{graph}");
        }
    }

    #[test]
    fn graph_shapes() {
        let t = berlin(2026, 6, 4, 9);
        let g = to_graph("FREQ=WEEKLY;UNTIL=20261231T225959Z", &t).unwrap();
        assert_eq!(g["pattern"]["daysOfWeek"], json!(["thursday"]));
        assert_eq!(g["pattern"]["firstDayOfWeek"], "monday");
        assert_eq!(g["range"]["type"], "endDate");
        assert_eq!(g["range"]["endDate"], "2026-12-31");
        assert_eq!(g["range"]["startDate"], "2026-06-04");
        assert_eq!(g["range"]["recurrenceTimeZone"], "W. Europe Standard Time");
        // Daily on weekdays is the same as weekly on those days.
        let g = to_graph("FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR", &t).unwrap();
        assert_eq!(g["pattern"]["type"], "weekly");
        // The end date reads back as the end of that day in the zone.
        assert_eq!(
            to_rrule(&g.clone(), &t).unwrap(),
            "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"
        );
        let mut ended = g;
        ended["range"] =
            json!({"type": "endDate", "startDate": "2026-06-04", "endDate": "2026-12-31"});
        assert_eq!(
            to_rrule(&ended, &t).unwrap(),
            "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;UNTIL=20261231T225959Z"
        );
    }

    #[test]
    fn refuses_what_outlook_cannot_hold() {
        let t = berlin(2026, 6, 4, 9);
        for rule in [
            "FREQ=HOURLY",
            "FREQ=MINUTELY",
            "FREQ=MONTHLY;BYMONTHDAY=1,15",
            "FREQ=MONTHLY;BYMONTHDAY=-1",
            "FREQ=MONTHLY;BYDAY=MO",
            "FREQ=MONTHLY;BYDAY=1MO,2TU",
            "FREQ=DAILY;BYHOUR=9,17",
            "FREQ=DAILY;INTERVAL=2;BYDAY=MO",
        ] {
            assert!(to_graph(rule, &t).is_err(), "{rule}");
        }
        let rdates = Recurrence {
            rrule: String::new(),
            rdates: vec![calendar_core::OccurrenceKey::Instant(Utc::now())],
            exdates: vec![],
        };
        assert!(check(&t, &rdates).is_err());
    }
}
