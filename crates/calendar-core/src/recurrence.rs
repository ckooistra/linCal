//! Bounded, time-zone-aware expansion of recurring events.
//!
//! Rules are evaluated by the `rrule` crate (RFC 5545) in the series' own
//! zone, so wall-clock times survive DST transitions. Expansion is always
//! limited to a requested range and to [`MAX_ITERATIONS`] candidate
//! occurrences; nothing is ever expanded without bound.

use std::collections::HashSet;

use chrono::{DateTime, Duration, TimeZone, Utc};
use chrono_tz::Tz;
use rrule::{RRule, RRuleSet, Unvalidated};

use crate::error::CoreError;
use crate::model::{CalendarEvent, EventTiming, Occurrence, Recurrence, occurrence_id};
use crate::occurrence_key::OccurrenceKey;
use crate::timing::{TimeRange, resolve_local};

/// Upper bound on candidates examined per series and query.
pub const MAX_ITERATIONS: usize = 50_000;

/// Upper bound used when computing the end of a finite series.
const MAX_FINITE_OCCURRENCES: u16 = 10_000;

struct Prepared {
    set: RRuleSet,
    kind: Kind,
}

#[derive(Clone, Copy)]
enum Kind {
    Zoned { tz: Tz, duration: Duration },
    Floating { duration: Duration },
    AllDay { days: i64 },
}

fn rrule_tz(tz: Tz) -> rrule::Tz {
    rrule::Tz::Tz(tz)
}

fn invalid(e: impl std::fmt::Display) -> CoreError {
    CoreError::InvalidRecurrence(e.to_string())
}

/// Whether a recurrence is an RDATE-only set (no RRULE): DTSTART plus the
/// explicit dates.
pub fn is_rdate_only(recurrence: &Recurrence) -> bool {
    recurrence.rrule.trim().is_empty()
}

