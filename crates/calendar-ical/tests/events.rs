use calendar_core::timing::resolve_local;
use calendar_core::{EventTiming, OccurrenceKey, Recurrence};
use calendar_ical::{EventData, parse, read_events, write_events, write_object};
use chrono::{Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};

const BERLIN: chrono_tz::Tz = chrono_tz::Europe::Berlin;

fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(h, min, 0)
        .unwrap()
}

fn berlin(y: i32, m: u32, d: u32, h: u32, min: u32) -> chrono::DateTime<Utc> {
    resolve_local(&BERLIN, local(y, m, d, h, min))
}

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
}

fn fixture() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/nextcloud-recurring.ics"
    ))
    .unwrap()
}

#[test]
fn reads_a_nextcloud_recurring_object() {
    let cal = parse(&fixture()).unwrap();
    let obj = read_events(&cal).unwrap();
    assert_eq!(obj.uid, "3a1f2c9e-standup@example.com");
    let master = obj.master.as_ref().unwrap();
    assert_eq!(master.title, "Team stand-up");
    assert_eq!(master.location, "Room 4.12, 2nd floor");
    assert_eq!(master.description, "Daily sync.\nKeep it short.");
    assert_eq!(
        master.timing,
        EventTiming::Zoned {
            start: berlin(2026, 9, 7, 9, 30),
            end: berlin(2026, 9, 7, 9, 45),
            time_zone: BERLIN
        }
    );
    let rec = master.recurrence.as_ref().unwrap();
    assert_eq!(rec.rrule, "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR");
    // Two EXDATEs plus the cancelled override.
    assert_eq!(
        rec.exdates,
        vec![
            OccurrenceKey::Instant(berlin(2026, 9, 10, 9, 30)),
            OccurrenceKey::Instant(berlin(2026, 9, 11, 9, 30)),
            OccurrenceKey::Instant(berlin(2026, 9, 17, 9, 30)),
        ]
    );
    assert_eq!(obj.overrides.len(), 1);
    assert_eq!(
        obj.overrides[0].recurrence_id,
        Some(OccurrenceKey::Instant(berlin(2026, 9, 15, 9, 30)))
    );
    assert_eq!(obj.overrides[0].title, "Team stand-up (moved)");
    assert!(obj.warnings.is_empty(), "{:?}", obj.warnings);
}

#[test]
fn editing_preserves_unknown_data_and_bumps_sequence() {
    let cal = parse(&fixture()).unwrap();
    let obj = read_events(&cal).unwrap();
    let mut master = obj.master.clone().unwrap();
    master.title = "Daily sync".into();
    let written = write_events(Some(&cal), &obj.uid, Some(&master), &obj.overrides, now());
    let ics = written.to_ics();
    for kept in [
        "ATTENDEE;CN=\"Doe, Jane\";PARTSTAT=ACCEPTED:mailto:jane@example.com",
        "X-NC-CUSTOM;X-PARAM=1:vendor data",
        "BEGIN:VALARM",
        "TRIGGER:-PT10M",
        "BEGIN:VTIMEZONE",
        "CREATED:20260901T080000Z",
        "LOCATION:Room 4.12\\, 2nd floor",
    ] {
        assert!(ics.contains(kept), "lost {kept}");
    }
    assert!(ics.contains("SUMMARY:Daily sync"));
    assert!(ics.contains("SEQUENCE:3"));
    // The override was not touched, so its SEQUENCE stays.
    let reread = read_events(&parse(&ics).unwrap()).unwrap();
    assert_eq!(reread.master.as_ref().unwrap().title, "Daily sync");
    assert_eq!(
        reread.master.as_ref().unwrap().recurrence,
        master.recurrence
    );
    assert_eq!(reread.overrides, obj.overrides);
    // The cancelled instance is now an EXDATE; its separate VEVENT is gone.
    assert_eq!(parse(&ics).unwrap().children("VEVENT").count(), 2);
}

