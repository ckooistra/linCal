//! `ucal`: Universal Calendar in the terminal.
//!
//! A second front end beside the desktop app, using the same engine
//! crates and the same database. Like the desktop app it holds no calendar
//! logic: each command reads its arguments, makes a few calls into
//! `calendar-storage` or `calendar-sync` and prints the result.

mod calendars;
mod show;
mod tui;

use std::collections::HashMap;
use std::error::Error;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use calendar_core::input::{parse_date, parse_length, parse_time_span};
use calendar_core::timing::{date_start, resolve_local};
use calendar_core::{Account, Calendar, EventDraft, EventTiming, Occurrence, TimeRange};
use calendar_storage::Store;
use calendar_sync::{
    CredentialStore, KeyringCredentials, MemoryCredentials, SyncEngine, SyncEvent, default_factory,
    import_ics,
};
use chrono::{Duration, NaiveDate, Utc};
use chrono_tz::Tz;
use clap::{Parser, Subcommand};

use show::Style;

type Fallible<T> = Result<T, Box<dyn Error>>;

/// Universal Calendar in the terminal. Uses the same calendars and
/// accounts as the desktop app.
#[derive(Parser)]
#[command(name = "ucal", version, after_help = EXAMPLES)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

const EXAMPLES: &str = "Examples:
  ucal                                   the calendar, full screen (? shows the keys)
  ucal agenda                            the next 7 days, printed
  ucal agenda mon --days 5               Monday to Friday
  ucal search dentist
  ucal add \"Dentist\" fri 14:00 --for 45m
  ucal add \"Standup\" tomorrow 9:30-9:45 -c Work --remind 5m
  ucal add \"Holiday\" 2026-12-24 --for 3d
  ucal import team.ics -c Work
  ucal sync";

#[derive(Subcommand)]
enum Command {
    /// Upcoming events, day by day, printed (`ucal` alone opens the full-screen view)
    Agenda {
        /// First day: today, tomorrow, a weekday such as fri, or 2026-12-24
        #[arg(default_value = "today")]
        date: String,
        /// How many days to show
        #[arg(short, long, default_value_t = 7, value_parser = clap::value_parser!(u16).range(1..=366))]
        days: u16,
    },
    /// Find events whose title, location or description contain every word
    Search {
        #[arg(required = true)]
        words: Vec<String>,
    },
    /// Add an event (all day when no time is given)
    Add {
        title: String,
        /// Day: today, tomorrow, a weekday such as fri, or 2026-12-24
        date: String,
        /// Start time, or start and end: 14:00, 2:30pm, 14:00-15:30
        time: Option<String>,
        /// Length: 45m, 1h30m, 2d (default: 1h, or 1 day without a time)
        #[arg(long = "for", value_name = "LENGTH")]
        length: Option<String>,
        /// Calendar name (default: the desktop app's default calendar)
        #[arg(short, long)]
        calendar: Option<String>,
        #[arg(short, long)]
        location: Option<String>,
        /// Description
        #[arg(short, long)]
        note: Option<String>,
        /// Reminder before the start, such as 10m or 1d (can be repeated)
        #[arg(short, long, value_name = "BEFORE")]
        remind: Vec<String>,
    },
    /// Import the events of an .ics file into a calendar
    Import {
        file: PathBuf,
        /// Calendar name (default: the desktop app's default calendar)
        #[arg(short, long)]
        calendar: Option<String>,
    },
    /// List calendars, and accounts that need attention
    Calendars,
    /// Synchronize every account now
    Sync,
}

/// Everything the commands share.
struct App {
    store: Store,
    engine: Arc<SyncEngine>,
    tz: Tz,
    today: NaiveDate,
    style: Style,
    /// What the sync engine reports, in order (see [`SyncEvent`]).
    sync_events: tokio::sync::mpsc::UnboundedReceiver<SyncEvent>,
}

/// Where the desktop app keeps its data (Tauri's app data directory), or
/// `UC_DATA_DIR`.
fn data_dir() -> Fallible<PathBuf> {
    const ID: &str = "org.universalcalendar.desktop";
    if let Some(dir) = std::env::var_os("UC_DATA_DIR") {
        return Ok(dir.into());
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home().map(|h| h.join(".local/share")))
    };
    Ok(base
        .ok_or("cannot find the data directory: set HOME")?
        .join(ID))
}