/// Days to skip at a time, for rules whose occurrences can be generated
/// from a later start without changing them: no COUNT (it counts from the
/// original start), and hourly, daily or weekly frequency, which the rule
/// engine steps in wall-clock time. Monthly and yearly rules are cheap to
/// iterate from the start; minutely and secondly ones are not skipped.
fn skippable_days(rule: &RRule) -> Option<u64> {
    if rule.get_count().is_some() {
        return None;
    }
    let n = u64::from(rule.get_interval().max(1));
    match rule.get_freq() {
        rrule::Frequency::Weekly => Some(7 * n),
        rrule::Frequency::Daily => Some(n),
        // Whole days that are a multiple of the interval.
        rrule::Frequency::Hourly => Some(n / gcd(n, 24)),
        _ => None,
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Builds the rule set for `timing` + `recurrence`. Floating series are
/// evaluated in `display_tz`; all-day series on UTC midnights (pure dates).
///
/// With `from`, the set may start later than the series (by whole rule
/// periods) when no occurrence before `from` is needed, so old series with
/// many occurrences are not iterated from their beginning on every query.
fn prepare(
    timing: &EventTiming,
    recurrence: &Recurrence,
    display_tz: &Tz,
    from: Option<DateTime<Utc>>,
) -> Result<Prepared, CoreError> {
    let (dt_start, kind) = match timing {
        EventTiming::Zoned {
            start,
            end,
            time_zone,
        } => (
            start.with_timezone(&rrule_tz(*time_zone)),
            Kind::Zoned {
                tz: *time_zone,
                duration: *end - *start,
            },
        ),
        EventTiming::Floating { start, end } => (
            resolve_local(display_tz, *start).with_timezone(&rrule_tz(*display_tz)),
            Kind::Floating {
                duration: *end - *start,
            },
        ),
        EventTiming::AllDay {
            start_date,
            end_date,
        } => (
            rrule::Tz::UTC.from_utc_datetime(&start_date.and_hms_opt(0, 0, 0).expect("valid")),
            Kind::AllDay {
                days: (*end_date - *start_date).num_days(),
            },
        ),
    };
    let to_set_time = |key: &OccurrenceKey| -> DateTime<rrule::Tz> {
        match key {
            OccurrenceKey::Instant(t) => t.with_timezone(&dt_start.timezone()),
            OccurrenceKey::Floating(t) => {
                resolve_local(display_tz, *t).with_timezone(&dt_start.timezone())
            }
            OccurrenceKey::Date(d) => rrule::Tz::UTC
                .from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("valid"))
                .with_timezone(&dt_start.timezone()),
        }
    };
    let mut rdates: Vec<_> = recurrence.rdates.iter().map(to_set_time).collect();
    let exdates = recurrence.exdates.iter().map(to_set_time).collect();
    if is_rdate_only(recurrence) {
        // RFC 5545 §3.8.5.2: DTSTART is the first instance of the set.
        rdates.push(dt_start);
        rdates.sort();
        rdates.dedup();
        let set = RRuleSet::new(dt_start)
            .set_rdates(rdates)
            .set_exdates(exdates);
        return Ok(Prepared { set, kind });
    }
    let rrule = until_in_utc(&recurrence.rrule, &kind, display_tz);
    let rule: RRule<Unvalidated> = rrule.parse().map_err(invalid)?;
    // Validation fills in defaults (BYDAY, BYHOUR, ...) from the real start,
    // so a later set start below generates the same occurrences.
    let rule = rule.validate(dt_start).map_err(invalid)?;
    let mut set_start = dt_start;
    if let (Some(from), Some(period)) = (from, skippable_days(&rule)) {
        let local_start = dt_start.naive_local();
        let target = from.with_timezone(&dt_start.timezone()).naive_local();
        let gap = (target - local_start).num_days();
        if gap > 0 {
            let period = i64::try_from(period).unwrap_or(i64::MAX);
            let mut skip = gap / period * period;
            // A skipped-to wall time that does not exist (DST gap): go back.
            while skip > 0 {
                let candidate = local_start + Duration::days(skip);
                if let Some(t) = dt_start
                    .timezone()
                    .from_local_datetime(&candidate)
                    .earliest()
                {
                    set_start = t;
                    break;
                }
                skip -= period;
            }
        }
    }
    let set = RRuleSet::new(set_start)
        .rrule(rule)
        .set_rdates(rdates)
        .set_exdates(exdates);
    Ok(Prepared { set, kind })
}

/// The rule with UNTIL as a UTC date-time, which the rule engine requires.
/// RFC 5545 has UNTIL take DTSTART's form: a date for all-day series
/// (inclusive: that whole day) and a local time for floating ones. Lenient
/// forms are read the same way: a date ends a timed series at the end of
/// that day, and a local time on a zoned series is in the series' zone.
fn until_in_utc<'a>(rrule: &'a str, kind: &Kind, display_tz: &Tz) -> std::borrow::Cow<'a, str> {
    let Some(value) = rrule.split(';').find_map(|p| {
        let (name, value) = p.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("UNTIL")
            .then_some(value.trim())
    }) else {
        return rrule.into();
    };
    let Ok(key) = value.parse::<OccurrenceKey>() else {
        return rrule.into(); // left for the rule engine to report
    };
    let zone = match kind {
        Kind::Zoned { tz, .. } => *tz,
        Kind::Floating { .. } => *display_tz,
        Kind::AllDay { .. } => chrono_tz::UTC,
    };
    let utc = match key {
        OccurrenceKey::Instant(_) => return rrule.into(),
        OccurrenceKey::Floating(t) => resolve_local(&zone, t),
        OccurrenceKey::Date(d) => match kind {
            // All-day occurrences start at UTC midnight (see `prepare`).
            Kind::AllDay { .. } => d.and_hms_opt(0, 0, 0).expect("valid").and_utc(),
            _ => resolve_local(&zone, d.and_hms_opt(23, 59, 59).expect("valid")),
        },
    };
    rrule
        .split(';')
        .map(|p| match p.split_once('=') {
            Some((name, _)) if name.trim().eq_ignore_ascii_case("UNTIL") => {
                format!("UNTIL={}", OccurrenceKey::Instant(utc))
            }
            _ => p.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(";")
        .into()
}

/// Checks that a recurrence is valid for the given timing.
pub fn validate(timing: &EventTiming, recurrence: &Recurrence) -> Result<(), CoreError> {
    prepare(timing, recurrence, &chrono_tz::UTC, None).map(|_| ())
}

impl Kind {
    fn occurrence(self, start: DateTime<rrule::Tz>) -> (OccurrenceKey, EventTiming) {
        match self {
            Kind::Zoned { tz, duration } => {
                let s = start.with_timezone(&Utc);
                (
                    OccurrenceKey::Instant(s),
                    EventTiming::Zoned {
                        start: s,
                        end: s + duration,
                        time_zone: tz,
                    },
                )
            }
            Kind::Floating { duration } => {
                let s = start.naive_local();
                (
                    OccurrenceKey::Floating(s),
                    EventTiming::Floating {
                        start: s,
                        end: s + duration,
                    },
                )
            }
            Kind::AllDay { days } => {
                let d = start.date_naive();
                (
                    OccurrenceKey::Date(d),
                    EventTiming::AllDay {
                        start_date: d,
                        end_date: d + Duration::days(days),
                    },
                )
            }
        }
    }
}

/// Occurrences of `master` overlapping `range`, as seen from `display_tz`.
///
/// `overridden` holds recurrence ids replaced by override rows; those slots
/// are skipped (the override rows are returned by the storage query).
/// Non-recurring events yield themselves if they overlap.
pub fn expand(
    master: &CalendarEvent,
    range: &TimeRange,
    display_tz: &Tz,
    overridden: &HashSet<OccurrenceKey>,
) -> Result<Vec<Occurrence>, CoreError> {
    let Some(recurrence) = &master.recurrence else {
        let occurrence = master.as_occurrence();
        return Ok(if occurrence.timing.bounds(display_tz).overlaps(range) {
            vec![occurrence]
        } else {
            Vec::new()
        });
    };
    // Occurrences ending after the range start can begin up to their
    // duration (plus zone offsets) earlier.
    let bounds = master.timing.index_bounds();
    let lead = (bounds.end - bounds.start) + Duration::days(2);
    let prepared = prepare(
        &master.timing,
        recurrence,
        display_tz,
        Some(range.start - lead),
    )?;
    let mut result = Vec::new();
    for (i, start) in prepared.set.into_iter().enumerate() {
        if i >= MAX_ITERATIONS {
            break;
        }
        let (key, timing) = prepared.kind.occurrence(start);
        let bounds = timing.bounds(display_tz);
        if bounds.start >= range.end {
            break;
        }
        if bounds.end <= range.start || overridden.contains(&key) {
            continue;
        }
        result.push(Occurrence {
            id: occurrence_id(&master.id, &key),
            event_id: master.id.clone(),
            series_id: Some(master.id.clone()),
            recurrence_id: Some(key),
            calendar_id: master.calendar_id.clone(),
            title: master.title.clone(),
            description: master.description.clone(),
            location: master.location.clone(),
            timing,
            rrule: Some(recurrence.rrule.clone()),
            reminders: master.reminders.clone(),
            sync_status: master.sync_status,
            revision: master.revision,
        });
    }
    Ok(result)
}

/// Whether `key` is a slot the series generates. EXDATE-excluded slots are
/// not occurrences.
pub fn has_occurrence(
    timing: &EventTiming,
    recurrence: &Recurrence,
    key: &OccurrenceKey,
    display_tz: &Tz,
) -> Result<bool, CoreError> {
    let near = match key {
        OccurrenceKey::Instant(t) => *t,
        OccurrenceKey::Floating(t) => t.and_utc(),
        OccurrenceKey::Date(d) => d.and_hms_opt(0, 0, 0).expect("valid").and_utc(),
    } - Duration::days(2);
    let prepared = prepare(timing, recurrence, display_tz, Some(near))?;
    for start in prepared.set.into_iter().take(MAX_ITERATIONS) {
        let (candidate, _) = prepared.kind.occurrence(start);
        if std::mem::discriminant(&candidate) != std::mem::discriminant(key) {
            return Ok(false);
        }
        if &candidate == key {
            return Ok(true);
        }
        if candidate > *key {
            break;
        }
    }
    Ok(false)
}

/// How occurrence ids move when a whole series is moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyShift {
    /// Wall-clock offset: for zoned series in `zone`, so an exception at
    /// 09:00 stays at the series' local time when the move crosses a DST
    /// change; for floating and all-day series a plain offset.
    pub delta: Duration,
    pub zone: Option<Tz>,
}

