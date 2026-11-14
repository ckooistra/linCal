//! The full-screen view (`ucal` in a terminal).
//!
//! Everything that happens arrives as a [`Msg`] on one channel: keys (from
//! a thread that waits for the terminal), query results (from background
//! tasks) and sync reports (from the engine). The loop draws, waits for
//! the next message, and hands the [`Effect`]s that [`State::update`]
//! returns to background tasks. Nothing slow runs between two draws, so
//! the screen answers keys even while the database or the network works.

mod draw;
mod state;

use std::sync::Arc;

use calendar_core::TimeRange;
use calendar_core::timing::date_start;
use calendar_storage::Store;
use chrono::{Duration, NaiveDate};
use chrono_tz::Tz;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use tokio::sync::mpsc::UnboundedSender;

use crate::{App, Fallible};
use state::{Effect, Msg, State};

/// How often accounts are checked when nothing else triggers a sync, as in
/// the desktop app.
const SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Search results listed at most.
const SEARCH_LIMIT: usize = 200;

fn range(tz: &Tz, start: NaiveDate, end: NaiveDate) -> TimeRange {
    TimeRange::new(date_start(tz, start), date_start(tz, end))
}

/// Reads keys on its own thread. `event::read` waits without using the
/// CPU; the thread ends when the view closes (the channel is gone).
fn read_keys(sender: UnboundedSender<Msg>) {
    std::thread::spawn(move || {
        loop {
            let msg = match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => Msg::Key(key),
                Ok(Event::Resize(..)) => Msg::Resized,
                Ok(_) => continue,
                Err(_) => break,
            };
            if sender.send(msg).is_err() {
                break;
            }
        }
    });
}

pub async fn run(app: App) -> Fallible<()> {
    let App {
        store,
        engine,
        tz,
        today,
        style,
        mut sync_events,
    } = app;
    let (sender, mut messages) = tokio::sync::mpsc::unbounded_channel();

    // Sync in the background while the view is open, as the desktop app
    // does. The engine's file locks keep the two from syncing an account
    // at the same time.
    let (scheduler, sync_loop) = calendar_sync::scheduler(Arc::clone(&engine), SYNC_INTERVAL);
    let sync_task = tokio::spawn(sync_loop);
    let forward = sender.clone();
    let forward_task = tokio::spawn(async move {
        while let Some(event) = sync_events.recv().await {
            if forward.send(Msg::Sync(event)).is_err() {
                break;
            }
        }
    });

    let (mut state, effects) = State::new(tz, today);
    state.colors = style.color;
    for effect in effects {
        perform(effect, &store, &sender, &scheduler, tz);
    }

    let mut terminal = ratatui::init();
    read_keys(sender.clone());
    let result = async {
        loop {
            terminal.draw(|frame| draw::draw(frame, &state))?;
            let Some(msg) = messages.recv().await else {
                return Ok(());
            };
            let mut effects = state.update(msg);
            // Take whatever else arrived meanwhile before drawing again.
            while let Ok(msg) = messages.try_recv() {
                effects.extend(state.update(msg));
            }
            for effect in effects {
                if effect == Effect::Quit {
                    return Ok(());
                }
                perform(effect, &store, &sender, &scheduler, tz);
            }
        }
    }
    .await;
    ratatui::restore();
    sync_task.abort();
    forward_task.abort();
    result
}

/// Starts the work for one effect; its result comes back as a message.
fn perform(
    effect: Effect,
    store: &Store,
    sender: &UnboundedSender<Msg>,
    scheduler: &calendar_sync::SchedulerHandle,
    tz: Tz,
) {
    let (store, sender) = (store.clone(), sender.clone());
    let send = move |msg| {
        let _ = sender.send(msg);
    };
    match effect {
        Effect::Load {
            generation,
            start,
            end,
        } => {
            tokio::spawn(async move {
                let result = store
                    .query_range(range(&tz, start, end), tz)
                    .await
                    .map_err(|e| e.to_string());
                send(Msg::Loaded {
                    generation,
                    range: (start, end),
                    result,
                });
            });
        }
        Effect::Search { generation, text } => {
            tokio::spawn(async move {
                let today = chrono::Utc::now().with_timezone(&tz).date_naive();
                // As `ucal search`: a year back and two ahead.
                let window = range(
                    &tz,
                    today - Duration::days(366),
                    today + Duration::days(2 * 366),
                );
                let result = store
                    .search(&text, window, tz, SEARCH_LIMIT, false)
                    .await
                    .map_err(|e| e.to_string());
                send(Msg::Found { generation, result });
            });
        }
        Effect::Create(draft) => {
            tokio::spawn(async move {
                let result = store
                    .create_event(&crate::new_event_id(), draft)
                    .await
                    .map(Box::new)
                    .map_err(|e| e.to_string());
                send(Msg::Created(result));
            });
        }
        Effect::Calendars => {
            tokio::spawn(async move {
                let calendars = store.list_calendars().await;
                let default = store.settings().await.ok().and_then(|s| {
                    s.get("defaultCalendar")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                });
                if let Ok(calendars) = calendars {
                    send(Msg::Calendars { calendars, default });
                }
            });
        }
        Effect::Statuses => {
            tokio::spawn(async move {
                if let Ok(statuses) = store.account_statuses().await {
                    send(Msg::Statuses(statuses));
                }
            });
        }
        Effect::SyncNow => {
            let scheduler = scheduler.clone();
            tokio::spawn(async move {
                let _ = store.retry_now().await;
                scheduler.trigger();
            });
        }
        Effect::Changed => scheduler.trigger(),
        Effect::Quit => {}
    }
}
