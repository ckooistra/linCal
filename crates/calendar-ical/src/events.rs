//! Mapping between VEVENTs and `calendar-core` events, in both directions.
//!
//! Reading produces provider-neutral fields; writing edits an existing
//! object in place (or builds a new one), touching only the properties this
//! application owns so everything else round-trips unchanged.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use chrono_tz::Tz;

use calendar_core::timing::resolve_local;
use calendar_core::{EventTiming, ObjectContent, OccurrenceKey, Recurrence};

use crate::IcalError;
use crate::content::{Component, Property, escape_text, unescape_text};
use crate::values::{
    IcalTime, format_date, format_local, format_utc, parse_duration, parse_time, parse_times,
    resolve_tzid,
};

/// One VEVENT in provider-neutral form.
pub use calendar_core::EventContent as EventData;

/// The events of one calendar object resource (all VEVENTs sharing a UID).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectEvents {
    pub uid: String,
    pub master: Option<EventData>,
    pub overrides: Vec<EventData>,
    /// Non-fatal problems (unknown time zones, skipped components).
    pub warnings: Vec<String>,
}

impl ObjectEvents {
    pub fn into_content(self) -> ObjectContent {
        ObjectContent {
            uid: self.uid,
            master: self.master,
            overrides: self.overrides,
        }
    }
}

/// How a time without a resolvable zone is read.
enum Resolved {
    Instant(DateTime<Utc>, Tz),
    Floating(chrono::NaiveDateTime),
    Date(NaiveDate),
}

fn resolve(time: &IcalTime, calendar: &Component, warnings: &mut Vec<String>) -> Resolved {
    match time {
        IcalTime::Date(d) => Resolved::Date(*d),
        IcalTime::Utc(t) => Resolved::Instant(*t, chrono_tz::UTC),
        IcalTime::Local(t, None) => Resolved::Floating(*t),
        IcalTime::Local(t, Some(tzid)) => match resolve_tzid(tzid, calendar) {
            Some(tz) => Resolved::Instant(resolve_local(&tz, *t), tz),
            None => {
                warnings.push(format!(
                    "unknown time zone {tzid:?}; treated as floating time"
                ));
                Resolved::Floating(*t)
            }
        },
    }
}

fn text(c: &Component, name: &str) -> String {
    c.property(name)
        .map(|p| unescape_text(&p.value))
        .unwrap_or_default()
}

fn read_timing(
    c: &Component,
    calendar: &Component,
    warnings: &mut Vec<String>,
) -> Result<EventTiming, IcalError> {
    let start_prop = c
        .property("DTSTART")
        .ok_or_else(|| IcalError::Value("VEVENT without DTSTART".into()))?;
    let start = resolve(&parse_time(start_prop)?, calendar, warnings);
    let end = match c.property("DTEND") {
        Some(p) => Some(resolve(&parse_time(p)?, calendar, warnings)),
        None => None,
    };
    let duration = match c.property("DURATION") {
        Some(p) => Some(parse_duration(p.value.trim())?),
        None => None,
    };
    Ok(match start {
        Resolved::Date(start_date) => {
            let end_date = match (end, duration) {
                (Some(Resolved::Date(d)), _) => d,
                (Some(Resolved::Instant(t, tz)), _) => t.with_timezone(&tz).date_naive(),
                (Some(Resolved::Floating(t)), _) => t.date(),
                (None, Some(d)) => start_date + Duration::days(d.days.max(1)),
                (None, None) => start_date + Duration::days(1),
            };
            EventTiming::AllDay {
                start_date,
                end_date: end_date.max(start_date + Duration::days(1)),
            }
        }
        Resolved::Instant(s, tz) => {
            let end = match (end, duration) {
                (Some(Resolved::Instant(e, _)), _) => e,
                (Some(Resolved::Floating(e)), _) => resolve_local(&tz, e),
                (Some(Resolved::Date(d)), _) => {
                    resolve_local(&tz, d.and_hms_opt(0, 0, 0).expect("valid"))
                }
                (None, Some(d)) => {
                    // Days are nominal (wall-clock) days; the rest is exact.
                    let local = s.with_timezone(&tz).naive_local() + Duration::days(d.days);
                    resolve_local(&tz, local) + Duration::seconds(d.seconds)
                }
                (None, None) => s,
            };
            EventTiming::Zoned {
                start: s,
                end: end.max(s),
                time_zone: tz,
            }
        }
        Resolved::Floating(s) => {
            let end = match (end, duration) {
                (Some(Resolved::Floating(e)), _) => e,
                (Some(Resolved::Instant(e, tz)), _) => e.with_timezone(&tz).naive_local(),
                (Some(Resolved::Date(d)), _) => d.and_hms_opt(0, 0, 0).expect("valid"),
                (None, Some(d)) => s + d.exact(),
                (None, None) => s,
            };
            EventTiming::Floating {
                start: s,
                end: end.max(s),
            }
        }
    })
}

