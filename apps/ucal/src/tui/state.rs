//! What the full-screen view shows, and how keys change it.
//!
//! No terminal and no database here. [`State::update`] takes a [`Msg`]
//! (a key, a query result, a sync report) and returns [`Effect`]s, the
//! work to do next, which `tui/mod.rs` carries out in the background. So
//! every key can be tested without a terminal, and nothing here waits.

use std::collections::HashSet;

use calendar_core::{Calendar, CalendarEvent, EventDraft, Occurrence, SyncStatus};
use calendar_storage::AccountStatus;
use calendar_sync::SyncEvent;
use chrono::{Datelike, Duration, Months, NaiveDate};
use chrono_tz::Tz;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::show;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Month,
    Week,
    Day,
    Agenda,
}

/// The agenda lists this many days from its first one.
pub const AGENDA_DAYS: i64 = 28;

/// Something that happened: a key, or the result of earlier work.
#[derive(Debug)]
pub enum Msg {
    Key(KeyEvent),
    Loaded {
        generation: u64,
        range: (NaiveDate, NaiveDate),
        result: Result<Vec<Occurrence>, String>,
    },
    Calendars {
        calendars: Vec<Calendar>,
        /// The desktop app's default calendar for new events.
        default: Option<String>,
    },
    Statuses(Vec<AccountStatus>),
    Created(Result<Box<CalendarEvent>, String>),
    Found {
        generation: u64,
        result: Result<Vec<Occurrence>, String>,
    },
    Sync(SyncEvent),
    /// The terminal was resized: draw again, nothing else.
    Resized,
}

/// Work for the event loop. Results come back as [`Msg`]s.
#[derive(Debug, PartialEq)]
pub enum Effect {
    /// Occurrences on the days `start..end`.
    Load {
        generation: u64,
        start: NaiveDate,
        end: NaiveDate,
    },
    Create(EventDraft),
    Search {
        generation: u64,
        text: String,
    },
    /// "Sync now": waiting retries become due, then a pass runs.
    SyncNow,
    /// A local change: the scheduler uploads it shortly.
    Changed,
    Statuses,
    Calendars,
    Quit,
}

#[derive(Debug)]
pub enum Overlay {
    None,
    Details(Box<Occurrence>),
    Help,
    Form(Form),
}

/// The new-event form.
#[derive(Debug, Default)]
pub struct Form {
    /// Text of each field; the calendar field uses `calendar` instead.
    pub values: [String; FIELDS.len()],
    pub focus: usize,
    /// Index into [`State::writable`].
    pub calendar: usize,
    pub error: Option<String>,
    pub saving: bool,
}

pub const FIELDS: [&str; 6] = ["Title", "Date", "Time", "Length", "Calendar", "Location"];
const TITLE: usize = 0;
const DATE: usize = 1;
const TIME: usize = 2;
const LENGTH: usize = 3;
pub const CALENDAR: usize = 4;
const LOCATION: usize = 5;

#[derive(Debug, Default)]
pub struct Search {
    pub query: String,
    /// Typing the query (as opposed to browsing the results).
    pub editing: bool,
    /// `None` while searching.
    pub results: Option<Vec<Occurrence>>,
    pub selected: usize,
    generation: u64,
}

pub struct State {
    pub tz: Tz,
    pub today: NaiveDate,
    pub view: View,
    /// The selected day.
    pub day: NaiveDate,
    /// First day of the agenda.
    pub agenda_start: NaiveDate,
    /// The selected event among the day's (week, day) or the agenda's.
    pub selected: usize,
    pub calendars: Vec<Calendar>,
    pub default_calendar: Option<String>,
    /// Occurrences on the days `loaded`.
    pub events: Vec<Occurrence>,
    loaded: Option<(NaiveDate, NaiveDate)>,
    requested: Option<(NaiveDate, NaiveDate)>,
    /// Of the newest load; older results are dropped.
    generation: u64,
    pub overlay: Overlay,
    pub search: Option<Search>,
    /// Accounts syncing right now.
    pub syncing: HashSet<String>,
    /// Accounts with a problem to act on.
    pub attention: usize,
    /// One line for the status bar, until the next key.
    pub message: Option<String>,
    /// Calendar colours (off with `NO_COLOR`).
    pub colors: bool,
}

