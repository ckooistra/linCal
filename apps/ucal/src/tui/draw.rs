//! Drawing the state with ratatui. Reads [`State`], changes nothing.

use calendar_core::{EventTiming, Occurrence};
use chrono::{Datelike, Duration, NaiveDate};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};

use super::state::{AGENDA_DAYS, Action, CALENDAR, Choice, FIELDS, Form, Overlay, State, View};
use crate::show;

const SELECTED: Style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
const HIGHLIGHT: Style = Style::new().add_modifier(Modifier::REVERSED);
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);

pub fn draw(frame: &mut Frame, state: &State) {
    let [top, main, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    top_bar(frame, state, top);
    if state.search.is_some() || state.view == View::Agenda {
        list(frame, state, main);
    } else {
        match state.view {
            View::Month => month(frame, state, main),
            View::Week => week(frame, state, main),
            _ => day(frame, state, main),
        }
    }
    status_bar(frame, state, bottom);
    match &state.overlay {
        Overlay::None => {}
        Overlay::Details(event) => details(frame, state, event),
        Overlay::Help => help(frame),
        Overlay::Form(form) => form_popup(frame, state, form),
        Overlay::Choose(choice) => choose(frame, choice),
    }
}

/// A dot in the calendar's colour.
fn dot(state: &State, calendar_id: &str) -> Span<'static> {
    let color = state
        .calendar(calendar_id)
        .and_then(|c| u32::from_str_radix(c.color.strip_prefix('#')?, 16).ok())
        .map(|rgb| Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8));
    match color {
        Some(color) if state.colors => Span::styled("●", Style::new().fg(color)),
        _ => Span::raw("●"),
    }
}

fn title(o: &Occurrence) -> &str {
    if o.title.trim().is_empty() {
        "(No title)"
    } else {
        o.title.trim()
    }
}

/// The start of a label, for narrow cells: `09:30`, `→` or nothing (all day).
fn short(label: &str) -> &str {
    if label == "all day" {
        ""
    } else {
        label.split(['–', ' ']).next().unwrap_or(label)
    }
}

/// `● 09:30 Standup`, for month and week cells.
fn compact_line<'a>(state: &State, o: &'a Occurrence, label: &'a str) -> Line<'a> {
    let mut spans = vec![dot(state, &o.calendar_id), Span::raw(" ")];
    if !short(label).is_empty() {
        spans.push(Span::styled(short(label), DIM));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::raw(title(o)));
    Line::from(spans)
}

/// `● 09:30–10:00  Standup · Room 3  (Work)`, for the day view and lists.
fn full_line<'a>(state: &'a State, o: &'a Occurrence, label: &str) -> Line<'a> {
    let mut spans = vec![
        dot(state, &o.calendar_id),
        Span::raw(format!(" {label:<11}  ")),
        Span::raw(title(o)),
    ];
    if !o.location.trim().is_empty() {
        spans.push(Span::styled(format!(" · {}", o.location.trim()), DIM));
    }
    if let Some(c) = state.calendar(&o.calendar_id) {
        spans.push(Span::styled(format!("  ({})", c.name), DIM));
    }
    if let Some(note) = show::status_note(o.sync_status) {
        spans.push(Span::raw(format!("  {note}")));
    }
    Line::from(spans)
}

fn heading(state: &State) -> String {
    if let Some(search) = &state.search {
        return match &search.results {
            Some(found) if !search.editing => {
                format!("Search: “{}” · {} found", search.query, found.len())
            }
            _ => "Search".to_owned(),
        };
    }
    let (start, end) = state.range();
    let last = end - Duration::days(1);
    match state.view {
        View::Month => state.day.format("%B %Y").to_string(),
        View::Week => {
            let first = if start.month() == last.month() {
                start.format("%-d")
            } else {
                start.format("%-d %b")
            };
            format!(
                "{first} – {} · week {}",
                last.format("%-d %b %Y"),
                state.day.iso_week().week()
            )
        }
        View::Day => state.day.format("%A %-d %B %Y").to_string(),
        View::Agenda => format!(
            "{} – {}",
            start.format("%a %-d %b"),
            last.format("%a %-d %b %Y")
        ),
    }
}

