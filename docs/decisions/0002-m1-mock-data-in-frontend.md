# 0002: Milestone 1 simulated data lives in the frontend

- **Date:** 2026-10-29
- **Status:** accepted for Milestone 1 only

## Context

Milestone 1 must validate frontend responsiveness with simulated data. The Rust calendar core, SQLite and Tauri commands belong to Milestone 2, and project guidance says to create modules only when a milestone needs them.

## Decision

The deterministic generator and an in-memory `MockEventSource` are TypeScript and live in `apps/desktop/src/services/mock/`. They sit behind the `EventSource` interface, which is async, range-based, cancellable with `AbortSignal`, and uses client-generated IDs. That interface is the seam Milestone 2 replaces with a Tauri-backed implementation.

No `crates/` directory exists yet. The Tauri layer contains only benchmark hooks.

## Consequences

- The M1 measurements exclude IPC serialisation and SQLite query cost. Milestone 2 must add those benchmarks.
- The mock can simulate latency and failures, which exercises loading states and optimistic rollback before real persistence exists.
- Recurring events are **pre-expanded** by the generator, labelled with a `seriesId`. This is a rendering test only; real RRULE expansion is a Rust-core responsibility.