/// `TZ` if it names a zone, else the system's, else UTC.
fn time_zone() -> Tz {
    std::env::var("TZ")
        .ok()
        .and_then(|name| name.trim_start_matches(':').parse().ok())
        .or_else(|| iana_time_zone::get_timezone().ok()?.parse().ok())
        .unwrap_or(chrono_tz::UTC)
}

/// The desktop app's keyring entries; `UC_CREDENTIALS=memory` for tests.
fn credentials() -> Arc<dyn CredentialStore> {
    if std::env::var("UC_CREDENTIALS").is_ok_and(|v| v == "memory") {
        Arc::new(MemoryCredentials::default())
    } else {
        Arc::new(KeyringCredentials::new("org.universalcalendar.desktop"))
    }
}

impl App {
    async fn open() -> Fallible<Self> {
        let dir = data_dir()?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let store = Store::open(&dir.join("calendar.db")).await?;
        // Somewhere to put events without any account, as in the desktop app.
        store.ensure_local_calendar().await?;
        // The engine reports from its own tasks; whoever runs the command
        // reads the reports in order.
        let (sender, sync_events) = tokio::sync::mpsc::unbounded_channel();
        let listener = Arc::new(move |event: SyncEvent| {
            let _ = sender.send(event);
        });
        // Built for every command: it also tells storage what each provider
        // can hold (e.g. Outlook's single reminder). The lock files keep it
        // and the desktop app from syncing the same account at once.
        let engine = SyncEngine::new(store.clone(), credentials(), default_factory(), listener)
            .with_lock_dir(dir);
        let tz = time_zone();
        let color = std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
            && std::env::var("TERM").is_ok_and(|t| t != "dumb");
        Ok(Self {
            store,
            engine: Arc::new(engine),
            tz,
            today: Utc::now().with_timezone(&tz).date_naive(),
            style: Style { color },
            sync_events,
        })
    }

    async fn calendars(&self) -> Fallible<(Vec<Account>, Vec<Calendar>)> {
        Ok((
            self.store.list_accounts().await?,
            self.store.list_calendars().await?,
        ))
    }

    /// The calendar named by `--calendar`, or the default one.
    async fn target(&self, name: Option<&str>) -> Fallible<Calendar> {
        let (accounts, calendars) = self.calendars().await?;
        let calendar = match name {
            Some(name) => calendars::find(name, &calendars, &accounts)?,
            None => {
                let settings = self.store.settings().await?;
                calendars::default(&calendars, settings.get("defaultCalendar"))?
            }
        };
        if calendar.read_only {
            return Err(format!("The calendar “{}” is read-only.", calendar.name).into());
        }
        Ok(calendar.clone())
    }

    /// One line if any account needs attention, so problems are never hidden.
    async fn attention_hint(&self) -> Fallible<Option<String>> {
        let statuses = self.store.account_statuses().await?;
        let troubled = statuses.iter().filter(|s| show::needs_attention(s)).count();
        Ok((troubled > 0).then(|| {
            let accounts = if troubled == 1 {
                "An account needs"
            } else {
                "Some accounts need"
            };
            format!("⚠ {accounts} attention: see `ucal calendars`.")
        }))
    }

    /// `found` under a heading per day, for each of `days` that has events.
    async fn render(&self, days: &[NaiveDate], found: &[Occurrence]) -> Fallible<String> {
        let calendars = self.store.list_calendars().await?;
        let by_id: HashMap<&str, &Calendar> =
            calendars.iter().map(|c| (c.id.as_str(), c)).collect();
        Ok(show::days(
            days, found, &by_id, &self.tz, self.today, self.style,
        ))
    }

    async fn agenda(&self, date: &str, days: u16) -> Fallible<()> {
        let first = parse_date(date, self.today)?;
        let last = first + Duration::days(i64::from(days));
        let range = TimeRange::new(date_start(&self.tz, first), date_start(&self.tz, last));
        let found = self.store.query_range(range, self.tz).await?;
        if found.is_empty() {
            let span = if days == 1 {
                format!("on {}", first.format("%a %-d %b"))
            } else {
                format!(
                    "from {} to {}",
                    first.format("%a %-d %b"),
                    (last - Duration::days(1)).format("%a %-d %b")
                )
            };
            println!("No events {span}.");
        } else {
            let all: Vec<_> = first.iter_days().take(usize::from(days)).collect();
            print!("{}", self.render(&all, &found).await?);
        }
        if let Some(hint) = self.attention_hint().await? {
            println!("\n{hint}");
        }
        Ok(())
    }