fn monday_of(day: NaiveDate) -> NaiveDate {
    day - Duration::days(i64::from(day.weekday().num_days_from_monday()))
}

fn add_months(day: NaiveDate, n: i32) -> NaiveDate {
    let months = Months::new(n.unsigned_abs());
    let moved = if n < 0 {
        day.checked_sub_months(months)
    } else {
        day.checked_add_months(months)
    };
    moved.unwrap_or(day)
}

fn letter(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(c.to_ascii_lowercase())
        }
        _ => None,
    }
}

impl State {
    /// The view on `today`, and the first work to do.
    pub fn new(tz: Tz, today: NaiveDate) -> (Self, Vec<Effect>) {
        let mut state = Self {
            tz,
            today,
            view: View::Month,
            day: today,
            agenda_start: today,
            selected: 0,
            calendars: Vec::new(),
            default_calendar: None,
            events: Vec::new(),
            loaded: None,
            requested: None,
            generation: 0,
            overlay: Overlay::None,
            search: None,
            syncing: HashSet::new(),
            attention: 0,
            message: None,
            colors: true,
        };
        let mut effects = vec![Effect::Calendars, Effect::Statuses];
        effects.extend(state.ensure_loaded());
        (state, effects)
    }

    /// Days shown by the current view, `start..end`.
    pub fn range(&self) -> (NaiveDate, NaiveDate) {
        match self.view {
            View::Month => {
                let start = monday_of(self.day.with_day(1).unwrap_or(self.day));
                (start, start + Duration::days(42))
            }
            View::Week => {
                let start = monday_of(self.day);
                (start, start + Duration::days(7))
            }
            View::Day => (self.day, self.day + Duration::days(1)),
            View::Agenda => (
                self.agenda_start,
                self.agenda_start + Duration::days(AGENDA_DAYS),
            ),
        }
    }

    /// Loads the visible days, plus as many again on each side so that the
    /// next step back or forward is usually instant. Nothing if they are
    /// loaded or on their way.
    fn ensure_loaded(&mut self) -> Option<Effect> {
        let (start, end) = self.range();
        let covers =
            |r: Option<(NaiveDate, NaiveDate)>| r.is_some_and(|(s, e)| s <= start && end <= e);
        if covers(self.loaded) || covers(self.requested) {
            return None;
        }
        let pad = (end - start).max(Duration::days(7));
        let (start, end) = (start - pad, end + pad);
        self.generation += 1;
        self.requested = Some((start, end));
        Some(Effect::Load {
            generation: self.generation,
            start,
            end,
        })
    }

    /// Loads again, e.g. after a change.
    fn reload(&mut self) -> Option<Effect> {
        self.loaded = None;
        self.requested = None;
        self.ensure_loaded()
    }

    /// Calendars that can take new events.
    pub fn writable(&self) -> Vec<&Calendar> {
        self.calendars.iter().filter(|c| !c.read_only).collect()
    }

    pub fn calendar(&self, id: &str) -> Option<&Calendar> {
        self.calendars.iter().find(|c| c.id == id)
    }

    /// The occurrences on `day` with their labels, in display order.
    pub fn on_day(&self, day: NaiveDate) -> Vec<(&Occurrence, String)> {
        show::on_day(&self.events, day, &self.tz)
    }

    /// The agenda's or the search results' rows: (day, occurrence, label).
    pub fn list(&self) -> Vec<(NaiveDate, &Occurrence, String)> {
        if let Some(search) = &self.search {
            let results = search.results.as_deref().unwrap_or_default();
            return results
                .iter()
                .filter_map(|o| {
                    let day = crate::start_day(&o.timing, &self.tz);
                    Some((day, o, show::day_label(o, day, &self.tz)?))
                })
                .collect();
        }
        (0..AGENDA_DAYS)
            .map(|n| self.agenda_start + Duration::days(n))
            .flat_map(|day| {
                self.on_day(day)
                    .into_iter()
                    .map(move |(o, label)| (day, o, label))
            })
            .collect()
    }

