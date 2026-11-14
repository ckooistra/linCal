//! Turning occurrences into text. Pure functions returning `String`s, so
//! tests can check the exact output.

use std::collections::HashMap;
use std::fmt::Write;

use calendar_core::{Calendar, EventTiming, Occurrence, SyncStatus};
use calendar_storage::AccountStatus;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

/// Whether to use colours (terminal output only, and not with `NO_COLOR`).
#[derive(Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    pub fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }

    pub fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    /// A dot in the calendar's colour, or nothing without colours.
    pub fn dot(self, hex: &str) -> String {
        let rgb = hex
            .strip_prefix('#')
            .filter(|h| h.len() == 6)
            .and_then(|h| u32::from_str_radix(h, 16).ok());
        match rgb {
            Some(rgb) if self.color => format!(
                "\x1b[38;2;{};{};{}m●\x1b[0m ",
                rgb >> 16,
                (rgb >> 8) & 0xff,
                rgb & 0xff
            ),
            _ => String::new(),
        }
    }
}

/// What to print for `occurrence` on `day`: `09:30–10:00`, `all day`, or
/// for an event over several days `22:00 →`, `all day` and `→ 06:00`.
/// `None` if it does not touch that day.
pub fn day_label(occurrence: &Occurrence, day: NaiveDate, tz: &Tz) -> Option<String> {
    if let EventTiming::AllDay {
        start_date,
        end_date,
    } = occurrence.timing
    {
        return (start_date <= day && day < end_date).then(|| "all day".to_owned());
    }
    let bounds = occurrence.timing.bounds(tz);
    let start = bounds.start.with_timezone(tz).naive_local();
    let end = bounds.end.with_timezone(tz).naive_local();
    let day_start = day.and_hms_opt(0, 0, 0)?;
    let next = day_start + Duration::days(1);
    let touches = start < next && (end > day_start || (start == end && start >= day_start));
    if !touches {
        return None;
    }
    let clock = |t: chrono::NaiveDateTime| t.format("%H:%M").to_string();
    Some(match (start >= day_start, end <= next) {
        (true, true) if start == end => clock(start),
        (true, true) => format!("{}–{}", clock(start), clock(end)),
        (true, false) => format!("{} →", clock(start)),
        (false, true) => format!("→ {}", clock(end)),
        (false, false) => "all day".to_owned(),
    })
}

/// The occurrences on `day` with their labels: all-day ones first, then
/// by start.
pub fn on_day<'a>(
    occurrences: &'a [Occurrence],
    day: NaiveDate,
    tz: &Tz,
) -> Vec<(&'a Occurrence, String)> {
    let mut found: Vec<_> = occurrences
        .iter()
        .filter_map(|o| Some((o, day_label(o, day, tz)?)))
        .collect();
    found.sort_by_key(|(o, label)| (label != "all day", o.timing.bounds(tz).start));
    found
}

/// "Sat 14 Nov", with the year if it isn't this year's, and "today" or
/// "tomorrow" when it is.
pub fn day_heading(day: NaiveDate, today: NaiveDate, style: Style) -> String {
    let mut text = if day.year() == today.year() {
        day.format("%a %-d %b").to_string()
    } else {
        day.format("%a %-d %b %Y").to_string()
    };
    text = style.bold(&text);
    let near = match (day - today).num_days() {
        0 => " · today",
        1 => " · tomorrow",
        -1 => " · yesterday",
        _ => "",
    };
    text + &style.dim(near)
}

pub fn status_note(status: SyncStatus) -> Option<&'static str> {
    match status {
        SyncStatus::PendingSync => Some("waiting to upload"),
        SyncStatus::SyncFailed => Some("⚠ not uploaded: see `ucal calendars`"),
        SyncStatus::Conflict => Some("⚠ conflict: review it in the desktop app"),
        _ => None,
    }
}

/// One line per occurrence, under a heading per day, for each of `days`
/// that has any. Occurrences need not be sorted.
pub fn days(
    days: &[NaiveDate],
    occurrences: &[Occurrence],
    calendars: &HashMap<&str, &Calendar>,
    tz: &Tz,
    today: NaiveDate,
    style: Style,
) -> String {
    let mut out = String::new();
    for &day in days {
        let lines = on_day(occurrences, day, tz);
        if lines.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let _ = writeln!(out, "{}", day_heading(day, today, style));
        for (o, label) in lines {
            let calendar = calendars.get(o.calendar_id.as_str());
            let dot = calendar.map_or(String::new(), |c| style.dot(&c.color));
            let title = if o.title.trim().is_empty() {
                "(No title)"
            } else {
                o.title.as_str()
            };
            let mut line = format!("  {dot}{label:<11}  {title}");
            if !o.location.trim().is_empty() {
                line += &style.dim(&format!(" · {}", o.location.trim()));
            }
            if let Some(c) = calendar {
                line += &style.dim(&format!("  ({})", c.name));
            }
            if let Some(note) = status_note(o.sync_status) {
                line += &format!("  {note}");
            }
            let _ = writeln!(out, "{}", line.trim_end());
        }
    }
    out
}