#[test]
fn unchanged_objects_are_written_back_unchanged() {
    let mut cal = parse(&fixture()).unwrap();
    // Without the cancelled instance (which writing folds into EXDATE), a
    // write of unmodified data must reproduce the object exactly.
    cal.components
        .retain(|c| c.property("STATUS").is_none_or(|p| p.value != "CANCELLED"));
    let obj = read_events(&cal).unwrap();
    let written = write_events(
        Some(&cal),
        &obj.uid,
        obj.master.as_ref(),
        &obj.overrides,
        now(),
    );
    let mut expected: Vec<_> = cal.components.clone();
    let mut got: Vec<_> = written.components.clone();
    expected.sort_by_key(|c| c.to_ics());
    got.sort_by_key(|c| c.to_ics());
    assert_eq!(got, expected);
}

#[test]
fn creates_new_objects_that_read_back_identically() {
    let master = EventData {
        title: "Planning; review, notes".into(),
        description: "Line 1\nLine 2".into(),
        location: String::new(),
        timing: EventTiming::Zoned {
            start: berlin(2026, 3, 27, 9, 0),
            end: berlin(2026, 3, 27, 10, 0),
            time_zone: BERLIN,
        },
        recurrence: Some(Recurrence {
            rrule: "FREQ=DAILY;COUNT=5".into(),
            rdates: vec![],
            exdates: vec![OccurrenceKey::Instant(berlin(2026, 3, 29, 9, 0))],
        }),
        recurrence_id: None,
    };
    let moved = EventData {
        title: "Planning (later)".into(),
        description: String::new(),
        location: String::new(),
        timing: EventTiming::Zoned {
            start: berlin(2026, 3, 30, 15, 0),
            end: berlin(2026, 3, 30, 16, 0),
            time_zone: BERLIN,
        },
        recurrence: None,
        recurrence_id: Some(OccurrenceKey::Instant(berlin(2026, 3, 30, 9, 0))),
    };
    let cal = write_events(
        None,
        "new-uid",
        Some(&master),
        std::slice::from_ref(&moved),
        now(),
    );
    let ics = cal.to_ics();
    assert!(ics.contains("DTSTART;TZID=Europe/Berlin:20260327T090000"));
    assert!(ics.contains("EXDATE;TZID=Europe/Berlin:20260329T090000"));
    assert!(ics.contains("RECURRENCE-ID;TZID=Europe/Berlin:20260330T090000"));
    assert!(ics.contains("PRODID:-//Universal Calendar"));
    let obj = read_events(&parse(&ics).unwrap()).unwrap();
    assert_eq!(obj.uid, "new-uid");
    assert_eq!(obj.master.as_ref(), Some(&master));
    assert_eq!(obj.overrides, vec![moved]);
}

#[test]
fn removed_overrides_disappear_from_the_object() {
    let cal = parse(&fixture()).unwrap();
    let obj = read_events(&cal).unwrap();
    let written = write_events(Some(&cal), &obj.uid, obj.master.as_ref(), &[], now());
    assert!(read_events(&written).unwrap().overrides.is_empty());
}

#[test]
fn all_day_floating_utc_and_duration_forms() {
    let text = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
        BEGIN:VEVENT\r\nUID:a\r\nDTSTART;VALUE=DATE:20261224\r\nDURATION:P2D\r\nSUMMARY:Holidays\r\nEND:VEVENT\r\n\
        END:VCALENDAR\r\n";
    let obj = read_events(&parse(text).unwrap()).unwrap();
    assert_eq!(
        obj.master.unwrap().timing,
        EventTiming::AllDay {
            start_date: NaiveDate::from_ymd_opt(2026, 12, 24).unwrap(),
            end_date: NaiveDate::from_ymd_opt(2026, 12, 26).unwrap()
        }
    );

    let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:b\r\nDTSTART:20261029T090000\r\nDURATION:PT45M\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let obj = read_events(&parse(text).unwrap()).unwrap();
    assert_eq!(
        obj.master.unwrap().timing,
        EventTiming::Floating {
            start: local(2026, 10, 29, 9, 0),
            end: local(2026, 10, 29, 9, 45)
        }
    );

    let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:c\r\nDTSTART:20261029T080000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let obj = read_events(&parse(text).unwrap()).unwrap();
    let EventTiming::Zoned {
        start,
        end,
        time_zone,
    } = obj.master.unwrap().timing
    else {
        panic!()
    };
    assert_eq!((end - start, time_zone), (Duration::zero(), chrono_tz::UTC));

    // DURATION days are nominal: 1 day across the DST change is 25 hours.
    let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:d\r\nDTSTART;TZID=Europe/Berlin:20261024T120000\r\nDURATION:P1D\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let obj = read_events(&parse(text).unwrap()).unwrap();
    let EventTiming::Zoned { start, end, .. } = obj.master.unwrap().timing else {
        panic!()
    };
    assert_eq!(end - start, Duration::hours(25));
}

