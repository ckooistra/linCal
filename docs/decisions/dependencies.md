# Dependency and license review

- **Last reviewed:** 2026-10-29 (Milestone 3)
- **Project license:** GPL-3.0-or-later

## Direct dependencies

| Dependency | Version | License | Why |
|---|---|---|---|
| `@svar-ui/svelte-calendar` | 2.7.1 (exact pin) | MIT | Calendar widget (see [0001](0001-svar-calendar.md)) |
| `@tauri-apps/api` | 2.12.4 | Apache-2.0 OR MIT | Tauri IPC (bench hooks only in M1) |
| `tauri` / `tauri-build` (Rust) | 2.12.5 / 2.7.4 | Apache-2.0 OR MIT | Desktop shell |
| `serde`, `serde_json` (Rust) | 1.x | MIT OR Apache-2.0 | Command and wire (de)serialisation |
| `sqlx` (sqlite, runtime-tokio, migrate) | 0.9.0 | MIT OR Apache-2.0 | SQLite access and embedded migrations |
| `libsqlite3-sys` (bundled SQLite) | 0.37.0 | MIT (SQLite itself: public domain) | Pulled in by SQLx; bundled so no system SQLite is needed |
| `rrule` | 0.14.0 | MIT OR Apache-2.0 | RFC 5545 recurrence evaluation |
| `chrono`, `chrono-tz` | 0.4 / 0.10 | MIT OR Apache-2.0 | Dates, times, IANA zones |
| `tokio` | 1.54 | MIT | Blocking pool for recurrence expansion; async runtime |
| `thiserror` | 2.x | MIT OR Apache-2.0 | Error types |
| `reqwest` (rustls, platform verifier) | 0.13 | MIT OR Apache-2.0 | CalDAV HTTP. TLS via `rustls` (Apache-2.0 OR ISC OR MIT) and `aws-lc-rs` (ISC / Apache-2.0 / MIT / BSD-3-Clause) |
| `roxmltree` | 0.21 | MIT OR Apache-2.0 | WebDAV multistatus parsing |
| `url` | 2.x | MIT OR Apache-2.0 | URL handling |
| `async-trait` | 0.1 | MIT OR Apache-2.0 | Object-safe async provider trait |
| `keyring` (zbus Secret Service store) | 4.2 | MIT OR Apache-2.0 (`zbus`: MIT) | OS credential storage |
| `tauri-plugin-opener` | 2.7 | Apache-2.0 OR MIT | Opens the system browser for OAuth sign-in (called from Rust only; no frontend permission). Pulls in `open` 5.4, `is-wsl`, `is-docker` (MIT) |
| `sha2` | 0.10 | MIT OR Apache-2.0 | PKCE S256 code challenge (already in the tree via other crates) |
| `base64` | 0.21 | MIT OR Apache-2.0 | PKCE / state encoding (already in the tree) |
| `getrandom` | 0.2 | MIT OR Apache-2.0 | OS randomness for PKCE verifiers and OAuth state (already in the tree) |
| `wiremock` (dev, and the optional `testing` feature of `calendar-google` / `calendar-microsoft`) | 0.6 | MIT/Apache-2.0 | Mock CalDAV server and stateful Google/Graph fakes in tests; never in release builds |
| Dev tool (not shipped): Radicale | 3.9 | GPL-3.0 | Local CalDAV server for end-to-end tests (`scripts/dev-radicale.sh`) |
| Dev: `svelte` 5.57, `vite` 8.3, `@sveltejs/vite-plugin-svelte` 7.3, `typescript` 6.0, `svelte-check` 4.7, `vitest` 5.0, `jsdom` 30, `@testing-library/svelte` 5.4, `@tauri-apps/cli` 2.12 | | MIT / Apache-2.0 | Build and test tooling |

TypeScript is held at 6.x because `svelte-check` 4.7 doesn't yet support TypeScript 7.

## Transitive audit

**Rust** (`cargo metadata`, linux-x86_64 resolve): 435 crates after Milestone 3. All are permissive and GPL-3.0-compatible:

- MIT and/or Apache-2.0 (including Apache-2.0 WITH LLVM-exception);
- Unicode-3.0;
- BSD-3-Clause, ISC, Zlib, 0BSD, Unlicense, CC0;
- MPL-2.0 for `cssparser`, `cssparser-macros`, `selectors`, `dtoa-short` and `option-ext`. MPL-2.0 is file-level copyleft and explicitly GPL-compatible.

Nothing needs review.

**npm production tree** (`npm ls --omit=dev --all`): 44 packages.

- 38 MIT, 3 Apache-2.0, 1 Apache-2.0 OR MIT, 1 ISC.
- 1 "SEE LICENSE IN license.txt": `@svar-ui/svelte-editor`, verified as MIT.

Oddity: SVAR declares `svelte-check` as a runtime dependency, so its tooling dependencies (chokidar and others) appear in the production tree. They aren't bundled into the app (tree-shaken by Vite).

## Security advisories

`npm audit` reports two **low** advisories: `deepmerge` prototype pollution, pulled in by `@sveltejs/vite-plugin-svelte`. That is build-time tooling only, it doesn't ship in the app, and no fix is available upstream yet. Re-check at each milestone.

## Not used

The following are not used: SVAR PRO, commercial widgets, paid APIs, hosted backends, telemetry, and remote fonts or CDNs. SVAR's CDN font loading is disabled, and the CSP blocks remote origins.