impl KeyShift {
    pub fn apply(&self, key: &OccurrenceKey) -> OccurrenceKey {
        match (key, self.zone) {
            (OccurrenceKey::Instant(t), Some(tz)) => {
                let local = t.with_timezone(&tz).naive_local() + self.delta;
                OccurrenceKey::Instant(resolve_local(&tz, local))
            }
            _ => key.shifted(self.delta),
        }
    }

    /// Whether ids move forward in time (for ordering renames).
    pub fn is_forward(&self) -> bool {
        self.delta > Duration::zero()
    }
}

/// Result of editing a whole series through one of its occurrences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retimed {
    /// New timing of the series master (its first occurrence).
    pub timing: EventTiming,
    /// How existing occurrence ids move; `None` when the timing kind
    /// changed and old EXDATEs/overrides can no longer be mapped.
    pub key_shift: Option<KeyShift>,
}

/// New master timing when the occurrence `key` of a series is given the
/// timing `edited` and the change applies to the whole series: the series
/// start moves by the same offset and takes the edited duration.
pub fn retime_series(master: &EventTiming, key: &OccurrenceKey, edited: &EventTiming) -> Retimed {
    let shift = match (master, key, edited) {
        (
            EventTiming::Zoned { start, .. },
            OccurrenceKey::Instant(original),
            EventTiming::Zoned {
                start: new,
                end,
                time_zone,
            },
        ) => {
            // Measured in wall-clock time: moving 09:00 to 10:00 means 10:00
            // for every occurrence, on either side of a DST change.
            let local = |t: &DateTime<Utc>| t.with_timezone(time_zone).naive_local();
            let delta = local(new) - local(original);
            let s = resolve_local(time_zone, local(start) + delta);
            Some((
                EventTiming::Zoned {
                    start: s,
                    end: s + (*end - *new),
                    time_zone: *time_zone,
                },
                KeyShift {
                    delta,
                    zone: Some(*time_zone),
                },
            ))
        }
        (
            EventTiming::Floating { start, .. },
            OccurrenceKey::Floating(original),
            EventTiming::Floating { start: new, end },
        ) => {
            let delta = *new - *original;
            let s = *start + delta;
            Some((
                EventTiming::Floating {
                    start: s,
                    end: s + (*end - *new),
                },
                KeyShift { delta, zone: None },
            ))
        }
        (
            EventTiming::AllDay { start_date, .. },
            OccurrenceKey::Date(original),
            EventTiming::AllDay {
                start_date: new,
                end_date,
            },
        ) => {
            let delta = *new - *original;
            let s = *start_date + delta;
            Some((
                EventTiming::AllDay {
                    start_date: s,
                    end_date: s + (*end_date - *new),
                },
                KeyShift { delta, zone: None },
            ))
        }
        _ => None,
    };
    match shift {
        Some((timing, shift)) => Retimed {
            timing,
            key_shift: Some(shift),
        },
        None => Retimed {
            timing: edited.clone(),
            key_shift: None,
        },
    }
}

