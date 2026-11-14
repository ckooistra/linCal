//! Reading dates, times and lengths typed by a person, as in
//! `ucal add "Dentist" fri 14:00 --for 45m`. Pure functions: "today" is
//! passed in, so results never depend on when the code runs.

use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Weekday};

use crate::error::CoreError;

/// Longest event length accepted from input.
const MAX_LENGTH: Duration = Duration::days(366);

fn input_error(message: String) -> CoreError {
    CoreError::Input(message)
}

/// A date: `today`, `tomorrow`, `yesterday`, a weekday (`mon`, `friday`:
/// the next one, today included) or `YYYY-MM-DD`.
pub fn parse_date(text: &str, today: NaiveDate) -> Result<NaiveDate, CoreError> {
    let word = text.trim().to_lowercase();
    match word.as_str() {
        "today" => return Ok(today),
        "tomorrow" => return Ok(today + Duration::days(1)),
        "yesterday" => return Ok(today - Duration::days(1)),
        _ => {}
    }
    if let Some(day) = weekday(&word) {
        let ahead = (7 + day.num_days_from_monday() - today.weekday().num_days_from_monday()) % 7;
        return Ok(today + Duration::days(i64::from(ahead)));
    }
    NaiveDate::parse_from_str(&word, "%Y-%m-%d").map_err(|_| {
        input_error(format!(
            "“{text}” is not a date; use today, tomorrow, a weekday such as fri, or 2026-12-24"
        ))
    })
}

fn weekday(word: &str) -> Option<Weekday> {
    const DAYS: [(&str, Weekday); 7] = [
        ("monday", Weekday::Mon),
        ("tuesday", Weekday::Tue),
        ("wednesday", Weekday::Wed),
        ("thursday", Weekday::Thu),
        ("friday", Weekday::Fri),
        ("saturday", Weekday::Sat),
        ("sunday", Weekday::Sun),
    ];
    // "mon", "tues", "thurs" and the full name all count.
    DAYS.iter()
        .find(|(name, _)| word.len() >= 3 && name.starts_with(word))
        .map(|&(_, day)| day)
}

/// A time of day: `14:00`, `9:30`, `2pm`, `2:30pm` or `12am`.
pub fn parse_time(text: &str) -> Result<NaiveTime, CoreError> {
    let error = || input_error(format!("“{text}” is not a time; use 14:30, 9:00 or 2:30pm"));
    let lower = text.trim().to_lowercase();
    let (clock, half) = match (lower.strip_suffix("am"), lower.strip_suffix("pm")) {
        (Some(clock), _) => (clock.trim_end(), Some(0)),
        (_, Some(clock)) => (clock.trim_end(), Some(12)),
        _ => (lower.as_str(), None),
    };
    let (hour, minute) = match clock.split_once(':') {
        Some((h, m)) if m.len() == 2 => (h, m),
        // Bare hours only with am/pm: "9" alone is too easy to misread.
        None if half.is_some() => (clock, "00"),
        _ => return Err(error()),
    };
    let digits = |s: &str| !s.is_empty() && s.len() <= 2 && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(hour) || !digits(minute) {
        return Err(error());
    }
    let (mut hour, minute): (u32, u32) = (
        hour.parse().map_err(|_| error())?,
        minute.parse().map_err(|_| error())?,
    );
    if let Some(offset) = half {
        if !(1..=12).contains(&hour) {
            return Err(error());
        }
        hour = hour % 12 + offset;
    }
    NaiveTime::from_hms_opt(hour, minute, 0).ok_or_else(error)
}

/// A start time, or a start and end: `14:00` or `14:00-15:30`. An end
/// before the start means the next day (`22:00-01:00`).
pub fn parse_time_span(text: &str) -> Result<(NaiveTime, Option<Duration>), CoreError> {
    let Some((start, end)) = text.split_once(['-', '–']) else {
        return Ok((parse_time(text)?, None));
    };
    let (start, end) = (parse_time(start)?, parse_time(end)?);
    let mut length = end - start;
    if length < Duration::zero() {
        length += Duration::days(1);
    }
    if length.is_zero() {
        return Err(input_error(format!("“{text}” ends when it starts")));
    }
    Ok((start, Some(length)))
}