/// Converts a time value to an occurrence key matching the series' timing.
fn key_for(
    time: &IcalTime,
    master: &EventTiming,
    calendar: &Component,
    warnings: &mut Vec<String>,
) -> OccurrenceKey {
    match (master, resolve(time, calendar, warnings)) {
        (EventTiming::AllDay { .. }, Resolved::Date(d)) => OccurrenceKey::Date(d),
        (EventTiming::AllDay { .. }, Resolved::Instant(t, tz)) => {
            OccurrenceKey::Date(t.with_timezone(&tz).date_naive())
        }
        (EventTiming::AllDay { .. }, Resolved::Floating(t)) => OccurrenceKey::Date(t.date()),
        (EventTiming::Floating { start, .. }, Resolved::Date(d)) => {
            OccurrenceKey::Floating(d.and_time(start.time()))
        }
        (EventTiming::Floating { .. }, Resolved::Floating(t)) => OccurrenceKey::Floating(t),
        (EventTiming::Floating { .. }, Resolved::Instant(t, _)) => {
            OccurrenceKey::Floating(t.naive_utc())
        }
        (
            EventTiming::Zoned {
                start, time_zone, ..
            },
            Resolved::Date(d),
        ) => {
            // A DATE exception on a timed series means that day's occurrence.
            let time = start.with_timezone(time_zone).time();
            OccurrenceKey::Instant(resolve_local(time_zone, d.and_time(time)))
        }
        (EventTiming::Zoned { .. }, Resolved::Instant(t, _)) => OccurrenceKey::Instant(t),
        (EventTiming::Zoned { time_zone, .. }, Resolved::Floating(t)) => {
            OccurrenceKey::Instant(resolve_local(time_zone, t))
        }
    }
}

