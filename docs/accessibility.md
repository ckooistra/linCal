# Accessibility

The target is WCAG 2.2 level AA for everything the app draws itself. Keyboard use and screen-reader labels matter most for a calendar on a desktop.

## Keyboard

- **Everything is reachable with Tab,** including events in the month, week and day grids. Each event is a button: **Enter** or **Space** opens it in the editor, and closing the editor returns focus to the event.
- **Agenda and search results:** ↑/↓ move between events, and Enter opens one. In the search box, Enter or ↓ jumps to the results and Esc closes the search.
- **Shortcuts** (T, J/K, M/W/D/A, H, N, Ctrl+F) don't fire while typing in a field. **?** (or the **?** button in the toolbar) lists them all.
- **Dialogs** trap focus, close with Esc, and return focus to where it was.
- **Settings** is a dialog with tabs. Arrow keys, Home and End move between tabs. In the group editor, Esc first leaves the editor, then the dialog, and focus stays on the group just edited.
- **The command palette** (Ctrl+K) is a combobox: typing filters, ↑/↓ choose (announced via `aria-activedescendant`), Enter runs, Esc closes.
- **Moving or resizing** an event by dragging is mouse-only. The keyboard alternative is the editor's date and time fields, and "this event / this and following / all" for series.

## Screen readers

- **Grid events** are announced with their title, date and time, calendar, and state, for example "Dentist, Tuesday 16 June, 10:30 to 11:15, Personal, not synced yet".
- **View and range changes** are announced through the toolbar heading (a polite live region).
- **Messages:** errors are announced at once (`role="alert"`), other messages politely (`role="status"`).
- **Forms:** every field has a label. Repeat, reminder and conflict-comparison controls are grouped and labelled. The comparison is a table with row and column headers.

## Colour and motion

- `apps/desktop/src/__tests__/contrast.test.ts` checks, for both themes:
  - **4.5:1 for text** on every background: normal, muted, accent and danger text, and text on accent buttons;
  - **3:1** for text-field borders and the focus ring;
  - **4.5:1 for event text** on any calendar colour (black or white, whichever contrasts more);
  - SVAR's secondary text (other-month days, headers, today). Its defaults reached only about 2.6:1 in the light theme and are overridden in `app.css`.
- Colour is never the only signal. Sync states also differ by pattern or outline and are named in labels, and read-only calendars are marked in text.
- With the desktop's **reduce animations** setting, animations and smooth scrolling are off.

## Automated checks

`apps/desktop/src/__tests__/a11y.test.ts` runs axe-core (4.14) against every dialog, the sidebar, toolbar, editor (new, recurring, read-only, conflict, custom repeat), agenda list and messages, and requires zero violations. axe-core is a test-only dependency (MPL-2.0) and is not shipped.

## Not yet verified

- **Testing with a screen reader** (Orca on Plasma or GNOME). The labels and roles are tested, but not how a screen reader reads them out.
- **SVAR's own controls inside the grid** ("+N more" in busy month cells) and its time grid use SVAR's markup; only event content is ours.
- **High-contrast desktop themes** aren't followed yet; the app uses its own light and dark themes.