/// A series divided at one occurrence ("this and following").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// The original series, ending before the split; `None` when the split
    /// is at its first occurrence (nothing remains before it).
    pub before: Option<Recurrence>,
    /// Timing of the new series: the occurrence at the split, unedited.
    pub after_timing: EventTiming,
    /// Recurrence of the new series.
    pub after: Recurrence,
}

/// Divides a series at `key`, which must be one of its occurrences (an
/// EXDATE-cancelled one counts, an RDATE does not). The original keeps the
/// occurrences before `key`: its rule ends with UNTIL at its last earlier
/// occurrence, or with a reduced COUNT. The new series starts at `key` with
/// the same rule and the remaining COUNT. RDATEs and EXDATEs go to the side
/// they fall on.
pub fn split_series(
    timing: &EventTiming,
    recurrence: &Recurrence,
    key: &OccurrenceKey,
) -> Result<Split, CoreError> {
    if is_rdate_only(recurrence) {
        return Err(CoreError::InvalidRecurrence(
            "a series made only of dates cannot be split".into(),
        ));
    }
    // Rule occurrences alone: COUNT counts them before EXDATEs apply.
    let rule_only = Recurrence {
        rrule: recurrence.rrule.clone(),
        ..Recurrence::default()
    };
    let prepared = prepare(timing, &rule_only, &chrono_tz::UTC, None)?;
    let count = prepared.set.get_rrule().first().and_then(|r| r.get_count());
    let mut before = 0u32;
    let mut last_before = None;
    let mut at = None;
    for start in prepared.set.into_iter().take(MAX_ITERATIONS) {
        let (candidate, occurrence) = prepared.kind.occurrence(start);
        if std::mem::discriminant(&candidate) != std::mem::discriminant(key) {
            break;
        }
        if candidate == *key {
            at = Some(occurrence);
            break;
        }
        if candidate > *key {
            break;
        }
        before += 1;
        last_before = Some(candidate);
    }
    let Some(after_timing) = at else {
        return Err(CoreError::InvalidRecurrence(format!(
            "{key} is not an occurrence of the series"
        )));
    };
    let kept = |end: String| -> String {
        let mut parts: Vec<&str> = recurrence
            .rrule
            .split(';')
            .filter(|p| {
                let name = p
                    .split('=')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_uppercase();
                !p.trim().is_empty() && name != "COUNT" && name != "UNTIL"
            })
            .collect();
        parts.push(&end);
        parts.join(";")
    };
    let after_rule = match count {
        Some(total) => kept(format!("COUNT={}", total - before)),
        None => recurrence.rrule.clone(),
    };
    let after = Recurrence {
        rrule: after_rule,
        rdates: recurrence
            .rdates
            .iter()
            .filter(|k| *k >= key)
            .copied()
            .collect(),
        exdates: recurrence
            .exdates
            .iter()
            .filter(|k| *k >= key)
            .copied()
            .collect(),
    };
    let before = last_before.map(|last| Recurrence {
        rrule: match count {
            Some(_) => kept(format!("COUNT={before}")),
            None => kept(format!("UNTIL={last}")),
        },
        rdates: recurrence
            .rdates
            .iter()
            .filter(|k| *k < key)
            .copied()
            .collect(),
        exdates: recurrence
            .exdates
            .iter()
            .filter(|k| *k < key)
            .copied()
            .collect(),
    });
    Ok(Split {
        before,
        after_timing,
        after,
    })
}