    /// The highlighted event, if the view has one.
    pub fn selected_event(&self) -> Option<&Occurrence> {
        if self.search.is_some() || self.view == View::Agenda {
            let selected = self.search.as_ref().map_or(self.selected, |s| s.selected);
            return self.list().get(selected).map(|(_, o, _)| *o);
        }
        match self.view {
            View::Week | View::Day => self.on_day(self.day).get(self.selected).map(|(o, _)| *o),
            _ => None,
        }
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        match msg {
            Msg::Key(key) => {
                self.message = None;
                self.key(key)
            }
            Msg::Loaded {
                generation,
                range,
                result,
            } => {
                if generation != self.generation {
                    return Vec::new(); // an older request
                }
                self.requested = None;
                match result {
                    Ok(events) => {
                        self.events = events;
                        self.loaded = Some(range);
                    }
                    Err(e) => self.message = Some(format!("Could not load events: {e}")),
                }
                Vec::new()
            }
            Msg::Calendars { calendars, default } => {
                self.calendars = calendars;
                self.default_calendar = default;
                Vec::new()
            }
            Msg::Statuses(statuses) => {
                self.attention = statuses.iter().filter(|s| show::needs_attention(s)).count();
                Vec::new()
            }
            Msg::Created(result) => self.created(result),
            Msg::Found { generation, result } => {
                if let Some(search) = self.search.as_mut().filter(|s| s.generation == generation) {
                    match result {
                        Ok(found) => search.results = Some(found),
                        Err(e) => {
                            search.results = Some(Vec::new());
                            self.message = Some(format!("Search failed: {e}"));
                        }
                    }
                }
                Vec::new()
            }
            Msg::Sync(event) => self.sync_event(event),
            Msg::Resized => Vec::new(),
        }
    }

    fn sync_event(&mut self, event: SyncEvent) -> Vec<Effect> {
        match event {
            SyncEvent::Syncing { account_id } => {
                self.syncing.insert(account_id);
                Vec::new()
            }
            SyncEvent::Synced {
                account_id,
                changed,
            } => {
                self.syncing.remove(&account_id);
                let mut effects = vec![Effect::Statuses];
                if changed {
                    effects.push(Effect::Calendars);
                    effects.extend(self.reload());
                }
                effects
            }
            SyncEvent::Failed { account_id, .. } => {
                self.syncing.remove(&account_id);
                vec![Effect::Statuses]
            }
        }
    }

    fn created(&mut self, result: Result<Box<CalendarEvent>, String>) -> Vec<Effect> {
        let Overlay::Form(form) = &mut self.overlay else {
            return Vec::new();
        };
        match result {
            Err(e) => {
                form.saving = false;
                form.error = Some(e);
                Vec::new()
            }
            Ok(event) => {
                self.overlay = Overlay::None;
                self.day = crate::start_day(&event.timing, &self.tz);
                self.agenda_start = self.day;
                let calendar = self
                    .calendar(&event.calendar_id)
                    .map_or("", |c| c.name.as_str());
                let mut message = format!("Added “{}” to {calendar}.", event.title);
                let mut effects = Vec::new();
                if event.sync_status == SyncStatus::PendingSync {
                    message += " It uploads shortly.";
                    effects.push(Effect::Changed);
                }
                self.message = Some(message);
                self.selected = 0;
                effects.extend(self.reload());
                effects
            }
        }
    }