/// A repeat rule in words, such as "Every 2 weeks on Mon, Wed, 5 times".
/// Rules with parts this doesn't describe are shown as they are.
pub fn repeat_text(rrule: &str) -> String {
    let mut words = None;
    let mut interval = 1;
    let mut days = Vec::new();
    let mut end = String::new();
    for part in rrule.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            return rrule.to_owned();
        };
        match key {
            "FREQ" => {
                words = Some(match value {
                    "DAILY" => ("day", "days"),
                    "WEEKLY" => ("week", "weeks"),
                    "MONTHLY" => ("month", "months"),
                    "YEARLY" => ("year", "years"),
                    _ => return rrule.to_owned(),
                });
            }
            "INTERVAL" => match value.parse::<u32>() {
                Ok(n) if n > 0 => interval = n,
                _ => return rrule.to_owned(),
            },
            "BYDAY" => {
                for day in value.split(',') {
                    let name = match day {
                        "MO" => "Mon",
                        "TU" => "Tue",
                        "WE" => "Wed",
                        "TH" => "Thu",
                        "FR" => "Fri",
                        "SA" => "Sat",
                        "SU" => "Sun",
                        _ => return rrule.to_owned(), // "2TU" and the like
                    };
                    days.push(name);
                }
            }
            "COUNT" => end = format!(", {value} times"),
            "UNTIL" => match NaiveDate::parse_from_str(value.get(..8).unwrap_or(value), "%Y%m%d") {
                Ok(day) => end = format!(", until {}", day.format("%-d %b %Y")),
                Err(_) => return rrule.to_owned(),
            },
            "WKST" => {}
            _ => return rrule.to_owned(),
        }
    }
    let Some((one, many)) = words else {
        return rrule.to_owned();
    };
    let mut text = if interval == 1 {
        format!("Every {one}")
    } else {
        format!("Every {interval} {many}")
    };
    if !days.is_empty() {
        text += &format!(" on {}", days.join(", "));
    }
    text + &end
}

/// Whether an account has a problem the user should act on.
pub fn needs_attention(status: &AccountStatus) -> bool {
    status.needs_sign_in || status.error.is_some() || status.conflicts > 0 || status.failed > 0
}

/// What needs the user's attention about an account, one item per line.
pub fn attention(status: &AccountStatus) -> Vec<String> {
    let mut notes = Vec::new();
    let count = |n: i64, one: &str, many: &str| {
        if n == 1 {
            one.to_owned()
        } else {
            format!("{n} {many}")
        }
    };
    if status.needs_sign_in {
        notes.push("⚠ The sign-in has expired: sign in again in the desktop app.".to_owned());
    } else if let Some(error) = &status.error {
        notes.push(format!("⚠ {error}"));
    }
    if status.conflicts > 0 {
        notes.push(format!(
            "⚠ {} to review in the desktop app.",
            count(status.conflicts, "1 conflict", "conflicts")
        ));
    }
    if status.failed > 0 {
        notes.push(format!(
            "⚠ {} could not be uploaded: review in the desktop app.",
            count(status.failed, "1 change", "changes")
        ));
    }
    if status.import_failures > 0 {
        notes.push(format!(
            "⚠ {} from the server could not be read.",
            count(status.import_failures, "1 event", "events")
        ));
    }
    if status.pending > 0 {
        notes.push(format!(
            "{} waiting to upload.",
            count(status.pending, "1 change", "changes")
        ));
    }
    notes
}