#[test]
fn thunderbird_tzids_and_unknown_zones() {
    let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:t\r\nDTSTART;TZID=/mozilla.org/20070129_1/Europe/Berlin:20261029T093000\r\nDTEND;TZID=/mozilla.org/20070129_1/Europe/Berlin:20261029T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let obj = read_events(&parse(text).unwrap()).unwrap();
    assert!(
        matches!(obj.master.unwrap().timing, EventTiming::Zoned { time_zone, .. } if time_zone == BERLIN)
    );

    let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u\r\nDTSTART;TZID=Somewhere Odd:20261029T093000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let obj = read_events(&parse(text).unwrap()).unwrap();
    assert!(matches!(
        obj.master.unwrap().timing,
        EventTiming::Floating { .. }
    ));
    assert_eq!(obj.warnings.len(), 1);
}

#[test]
fn rejects_objects_without_required_fields() {
    assert!(read_events(&parse("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").unwrap()).is_err());
    let no_uid = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:20261029T080000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    assert!(read_events(&parse(no_uid).unwrap()).is_err());
    let cancelled = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x\r\nSTATUS:CANCELLED\r\nDTSTART:20261029T080000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    assert!(
        read_events(&parse(cancelled).unwrap())
            .unwrap()
            .master
            .is_none()
    );
}

// --- audit regressions -------------------------------------------------------

fn vcal(events: &str) -> String {
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//t//EN\r\n{events}END:VCALENDAR\r\n")
}

/// F03: moving a series renames its overrides; their unmodelled data stays.
#[test]
fn moved_overrides_keep_their_unmodelled_properties() {
    let text = vcal(concat!(
        "BEGIN:VEVENT\r\nUID:s\r\nDTSTART;TZID=Europe/Berlin:20260601T090000\r\n",
        "DTEND;TZID=Europe/Berlin:20260601T093000\r\nRRULE:FREQ=DAILY;COUNT=5\r\nSUMMARY:Daily\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:s\r\nRECURRENCE-ID;TZID=Europe/Berlin:20260603T090000\r\n",
        "DTSTART;TZID=Europe/Berlin:20260603T110000\r\nDTEND;TZID=Europe/Berlin:20260603T113000\r\n",
        "SUMMARY:Moved\r\nATTENDEE;CN=Bob:mailto:bob@example.com\r\nX-VENDOR:keep\r\n",
        "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT5M\r\nEND:VALARM\r\nEND:VEVENT\r\n",
    ));
    let cal = parse(&text).unwrap();
    let obj = read_events(&cal).unwrap();
    // The whole series moves one hour later, the override with it.
    let shift = Duration::hours(1);
    let mut master = obj.master.clone().unwrap();
    master.timing = master.timing.shifted(shift);
    let mut moved = obj.overrides[0].clone();
    let old_key = moved.recurrence_id.unwrap();
    let new_key = old_key.shifted(shift);
    moved.recurrence_id = Some(new_key);
    let written = write_object(
        Some(&cal),
        &obj.uid,
        Some(&master),
        std::slice::from_ref(&moved),
        &[(new_key, old_key)],
        now(),
    )
    .to_ics();
    for kept in [
        "ATTENDEE;CN=Bob:mailto:bob@example.com",
        "X-VENDOR:keep",
        "TRIGGER:-PT5M",
    ] {
        assert!(written.contains(kept), "{kept} missing:\n{written}");
    }
    assert!(
        written.contains("RECURRENCE-ID;TZID=Europe/Berlin:20260603T100000"),
        "{written}"
    );
    assert_eq!(written.matches("BEGIN:VEVENT").count(), 2);
}