    fn key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return vec![Effect::Quit];
        }
        match &mut self.overlay {
            Overlay::Form(_) => return self.form_key(key),
            Overlay::Details(_) | Overlay::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter) || letter(&key) == Some('q') {
                    self.overlay = Overlay::None;
                }
                return Vec::new();
            }
            Overlay::None => {}
        }
        if self.search.as_ref().is_some_and(|s| s.editing) {
            return self.search_key(key);
        }
        if (ctrl && key.code == KeyCode::Char('f')) || key.code == KeyCode::Char('/') {
            let query = self.search.take().map(|s| s.query).unwrap_or_default();
            self.search = Some(Search {
                query,
                editing: true,
                ..Search::default()
            });
            return Vec::new();
        }
        let in_list = self.search.is_some() || self.view == View::Agenda;
        match (key.code, letter(&key)) {
            (KeyCode::Esc, _) | (_, Some('q')) => {
                if self.search.take().is_some() {
                    Vec::new()
                } else {
                    vec![Effect::Quit]
                }
            }
            (_, Some('?')) => {
                self.overlay = Overlay::Help;
                Vec::new()
            }
            (_, Some('t')) => {
                self.search = None;
                self.go_to(self.today)
            }
            (_, Some(c @ ('m' | 'w' | 'd' | 'a'))) => {
                self.search = None;
                self.view = match c {
                    'm' => View::Month,
                    'w' => View::Week,
                    'd' => View::Day,
                    _ => View::Agenda,
                };
                self.go_to(self.day)
            }
            (KeyCode::Left, _) | (_, Some('h')) if !in_list => {
                self.go_to(self.day - Duration::days(1))
            }
            (KeyCode::Right, _) | (_, Some('l')) if !in_list => {
                self.go_to(self.day + Duration::days(1))
            }
            (KeyCode::Up, _) | (_, Some('k')) => self.step(-1),
            (KeyCode::Down, _) | (_, Some('j')) => self.step(1),
            (KeyCode::PageUp, _) | (_, Some('[')) => self.period(-1),
            (KeyCode::PageDown, _) | (_, Some(']')) => self.period(1),
            (KeyCode::Enter, _) => {
                if self.view == View::Month && self.search.is_none() {
                    self.view = View::Day;
                    return self.go_to(self.day);
                }
                if let Some(event) = self.selected_event() {
                    self.overlay = Overlay::Details(Box::new(event.clone()));
                }
                Vec::new()
            }
            (_, Some('n')) => {
                self.open_form();
                Vec::new()
            }
            (_, Some('s')) => {
                self.message = Some("Syncing…".to_owned());
                vec![Effect::SyncNow]
            }
            (_, Some('r')) => {
                let mut effects = vec![Effect::Calendars, Effect::Statuses];
                effects.extend(self.reload());
                effects
            }
            _ => Vec::new(),
        }
    }

    /// Selects `day` in the current view and loads what it shows.
    fn go_to(&mut self, day: NaiveDate) -> Vec<Effect> {
        self.day = day;
        let (start, end) = (
            self.agenda_start,
            self.agenda_start + Duration::days(AGENDA_DAYS),
        );
        if self.view != View::Agenda || day < start || day >= end {
            self.agenda_start = day;
        }
        self.selected = 0;
        if self.view == View::Agenda {
            // The first event on or after the day.
            self.selected = self
                .list()
                .iter()
                .position(|(d, _, _)| *d >= day)
                .unwrap_or(0);
        }
        self.ensure_loaded().into_iter().collect()
    }

    /// Up and down: a week in the month view, else the previous or next event.
    fn step(&mut self, n: i64) -> Vec<Effect> {
        if let Some(search) = &mut self.search {
            let count = search.results.as_ref().map_or(0, Vec::len);
            search.selected = search
                .selected
                .saturating_add_signed(n as isize)
                .min(count.saturating_sub(1));
            return Vec::new();
        }
        match self.view {
            View::Month => self.go_to(self.day + Duration::days(7 * n)),
            View::Agenda => {
                let days: Vec<NaiveDate> = self.list().iter().map(|(d, _, _)| *d).collect();
                if days.is_empty() {
                    return Vec::new();
                }
                self.selected = self
                    .selected
                    .saturating_add_signed(n as isize)
                    .min(days.len() - 1);
                self.day = days[self.selected];
                Vec::new()
            }
            View::Week | View::Day => {
                let count = self.on_day(self.day).len();
                self.selected = self
                    .selected
                    .saturating_add_signed(n as isize)
                    .min(count.saturating_sub(1));
                Vec::new()
            }
        }
    }

    /// Page up and down: the previous or next month, week, day or agenda page.
    fn period(&mut self, n: i32) -> Vec<Effect> {
        self.search = None;
        match self.view {
            View::Month => self.go_to(add_months(self.day, n)),
            View::Week => self.go_to(self.day + Duration::days(7 * i64::from(n))),
            View::Day => self.go_to(self.day + Duration::days(i64::from(n))),
            View::Agenda => {
                self.agenda_start += Duration::days(AGENDA_DAYS * i64::from(n));
                self.day = self.agenda_start;
                self.selected = 0;
                self.ensure_loaded().into_iter().collect()
            }
        }
    }

    fn open_form(&mut self) {
        let writable = self.writable();
        if writable.is_empty() {
            self.message = Some("No calendar can take new events.".to_owned());
            return;
        }
        let calendar = self
            .default_calendar
            .as_deref()
            .and_then(|id| writable.iter().position(|c| c.id == id))
            .unwrap_or(0);
        let mut form = Form {
            calendar,
            ..Form::default()
        };
        form.values[DATE] = self.day.format("%Y-%m-%d").to_string();
        self.overlay = Overlay::Form(form);
    }

    fn form_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let calendars = self.writable().len();
        let draft = self.form_draft();
        let Overlay::Form(form) = &mut self.overlay else {
            return Vec::new();
        };
        if form.saving {
            return Vec::new();
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Tab | KeyCode::Down => form.focus = (form.focus + 1) % FIELDS.len(),
            KeyCode::BackTab | KeyCode::Up => {
                form.focus = (form.focus + FIELDS.len() - 1) % FIELDS.len();
            }
            KeyCode::Left | KeyCode::Right if form.focus == CALENDAR && calendars > 0 => {
                let step = if key.code == KeyCode::Left {
                    calendars - 1
                } else {
                    1
                };
                form.calendar = (form.calendar + step) % calendars;
            }
            KeyCode::Enter => match draft {
                Ok(draft) => {
                    form.saving = true;
                    form.error = None;
                    return vec![Effect::Create(draft)];
                }
                Err(e) => form.error = Some(e),
            },
            KeyCode::Backspace if form.focus != CALENDAR => {
                form.values[form.focus].pop();
            }
            KeyCode::Char(c) if form.focus != CALENDAR && !ctrl => form.values[form.focus].push(c),
            _ => {}
        }
        Vec::new()
    }

    /// The event the form describes, or what is wrong with it.
    fn form_draft(&self) -> Result<EventDraft, String> {
        let Overlay::Form(form) = &self.overlay else {
            return Err("no form".to_owned());
        };
        let text = |i: usize| form.values[i].trim();
        if text(TITLE).is_empty() {
            return Err("Give the event a title.".to_owned());
        }
        let optional = |i: usize| Some(text(i)).filter(|t| !t.is_empty());
        let timing = crate::timing(
            self.tz,
            self.today,
            text(DATE),
            optional(TIME),
            optional(LENGTH),
        )
        .map_err(|e| {
            let e = e.to_string();
            // Messages start in lower case for the "ucal: …" prefix.
            let mut chars = e.chars();
            chars
                .next()
                .map_or(e.clone(), |c| c.to_uppercase().chain(chars).collect())
                + "."
        })?;
        let calendar = self
            .writable()
            .get(form.calendar)
            .map(|c| c.id.clone())
            .ok_or("No calendar can take new events.")?;
        Ok(EventDraft {
            calendar_id: calendar,
            title: text(TITLE).to_owned(),
            description: String::new(),
            location: text(LOCATION).to_owned(),
            timing,
            rrule: None,
            reminders: None,
        })
    }

    fn search_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(search) = &mut self.search else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => {
                if search.results.is_some() {
                    search.editing = false;
                } else {
                    self.search = None;
                }
            }
            KeyCode::Enter if !search.query.trim().is_empty() => {
                self.generation += 1;
                search.generation = self.generation;
                search.editing = false;
                search.results = None;
                search.selected = 0;
                return vec![Effect::Search {
                    generation: search.generation,
                    text: search.query.trim().to_owned(),
                }];
            }
            KeyCode::Backspace => {
                search.query.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                search.query.push(c)
            }
            _ => {}
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calendar_core::EventTiming;
    use chrono::NaiveDateTime;

    fn tz() -> Tz {
        "America/Toronto".parse().unwrap()
    }

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn press(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(state: &mut State, text: &str) {
        for c in text.chars() {
            state.update(press(KeyCode::Char(c)));
        }
    }

    fn calendar(id: &str, name: &str, read_only: bool) -> Calendar {
        Calendar {
            id: id.into(),
            account_id: "local".into(),
            name: name.into(),
            color: "#2f9e44".into(),
            read_only,
        }
    }

    fn event(title: &str, start: &str, minutes: i64) -> Occurrence {
        let start = calendar_core::timing::resolve_local(
            &tz(),
            NaiveDateTime::parse_from_str(start, "%Y-%m-%d %H:%M").unwrap(),
        );
        Occurrence {
            id: title.into(),
            event_id: title.into(),
            series_id: None,
            recurrence_id: None,
            calendar_id: "personal".into(),
            title: title.into(),
            description: String::new(),
            location: String::new(),
            timing: EventTiming::Zoned {
                start,
                end: start + Duration::minutes(minutes),
                time_zone: tz(),
            },
            rrule: None,
            reminders: Vec::new(),
            sync_status: SyncStatus::Saved,
            revision: 1,
        }
    }

    /// A state on Saturday 14 November 2026 with its first load answered.
    fn ready(events: Vec<Occurrence>) -> State {
        let (mut state, effects) = State::new(tz(), date("2026-11-14"));
        let Some(Effect::Load {
            generation,
            start,
            end,
        }) = effects
            .into_iter()
            .find(|e| matches!(e, Effect::Load { .. }))
        else {
            panic!("the first view loads");
        };
        state.update(Msg::Loaded {
            generation,
            range: (start, end),
            result: Ok(events),
        });
        state.update(Msg::Calendars {
            calendars: vec![
                calendar("holidays", "Holidays", true),
                calendar("personal", "Personal", false),
                calendar("work", "Work", false),
            ],
            default: Some("work".into()),
        });
        state
    }

    #[test]
    fn the_month_view_loads_six_weeks_and_its_neighbours() {
        let (state, effects) = State::new(tz(), date("2026-11-14"));
        assert_eq!(state.range(), (date("2026-10-26"), date("2026-12-07")));
        assert!(effects.contains(&Effect::Load {
            generation: 1,
            start: date("2026-09-14"),
            end: date("2027-01-18"),
        }));
    }

    #[test]
    fn moving_within_loaded_days_loads_nothing() {
        let mut state = ready(Vec::new());
        assert!(state.update(press(KeyCode::Down)).is_empty());
        assert_eq!(state.day, date("2026-11-21"));
        assert!(state.update(press(KeyCode::Char('w'))).is_empty());
        assert_eq!(state.range(), (date("2026-11-16"), date("2026-11-23")));
        // Months later: a new load.
        for _ in 0..3 {
            state.update(press(KeyCode::Char('m')));
            state.update(press(KeyCode::PageDown));
        }
        assert_eq!(state.day, date("2027-02-21"));
        assert!(state.requested.is_some());
    }

    #[test]
    fn results_of_older_loads_are_dropped() {
        let mut state = ready(vec![event("Old", "2026-11-14 10:00", 30)]);
        let first = state.update(press(KeyCode::Char('r')));
        let second = state.update(press(KeyCode::Char('r')));
        let generation = |effects: &[Effect]| {
            effects.iter().find_map(|e| match e {
                Effect::Load { generation, .. } => Some(*generation),
                _ => None,
            })
        };
        let range = state.requested.unwrap();
        state.update(Msg::Loaded {
            generation: generation(&first).unwrap(),
            range,
            result: Ok(Vec::new()),
        });
        assert_eq!(state.events.len(), 1, "a stale answer changes nothing");
        state.update(Msg::Loaded {
            generation: generation(&second).unwrap(),
            range,
            result: Ok(Vec::new()),
        });
        assert!(state.events.is_empty());
    }

    #[test]
    fn keys_switch_views_and_select_events() {
        let mut state = ready(vec![
            event("Standup", "2026-11-14 09:30", 15),
            event("Lunch", "2026-11-14 12:00", 60),
        ]);
        state.update(press(KeyCode::Char('D')));
        assert_eq!(state.view, View::Day);
        assert_eq!(state.selected_event().unwrap().title, "Standup");
        state.update(press(KeyCode::Down));
        state.update(press(KeyCode::Down));
        assert_eq!(
            state.selected_event().unwrap().title,
            "Lunch",
            "stops at the last"
        );
        state.update(press(KeyCode::Enter));
        assert!(matches!(&state.overlay, Overlay::Details(o) if o.title == "Lunch"));
        assert!(
            state.update(press(KeyCode::Char('q'))).is_empty(),
            "q closes the details"
        );
        assert!(matches!(state.overlay, Overlay::None));

        state.update(press(KeyCode::Right));
        state.update(press(KeyCode::Char('t')));
        assert_eq!(state.day, date("2026-11-14"));
        state.update(press(KeyCode::Char('a')));
        assert_eq!(state.list().len(), 2);
        assert_eq!(state.update(press(KeyCode::Char('q'))), vec![Effect::Quit]);
    }

    #[test]
    fn the_form_creates_an_event_in_the_default_calendar() {
        let mut state = ready(Vec::new());
        state.update(press(KeyCode::Char('n')));
        state.update(press(KeyCode::Enter));
        assert!(
            matches!(&state.overlay, Overlay::Form(f) if f.error.as_deref() == Some("Give the event a title."))
        );
        typed(&mut state, "Dentist");
        state.update(press(KeyCode::Tab)); // date: prefilled with the selected day
        state.update(press(KeyCode::Tab));
        typed(&mut state, "25:00");
        state.update(press(KeyCode::Enter));
        let Overlay::Form(form) = &state.overlay else {
            panic!()
        };
        assert!(
            form.error
                .as_deref()
                .unwrap()
                .starts_with("“25:00” is not a time")
        );
        for _ in 0..5 {
            state.update(press(KeyCode::Backspace));
        }
        typed(&mut state, "2pm");
        state.update(press(KeyCode::Tab));
        typed(&mut state, "45m");
        state.update(press(KeyCode::Tab));
        state.update(press(KeyCode::Left)); // Work → Personal
        let effects = state.update(press(KeyCode::Enter));
        let [Effect::Create(draft)] = effects.as_slice() else {
            panic!("{effects:?}")
        };
        assert_eq!(draft.title, "Dentist");
        assert_eq!(draft.calendar_id, "personal");
        let EventTiming::Zoned { start, end, .. } = draft.timing else {
            panic!()
        };
        assert_eq!(end - start, Duration::minutes(45));
        assert_eq!(
            start.with_timezone(&tz()).naive_local().to_string(),
            "2026-11-14 14:00:00"
        );
    }

    #[test]
    fn a_saved_event_closes_the_form_and_reloads() {
        let mut state = ready(Vec::new());
        state.update(press(KeyCode::Char('n')));
        let event = CalendarEvent {
            id: "e".into(),
            calendar_id: "work".into(),
            title: "Dentist".into(),
            description: String::new(),
            location: String::new(),
            timing: event("x", "2026-11-20 14:00", 30).timing,
            recurrence: None,
            series: None,
            reminders: Vec::new(),
            sync_status: SyncStatus::PendingSync,
            revision: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let effects = state.update(Msg::Created(Ok(Box::new(event))));
        assert!(matches!(state.overlay, Overlay::None));
        assert_eq!(state.day, date("2026-11-20"));
        assert_eq!(
            state.message.as_deref(),
            Some("Added “Dentist” to Work. It uploads shortly.")
        );
        assert!(effects.contains(&Effect::Changed));
        assert!(effects.iter().any(|e| matches!(e, Effect::Load { .. })));
    }

    #[test]
    fn search_runs_on_enter_and_lists_results() {
        let mut state = ready(Vec::new());
        state.update(press(KeyCode::Char('/')));
        typed(&mut state, "dent");
        let effects = state.update(press(KeyCode::Enter));
        let [Effect::Search { generation, text }] = effects.as_slice() else {
            panic!("{effects:?}")
        };
        assert_eq!(text, "dent");
        state.update(Msg::Found {
            generation: *generation,
            result: Ok(vec![event("Dentist", "2026-12-01 09:00", 30)]),
        });
        assert_eq!(state.selected_event().unwrap().title, "Dentist");
        assert!(
            state.update(press(KeyCode::Esc)).is_empty(),
            "Esc leaves the search"
        );
        assert!(state.search.is_none());
    }

    #[test]
    fn sync_reports_update_the_status_and_reload_on_changes() {
        let mut state = ready(Vec::new());
        state.update(Msg::Sync(SyncEvent::Syncing {
            account_id: "a".into(),
        }));
        assert!(state.syncing.contains("a"));
        let effects = state.update(Msg::Sync(SyncEvent::Synced {
            account_id: "a".into(),
            changed: true,
        }));
        assert!(state.syncing.is_empty());
        assert!(effects.contains(&Effect::Statuses));
        assert!(effects.iter().any(|e| matches!(e, Effect::Load { .. })));
    }
}