/// "last synced 14:05" or "last synced 3 Nov", in the local time zone.
pub fn last_synced(ms: Option<i64>, today: NaiveDate) -> String {
    match ms.and_then(|ms| Utc.timestamp_millis_opt(ms).single()) {
        None => "not synced yet".to_owned(),
        Some(at) => {
            let at: DateTime<Local> = at.with_timezone(&Local);
            if at.date_naive() == today {
                format!("last synced {}", at.format("%H:%M"))
            } else {
                format!("last synced {}", at.format("%-d %b"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calendar_core::EventTiming;
    use chrono::NaiveDateTime;

    fn tz() -> Tz {
        "Europe/Paris".parse().unwrap()
    }

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn local(s: &str) -> DateTime<Utc> {
        let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        calendar_core::timing::resolve_local(&tz(), naive)
    }

    fn event(title: &str, timing: EventTiming) -> Occurrence {
        Occurrence {
            id: title.to_owned(),
            event_id: title.to_owned(),
            series_id: None,
            recurrence_id: None,
            calendar_id: "work".to_owned(),
            title: title.to_owned(),
            description: String::new(),
            location: String::new(),
            timing,
            rrule: None,
            reminders: Vec::new(),
            sync_status: SyncStatus::Saved,
            revision: 1,
        }
    }

    fn zoned(title: &str, start: &str, end: &str) -> Occurrence {
        event(
            title,
            EventTiming::Zoned {
                start: local(start),
                end: local(end),
                time_zone: tz(),
            },
        )
    }

    #[test]
    fn events_over_several_days_show_on_each() {
        let night = zoned("Night train", "2026-11-14 22:00", "2026-11-16 06:30");
        let label = |d| day_label(&night, date(d), &tz());
        assert_eq!(label("2026-11-13"), None);
        assert_eq!(label("2026-11-14").as_deref(), Some("22:00 →"));
        assert_eq!(label("2026-11-15").as_deref(), Some("all day"));
        assert_eq!(label("2026-11-16").as_deref(), Some("→ 06:30"));
        assert_eq!(label("2026-11-17"), None);

        let late = zoned("Late", "2026-11-14 23:00", "2026-11-15 00:00");
        assert_eq!(
            day_label(&late, date("2026-11-14"), &tz()).as_deref(),
            Some("23:00–00:00")
        );
        assert_eq!(day_label(&late, date("2026-11-15"), &tz()), None);

        let trip = event(
            "Trip",
            EventTiming::AllDay {
                start_date: date("2026-11-14"),
                end_date: date("2026-11-16"),
            },
        );
        assert_eq!(
            day_label(&trip, date("2026-11-15"), &tz()).as_deref(),
            Some("all day")
        );
        assert_eq!(day_label(&trip, date("2026-11-16"), &tz()), None);
    }

    #[test]
    fn the_agenda_lists_days_with_events() {
        let work = Calendar {
            id: "work".into(),
            account_id: "local".into(),
            name: "Work".into(),
            color: "#1971c2".into(),
            read_only: false,
        };
        let calendars = HashMap::from([("work", &work)]);
        let mut standup = zoned("Standup", "2026-11-16 09:30", "2026-11-16 09:45");
        standup.location = "Room 3".into();
        standup.sync_status = SyncStatus::PendingSync;
        let holiday = event(
            "Holiday",
            EventTiming::AllDay {
                start_date: date("2026-11-16"),
                end_date: date("2026-11-17"),
            },
        );
        let today = date("2026-11-14");
        let all_days: Vec<_> = (0..7).map(|n| today + Duration::days(n)).collect();
        let text = days(
            &all_days,
            &[standup, holiday],
            &calendars,
            &tz(),
            today,
            Style { color: false },
        );
        assert_eq!(
            text,
            "Mon 16 Nov\n\
             \x20 all day      Holiday  (Work)\n\
             \x20 09:30–09:45  Standup · Room 3  (Work)  waiting to upload\n"
        );
        let coloured = days(
            &all_days[..3],
            &[zoned("Lunch", "2026-11-15 12:00", "2026-11-15 13:00")],
            &calendars,
            &tz(),
            today,
            Style { color: true },
        );
        assert!(coloured.contains("\x1b[38;2;25;113;194m●"), "{coloured:?}");
        assert!(coloured.contains("tomorrow"));
    }

    #[test]
    fn repeat_rules_in_words() {
        assert_eq!(repeat_text("FREQ=DAILY;COUNT=5"), "Every day, 5 times");
        assert_eq!(
            repeat_text("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;WKST=MO"),
            "Every 2 weeks on Mon, Wed"
        );
        assert_eq!(
            repeat_text("FREQ=YEARLY;UNTIL=20301231T235959Z"),
            "Every year, until 31 Dec 2030"
        );
        for rule in [
            "FREQ=MONTHLY;BYDAY=2TU",
            "FREQ=WEEKLY;BYSETPOS=1",
            "FREQ=HOURLY",
        ] {
            assert_eq!(repeat_text(rule), rule);
        }
    }

    #[test]
    fn accounts_report_what_needs_attention() {
        let status = AccountStatus {
            account_id: "a".into(),
            remote: true,
            last_synced_at: None,
            error: Some("Offline: the server could not be reached.".into()),
            pending: 2,
            failed: 0,
            conflicts: 1,
            import_failures: 0,
            approximated: 0,
            needs_sign_in: false,
        };
        assert_eq!(
            attention(&status),
            [
                "⚠ Offline: the server could not be reached.",
                "⚠ 1 conflict to review in the desktop app.",
                "2 changes waiting to upload.",
            ]
        );
    }
}
