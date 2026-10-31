-- The server's version of an object whose upload conflicted, kept so the
-- user can compare it with the local change before choosing.
ALTER TABLE remote_objects ADD COLUMN conflict_etag TEXT;
ALTER TABLE remote_objects ADD COLUMN conflict_ical TEXT;
