# 0004: CalDAV client, lossless iCalendar and the sync model

- **Date:** 2026-10-29
- **Status:** accepted (Milestone 3)

## Decisions

1. **Own lossless iCalendar layer (`calendar-ical`)** instead of the `icalendar` crate.
   - Round trips must preserve every property, parameter and component the app doesn't model (alarms, attendees, VTIMEZONEs, `X-` fields).
   - Editing therefore happens in place on the original object, and only properties whose *meaning* changed are rewritten. An unedited object writes back identically; a test checks this.
   - The content-line grammar is small: folding, parameters with quoting and RFC 6868, and TEXT escaping.
2. **TZID resolution.** IANA names directly, Mozilla-style prefixes, a table of common Windows zone names, then the VTIMEZONE's `X-LIC-LOCATION`. Unknown zones fall back to floating time with a warning. VTIMEZONE definitions are *not* used to compute offsets; IANA data from `chrono-tz` is authoritative.
3. **HTTP and XML.**
   - **HTTP:** `reqwest` with rustls and the platform certificate verifier, so system CAs and enterprise roots are trusted.
   - **XML:** `roxmltree` for namespace-aware parsing.
   - **Request bodies:** static strings with escaped values.
   - **Auth:** Basic auth only, over TLS.
   - **Redirects:** only same-origin redirects are followed, so credentials are never sent to a host the user didn't enter.
4. **Object-oriented provider contract.** Href, ETag and iCalendar text, so the queue, conflicts and round-trip preservation are shared by all providers (see `provider-integration.md`).
5. **Durable queue, one operation per object, with a generation counter**, in the same transaction as the local change. Pending local changes win over incoming server data until uploaded; the ETag precondition turns concurrent changes into conflicts.
6. **Ids.** Pulled events get ids derived from their href (`<calendar>/<file name>`), which are stable across syncs and unique per calendar. Overrides keep the `<series>::<recurrence id>` scheme.
7. **Credentials.** Credentials are stored with `keyring` 4 (Secret Service via zbus on Linux). They are never in SQLite, logs or error messages. `UC_CREDENTIALS=memory` exists for testing only.
8. **Simulated data is kept beside real accounts.** Loading a dataset only replaces simulated accounts.

## Consequences and limits

- Events in synchronized calendars can't change calendar yet.
- Digest auth, MKCALENDAR, iTIP and VTIMEZONE generation are open items (listed in `synchronization.md`).
- Instances imported without their series (e.g. a single invitation occurrence) are shown as one-off events and written back as overrides.