fn keys(
    c: &Component,
    name: &str,
    master: &EventTiming,
    calendar: &Component,
    warnings: &mut Vec<String>,
) -> Result<Vec<OccurrenceKey>, IcalError> {
    let mut out = Vec::new();
    for p in c.properties_named(name) {
        for t in parse_times(p)? {
            out.push(key_for(&t, master, calendar, warnings));
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn is_cancelled(c: &Component) -> bool {
    c.property("STATUS")
        .is_some_and(|p| p.value.trim().eq_ignore_ascii_case("CANCELLED"))
}

/// Reads all VEVENTs of a calendar object.
///
/// Cancelled overrides become EXDATEs of the master. A cancelled one-off
/// event yields no master. Objects holding only overrides (e.g. a single
/// invitation instance) return them with `master: None`.
pub fn read_events(calendar: &Component) -> Result<ObjectEvents, IcalError> {
    let mut warnings = Vec::new();
    let vevents: Vec<&Component> = calendar.children("VEVENT").collect();
    let first = vevents
        .first()
        .ok_or_else(|| IcalError::Value("object contains no VEVENT".into()))?;
    let uid = first
        .property("UID")
        .map(|p| p.value.trim().to_owned())
        .filter(|u| !u.is_empty())
        .ok_or_else(|| IcalError::Value("VEVENT without UID".into()))?;
    if calendar.children("VTODO").next().is_some() || calendar.children("VJOURNAL").next().is_some()
    {
        warnings.push("tasks and journal entries are ignored".into());
    }

    let master_c = vevents
        .iter()
        .find(|c| c.property("RECURRENCE-ID").is_none());
    let mut master = match master_c {
        Some(c) => {
            let timing = read_timing(c, calendar, &mut warnings)?;
            let recurrence = match c.property("RRULE") {
                Some(rule) => {
                    if c.properties_named("RRULE").count() > 1 {
                        warnings.push("only the first RRULE is used".into());
                    }
                    Some(Recurrence {
                        rrule: rule.value.trim().to_owned(),
                        rdates: keys(c, "RDATE", &timing, calendar, &mut warnings)?,
                        exdates: keys(c, "EXDATE", &timing, calendar, &mut warnings)?,
                    })
                }
                None => None,
            };
            if is_cancelled(c) && recurrence.is_none() {
                None
            } else {
                Some(EventData {
                    title: text(c, "SUMMARY"),
                    description: text(c, "DESCRIPTION"),
                    location: text(c, "LOCATION"),
                    timing,
                    recurrence,
                    recurrence_id: None,
                })
            }
        }
        None => None,
    };

    let mut overrides = Vec::new();
    for c in vevents
        .iter()
        .filter(|c| c.property("RECURRENCE-ID").is_some())
    {
        let timing = read_timing(c, calendar, &mut warnings)?;
        let rid = parse_time(c.property("RECURRENCE-ID").expect("filtered"))?;
        let reference = master.as_ref().map_or(&timing, |m| &m.timing);
        let key = key_for(&rid, reference, calendar, &mut warnings);
        if is_cancelled(c) {
            if let Some(rec) = master.as_mut().and_then(|m| m.recurrence.as_mut())
                && !rec.exdates.contains(&key)
            {
                rec.exdates.push(key);
                rec.exdates.sort();
            }
            continue;
        }
        overrides.push(EventData {
            title: text(c, "SUMMARY"),
            description: text(c, "DESCRIPTION"),
            location: text(c, "LOCATION"),
            timing,
            recurrence: None,
            recurrence_id: Some(key),
        });
    }
    Ok(ObjectEvents {
        uid,
        master,
        overrides,
        warnings,
    })
}

// --- writing -----------------------------------------------------------------

fn time_property(name: &str, timing: &EventTiming, at_start: bool) -> Property {
    match timing {
        EventTiming::Zoned {
            start,
            end,
            time_zone,
        } => {
            let t = if at_start { *start } else { *end };
            if *time_zone == chrono_tz::UTC {
                Property::new(name, format_utc(t))
            } else {
                Property::new(name, format_local(t.with_timezone(time_zone).naive_local()))
                    .with_param("TZID", time_zone.name())
            }
        }
        EventTiming::Floating { start, end } => {
            Property::new(name, format_local(if at_start { *start } else { *end }))
        }
        EventTiming::AllDay {
            start_date,
            end_date,
        } => Property::new(
            name,
            format_date(if at_start { *start_date } else { *end_date }),
        )
        .with_param("VALUE", "DATE"),
    }
}

fn key_property(name: &str, key: &OccurrenceKey, series: &EventTiming) -> Property {
    match (key, series) {
        (OccurrenceKey::Instant(t), EventTiming::Zoned { time_zone, .. })
            if *time_zone != chrono_tz::UTC =>
        {
            Property::new(name, format_local(t.with_timezone(time_zone).naive_local()))
                .with_param("TZID", time_zone.name())
        }
        (OccurrenceKey::Instant(t), _) => Property::new(name, format_utc(*t)),
        (OccurrenceKey::Floating(t), _) => Property::new(name, format_local(*t)),
        (OccurrenceKey::Date(d), _) => {
            Property::new(name, format_date(*d)).with_param("VALUE", "DATE")
        }
    }
}

fn set_text(c: &mut Component, name: &str, value: &str) {
    if value.is_empty() {
        c.remove(name);
    } else if c
        .property(name)
        .is_some_and(|p| unescape_text(&p.value) == value)
    {
        // Unchanged: keep the original encoding and parameters.
    } else {
        let mut p = Property::new(name, escape_text(value));
        if let Some(old) = c.property(name) {
            // Keep LANGUAGE/ALTREP etc. on edited text.
            p.params = old.params.clone();
        }
        c.set(p);
    }
}

fn apply(
    c: &mut Component,
    event: &EventData,
    series_timing: &EventTiming,
    now: DateTime<Utc>,
    existed: bool,
    reference: &Component,
) {
    let before = c.clone();
    let mut w = Vec::new();
    set_text(c, "SUMMARY", &event.title);
    set_text(c, "DESCRIPTION", &event.description);
    set_text(c, "LOCATION", &event.location);
    // Rewrite time and recurrence properties only when their meaning
    // changed, so equivalent encodings (DURATION, comma lists, custom
    // TZIDs) survive unedited.
    if read_timing(c, reference, &mut w).ok().as_ref() != Some(&event.timing) {
        c.set(time_property("DTSTART", &event.timing, true));
        c.remove("DURATION");
        c.set(time_property("DTEND", &event.timing, false));
    }
    match &event.recurrence {
        Some(rec) => {
            if c.property("RRULE").map(|p| p.value.trim()) != Some(rec.rrule.as_str()) {
                c.set(Property::new("RRULE", rec.rrule.clone()));
            }
            if keys(c, "EXDATE", &event.timing, reference, &mut w)
                .ok()
                .as_ref()
                != Some(&rec.exdates)
            {
                let ex = rec
                    .exdates
                    .iter()
                    .map(|k| key_property("EXDATE", k, &event.timing))
                    .collect();
                c.set_all("EXDATE", ex);
            }
            if keys(c, "RDATE", &event.timing, reference, &mut w)
                .ok()
                .as_ref()
                != Some(&rec.rdates)
            {
                let rd = rec
                    .rdates
                    .iter()
                    .map(|k| key_property("RDATE", k, &event.timing))
                    .collect();
                c.set_all("RDATE", rd);
            }
        }
        None => {
            c.remove("RRULE");
            c.remove("EXDATE");
            c.remove("RDATE");
        }
    }
    if let Some(key) = &event.recurrence_id {
        let current = c
            .property("RECURRENCE-ID")
            .and_then(|p| parse_time(p).ok())
            .map(|t| key_for(&t, series_timing, reference, &mut w));
        if current.as_ref() != Some(key) {
            c.set(key_property("RECURRENCE-ID", key, series_timing));
        }
    }
    let changed = !existed || *c != before;
    if changed {
        c.set(Property::new("DTSTAMP", format_utc(now)));
        c.set(Property::new("LAST-MODIFIED", format_utc(now)));
        if existed {
            let seq = c
                .property("SEQUENCE")
                .and_then(|p| p.value.trim().parse::<i64>().ok())
                .unwrap_or(0);
            c.set(Property::new("SEQUENCE", (seq + 1).to_string()));
        }
    }
}

pub const PRODID: &str = "-//Universal Calendar//Universal Calendar 0.1//EN";

/// Writes `master` and `overrides` into a calendar object.
///
/// With `existing`, the matching VEVENTs are edited in place and every
/// property this application does not manage is preserved; overrides no
/// longer present are removed. Without it, a new VCALENDAR is created.
pub fn write_events(
    existing: Option<&Component>,
    uid: &str,
    master: Option<&EventData>,
    overrides: &[EventData],
    now: DateTime<Utc>,
) -> Component {
    let mut cal = existing.cloned().unwrap_or_else(|| {
        let mut c = Component::new("VCALENDAR");
        c.properties.push(Property::new("VERSION", "2.0"));
        c.properties.push(Property::new("PRODID", PRODID));
        c
    });
    let series_timing = master.map(|m| m.timing.clone());
    // Identify existing VEVENTs by recurrence id, resolving TZIDs against
    // the original calendar (it may define custom VTIMEZONEs).
    let mut warnings = Vec::new();
    let reference = Component {
        components: cal.children("VTIMEZONE").cloned().collect(),
        ..Component::new("VCALENDAR")
    };
    let (old, others): (Vec<Component>, Vec<Component>) =
        cal.components.drain(..).partition(|c| c.name == "VEVENT");
    cal.components = others;
    let mut old_events: Vec<(Option<OccurrenceKey>, Component)> = old
        .into_iter()
        .map(|c| {
            let key = c
                .property("RECURRENCE-ID")
                .and_then(|p| parse_time(p).ok())
                .map(|t| {
                    let series = series_timing
                        .as_ref()
                        .or(overrides.first().map(|o| &o.timing));
                    series.map_or(OccurrenceKey::Date(NaiveDate::MIN), |s| {
                        key_for(&t, s, &reference, &mut warnings)
                    })
                });
            (key, c)
        })
        .collect();
    let mut take = |rid: Option<&OccurrenceKey>| -> Option<Component> {
        let pos = old_events.iter().position(|(k, _)| k.as_ref() == rid)?;
        Some(old_events.remove(pos).1)
    };

    let mut new_events = Vec::new();
    if let Some(m) = master {
        let found = take(None);
        let existed = found.is_some();
        let mut c = found.unwrap_or_else(|| {
            let mut c = Component::new("VEVENT");
            c.properties.push(Property::new("UID", uid));
            c
        });
        apply(&mut c, m, &m.timing, now, existed, &reference);
        new_events.push(c);
    }
    for o in overrides {
        let series = series_timing.as_ref().unwrap_or(&o.timing);
        let found = take(o.recurrence_id.as_ref());
        let existed = found.is_some();
        let mut c = found.unwrap_or_else(|| {
            let mut c = Component::new("VEVENT");
            c.properties.push(Property::new("UID", uid));
            c
        });
        apply(&mut c, o, series, now, existed, &reference);
        new_events.push(c);
    }
    // VTIMEZONEs and other components first, then events.
    cal.components.extend(new_events);
    cal
}
