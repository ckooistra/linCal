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
use state::{Effect, Msg, Scope, State};

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
        Effect::Update { id, draft, scope } => {
            let scheduler = scheduler.clone();
            tokio::spawn(async move {
                let result = match scope {
                    // A new calendar moves the event (or series) there.
                    Scope::Event | Scope::Series => {
                        // Moving off a server queues a deletion there.
                        if matches!(store.object_of(&id).await, Ok(Some(_))) {
                            scheduler.trigger();
                        }
                        store
                            .update_and_move(&id, &crate::new_event_id(), draft)
                            .await
                            .map(|e| e.as_occurrence())
                    }
                    Scope::Occurrence => store.update_occurrence(&id, draft, tz).await,
                    Scope::Following => store
                        .update_following(&id, &crate::new_event_id(), draft)
                        .await
                        .map(|e| e.as_occurrence()),
                };
                send(Msg::Updated(
                    result.map(Box::new).map_err(|e| e.to_string()),
                ));
            });
        }
        Effect::Delete { id, scope, title } => {
            tokio::spawn(async move {
                let result = async {
                    // Asked first: afterwards the link to the server is gone.
                    let remote = store.object_of(&id).await?.is_some();
                    match scope {
                        Scope::Event => store.delete_event(&id).await?,
                        Scope::Occurrence => store.delete_occurrence(&id).await?,
                        Scope::Following => store.delete_following(&id).await?,
                        Scope::Series => store.delete_series(&id).await?,
                    }
                    Ok::<_, calendar_storage::StorageError>(remote)
                }
                .await;
                send(Msg::Deleted {
                    title,
                    result: result.map_err(|e| e.to_string()),
                });
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

#[cfg(test)]
mod tests {
    use super::*;
    use calendar_core::{EventDraft, EventTiming, Occurrence};
    use calendar_sync::{MemoryCredentials, SyncEngine, default_factory};
    use chrono::NaiveDateTime;

    fn tz() -> Tz {
        "America/Toronto".parse().unwrap()
    }

    fn draft_of(o: &Occurrence, title: &str) -> EventDraft {
        EventDraft {
            calendar_id: o.calendar_id.clone(),
            title: title.into(),
            description: o.description.clone(),
            location: o.location.clone(),
            timing: o.timing.clone(),
            rrule: o.rrule.clone(),
            reminders: None,
        }
    }

    /// Edits and deletions, carried out as the view does, on a real database.
    #[test]
    fn edits_and_deletions_reach_the_database() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let store = Store::open_in_memory().await.unwrap();
            store.ensure_local_calendar().await.unwrap();
            let engine = Arc::new(SyncEngine::new(
                store.clone(),
                Arc::new(MemoryCredentials::default()),
                default_factory(),
                Arc::new(|_| {}),
            ));
            let (scheduler, _never_run) = calendar_sync::scheduler(engine, SYNC_INTERVAL);
            let (sender, mut messages) = tokio::sync::mpsc::unbounded_channel();
            let monday = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
            let start = calendar_core::timing::resolve_local(
                &tz(),
                NaiveDateTime::parse_from_str("2026-11-16 09:30", "%Y-%m-%d %H:%M").unwrap(),
            );
            store
                .create_event(
                    "standup",
                    EventDraft {
                        calendar_id: "local-personal".into(),
                        title: "Standup".into(),
                        description: String::new(),
                        location: String::new(),
                        timing: EventTiming::Zoned {
                            start,
                            end: start + Duration::minutes(15),
                            time_zone: tz(),
                        },
                        rrule: Some("FREQ=DAILY;COUNT=5".into()),
                        reminders: None,
                    },
                )
                .await
                .unwrap();
            let week = || async {
                store
                    .query_range(range(&tz(), monday, monday + Duration::days(7)), tz())
                    .await
                    .unwrap()
            };
            let titles = |found: Vec<Occurrence>| -> Vec<String> {
                found.into_iter().map(|o| o.title).collect()
            };
            let mut run = async |effect| {
                perform(effect, &store, &sender, &scheduler, tz());
                messages.recv().await.unwrap()
            };

            // Wednesday and the following ones.
            let wednesday = week().await[2].clone();
            let msg = run(Effect::Update {
                id: wednesday.id.clone(),
                draft: draft_of(&wednesday, "Standup (new team)"),
                scope: Scope::Following,
            })
            .await;
            assert!(
                matches!(msg, Msg::Updated(Ok(ref o)) if o.title == "Standup (new team)"),
                "{msg:?}"
            );
            assert_eq!(
                titles(week().await),
                [
                    "Standup",
                    "Standup",
                    "Standup (new team)",
                    "Standup (new team)",
                    "Standup (new team)"
                ]
            );

            // Only Thursday's.
            let thursday = week().await[3].clone();
            let msg = run(Effect::Delete {
                id: thursday.id,
                scope: Scope::Occurrence,
                title: thursday.title,
            })
            .await;
            assert!(
                matches!(
                    msg,
                    Msg::Deleted {
                        result: Ok(false),
                        ..
                    }
                ),
                "{msg:?}"
            );
            assert_eq!(week().await.len(), 4);

            // Only Monday's, then the whole first series.
            let first = week().await[0].clone();
            let msg = run(Effect::Update {
                id: first.id.clone(),
                draft: draft_of(&first, "Kick-off"),
                scope: Scope::Occurrence,
            })
            .await;
            assert!(matches!(msg, Msg::Updated(Ok(_))), "{msg:?}");
            assert_eq!(week().await[0].title, "Kick-off");
            let msg = run(Effect::Delete {
                id: week().await[1].id.clone(),
                scope: Scope::Series,
                title: "Standup".into(),
            })
            .await;
            assert!(
                matches!(
                    msg,
                    Msg::Deleted {
                        result: Ok(false),
                        ..
                    }
                ),
                "{msg:?}"
            );
            assert_eq!(
                titles(week().await),
                ["Standup (new team)", "Standup (new team)"]
            );

            // The new series as a whole.
            let rest = week().await[0].clone();
            let msg = run(Effect::Update {
                id: rest.id.clone(),
                draft: draft_of(&rest, "Team standup"),
                scope: Scope::Series,
            })
            .await;
            assert!(matches!(msg, Msg::Updated(Ok(_))), "{msg:?}");
            assert_eq!(titles(week().await), ["Team standup", "Team standup"]);

            // And to another calendar: the series moves, under a new id.
            store
                .replace_all(
                    &calendar_core::sim::accounts(),
                    &calendar_core::sim::calendars(),
                    &[],
                )
                .await
                .unwrap();
            let rest = week().await[0].clone();
            let mut draft = draft_of(&rest, "Team standup");
            draft.calendar_id = "cal-family".into();
            let msg = run(Effect::Update {
                id: rest.id.clone(),
                draft,
                scope: Scope::Series,
            })
            .await;
            let Msg::Updated(Ok(moved)) = msg else {
                panic!("{msg:?}")
            };
            assert_ne!(Some(&moved.id), rest.series_id.as_ref(), "a new id");
            let shown = week().await;
            assert_eq!(shown.len(), 2);
            assert!(
                shown
                    .iter()
                    .all(|o| o.calendar_id == "cal-family"
                        && o.series_id.as_ref() == Some(&moved.id))
            );
        });
    }
}