/// `left` at the start of a one-line `area` and, if there is room for
/// both, `right` at its end.
fn sides(frame: &mut Frame, area: Rect, left: Line, right: Line) {
    let fits = left.width() + right.width() < usize::from(area.width);
    frame.render_widget(Paragraph::new(left), area);
    if fits {
        frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), area);
    }
}

fn top_bar(frame: &mut Frame, state: &State, area: Rect) {
    let left = Line::from(vec![
        Span::styled(" Universal Calendar  ", DIM),
        Span::styled(heading(state), BOLD),
    ]);
    let tabs: Vec<Span> = [
        (View::Month, "Month"),
        (View::Week, "Week"),
        (View::Day, "Day"),
        (View::Agenda, "Agenda"),
    ]
    .into_iter()
    .flat_map(|(view, name)| {
        let style = if view == state.view && state.search.is_none() {
            SELECTED
        } else {
            DIM
        };
        [
            Span::styled(format!("[{}]{}", &name[..1], &name[1..]), style),
            Span::raw(" "),
        ]
    })
    .collect();
    sides(frame, area, left, Line::from(tabs));
}

fn status_bar(frame: &mut Frame, state: &State, area: Rect) {
    if let Some(search) = state.search.as_ref().filter(|s| s.editing) {
        let text = Line::from(format!(" Search: {}", search.query));
        let width = text.width() as u16;
        sides(
            frame,
            area,
            text,
            Line::styled("Enter search · Esc cancel ", DIM),
        );
        frame.set_cursor_position(Position::new(area.x + width.min(area.width), area.y));
        return;
    }
    let left = if let Some(message) = &state.message {
        Line::from(format!(" {message}"))
    } else if !state.syncing.is_empty() {
        Line::from(" ⟳ Syncing…")
    } else if state.attention > 0 {
        let who = if state.attention == 1 {
            "An account needs"
        } else {
            "Some accounts need"
        };
        Line::styled(
            format!(" ⚠ {who} attention: run `ucal calendars`"),
            Style::new().fg(Color::Yellow),
        )
    } else {
        Line::from("")
    };
    let hints = Line::styled(
        "n new  e edit  x delete  / search  s sync  ? help  q quit ",
        DIM,
    );
    sides(frame, area, left, hints);
}

fn month(frame: &mut Frame, state: &State, area: Rect) {
    let (start, _) = state.range();
    let mut rows = vec![Constraint::Length(1)];
    rows.extend([Constraint::Fill(1); 6]);
    let rows = Layout::vertical(rows).split(area);
    let columns = Layout::horizontal([Constraint::Ratio(1, 7); 7]);
    for (i, cell) in columns.split(rows[0]).iter().enumerate() {
        let name = (start + Duration::days(i as i64)).format("%a").to_string();
        frame.render_widget(
            Paragraph::new(Span::styled(name, DIM)).alignment(Alignment::Center),
            *cell,
        );
    }
    for week in 0..6 {
        for (i, cell) in columns.split(rows[week + 1]).iter().enumerate() {
            let day = start + Duration::days((week * 7 + i) as i64);
            month_cell(frame, state, day, *cell);
        }
    }
}