    async fn search(&self, words: &[String]) -> Fallible<()> {
        const LIMIT: usize = 100;
        let text = words.join(" ");
        // A year back and two ahead: what a person is likely to look for.
        let range = TimeRange::new(
            date_start(&self.tz, self.today - Duration::days(366)),
            date_start(&self.tz, self.today + Duration::days(2 * 366)),
        );
        let found = self
            .store
            .search(&text, range, self.tz, LIMIT, false)
            .await?;
        if found.is_empty() {
            println!("Nothing found for “{text}”.");
            return Ok(());
        }
        // Results are sorted by start; each is listed on the day it starts.
        let mut days: Vec<NaiveDate> = found
            .iter()
            .map(|o| start_day(&o.timing, &self.tz))
            .collect();
        days.dedup();
        let mut shown = String::new();
        for day in days {
            let starting: Vec<_> = found
                .iter()
                .filter(|o| start_day(&o.timing, &self.tz) == day)
                .cloned()
                .collect();
            if !shown.is_empty() {
                shown.push('\n');
            }
            shown += &self.render(&[day], &starting).await?;
        }
        print!("{shown}");
        if found.len() == LIMIT {
            println!("\nShowing the first {LIMIT}; add words to narrow the search.");
        }
        Ok(())
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("ucal: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli.command)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("ucal: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(command: Option<Command>) -> Fallible<ExitCode> {
    let mut app = App::open().await?;
    let command = match command {
        Some(command) => command,
        None if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() => {
            tui::run(app).await?;
            return Ok(ExitCode::SUCCESS);
        }
        // Not a terminal (a pipe or a script): print instead.
        None => Command::Agenda {
            date: "today".into(),
            days: 7,
        },
    };
    match command {
        Command::Agenda { date, days } => app.agenda(&date, days).await?,
        Command::Search { words } => app.search(&words).await?,
        Command::Add {
            title,
            date,
            time,
            length,
            calendar,
            location,
            note,
            remind,
        } => {
            let calendar = app.target(calendar.as_deref()).await?;
            let timing = timing(app.tz, app.today, &date, time.as_deref(), length.as_deref())?;
            let reminders = remind
                .iter()
                .map(|r| {
                    let minutes = parse_length(r)?.num_minutes();
                    u32::try_from(minutes).map_err(|_| "reminder too far ahead".into())
                })
                .collect::<Fallible<Vec<u32>>>()?;
            let draft = EventDraft {
                calendar_id: calendar.id.clone(),
                title,
                description: note.unwrap_or_default(),
                location: location.unwrap_or_default(),
                timing,
                rrule: None,
                reminders: Some(calendar_core::normalize_reminders(reminders)),
            };
            let event = app.store.create_event(&new_event_id(), draft).await?;
            println!("Added to {}:", calendar.name);
            let day = start_day(&event.timing, &app.tz);
            print!("{}", app.render(&[day], &[event.as_occurrence()]).await?);
            if event.sync_status == calendar_core::SyncStatus::PendingSync {
                println!(
                    "It uploads on the next sync: run `ucal sync`, or the desktop app does it within 5 minutes."
                );
            }
        }
        Command::Import { file, calendar } => {
            let calendar = app.target(calendar.as_deref()).await?;
            let size = std::fs::metadata(&file)
                .map_err(|e| format!("{}: {e}", file.display()))?
                .len();
            if size > calendar_sync::import::MAX_IMPORT_BYTES as u64 {
                return Err(format!("{} is larger than 20 MB.", file.display()).into());
            }
            let text =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let report = import_ics(&app.store, &calendar.id, text).await?;
            print_import(&report, &calendar.name);
            if report.imported > 0 && calendar.account_id != "local" {
                println!(
                    "They upload on the next sync: run `ucal sync`, or the desktop app does it within 5 minutes."
                );
            }
        }
        Command::Calendars => {
            let (accounts, calendars) = app.calendars().await?;
            let statuses = app.store.account_statuses().await?;
            let settings = app.store.settings().await?;
            let default = calendars::default(&calendars, settings.get("defaultCalendar"))
                .ok()
                .map(|c| c.id.clone());
            for (i, account) in accounts.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                let status = statuses.iter().find(|s| s.account_id == account.id);
                let mut heading = app.style.bold(&account.name);
                if let Some(s) = status.filter(|s| s.remote) {
                    heading += &app.style.dim(&format!(
                        " · {}",
                        show::last_synced(s.last_synced_at, app.today)
                    ));
                }
                println!("{heading}");
                for c in calendars.iter().filter(|c| c.account_id == account.id) {
                    let mut line = format!("  {}{}", app.style.dot(&c.color), c.name);
                    if c.read_only {
                        line += &app.style.dim("  read-only");
                    }
                    if default.as_deref() == Some(c.id.as_str()) {
                        line += &app.style.dim("  default");
                    }
                    println!("{line}");
                }
                for note in status.map(show::attention).unwrap_or_default() {
                    println!("  {note}");
                }
            }
        }
        Command::Sync => {
            let accounts = app.store.remote_accounts().await?;
            if accounts.is_empty() {
                println!("No accounts to sync: add one in the desktop app.");
                return Ok(ExitCode::SUCCESS);
            }
            let n = accounts.len();
            println!("Syncing {n} account{}…", if n == 1 { "" } else { "s" });
            // "Sync now": waiting retries are due immediately.
            app.store.retry_now().await?;
            app.engine.sync_all(true).await?;
            let names: HashMap<String, String> = accounts
                .into_iter()
                .map(|a| (a.account.id, a.account.name))
                .collect();
            let name = |id: &str| names.get(id).map_or(id, String::as_str).to_owned();
            let mut failed = false;
            while let Ok(event) = app.sync_events.try_recv() {
                match event {
                    SyncEvent::Synced { account_id, .. } => println!("✓ {}", name(&account_id)),
                    SyncEvent::Failed {
                        account_id,
                        message,
                        ..
                    } => {
                        failed = true;
                        println!("✗ {}: {message}", name(&account_id));
                    }
                    _ => {}
                }
            }
            if let Some(hint) = app.attention_hint().await? {
                println!("{hint}");
            }
            if failed {
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The day an event starts on, as seen in `tz`.
fn start_day(timing: &EventTiming, tz: &Tz) -> NaiveDate {
    match timing {
        EventTiming::AllDay { start_date, .. } => *start_date,
        _ => timing.bounds(tz).start.with_timezone(tz).date_naive(),
    }
}

/// When the event happens, from what was typed.
fn timing(
    tz: Tz,
    today: NaiveDate,
    date: &str,
    time: Option<&str>,
    length: Option<&str>,
) -> Fallible<EventTiming> {
    let day = parse_date(date, today)?;
    let length = length.map(parse_length).transpose()?;
    let Some(time) = time else {
        let length = length.unwrap_or(Duration::days(1));
        if length.num_seconds() % 86_400 != 0 {
            return Err("an all-day event lasts whole days, such as 2d; give a start time for a shorter one".into());
        }
        return Ok(EventTiming::AllDay {
            start_date: day,
            end_date: day + length,
        });
    };
    let (start, until_end) = parse_time_span(time)?;
    let length = match (until_end, length) {
        (Some(_), Some(_)) => return Err("give either an end time or a length, not both".into()),
        (Some(l), None) | (None, Some(l)) => l,
        (None, None) => Duration::hours(1),
    };
    let start = resolve_local(&tz, day.and_time(start));
    Ok(EventTiming::Zoned {
        start,
        end: start + length,
        time_zone: tz,
    })
}

/// A new id in the desktop app's style for events created here.
fn new_event_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("local-ucal-{nanos:x}-{:x}", std::process::id())
}

fn print_import(report: &calendar_storage::ImportReport, calendar: &str) {
    let events = |n: usize| {
        if n == 1 {
            "1 event".to_owned()
        } else {
            format!("{n} events")
        }
    };
    if report.imported > 0 {
        println!("Imported {} into {calendar}.", events(report.imported));
    }
    if report.already_present > 0 {
        let verb = if report.already_present == 1 {
            "was"
        } else {
            "were"
        };
        println!(
            "{} {verb} already in this calendar.",
            events(report.already_present)
        );
    }
    if report.imported == 0 && report.already_present == 0 && report.failed.is_empty() {
        println!("The file has no events.");
    }
    for problem in &report.adjusted {
        println!("  changed  {}: {}", problem.title, problem.reason);
    }
    for problem in &report.failed {
        println!("  skipped  {}: {}", problem.title, problem.reason);
    }
    if report.unlisted > 0 {
        println!("  … and {} more.", report.unlisted);
    }
}
