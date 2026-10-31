//! Three-way merge of an object changed both locally and on the server.
//!
//! `base` is the server copy both sides started from. Per event component
//! (the master and each override, matched by recurrence id) and per field,
//! a value changed on only one side wins; a value changed on both sides to
//! different values is a conflict. Cancelled occurrences (EXDATEs) merge as
//! sets, so cancellations made on both sides combine.

use std::collections::BTreeSet;

use crate::model::{EventContent, ObjectContent, Recurrence};
use crate::occurrence_key::OccurrenceKey;

/// A field changed differently on both sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldConflict {
    /// The override's recurrence id, or `None` for the master.
    pub component: Option<OccurrenceKey>,
    /// `title`, `description`, `location`, `time`, `repeat`, `reminders`,
    /// or `event` (removed on one side, changed on the other).
    pub field: &'static str,
}

/// The merged object, or the fields that conflict.
pub fn three_way(
    base: &ObjectContent,
    local: &ObjectContent,
    server: &ObjectContent,
) -> Result<ObjectContent, Vec<FieldConflict>> {
    let key = |c: &EventContent| c.recurrence_id;
    let mut keys: BTreeSet<Option<OccurrenceKey>> = BTreeSet::new();
    for content in [base, local, server] {
        keys.extend(content.master.iter().map(key));
        keys.extend(content.overrides.iter().map(key));
    }
    let find = |content: &ObjectContent, k: Option<OccurrenceKey>| -> Option<EventContent> {
        content
            .master
            .iter()
            .chain(&content.overrides)
            .find(|c| c.recurrence_id == k)
            .cloned()
    };
    let mut conflicts = Vec::new();
    let mut master = None;
    let mut overrides = Vec::new();
    for k in keys {
        let (b, l, s) = (find(base, k), find(local, k), find(server, k));
        let merged = if l == b {
            s
        } else if s == b || l == s {
            l
        } else {
            match (b, l, s) {
                (Some(b), Some(l), Some(s)) => merge_component(&b, &l, &s, k, &mut conflicts),
                // Only one side created it, or both created it differently,
                // or one side removed what the other changed.
                _ => {
                    conflicts.push(FieldConflict {
                        component: k,
                        field: "event",
                    });
                    None
                }
            }
        };
        match (k, merged) {
            (_, None) => {}
            (None, Some(m)) => master = Some(m),
            (Some(_), Some(o)) => overrides.push(o),
        }
    }
    if conflicts.is_empty() {
        Ok(ObjectContent {
            uid: server.uid.clone(),
            master,
            overrides,
        })
    } else {
        Err(conflicts)
    }
}

/// One side's change, if only one side changed it.
fn pick<T: PartialEq + Clone>(b: &T, l: &T, s: &T) -> Option<T> {
    if l == b || l == s {
        Some(s.clone())
    } else if s == b {
        Some(l.clone())
    } else {
        None
    }
}

fn merge_component(
    b: &EventContent,
    l: &EventContent,
    s: &EventContent,
    component: Option<OccurrenceKey>,
    conflicts: &mut Vec<FieldConflict>,
) -> Option<EventContent> {
    let mut conflict = |field| {
        conflicts.push(FieldConflict { component, field });
    };
    let mut field = |name, b, l, s| {
        let merged = pick(b, l, s);
        if merged.is_none() {
            conflict(name);
        }
        merged
    };
    let title = field("title", &b.title, &l.title, &s.title);
    let description = field(
        "description",
        &b.description,
        &l.description,
        &s.description,
    );
    let location = field("location", &b.location, &l.location, &s.location);
    let timing = pick(&b.timing, &l.timing, &s.timing);
    let reminders = pick(&b.reminders, &l.reminders, &s.reminders);
    let recurrence = merge_recurrence(&b.recurrence, &l.recurrence, &s.recurrence);
    if timing.is_none() {
        conflicts.push(FieldConflict {
            component,
            field: "time",
        });
    }
    if reminders.is_none() {
        conflicts.push(FieldConflict {
            component,
            field: "reminders",
        });
    }
    if recurrence.is_none() {
        conflicts.push(FieldConflict {
            component,
            field: "repeat",
        });
    }
    Some(EventContent {
        title: title?,
        description: description?,
        location: location?,
        timing: timing?,
        recurrence: recurrence?,
        recurrence_id: component,
        reminders: reminders?,
    })
}

