# Metadata-first architecture

ScreenUse 0.2 is optimized for one person leaving the app running all day on a Windows computer. The primary constraints are:

1. no timer discipline;
2. low CPU and disk writes;
3. enough context to distinguish projects and tasks;
4. zero mandatory model cost;
5. corrections should reduce future work.

This document records the design decisions behind the 0.2 runtime.

## 1. What is actually needed for time attribution

Most computer work can be separated using a small set of signals:

| Signal | Typical value | What it identifies |
| --- | --- | --- |
| foreground process | `Code.exe`, `chrome.exe`, `WINWORD.EXE` | broad category |
| window title | repository, document, meeting or page title | task/topic |
| active URL | host and path | website/service/project |
| editor workspace | repository/root folder | project |
| active file | filename and last path components | task/context |
| Git branch | feature or issue name | development task |
| idle seconds | system last-input time | active vs away |

Screenshots contain more information, but they are not proportionally more useful for the narrow question “how much time went to each personal work item?”. They also make every second of runtime generate image bytes and create a need for OCR, image indexing or vision-model calls.

ScreenUse therefore treats image capture as a different product category rather than an optional detail of time tracking.

## 2. One canonical foreground stream

The desktop foreground window is the canonical activity stream. Browser and editor extensions do not create independent sessions because that would double-count time.

Instead:

1. the Chromium extension stores the most recent active-tab context in process memory;
2. the VS Code extension stores the most recent editor context in process memory;
3. the Windows collector reads the current foreground app;
4. when that app is a supported browser or editor, the matching fresh context enriches the foreground event;
5. only the enriched foreground event reaches SQLite and the classifier.

Extension context expires after 120 seconds. A stale tab or editor cannot be attached to a later foreground window indefinitely.

## 3. Stable IDs and coalesced heartbeats

A naive sampler creates a new row on every observation. ScreenUse observes every 10 seconds by default but does not persist samples that way.

For each uninterrupted context, the collector generates one UUID. The UUID remains the same for:

- context start;
- periodic heartbeat;
- context end.

`raw_events.id` is the primary key and the database uses `INSERT OR REPLACE`. Each 10-second update replaces one row and extends one session instead of appending another. A new row and time block are created only after app/title/URL/file/workspace or active/idle state produces a stable context change.

This follows the event/heartbeat idea used by ActivityWatch, adapted to ScreenUse's existing SQLite schema.

## 4. Context lifecycle

```text
poll foreground metadata
        │
        ├── same signature ──────────────────────── replace event by stable ID and extend one block
        │
        └── signature changed
                ├── first observation ───────────── keep as pending; do not split
                ├── returns immediately ─────────── discard pending change and continue old block
                └── second consecutive observation
                        ├── close old block at the first observed switch time
                        ├── finalize classification and mark it awaiting confirmation
                        └── create one new stable context ID
```

The active session uses the current window, host, workspace or file as its readable summary. Repeated observations with the same context signature extend that session in place. A one-observation loading or waiting state is folded into the surrounding activity, while a real switch such as paper reading → literature search → PDF reading becomes separate blocks awaiting confirmation.

## 5. Classification pipeline

### 5.1 Learned rule

Rules created from confirmed sessions have the highest confidence. The existing database rule matcher checks app, title, URL, file, workspace and metadata text before generic local classification.

### 5.2 Local category

The local classifier handles common cases without a model:

- IDE and development sites → 开发;
- chat, mail and meeting apps/sites → 沟通;
- word processors and knowledge tools → 写作;
- courses, papers, PDFs and learning sites → 学习;
- media/game apps and sites → 娱乐;
- idle threshold → 离开;
- otherwise → 杂务.

### 5.3 Project and task

Project scoring uses:

- exact project-name appearance in title, URL, path or workspace;
- workspace name similarity;
- non-generic project-name tokens;
- matching category.

For IDE contexts, a meaningful unmatched workspace can create a project automatically. The first active task is reused unless a task title has a stronger token match.

### 5.4 Optional AI

AI is outside the normal ingestion path. It is eligible only when:

- mode is `manual` or `auto`;
- a model and credential are configured;
- the session is not confirmed;
- confidence is below the threshold;
- category is not 离开;
- duration reaches the configured minimum.

The request contains at most 80 compact events. URL query strings and fragments are removed, file/workspace paths are shortened, output is capped, and the HTTP request times out after 30 seconds. Failure leaves the local result intact.

## 6. Storage lifecycle

### Long-lived

- `work_sessions`
- `projects`
- `tasks`
- `attribution_rules`
- `plan_items`
- export records

### Rotated

- `raw_events`, default 30 days;
- completed/failed optional AI jobs, same retention window.

### Removed during v0.1 migration

- files under `media-cache`;
- rows in `media_chunks`;
- screenshot-backed analysis jobs;
- seed/demo sessions.

The maintenance worker runs every six hours. Manual “优化” additionally checkpoints and vacuums the database.

## 7. UI workflow

The default interface is intentionally not a monitoring console.

1. **Today:** active time, project coverage, context count, longest block, category distribution and recent activity.
2. **Review inbox:** only sessions without a project or with low confidence.
3. **Timeline:** search, edit, confirm, merge, split and learn a rule.
4. **Projects:** current-day project and task totals.
5. **Settings:** collection interval, retention, optional AI, imports, export and backup.

The user should not need to watch an analysis queue or decide when to start a timer.

## 8. Reference products and retained ideas

| Product | Retained idea | Not copied |
| --- | --- | --- |
| [ActivityWatch](https://docs.activitywatch.net/en/latest/) | watchers, AFK status, events/heartbeats, local categorization | separate server/bucket UI and full query language |
| [RescueTime](https://www.rescuetime.com/features) | background automatic tracking, day reports, suggestion review | subscription/cloud requirement and productivity scoring as the core model |
| [Timing](https://timingapp.com/) | post-hoc timeline correction and rules | macOS-only architecture and billing-oriented workflows |
| [WakaTime](https://wakatime.com/) | editor metadata is a strong development signal | cloud-first coding analytics |
| [screenpipe](https://github.com/screenpipe/screenpipe) | demonstrates the value of searchable personal context | continuous screen/audio capture, OCR and high storage footprint |

## 9. Current platform boundary

The system-level foreground collector in 0.2 is implemented for Windows. The application data model and extension ingestion are platform-neutral, but macOS and Linux need native foreground-window and idle adapters before they can provide equivalent end-to-end tracking.

## 10. Next high-value improvements

Further work should preserve the low-overhead constraint. The best candidates are:

- Windows event hooks to supplement polling for immediate window changes;
- a rule editor with match previews and conflict diagnostics;
- day/week aggregate tables for very long history without loading raw sessions;
- import from ActivityWatch canonical events;
- signed extension packages and desktop release automation;
- native macOS and Linux adapters.

Continuous screenshots, keystroke logging, page-body capture and always-on language-model calls are explicitly out of scope for the default product.