fn month_cell(frame: &mut Frame, state: &State, day: NaiveDate, area: Rect) {
    let mut number = Span::raw(format!(" {} ", day.day()));
    if day == state.today {
        number = number.style(HIGHLIGHT);
    }
    let mut block = Block::bordered().title(number);
    if day == state.day {
        block = block.border_style(SELECTED);
    } else if day.month() != state.day.month() {
        block = block.border_style(DIM).title_style(DIM);
    }
    let inner = block.inner(area);
    let found = state.on_day(day);
    let room = usize::from(inner.height);
    let mut lines: Vec<Line> = found
        .iter()
        .map(|(o, label)| compact_line(state, o, label))
        .collect();
    if lines.len() > room && room > 0 {
        let hidden = lines.len() - (room - 1);
        lines.truncate(room - 1);
        lines.push(Line::styled(format!("+{hidden} more"), DIM));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn week(frame: &mut Frame, state: &State, area: Rect) {
    let (start, _) = state.range();
    let columns = Layout::horizontal([Constraint::Ratio(1, 7); 7]).split(area);
    for (i, cell) in columns.iter().enumerate() {
        let day = start + Duration::days(i as i64);
        let mut name = Span::raw(day.format(" %a %-d ").to_string());
        if day == state.today {
            name = name.style(HIGHLIGHT);
        }
        let mut block = Block::bordered().title(name);
        if day == state.day {
            block = block.border_style(SELECTED);
        }
        let found = state.on_day(day);
        let lines: Vec<Line> = found
            .iter()
            .enumerate()
            .map(|(n, (o, label))| {
                let line = compact_line(state, o, label);
                if day == state.day && n == state.selected {
                    line.style(HIGHLIGHT)
                } else {
                    line
                }
            })
            .collect();
        frame.render_widget(Paragraph::new(lines).block(block), *cell);
    }
}

fn day(frame: &mut Frame, state: &State, area: Rect) {
    let block = Block::bordered().title(show::day_heading(
        state.day,
        state.today,
        show::Style { color: false },
    ));
    let found = state.on_day(state.day);
    if found.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled("No events. Press n to add one.", DIM)).block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = found
        .iter()
        .map(|(o, label)| ListItem::new(full_line(state, o, label)))
        .collect();
    let mut list_state = ListState::default().with_selected(Some(state.selected));
    frame.render_stateful_widget(
        List::new(items).block(block).highlight_style(HIGHLIGHT),
        area,
        &mut list_state,
    );
}

/// The agenda, or search results: a heading per day, then its events.
fn list(frame: &mut Frame, state: &State, area: Rect) {
    let block = Block::bordered();
    let rows = state.list();
    let empty = match &state.search {
        Some(search) if search.results.is_none() && !search.editing => {
            Some("Searching…".to_owned())
        }
        Some(search) if search.results.is_none() => {
            Some("Type words to find, then Enter.".to_owned())
        }
        Some(search) if rows.is_empty() => Some(format!("Nothing found for “{}”.", search.query)),
        None if rows.is_empty() => Some(format!(
            "No events in these {AGENDA_DAYS} days. Page Down shows the next ones."
        )),
        _ => None,
    };
    if let Some(text) = empty {
        frame.render_widget(Paragraph::new(Line::styled(text, DIM)).block(block), area);
        return;
    }
    let selected = state.search.as_ref().map_or(state.selected, |s| s.selected);
    let mut items = Vec::new();
    let mut selected_item = 0;
    let mut last_day = None;
    for (n, (day, o, label)) in rows.iter().enumerate() {
        if last_day != Some(*day) {
            if last_day.is_some() {
                items.push(ListItem::new(""));
            }
            items.push(ListItem::new(Line::styled(
                show::day_heading(*day, state.today, show::Style { color: false }),
                BOLD,
            )));
            last_day = Some(*day);
        }
        if n == selected {
            selected_item = items.len();
        }
        items.push(ListItem::new(full_line(state, o, label)));
    }
    let mut list_state = ListState::default().with_selected(Some(selected_item));
    frame.render_stateful_widget(
        List::new(items).block(block).highlight_style(HIGHLIGHT),
        area,
        &mut list_state,
    );
}

/// A box of `width` × `height` in the middle of the screen (smaller if the
/// screen is).
fn popup(frame: &mut Frame, width: u16, height: u16, title: &str) -> (Rect, Block<'static>) {
    let area = frame.area();
    let (width, height) = (width.min(area.width), height.min(area.height));
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    (
        rect,
        Block::bordered()
            .title(format!(" {title} "))
            .border_style(SELECTED),
    )
}

/// "Sat 14 Nov 2026, 09:30–10:00", "Sat 14 – Mon 16 Nov 2026, all day", …
fn when(o: &Occurrence, state: &State) -> String {
    let tz = &state.tz;
    if let EventTiming::AllDay {
        start_date,
        end_date,
    } = o.timing
    {
        let last = end_date - Duration::days(1);
        return if last <= start_date {
            format!("{}, all day", start_date.format("%a %-d %b %Y"))
        } else {
            format!(
                "{} – {}, all day",
                start_date.format("%a %-d %b"),
                last.format("%a %-d %b %Y")
            )
        };
    }
    let bounds = o.timing.bounds(tz);
    let (start, end) = (
        bounds.start.with_timezone(tz).naive_local(),
        bounds.end.with_timezone(tz).naive_local(),
    );
    if start.date() == end.date()
        || end
            == (start.date() + Duration::days(1))
                .and_hms_opt(0, 0, 0)
                .unwrap_or(end)
    {
        format!(
            "{}, {}–{}",
            start.format("%a %-d %b %Y"),
            start.format("%H:%M"),
            end.format("%H:%M")
        )
    } else {
        format!(
            "{} – {}",
            start.format("%a %-d %b %H:%M"),
            end.format("%a %-d %b %Y %H:%M")
        )
    }
}

fn minutes(m: u32) -> String {
    match m {
        0 => "at the start".to_owned(),
        m if m % 1440 == 0 => format!("{} d", m / 1440),
        m if m % 60 == 0 => format!("{} h", m / 60),
        m => format!("{m} min"),
    }
}

fn details(frame: &mut Frame, state: &State, o: &Occurrence) {
    let (rect, block) = popup(frame, 70, 16, "Event");
    let label = |name: &str| Span::styled(format!("{name:<11}"), DIM);
    let mut lines = vec![
        Line::styled(title(o).to_owned(), BOLD),
        Line::from(""),
        Line::from(vec![label("When"), Span::raw(when(o, state))]),
    ];
    if let Some(rule) = &o.rrule {
        lines.push(Line::from(vec![
            label("Repeats"),
            Span::raw(show::repeat_text(rule)),
        ]));
    }
    if let Some(c) = state.calendar(&o.calendar_id) {
        lines.push(Line::from(vec![
            label("Calendar"),
            dot(state, &c.id),
            Span::raw(format!(" {}", c.name)),
        ]));
    }
    if !o.location.trim().is_empty() {
        lines.push(Line::from(vec![
            label("Where"),
            Span::raw(o.location.trim().to_owned()),
        ]));
    }
    if !o.reminders.is_empty() {
        let list: Vec<_> = o.reminders.iter().map(|&m| minutes(m)).collect();
        let text = if o.reminders == [0] {
            list.join(", ")
        } else {
            format!("{} before", list.join(", "))
        };
        lines.push(Line::from(vec![label("Reminders"), Span::raw(text)]));
    }
    if let Some(note) = show::status_note(o.sync_status) {
        lines.push(Line::from(vec![label("Sync"), Span::raw(note)]));
    }
    if !o.description.trim().is_empty() {
        lines.push(Line::from(""));
        lines.extend(
            o.description
                .trim()
                .lines()
                .map(|l| Line::from(l.to_owned())),
        );
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(block.title_bottom(Line::styled(" e edit · x delete · Esc close ", DIM)))
            .wrap(Wrap { trim: false }),
        rect,
    );
}

const KEYS: [(&str, &str); 15] = [
    ("m  w  d  a", "month, week, day, agenda"),
    ("t", "today"),
    ("← →  h l", "previous or next day"),
    ("↑ ↓  k j", "previous or next event (a week in the month)"),
    (
        "PgUp PgDn  [ ]",
        "previous or next month, week, day or page",
    ),
    ("Enter", "open the day (month) or the event"),
    ("n", "new event"),
    ("e", "change the selected event"),
    ("x  Delete", "delete the selected event"),
    ("/  Ctrl+F", "search"),
    ("s", "sync now"),
    ("r", "read again from the database"),
    ("Esc", "close, or leave the search"),
    ("?", "these keys"),
    ("q", "quit"),
];

fn help(frame: &mut Frame) {
    let (rect, block) = popup(frame, 70, KEYS.len() as u16 + 3, "Keys");
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::styled(format!(" {key:<16}"), BOLD),
                Span::raw(*what),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(block.title_bottom(Line::styled(" Esc close ", DIM))),
        rect,
    );
}