/// The rule and extra dates merge as values; cancelled occurrences as sets.
fn merge_recurrence(
    b: &Option<Recurrence>,
    l: &Option<Recurrence>,
    s: &Option<Recurrence>,
) -> Option<Option<Recurrence>> {
    if let Some(merged) = pick(b, l, s) {
        return Some(merged);
    }
    let (Some(b), Some(l), Some(s)) = (b, l, s) else {
        return None;
    };
    let rrule = pick(&b.rrule, &l.rrule, &s.rrule)?;
    let rdates = pick(&b.rdates, &l.rdates, &s.rdates)?;
    let set = |v: &[OccurrenceKey]| v.iter().copied().collect::<BTreeSet<_>>();
    let (base, local, server) = (set(&b.exdates), set(&l.exdates), set(&s.exdates));
    // Kept by both, or added by either.
    let exdates = base
        .iter()
        .filter(|k| local.contains(k) && server.contains(k))
        .chain(local.difference(&base))
        .chain(server.difference(&base))
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Some(Some(Recurrence {
        rrule,
        rdates,
        exdates,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EventTiming;
    use chrono::{Duration, TimeZone, Utc};

    fn at(day: u32, hour: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, day, hour, 0, 0).unwrap()
    }

    fn event(title: &str, day: u32) -> EventContent {
        EventContent {
            title: title.into(),
            description: String::new(),
            location: String::new(),
            timing: EventTiming::Zoned {
                start: at(day, 9),
                end: at(day, 9) + Duration::minutes(30),
                time_zone: chrono_tz::UTC,
            },
            recurrence: None,
            recurrence_id: None,
            reminders: Vec::new(),
        }
    }

    fn series(title: &str) -> ObjectContent {
        let mut master = event(title, 1);
        master.recurrence = Some(Recurrence {
            rrule: "FREQ=DAILY;COUNT=10".into(),
            ..Recurrence::default()
        });
        ObjectContent {
            uid: "u".into(),
            master: Some(master),
            overrides: Vec::new(),
        }
    }

    fn key(day: u32) -> OccurrenceKey {
        OccurrenceKey::Instant(at(day, 9))
    }

    #[test]
    fn different_fields_merge() {
        let base = series("Stand-up");
        let mut local = base.clone();
        local.master.as_mut().unwrap().title = "Daily".into();
        local.master.as_mut().unwrap().reminders = vec![10];
        let mut server = base.clone();
        server.master.as_mut().unwrap().location = "Room 4".into();
        let merged = three_way(&base, &local, &server).unwrap();
        let m = merged.master.unwrap();
        assert_eq!((m.title.as_str(), m.location.as_str()), ("Daily", "Room 4"));
        assert_eq!(m.reminders, vec![10]);
    }

    #[test]
    fn the_same_field_changed_differently_conflicts() {
        let base = series("Stand-up");
        let mut local = base.clone();
        local.master.as_mut().unwrap().title = "Mine".into();
        let mut server = base.clone();
        server.master.as_mut().unwrap().title = "Theirs".into();
        assert_eq!(
            three_way(&base, &local, &server).unwrap_err(),
            vec![FieldConflict {
                component: None,
                field: "title"
            }]
        );
        // The same change on both sides is no conflict.
        assert!(three_way(&base, &local, &local).is_ok());
    }

    #[test]
    fn cancellations_and_overrides_combine() {
        let base = series("Stand-up");
        let mut local = base.clone();
        local
            .master
            .as_mut()
            .unwrap()
            .recurrence
            .as_mut()
            .unwrap()
            .exdates = vec![key(3)];
        let mut moved = event("Stand-up (late)", 5);
        moved.recurrence_id = Some(key(5));
        local.overrides.push(moved.clone());
        let mut server = base.clone();
        server
            .master
            .as_mut()
            .unwrap()
            .recurrence
            .as_mut()
            .unwrap()
            .exdates = vec![key(4)];
        let merged = three_way(&base, &local, &server).unwrap();
        let rec = merged.master.unwrap().recurrence.unwrap();
        assert_eq!(rec.exdates, vec![key(3), key(4)]);
        assert_eq!(merged.overrides, vec![moved]);
    }

    #[test]
    fn removed_on_one_side_and_changed_on_the_other_conflicts() {
        let base = ObjectContent {
            uid: "u".into(),
            master: Some(event("Lunch", 2)),
            overrides: Vec::new(),
        };
        let mut local = base.clone();
        local.master.as_mut().unwrap().title = "Lunch with Anna".into();
        let server = ObjectContent {
            master: None,
            ..base.clone()
        };
        assert_eq!(
            three_way(&base, &local, &server).unwrap_err()[0].field,
            "event"
        );
        // Removed on the server, untouched here: it goes.
        assert_eq!(three_way(&base, &base, &server).unwrap().master, None);
    }

    #[test]
    fn rule_changes_on_both_sides_conflict() {
        let base = series("Stand-up");
        let mut local = base.clone();
        local
            .master
            .as_mut()
            .unwrap()
            .recurrence
            .as_mut()
            .unwrap()
            .rrule = "FREQ=WEEKLY".into();
        let mut server = base.clone();
        server
            .master
            .as_mut()
            .unwrap()
            .recurrence
            .as_mut()
            .unwrap()
            .rrule = "FREQ=DAILY;COUNT=5".into();
        assert_eq!(
            three_way(&base, &local, &server).unwrap_err()[0].field,
            "repeat"
        );
    }
}