/// A length: `45m`, `1h`, `1h30m`, `2d`. Days, hours and minutes, in
/// that order, each at most once; up to a year.
pub fn parse_length(text: &str) -> Result<Duration, CoreError> {
    let error = || input_error(format!("“{text}” is not a length; use 45m, 1h30m or 2d"));
    let lower = text.trim().to_lowercase();
    let mut rest = lower.as_str();
    let mut total = Duration::zero();
    let mut units = ['d', 'h', 'm'].as_slice();
    if rest.is_empty() {
        return Err(error());
    }
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let unit = rest[digits..].chars().next().ok_or_else(error)?;
        let position = units.iter().position(|&u| u == unit).ok_or_else(error)?;
        if digits == 0 || digits > 6 {
            return Err(error());
        }
        let n: i64 = rest[..digits].parse().map_err(|_| error())?;
        total += match unit {
            'd' => Duration::days(n),
            'h' => Duration::hours(n),
            _ => Duration::minutes(n),
        };
        units = &units[position + 1..];
        rest = &rest[digits + 1..];
    }
    if total <= Duration::zero() || total > MAX_LENGTH {
        return Err(error());
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn time(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn dates_in_words_count_from_today() {
        let today = date("2026-11-14"); // a Saturday
        assert_eq!(parse_date("today", today), Ok(today));
        assert_eq!(parse_date("Tomorrow", today), Ok(date("2026-11-15")));
        assert_eq!(parse_date("yesterday", today), Ok(date("2026-11-13")));
        assert_eq!(parse_date("mon", today), Ok(date("2026-11-16")));
        assert_eq!(parse_date("friday", today), Ok(date("2026-11-20")));
        assert_eq!(parse_date("thurs", today), Ok(date("2026-11-19")));
        assert_eq!(
            parse_date("sat", today),
            Ok(today),
            "today's weekday is today"
        );
        assert_eq!(parse_date("2026-12-24", today), Ok(date("2026-12-24")));
    }

    #[test]
    fn unclear_dates_are_refused() {
        let today = date("2026-11-14");
        for text in ["mo", "", "2026-02-30", "24/12/2026", "fridays"] {
            assert!(parse_date(text, today).is_err(), "{text}");
        }
    }

    #[test]
    fn times_in_24_hour_and_12_hour_form() {
        assert_eq!(parse_time("14:00"), Ok(time(14, 0)));
        assert_eq!(parse_time("9:05"), Ok(time(9, 5)));
        assert_eq!(parse_time("2pm"), Ok(time(14, 0)));
        assert_eq!(parse_time("2:30 PM"), Ok(time(14, 30)));
        assert_eq!(parse_time("12am"), Ok(time(0, 0)));
        assert_eq!(parse_time("12pm"), Ok(time(12, 0)));
        assert_eq!(parse_time("0:00"), Ok(time(0, 0)));
        for text in ["9", "24:00", "13pm", "0am", "9:5", "9:60", "noon", "1:2:3"] {
            assert!(parse_time(text).is_err(), "{text}");
        }
    }

    #[test]
    fn time_spans_may_cross_midnight() {
        assert_eq!(parse_time_span("10:00"), Ok((time(10, 0), None)));
        assert_eq!(
            parse_time_span("14:00-15:30"),
            Ok((time(14, 0), Some(Duration::minutes(90))))
        );
        assert_eq!(
            parse_time_span("22:00–1am"),
            Ok((time(22, 0), Some(Duration::hours(3))))
        );
        assert!(parse_time_span("10:00-10:00").is_err());
    }

    #[test]
    fn lengths_in_days_hours_and_minutes() {
        assert_eq!(parse_length("45m"), Ok(Duration::minutes(45)));
        assert_eq!(parse_length("1H30m"), Ok(Duration::minutes(90)));
        assert_eq!(parse_length("2d"), Ok(Duration::days(2)));
        assert_eq!(parse_length("1d2h"), Ok(Duration::hours(26)));
        for text in ["", "0m", "h", "30", "1m1h", "1h1h", "400d", "1.5h", "-1h"] {
            assert!(parse_length(text).is_err(), "{text}");
        }
    }
}
