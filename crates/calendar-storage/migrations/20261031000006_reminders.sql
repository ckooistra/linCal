-- Reminders as minutes before the start, comma-separated ("15,1440").
ALTER TABLE events ADD COLUMN reminders TEXT NOT NULL DEFAULT '';

-- Version of the provider readers a calendar's events were last read with.
-- When the readers learn more (here: reminders), the sync engine reads
-- every object of the calendar again once.
ALTER TABLE calendars ADD COLUMN reader_version INTEGER NOT NULL DEFAULT 0;