/// Zone-independent upper bound of a series for indexing (see
/// [`EventTiming::index_bounds`]): `None` for unbounded or very long series.
pub fn index_end(
    timing: &EventTiming,
    recurrence: &Recurrence,
) -> Result<Option<DateTime<Utc>>, CoreError> {
    let prepared = prepare(timing, recurrence, &chrono_tz::UTC, None)?;
    if let Some(rule) = prepared.set.get_rrule().first()
        && rule.get_count().is_none()
        && rule.get_until().is_none()
    {
        return Ok(None);
    }
    let result = prepared.set.all(MAX_FINITE_OCCURRENCES);
    if result.limited {
        return Ok(None);
    }
    Ok(result
        .dates
        .last()
        .map(|last| prepared.kind.occurrence(*last).1.index_bounds().end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CalendarEvent, SyncStatus};
    use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

    const BERLIN: Tz = chrono_tz::Europe::Berlin;

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap()
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn zoned(start: NaiveDateTime, minutes: i64) -> EventTiming {
        let s = resolve_local(&BERLIN, start);
        EventTiming::Zoned {
            start: s,
            end: s + Duration::minutes(minutes),
            time_zone: BERLIN,
        }
    }

    fn series(timing: EventTiming, rrule: &str) -> CalendarEvent {
        CalendarEvent {
            reminders: Vec::new(),
            id: "s".into(),
            calendar_id: "cal".into(),
            title: "Series".into(),
            description: String::new(),
            location: String::new(),
            timing,
            recurrence: Some(Recurrence {
                rrule: rrule.into(),
                ..Recurrence::default()
            }),
            series: None,
            sync_status: SyncStatus::Saved,
            revision: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn range(from: NaiveDate, to: NaiveDate) -> TimeRange {
        TimeRange::new(
            resolve_local(&BERLIN, from.and_hms_opt(0, 0, 0).unwrap()),
            resolve_local(&BERLIN, to.and_hms_opt(0, 0, 0).unwrap()),
        )
    }

    fn starts(occ: &[Occurrence]) -> Vec<NaiveDateTime> {
        occ.iter()
            .map(|o| match o.timing {
                EventTiming::Zoned { start, .. } => start.with_timezone(&BERLIN).naive_local(),
                EventTiming::Floating { start, .. } => start,
                EventTiming::AllDay { start_date, .. } => start_date.and_hms_opt(0, 0, 0).unwrap(),
            })
            .collect()
    }

    fn run(event: &CalendarEvent, r: TimeRange) -> Vec<Occurrence> {
        expand(event, &r, &BERLIN, &HashSet::new()).unwrap()
    }

    #[test]
    fn daily_with_count() {
        let e = series(zoned(local(2026, 6, 1, 8, 0), 30), "FREQ=DAILY;COUNT=5");
        let occ = run(&e, range(date(2026, 1, 1), date(2027, 1, 1)));
        assert_eq!(occ.len(), 5);
        assert_eq!(starts(&occ)[4], local(2026, 6, 5, 8, 0));
        assert_eq!(occ[1].id, "s::20260602T060000Z");
    }

    #[test]
    fn weekly_by_day_only_within_range() {
        let e = series(
            zoned(local(2026, 6, 1, 9, 30), 15),
            "FREQ=WEEKLY;BYDAY=MO,WE,FR",
        );
        let occ = run(&e, range(date(2026, 6, 8), date(2026, 6, 15)));
        assert_eq!(
            starts(&occ),
            vec![
                local(2026, 6, 8, 9, 30),
                local(2026, 6, 10, 9, 30),
                local(2026, 6, 12, 9, 30)
            ]
        );
    }

    #[test]
    fn monthly_on_the_31st_skips_short_months() {
        let e = series(
            zoned(local(2026, 1, 31, 12, 0), 60),
            "FREQ=MONTHLY;BYMONTHDAY=31",
        );
        let occ = run(&e, range(date(2026, 1, 1), date(2026, 8, 1)));
        let months: Vec<u32> = starts(&occ).iter().map(|t| t.date().month()).collect();
        assert_eq!(months, vec![1, 3, 5, 7]);
    }

    #[test]
    fn yearly_all_day_birthday() {
        let e = series(
            EventTiming::AllDay {
                start_date: date(2020, 2, 29),
                end_date: date(2020, 3, 1),
            },
            "FREQ=YEARLY",
        );
        // Feb 29 exists only in leap years.
        let occ = run(&e, range(date(2021, 1, 1), date(2029, 1, 1)));
        let years: Vec<i32> = starts(&occ).iter().map(|t| t.date().year()).collect();
        assert_eq!(years, vec![2024, 2028]);
        assert!(matches!(occ[0].recurrence_id, Some(OccurrenceKey::Date(_))));
    }

    #[test]
    fn keeps_wall_clock_time_across_dst() {
        // Weekly 09:30 Berlin, spanning both 2026 DST transitions.
        let e = series(zoned(local(2026, 3, 1, 9, 30), 30), "FREQ=WEEKLY");
        let occ = run(&e, range(date(2026, 3, 1), date(2026, 11, 30)));
        for (o, local_start) in occ.iter().zip(starts(&occ)) {
            assert_eq!(
                (local_start.hour(), local_start.minute()),
                (9, 30),
                "{}",
                o.id
            );
        }
        let EventTiming::Zoned { start: winter, .. } = occ[0].timing else {
            panic!()
        };
        let EventTiming::Zoned { start: summer, .. } = occ[5].timing else {
            panic!()
        };
        assert_eq!(winter.hour(), 8); // CET = UTC+1
        assert_eq!(summer.hour(), 7); // CEST = UTC+2
    }

    #[test]
    fn exdates_and_rdates() {
        let mut e = series(zoned(local(2026, 6, 1, 8, 0), 30), "FREQ=DAILY;COUNT=3");
        let rec = e.recurrence.as_mut().unwrap();
        rec.exdates.push(OccurrenceKey::Instant(resolve_local(
            &BERLIN,
            local(2026, 6, 2, 8, 0),
        )));
        rec.rdates.push(OccurrenceKey::Instant(resolve_local(
            &BERLIN,
            local(2026, 6, 10, 15, 0),
        )));
        let occ = run(&e, range(date(2026, 6, 1), date(2026, 7, 1)));
        assert_eq!(
            starts(&occ),
            vec![
                local(2026, 6, 1, 8, 0),
                local(2026, 6, 3, 8, 0),
                local(2026, 6, 10, 15, 0)
            ]
        );
    }

    #[test]
    fn overridden_slots_are_skipped() {
        let e = series(zoned(local(2026, 6, 1, 8, 0), 30), "FREQ=DAILY;COUNT=3");
        let moved = OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 6, 2, 8, 0)));
        let occ = expand(
            &e,
            &range(date(2026, 6, 1), date(2026, 7, 1)),
            &BERLIN,
            &HashSet::from([moved]),
        )
        .unwrap();
        assert_eq!(occ.len(), 2);
        assert!(occ.iter().all(|o| o.recurrence_id != Some(moved)));
    }

    #[test]
    fn unbounded_rule_expands_only_the_requested_range() {
        let e = series(zoned(local(2020, 1, 1, 7, 0), 60), "FREQ=DAILY");
        let occ = run(&e, range(date(2035, 5, 1), date(2035, 5, 8)));
        assert_eq!(occ.len(), 7);
        assert_eq!(
            index_end(&e.timing, e.recurrence.as_ref().unwrap()).unwrap(),
            None
        );
    }

    #[test]
    fn partially_overlapping_occurrences_are_included() {
        // 23:00–01:00 daily: the one starting the day before reaches into the range.
        let e = series(zoned(local(2026, 6, 1, 23, 0), 120), "FREQ=DAILY");
        let occ = run(&e, range(date(2026, 6, 5), date(2026, 6, 6)));
        assert_eq!(
            starts(&occ),
            vec![local(2026, 6, 4, 23, 0), local(2026, 6, 5, 23, 0)]
        );
    }

    #[test]
    fn floating_series_follow_the_viewer_zone() {
        let e = series(
            EventTiming::Floating {
                start: local(2026, 6, 1, 9, 0),
                end: local(2026, 6, 1, 10, 0),
            },
            "FREQ=DAILY;COUNT=2",
        );
        let tokyo = chrono_tz::Asia::Tokyo;
        let r = TimeRange::new(
            Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap(),
        );
        let occ = expand(&e, &r, &tokyo, &HashSet::new()).unwrap();
        assert_eq!(occ.len(), 2);
        assert_eq!(
            occ[0].timing.bounds(&tokyo).start,
            Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap()
        );
        assert_eq!(
            occ[0].recurrence_id,
            Some(OccurrenceKey::Floating(local(2026, 6, 1, 9, 0)))
        );
    }

    #[test]
    fn finite_series_have_an_index_end() {
        let e = series(zoned(local(2026, 6, 1, 8, 0), 30), "FREQ=WEEKLY;COUNT=3");
        let end = index_end(&e.timing, e.recurrence.as_ref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(end, resolve_local(&BERLIN, local(2026, 6, 15, 8, 30)));
    }

    #[test]
    fn rejects_invalid_rules() {
        let e = series(zoned(local(2026, 6, 1, 8, 0), 30), "FREQ=SOMETIMES");
        assert!(matches!(
            validate(&e.timing, e.recurrence.as_ref().unwrap()),
            Err(CoreError::InvalidRecurrence(_))
        ));
    }

    #[test]
    fn retiming_a_series_moves_its_start_by_the_same_offset() {
        let master = zoned(local(2026, 1, 5, 9, 30), 30);
        let key = OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 6, 8, 9, 30)));
        let edited = zoned(local(2026, 6, 8, 10, 30), 60);
        let r = retime_series(&master, &key, &edited);
        assert_eq!(r.timing, zoned(local(2026, 1, 5, 10, 30), 60));
        let shift = r.key_shift.unwrap();
        assert_eq!(shift.delta, Duration::hours(1));
        assert_eq!(
            shift.apply(&key),
            OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 6, 8, 10, 30)))
        );
        // Wall-clock: an exception on the other side of DST keeps its local time.
        let march = zoned(local(2026, 3, 25, 9, 0), 60);
        let first = OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 3, 25, 9, 0)));
        let r = retime_series(&march, &first, &zoned(local(2026, 3, 24, 9, 0), 60));
        let excluded = OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 3, 29, 9, 0)));
        assert_eq!(
            r.key_shift.unwrap().apply(&excluded),
            OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 3, 28, 9, 0)))
        );

        let all_day = EventTiming::AllDay {
            start_date: date(2026, 6, 8),
            end_date: date(2026, 6, 9),
        };
        let r = retime_series(&master, &key, &all_day);
        assert_eq!(
            r,
            Retimed {
                timing: all_day,
                key_shift: None
            }
        );
    }

    /// Keys in `r` when iterating the whole set from the series start.
    fn keys_without_skipping(e: &CalendarEvent, r: TimeRange) -> Vec<OccurrenceKey> {
        let prepared = prepare(&e.timing, e.recurrence.as_ref().unwrap(), &BERLIN, None).unwrap();
        prepared
            .set
            .into_iter()
            .map(|start| prepared.kind.occurrence(start))
            .take_while(|(_, t)| t.bounds(&BERLIN).start < r.end)
            .filter(|(_, t)| t.bounds(&BERLIN).end > r.start)
            .map(|(k, _)| k)
            .collect()
    }

    #[test]
    fn skipping_ahead_generates_the_same_occurrences() {
        for (start, rule) in [
            (local(2024, 1, 1, 0, 15), "FREQ=HOURLY;INTERVAL=5"),
            (local(2001, 1, 2, 9, 0), "FREQ=WEEKLY;INTERVAL=3"),
            (
                local(2001, 1, 2, 9, 0),
                "FREQ=WEEKLY;BYDAY=MO,TH;INTERVAL=2",
            ),
            (local(2003, 3, 30, 2, 30), "FREQ=DAILY;INTERVAL=3"),
            (local(2024, 2, 3, 1, 7), "FREQ=HOURLY;INTERVAL=7"),
            (
                local(2019, 7, 3, 1, 7),
                "FREQ=HOURLY;INTERVAL=36;BYMINUTE=0,30",
            ),
        ] {
            let e = series(zoned(start, 30), rule);
            for r in [
                range(date(2026, 3, 10), date(2026, 4, 10)),
                range(date(2026, 10, 5), date(2026, 11, 5)),
            ] {
                let skipped: Vec<_> = run(&e, r)
                    .iter()
                    .map(|o| o.recurrence_id.unwrap())
                    .collect();
                assert!(!skipped.is_empty(), "{rule}");
                assert_eq!(skipped, keys_without_skipping(&e, r), "{rule}");
            }
        }
    }

    #[test]
    fn old_high_frequency_series_reach_the_requested_range() {
        // Hourly since 2010: far more than MAX_ITERATIONS slots before 2026.
        let e = series(zoned(local(2010, 1, 1, 0, 15), 30), "FREQ=HOURLY");
        let occ = run(&e, range(date(2026, 6, 1), date(2026, 6, 2)));
        assert_eq!(occ.len(), 24);
        let rec = e.recurrence.as_ref().unwrap();
        assert!(has_occurrence(&e.timing, rec, &occ[5].recurrence_id.unwrap(), &BERLIN).unwrap());
    }

    #[test]
    fn rdate_only_sets_include_the_start_and_extra_dates() {
        let mut e = series(zoned(local(2026, 6, 10, 9, 0), 30), "");
        e.recurrence.as_mut().unwrap().rdates = vec![
            OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 5, 1, 9, 0))),
            OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 6, 11, 14, 0))),
        ];
        let occ = run(&e, range(date(2026, 4, 1), date(2026, 7, 1)));
        assert_eq!(
            starts(&occ),
            vec![
                local(2026, 5, 1, 9, 0),
                local(2026, 6, 10, 9, 0),
                local(2026, 6, 11, 14, 0)
            ]
        );
        let rec = e.recurrence.as_ref().unwrap();
        assert!(validate(&e.timing, rec).is_ok());
        assert_eq!(
            index_end(&e.timing, rec).unwrap(),
            Some(resolve_local(&BERLIN, local(2026, 6, 11, 14, 30)))
        );
    }

    #[test]
    fn detects_generated_slots() {
        let e = series(zoned(local(2026, 6, 1, 8, 0), 30), "FREQ=DAILY;COUNT=3");
        let rec = e.recurrence.as_ref().unwrap();
        let slot = |d| OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 6, d, 8, 0)));
        assert!(has_occurrence(&e.timing, rec, &slot(3), &BERLIN).unwrap());
        assert!(!has_occurrence(&e.timing, rec, &slot(4), &BERLIN).unwrap());
    }

    #[test]
    fn splitting_keeps_every_occurrence_on_one_side() {
        let all_day = |y, m, d| EventTiming::AllDay {
            start_date: date(y, m, d),
            end_date: date(y, m, d) + Duration::days(1),
        };
        let floating = EventTiming::Floating {
            start: local(2026, 3, 20, 9, 0),
            end: local(2026, 3, 20, 10, 0),
        };
        let cases = [
            // Across the DST change, with COUNT.
            (zoned(local(2026, 3, 25, 9, 0), 30), "FREQ=DAILY;COUNT=10"),
            (
                zoned(local(2026, 3, 2, 9, 0), 60),
                "FREQ=WEEKLY;BYDAY=MO,WE,FR;UNTIL=20260501T000000Z",
            ),
            (
                zoned(local(2026, 1, 30, 18, 0), 60),
                "FREQ=MONTHLY;BYDAY=-1FR",
            ),
            (zoned(local(2026, 1, 5, 8, 0), 15), "FREQ=WEEKLY;INTERVAL=2"),
            (all_day(2026, 2, 14), "FREQ=YEARLY;COUNT=5"),
            (all_day(2026, 3, 1), "FREQ=DAILY;INTERVAL=3"),
            (floating, "FREQ=DAILY;COUNT=40"),
        ];
        let window = range(date(2025, 12, 1), date(2031, 1, 1));
        for (timing, rule) in cases {
            let mut original = series(timing.clone(), rule);
            let first = run(&original, window);
            // A cancelled occurrence on each side of the split.
            let keys: Vec<OccurrenceKey> = first.iter().map(|o| o.recurrence_id.unwrap()).collect();
            let rec = original.recurrence.as_mut().unwrap();
            rec.exdates = vec![keys[1], keys[keys.len() - 2]];
            let full = run(&original, window);
            for at in [2, keys.len() / 2, keys.len() - 2] {
                let split = split_series(&timing, original.recurrence.as_ref().unwrap(), &keys[at])
                    .unwrap();
                let mut head = original.clone();
                head.recurrence = split.before.clone();
                assert!(head.recurrence.is_some(), "{rule} at {at}");
                let mut tail = original.clone();
                tail.timing = split.after_timing.clone();
                tail.recurrence = Some(split.after.clone());
                let mut joined = run(&head, window);
                joined.extend(run(&tail, window));
                let starts_of = |v: &[Occurrence]| {
                    let mut k: Vec<_> = v.iter().map(|o| o.recurrence_id.unwrap()).collect();
                    k.sort();
                    k
                };
                assert_eq!(starts_of(&joined), starts_of(&full), "{rule} split at {at}");
                assert!(
                    run(&tail, window)
                        .iter()
                        .all(|o| o.recurrence_id.unwrap() >= keys[at])
                );
            }
            // At the first occurrence nothing remains before.
            let split =
                split_series(&timing, original.recurrence.as_ref().unwrap(), &keys[0]).unwrap();
            assert_eq!(split.before, None);
            assert_eq!(split.after_timing, timing);
        }
    }

    #[test]
    fn split_rules_and_refusals() {
        let timing = zoned(local(2026, 6, 1, 9, 0), 30);
        let rec = Recurrence {
            rrule: "FREQ=DAILY;COUNT=10;INTERVAL=1".into(),
            ..Recurrence::default()
        };
        let key = |d| OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 6, d, 9, 0)));
        let split = split_series(&timing, &rec, &key(4)).unwrap();
        assert_eq!(split.before.unwrap().rrule, "FREQ=DAILY;INTERVAL=1;COUNT=3");
        assert_eq!(split.after.rrule, "FREQ=DAILY;INTERVAL=1;COUNT=7");
        let open = Recurrence {
            rrule: "FREQ=DAILY".into(),
            ..Recurrence::default()
        };
        let split = split_series(&timing, &open, &key(4)).unwrap();
        // UNTIL is the last earlier occurrence, in UTC (09:00 CEST).
        assert_eq!(
            split.before.unwrap().rrule,
            "FREQ=DAILY;UNTIL=20260603T070000Z"
        );
        assert_eq!(split.after.rrule, "FREQ=DAILY");
        // Not a slot of the series.
        let off = OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 6, 4, 10, 0)));
        assert!(split_series(&timing, &open, &off).is_err());
        assert!(split_series(&timing, &open, &OccurrenceKey::Date(date(2026, 6, 4))).is_err());
    }

    #[test]
    fn until_takes_the_form_of_the_start() {
        let all_day = EventTiming::AllDay {
            start_date: date(2026, 1, 1),
            end_date: date(2026, 1, 2),
        };
        let floating = EventTiming::Floating {
            start: local(2026, 1, 1, 9, 0),
            end: local(2026, 1, 1, 10, 0),
        };
        let timed = zoned(local(2026, 1, 1, 9, 0), 30);
        let window = range(date(2025, 12, 1), date(2026, 2, 1));
        for (timing, rule, expected) in [
            (&all_day, "FREQ=DAILY;UNTIL=20260105", 5),
            (&all_day, "FREQ=DAILY;UNTIL=20260105T000000Z", 5),
            (&floating, "FREQ=DAILY;UNTIL=20260105T090000", 5),
            (&floating, "FREQ=DAILY;UNTIL=20260105T085959", 4),
            (&timed, "until=20260105;FREQ=DAILY", 5),
            (&timed, "FREQ=DAILY;UNTIL=20260105T090000", 5),
            (&timed, "FREQ=DAILY;UNTIL=20260105T080000Z", 5),
            (&timed, "FREQ=DAILY;UNTIL=20260105T075959Z", 4),
        ] {
            assert_eq!(
                run(&series(timing.clone(), rule), window).len(),
                expected,
                "{rule}"
            );
        }
    }
}