/// F04: a recurrence set given by RDATE alone survives reading and writing.
#[test]
fn rdate_only_recurrence_round_trips() {
    let text = vcal(concat!(
        "BEGIN:VEVENT\r\nUID:r\r\nDTSTART:20260610T090000Z\r\nDTEND:20260610T100000Z\r\n",
        "RDATE:20260611T090000Z,20260501T090000Z\r\nSUMMARY:Extra dates\r\nEND:VEVENT\r\n",
    ));
    let cal = parse(&text).unwrap();
    let obj = read_events(&cal).unwrap();
    let rec = obj.master.as_ref().unwrap().recurrence.as_ref().unwrap();
    assert_eq!(rec.rrule, "");
    assert_eq!(rec.rdates.len(), 2);
    let written = write_events(Some(&cal), &obj.uid, obj.master.as_ref(), &[], now()).to_ics();
    assert!(
        written.contains("RDATE:20260611T090000Z,20260501T090000Z"),
        "{written}"
    );
    assert!(!written.contains("RRULE"), "{written}");
    let mut renamed = obj.master.clone().unwrap();
    renamed.title = "Renamed".into();
    let written = write_events(Some(&cal), &obj.uid, Some(&renamed), &[], now()).to_ics();
    assert!(
        written.contains("RDATE:20260611T090000Z,20260501T090000Z"),
        "{written}"
    );
}

/// F06: absurd values are errors, not panics.
#[test]
fn huge_durations_are_rejected_without_panicking() {
    for duration in [
        "P999999999999D",
        "P9223372036854775807W",
        "PT9223372036854775807S",
    ] {
        let text = vcal(&format!(
            "BEGIN:VEVENT\r\nUID:d\r\nDTSTART:20260610T090000Z\r\nDURATION:{duration}\r\nSUMMARY:x\r\nEND:VEVENT\r\n"
        ));
        let cal = parse(&text).unwrap();
        assert!(read_events(&cal).is_err(), "{duration}");
    }
    let text = vcal(
        "BEGIN:VEVENT\r\nUID:d\r\nDTSTART;VALUE=DATE:99991231\r\nDURATION:P36000D\r\nSUMMARY:x\r\nEND:VEVENT\r\n",
    );
    // Far in the future but representable: read, not a panic.
    let cal = parse(&text).unwrap();
    assert!(read_events(&cal).is_ok());
}

/// F16: a cancelled series master cancels the whole series.
#[test]
fn cancelled_series_yield_no_events() {
    let text = vcal(concat!(
        "BEGIN:VEVENT\r\nUID:c\r\nDTSTART:20260610T090000Z\r\nDTEND:20260610T100000Z\r\n",
        "RRULE:FREQ=DAILY;COUNT=2\r\nSTATUS:CANCELLED\r\nSUMMARY:Off\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:c\r\nRECURRENCE-ID:20260611T090000Z\r\nDTSTART:20260611T120000Z\r\n",
        "DTEND:20260611T130000Z\r\nSUMMARY:Off too\r\nEND:VEVENT\r\n",
    ));
    let obj = read_events(&parse(&text).unwrap()).unwrap();
    assert!(obj.master.is_none());
    assert!(obj.overrides.is_empty());
}

/// F16: approximations are reported, not silent.
#[test]
fn approximations_produce_warnings() {
    let text = vcal(concat!(
        "BEGIN:VEVENT\r\nUID:w\r\nDTSTART:20260601T090000Z\r\nDTEND:20260601T100000Z\r\n",
        "RRULE:FREQ=DAILY;COUNT=9\r\nRDATE;VALUE=PERIOD:20260620T090000Z/PT3H\r\nSUMMARY:x\r\nEND:VEVENT\r\n",
        "BEGIN:VEVENT\r\nUID:w\r\nRECURRENCE-ID;RANGE=THISANDFUTURE:20260605T090000Z\r\n",
        "DTSTART:20260605T110000Z\r\nDTEND:20260605T120000Z\r\nSUMMARY:later\r\nEND:VEVENT\r\n",
    ));
    let obj = read_events(&parse(&text).unwrap()).unwrap();
    assert_eq!(obj.warnings.len(), 2, "{:?}", obj.warnings);
}