fn form_popup(frame: &mut Frame, state: &State, form: &Form) {
    let name = if form.editing.is_some() {
        "Change event"
    } else {
        "New event"
    };
    let (rect, block) = popup(frame, 64, 14, name);
    let hints = [
        "",
        "today, fri, 2026-12-24",
        "14:00, 2:30pm, 9-10:30am; empty: all day",
        "45m, 1h30m, 2d",
        "",
        "",
    ];
    let mut lines = Vec::new();
    let mut cursor = None;
    for (i, name) in FIELDS.iter().enumerate() {
        let focused = i == form.focus;
        let value = if i == CALENDAR {
            let writable = state.writable();
            let name = writable.get(form.calendar).map_or("", |c| c.name.as_str());
            let mut spans = vec![Span::raw("‹ ")];
            if let Some(c) = writable.get(form.calendar) {
                spans.push(dot(state, &c.id));
                spans.push(Span::raw(" "));
            }
            spans.push(Span::raw(format!("{name} ›")));
            spans
        } else {
            let text = &form.values[i];
            if focused {
                cursor = Some((
                    rect.x + 1 + 11 + text.chars().count() as u16,
                    rect.y + 1 + i as u16,
                ));
            }
            let mut spans = vec![Span::raw(text.clone())];
            if text.is_empty() && !hints[i].is_empty() {
                spans.push(Span::styled(hints[i], DIM));
            }
            spans
        };
        let mut spans = vec![Span::styled(
            format!("{name:<10} "),
            if focused { SELECTED } else { DIM },
        )];
        spans.extend(value);
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    if form.saving {
        lines.push(Line::from("Saving…"));
    } else if let Some(error) = &form.error {
        lines.push(Line::styled(error.clone(), Style::new().fg(Color::Red)));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(block.title_bottom(Line::styled(
                " Tab next · ←→ calendar · Enter save · Esc cancel ",
                DIM,
            )))
            .wrap(Wrap { trim: false }),
        rect,
    );
    if let Some((x, y)) = cursor
        && x < rect.right()
        && y < rect.bottom()
    {
        frame.set_cursor_position(Position::new(x, y));
    }
}

/// Which occurrences of a repeating event to change or delete; or, for
/// one that doesn't repeat, whether to delete it.
fn choose(frame: &mut Frame, choice: &Choice) {
    let event = title(&choice.event);
    let deleting = matches!(choice.action, Action::Delete);
    let mut lines = vec![Line::styled(event.to_owned(), BOLD), Line::from("")];
    let options: &[(&str, &str)] = match (choice.event.series_id.is_some(), deleting) {
        (true, true) => &[
            ("o", "Delete only this occurrence"),
            ("f", "Delete this and the following ones"),
            ("a", "Delete all occurrences"),
            ("Esc", "Keep it"),
        ],
        (true, false) if choice.moves() => {
            &[("a", "Move all occurrences"), ("Esc", "Back to the form")]
        }
        (true, false) => &[
            ("o", "Change only this occurrence"),
            ("f", "Change this and the following ones"),
            ("a", "Change all occurrences"),
            ("Esc", "Back to the form"),
        ],
        (false, _) => &[("y", "Delete it"), ("Esc", "Keep it")],
    };
    lines.extend(options.iter().map(|(key, what)| {
        Line::from(vec![
            Span::styled(format!(" {key:<5}"), BOLD),
            Span::raw(*what),
        ])
    }));
    if choice.moves() && choice.event.series_id.is_some() {
        lines.push(Line::from(""));
        lines.push(Line::styled("It moves with all its occurrences.", DIM));
    }
    let name = match (choice.event.series_id.is_some(), deleting) {
        (true, true) => "Delete a repeating event",
        (true, false) => "Change a repeating event",
        (false, _) => "Delete event",
    };
    let (rect, block) = popup(frame, 52, lines.len() as u16 + 2, name);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::super::state::Msg;
    use super::*;
    use calendar_core::{Calendar, SyncStatus};
    use chrono::NaiveDateTime;
    use chrono_tz::Tz;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn tz() -> Tz {
        "America/Toronto".parse().unwrap()
    }

    fn event(title: &str, start: &str) -> Occurrence {
        let start = calendar_core::timing::resolve_local(
            &tz(),
            NaiveDateTime::parse_from_str(start, "%Y-%m-%d %H:%M").unwrap(),
        );
        Occurrence {
            id: title.into(),
            event_id: title.into(),
            series_id: None,
            recurrence_id: None,
            calendar_id: "work".into(),
            title: title.into(),
            description: "Bring the card.".into(),
            location: "Main St".into(),
            timing: EventTiming::Zoned {
                start,
                end: start + Duration::minutes(45),
                time_zone: tz(),
            },
            rrule: None,
            reminders: vec![60],
            sync_status: SyncStatus::PendingSync,
            revision: 1,
        }
    }

    fn state() -> State {
        let today = NaiveDate::from_ymd_opt(2026, 11, 14).unwrap();
        let (mut state, effects) = State::new(tz(), today);
        let generation = effects
            .iter()
            .find_map(|e| match e {
                super::super::state::Effect::Load {
                    generation,
                    start,
                    end,
                } => Some((*generation, *start, *end)),
                _ => None,
            })
            .unwrap();
        state.update(Msg::Loaded {
            generation: generation.0,
            range: (generation.1, generation.2),
            result: Ok(vec![event("Dentist", "2026-11-17 14:00")]),
        });
        state.update(Msg::Calendars {
            calendars: vec![Calendar {
                id: "work".into(),
                account_id: "a".into(),
                name: "Work".into(),
                color: "#1971c2".into(),
                read_only: false,
            }],
            default: None,
        });
        state
    }

    fn screen(state: &State, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, state)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn press(state: &mut State, c: char) {
        state.update(Msg::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )));
    }

    #[test]
    fn every_view_draws_its_events() {
        let mut state = state();
        let month = screen(&state, 120, 40);
        assert!(month.contains("November 2026"), "{month}");
        assert!(month.contains("14:00 Dentist"), "{month}");
        press(&mut state, 'w');
        press(&mut state, ']');
        let week = screen(&state, 120, 30);
        assert!(week.contains("16 – 22 Nov 2026 · week 47"), "{week}");
        assert!(week.contains("Tue 17"));
        assert!(week.contains("14:00 Dentist"));
        press(&mut state, 't');
        press(&mut state, 'a');
        let agenda = screen(&state, 120, 30);
        assert!(agenda.contains("Tue 17 Nov"), "{agenda}");
        assert!(
            agenda.contains("14:00–14:45  Dentist · Main St  (Work)  waiting to upload"),
            "{agenda}"
        );
        // Tiny terminals don't crash.
        for view in ['m', 'w', 'd', 'a'] {
            press(&mut state, view);
            screen(&state, 20, 5);
        }
    }

    #[test]
    fn details_help_and_the_form_draw_over_the_view() {
        let mut state = state();
        press(&mut state, 'a');
        state.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        let details = screen(&state, 100, 30);
        assert!(
            details.contains("Tue 17 Nov 2026, 14:00–14:45"),
            "{details}"
        );
        assert!(details.contains("1 h before"));
        assert!(details.contains("Bring the card."));
        press(&mut state, 'q');
        press(&mut state, '?');
        assert!(screen(&state, 100, 30).contains("new event"));
        press(&mut state, 'q');
        press(&mut state, 'n');
        let form = screen(&state, 100, 30);
        assert!(form.contains("New event"), "{form}");
        assert!(form.contains("‹ ● Work ›"), "{form}");
    }

    #[test]
    fn editing_and_deleting_draw_their_questions() {
        let mut state = state();
        press(&mut state, 'a');
        press(&mut state, 'e');
        let form = screen(&state, 100, 30);
        assert!(form.contains("Change event"), "{form}");
        assert!(form.contains("Dentist") && form.contains("45m"), "{form}");
        state.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        press(&mut state, 'x');
        let ask = screen(&state, 100, 30);
        assert!(
            ask.contains("Delete event") && ask.contains("y    Delete it"),
            "{ask}"
        );
        press(&mut state, 'n');
        // A repeating event asks which occurrences.
        if let Some(event) = state.events.first_mut() {
            event.series_id = Some("s".into());
        }
        press(&mut state, 'x');
        let ask = screen(&state, 100, 30);
        assert!(ask.contains("Delete a repeating event"), "{ask}");
        assert!(
            ask.contains("f    Delete this and the following ones"),
            "{ask}"
        );
    }

    /// Drawing time with the simulated datasets, as the desktop app's
    /// benchmark uses. Run with
    /// `cargo test --release -p ucal -- --ignored --nocapture drawing_time`.
    #[test]
    #[ignore = "benchmark"]
    fn drawing_time() {
        use calendar_core::sim::{self, DatasetOptions};
        use calendar_storage::Store;
        use std::time::Instant;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let anchor = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
        println!("| events | view | occurrences loaded | p50 ms | p95 ms | max ms |");
        println!("|---:|---|---:|---:|---:|---:|");
        for size in [100, 1_000, 5_000] {
            let data = sim::generate(&DatasetOptions {
                seed: sim::DEFAULT_SEED,
                event_count: size,
                anchor,
                time_zone: tz(),
            });
            let (mut state, effects) = State::new(tz(), anchor);
            let Some(super::super::state::Effect::Load {
                generation,
                start,
                end,
            }) = effects
                .into_iter()
                .find(|e| matches!(e, super::super::state::Effect::Load { .. }))
            else {
                panic!("loads")
            };
            let events = runtime.block_on(async {
                let store = Store::open_in_memory().await.unwrap();
                store
                    .replace_all(&data.accounts, &data.calendars, &data.events)
                    .await
                    .unwrap();
                store
                    .query_range(super::super::range(&tz(), start, end), tz())
                    .await
                    .unwrap()
            });
            let loaded = events.len();
            state.update(Msg::Loaded {
                generation,
                range: (start, end),
                result: Ok(events),
            });
            state.update(Msg::Calendars {
                calendars: data.calendars.clone(),
                default: None,
            });
            let mut terminal = Terminal::new(TestBackend::new(200, 60)).unwrap();
            for view in ['m', 'w', 'd', 'a'] {
                press(&mut state, view);
                let mut samples: Vec<f64> = (0..40)
                    .map(|i| {
                        // Move each time, as a person paging through would.
                        press(&mut state, if i % 2 == 0 { 'l' } else { 'h' });
                        let t = Instant::now();
                        terminal.draw(|f| draw(f, &state)).unwrap();
                        t.elapsed().as_secs_f64() * 1e3
                    })
                    .collect();
                samples.sort_by(f64::total_cmp);
                let at = |q: f64| samples[((samples.len() - 1) as f64 * q) as usize];
                println!(
                    "| {size} | {view} | {loaded} | {:.2} | {:.2} | {:.2} |",
                    at(0.5),
                    at(0.95),
                    samples[samples.len() - 1]
                );
            }
        }
    }
}
