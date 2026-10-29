-- Initial schema (Milestone 2): local accounts, calendars, events and settings.
-- Sync state, pending operations and provider metadata arrive with CalDAV (Milestone 3).

CREATE TABLE accounts (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL,
    provider    TEXT NOT NULL,
    created_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE calendars (
    id          TEXT PRIMARY KEY NOT NULL,
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    color       TEXT NOT NULL,
    read_only   INTEGER NOT NULL DEFAULT 0,
    sort_order  INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX calendars_account ON calendars(account_id);

-- One row per one-off event, series master, or override of one occurrence.
--
-- timing_kind: 'zoned' | 'floating' | 'all-day'
-- start_value/end_value: exact value (RFC 3339 UTC, local ISO wall time, or ISO date)
-- range_start_ms/range_end_ms: zone-independent index bounds in epoch ms.
--   Floating and all-day values are their wall time read as UTC, so queries
--   apply +/-14 h slack. For series masters range_end_ms is the end of the
--   last occurrence, or the maximum integer when the series is unbounded.
CREATE TABLE events (
    id              TEXT PRIMARY KEY NOT NULL,
    calendar_id     TEXT NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    location        TEXT NOT NULL DEFAULT '',
    timing_kind     TEXT NOT NULL CHECK (timing_kind IN ('zoned', 'floating', 'all-day')),
    start_value     TEXT NOT NULL,
    end_value       TEXT NOT NULL,
    time_zone       TEXT,
    range_start_ms  INTEGER NOT NULL,
    range_end_ms    INTEGER NOT NULL,
    rrule           TEXT,
    rdates          TEXT NOT NULL DEFAULT '',
    exdates         TEXT NOT NULL DEFAULT '',
    series_id       TEXT REFERENCES events(id) ON DELETE CASCADE,
    recurrence_id   TEXT,
    sync_status     TEXT NOT NULL,
    revision        INTEGER NOT NULL DEFAULT 1,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    CHECK ((series_id IS NULL) = (recurrence_id IS NULL)),
    CHECK (rrule IS NULL OR series_id IS NULL)
) STRICT;

CREATE INDEX events_range ON events(range_start_ms, range_end_ms);
CREATE INDEX events_calendar_range ON events(calendar_id, range_start_ms);
CREATE UNIQUE INDEX events_override ON events(series_id, recurrence_id) WHERE series_id IS NOT NULL;

-- Non-secret application settings as JSON values. Secrets never go here.
CREATE TABLE application_settings (
    key    TEXT PRIMARY KEY NOT NULL,
    value  TEXT NOT NULL
) STRICT;
