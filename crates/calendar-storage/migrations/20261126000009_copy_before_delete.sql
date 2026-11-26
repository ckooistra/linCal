-- A move between accounts is a copy plus a deletion. The deletion of the
-- original waits until the copy has reached its server: it names the copy's
-- queued upload (after_calendar, after_href) and is not sent while that
-- upload is still queued. If the copy is given up, the original is kept.
ALTER TABLE pending_operations ADD COLUMN after_calendar TEXT REFERENCES calendars(id) ON DELETE SET NULL;
ALTER TABLE pending_operations ADD COLUMN after_href TEXT;
