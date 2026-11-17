# ADR-024: A cross-platform Rust core

[中文](024-rust-core.md) · **English**

Status: decided. M1 is implemented and verified; M2 (the app switched to the Rust core) is implemented and passed its performance check; the AI session connectors from M3 have moved to Rust. Other M3 work proceeds feature by feature. The timeline library's schema, interfaces and permission rules ([timeline spec](../specs/timeline-v1.md)) do not depend on this decision.

## Context

The Mac core — timeline library, query service, connectors, model calls — was written in Swift and leaned more and more on Apple frameworks (CryptoKit, Keychain, Network, EventKit). A Windows version is planned, with capture and permissions rebuilt there; Android is already a chosen direction.

The next large pieces of core code — the data organization pipeline, the mail connector, the task mechanism (see the [agent platform](../specs/agent-platform-v1.md)) — would have to be written twice if they stayed in Swift. "Swift core with a native Windows UI" was judged too awkward; a cross-platform core was preferred.

## Options

| Option | Benefits | Costs |
|---|---|---|
| Keep the Swift core and compile it on Windows too | No rewrite | Thin Swift ecosystem and tooling on Windows; every Apple framework needs a replacement; nothing shared with Android |
| **Rust core, native UI per platform (chosen)** | One core for Mac, Windows, Linux and Android; on desktop it runs as its own process, so any frontend can call it; mature libraries for SQLite, HTTP, MCP, MIME, mail sync and key storage; deterministic memory release, no refcounting or GC overhead | Rewrite about 7,500 lines of Swift core and tests; one more helper process to manage (about 9 MB idle, measured); one more step in the build |
| Kotlin Multiplatform core | Most natural on Android | Heavy runtime and interop on Mac and Windows; mail and SQLite libraries less mature than Rust's |
| Everything in Tauri (Rust core + web UI) | One UI codebase | The Mac UI loses its native feel and conflicts with the design system; system capture still needs native plugins |
| C++ core | Cross-platform | High cost for memory safety and dependency management, no advantage |

## Decision

### 1. Three parts

| Part | Contents | Implementation |
|---|---|---|
| **Rust core** | Timeline library, query tools, permissions and audit; local HTTP, MCP server and CLI; data organization; fixed pipelines; model calls and usage; settings; **every connector that reads files or the network** (Mail.app and Thunderbird local data, IMAP, browser history, AI session logs, notes folders) | One Rust library shared by all platforms, plus a headless CLI build that can run on its own on a NAS or Linux box |
| **Platform frontend** | UI | SwiftUI on Mac, Kotlin on Android; Windows to be chosen between Tauri and WinUI |
| **System integration** | Whatever must call OS APIs or needs permissions the OS grants per app: screen and app capture, permission prompts and guidance, system calendars and reminders, the MailKit live extension, notifications, menu bar, widgets | Written per platform, in the frontend's process |

Connector rule: **reading files or the network belongs in the core; calling OS APIs belongs in the platform layer.**

The Mac app itself uses only Swift and Rust, with no Python dependency. Transcription, voiceprints and other models are external services: the core calls their APIs and validates results, never starts a Python worker and never manages model environments. Such services may run locally, in any language; development, test and hardware-debugging scripts are not runtime dependencies. External agents use MCP or the CLI.

### 2. Boundary rules

- **Only the core opens the product database.** Frontends and system integration never read or write SQLite directly.
- **One way in for writes.** Every connector, in the core or the platform layer, writes through the core's ingest API, with the same redaction, deduplication and filtering.
- **One way out for reads.** The UI, MCP, the CLI and built-in tasks use the same query tools, with the same permission filters and audit.
- **Secrets are read and written by the core** (Keychain on Mac, Credential Manager on Windows); **system permissions are requested by the platform layer**, because the OS grants them per app.
- **The core depends on no platform UI framework.** Where it needs a platform capability, it defines an interface the platform layer implements and injects.

### 3. Layout and process boundary

- The core lives in `core/`, one Cargo workspace. First crates:
  - `candid-store`: schema, ingest, query tools, clients and audit;
  - `candid-service`: loopback HTTP and MCP;
  - `candid-cli`: the `candid` command, including the headless service mode `candid serve`.

  Models, organization and fixed pipelines go into modules of these crates first; a new crate only when an independent dependency calls for it.
- **On desktop the core is a separate process, not an in-process binding.** It runs as `candid serve`, a helper inside the app bundle (`Contents/Helpers/candid`, signed with the app), started and supervised by the app and restarted if it exits abnormally. UI, capture, MCP, CLI and external agents all reach the core through the same local interface; no binding generators such as uniffi.
  - Between the app and the core: a local channel only the current user can open (a Unix domain socket; a named pipe on Windows). External clients keep using token-authenticated loopback HTTP. Both expose the same tools with the same permissions and audit.
  - On top of the query tools, the desktop adds three things: the app's own ingest API (capture writes), a "new data" push channel (UI refresh), and a health check for the core process.
  - Calls pass paginated results and file paths, never large bytes such as screenshots or recordings.
  - Reads that need Full Disk Access (such as Mail.app data) run in the core process. A helper started by the app usually inherits the app's permissions; this is verified before mail access is enabled.
- **Android later**: a resident helper process does not suit phones; in-process calls (uniffi or a thin hand-written JNI) will be chosen then.
- The core's async runtime is limited to one or two worker threads.

### 4. Migration steps

| Step | Contents | Acceptance |
|---|---|---|
| M1 library and service | Rust `candid-store`, `-service`, `-cli`; schema identical to the Swift version, so both open the same database file; tests share the same synthetic fixtures | All existing library, query and service test scenarios ported and passing; on a copy of a real library both CLIs return the same results for the same queries (counts and difference counts only) |
| M2 app switch | The app starts and manages the `candid serve` helper; UI and capture call the core over the local channel; the Swift library, query interface, service and CLI are deleted; the UI is unchanged | All performance conditions below met; existing UI self-checks and tests pass |
| M3 new features in Rust | Model service calls and usage, file-based connectors, data organization, the task mechanism; mail afterwards | Each per its spec; with no Python environment, capture, saving, import, playback, search and MCP all work; approved model services can be connected and disconnecting one only affects processing |

**Performance acceptance** (required before M2, same data and machine as the Swift version):

- idle memory no higher than the Swift version (about 77 MB);
- peak memory during the first sync of a real-size library no higher than the Swift version;
- typical query latency no higher than the Swift version (`overview` about 60 ms);
- app size grows by at most 15 MB.

## Costs and reversibility

- **Rewrite**: about 7,500 lines of Swift core and tests. The UI (about 11,000 lines) and desktop capture are untouched.
- **Engineering complexity**: two languages, starting and supervising a helper process, universal binaries and signing; crash reports need Rust debug symbols; cross-language issues are harder to debug.
- **Data compatibility**: the schema and data-root layout are defined by the timeline spec, independent of the implementation language.
- **The Windows UI** is undecided; this decision only guarantees that the core and its interfaces can be shared.
