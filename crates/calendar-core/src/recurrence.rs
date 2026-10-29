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

/// Builds the rule set for `timing` + `recurrence`. Floating series are
/// evaluated in `display_tz`; all-day series on UTC midnights (pure dates).
fn prepare(
    timing: &EventTiming,
    recurrence: &Recurrence,
    display_tz: &Tz,
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
    let rule: RRule<Unvalidated> = recurrence.rrule.parse().map_err(invalid)?;
    let rule = rule.validate(dt_start).map_err(invalid)?;
    let set = RRuleSet::new(dt_start)
        .rrule(rule)
        .set_rdates(recurrence.rdates.iter().map(to_set_time).collect())
        .set_exdates(recurrence.exdates.iter().map(to_set_time).collect());
    Ok(Prepared { set, kind })
}

/// Checks that a recurrence is valid for the given timing.
pub fn validate(timing: &EventTiming, recurrence: &Recurrence) -> Result<(), CoreError> {
    prepare(timing, recurrence, &chrono_tz::UTC).map(|_| ())
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
    let prepared = prepare(&master.timing, recurrence, display_tz)?;
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
    let prepared = prepare(timing, recurrence, display_tz)?;
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

/// Result of editing a whole series through one of its occurrences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retimed {
    /// New timing of the series master (its first occurrence).
    pub timing: EventTiming,
    /// How far existing occurrence ids moved; `None` when the timing kind
    /// changed and old EXDATEs/overrides can no longer be mapped.
    pub key_shift: Option<Duration>,
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
            let delta = *new - *original;
            let s = *start + delta;
            Some((
                EventTiming::Zoned {
                    start: s,
                    end: s + (*end - *new),
                    time_zone: *time_zone,
                },
                delta,
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
                delta,
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
                delta,
            ))
        }
        _ => None,
    };
    match shift {
        Some((timing, delta)) => Retimed {
            timing,
            key_shift: Some(delta),
        },
        None => Retimed {
            timing: edited.clone(),
            key_shift: None,
        },
    }
}

/// Zone-independent upper bound of a series for indexing (see
/// [`EventTiming::index_bounds`]): `None` for unbounded or very long series.
pub fn index_end(
    timing: &EventTiming,
    recurrence: &Recurrence,
) -> Result<Option<DateTime<Utc>>, CoreError> {
    let prepared = prepare(timing, recurrence, &chrono_tz::UTC)?;
    let rule = &prepared.set.get_rrule()[0];
    if rule.get_count().is_none() && rule.get_until().is_none() {
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
        assert_eq!(r.key_shift, Some(Duration::hours(1)));
        assert_eq!(
            key.shifted(Duration::hours(1)),
            OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 6, 8, 10, 30)))
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

    #[test]
    fn detects_generated_slots() {
        let e = series(zoned(local(2026, 6, 1, 8, 0), 30), "FREQ=DAILY;COUNT=3");
        let rec = e.recurrence.as_ref().unwrap();
        let slot = |d| OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 6, d, 8, 0)));
        assert!(has_occurrence(&e.timing, rec, &slot(3), &BERLIN).unwrap());
        assert!(!has_occurrence(&e.timing, rec, &slot(4), &BERLIN).unwrap());
    }
}
