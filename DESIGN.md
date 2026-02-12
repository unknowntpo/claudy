# Claudy Architecture

A TUI monitor for Claude Code sessions, built with Rust + ratatui.
Watches `~/.claude/projects/` for real-time JSONL session updates.

~1,500 lines of Rust across 6 modules.

## Module Map

```
src/
  main.rs      Entry point, CLI args, terminal lifecycle
  app.rs       App state machine, event loop, dedup/filter/sort
  session.rs   Session discovery, JSONL parsing, incremental I/O
  message.rs   JSONL deserialization, metadata extraction
  watcher.rs   File system monitoring (notify crate)
  ui.rs        TUI rendering (ratatui)
```

## TUI Layout

```
+--[ Sessions [active] (N) ]--------+--[ Chat - <display_name> ]--------+
| ● session-a (main)     [12] 10:42 | [10:41] User:                     |
| ○ session-b (feat-x)    [5] 09:30 |   How do I fix the bug?           |
|   session-c              [2] 08:15 | [10:42] Assistant:                |
|                                    |   Let me look at the code...      |
|                                    | [10:42] Tool:                     |
+--[ Session Info ]------------------+   [tool: Read]                    |
| Title: my-project                  | [10:42] User:                     |
| ID: 6ca04320-22ab-4664-b059-dec... |   [tool result]                   |
| Branch: feat-throughput-experiment |                                    |
| CWD: ~/repo/project               |                                    |
| Tokens: 4.1M in / 8.3K out        |                                    |
| Messages: 184                      |                                    |
| Status: active                     |                                    |
| Summary: ...                       |                                    |
+------------------------------------+------------------------------------+
  q:quit  Tab:focus  j/k:nav  Enter:select  r:refresh  /:filter  a:active
```

Left pane: 35% width (session list 65%, info 35% vertical split).
Right pane: 65% width (chat stream with scroll).

## Data Flow

### Startup

```
main.rs
  |
  v
App::new(base_path)
  |
  +-> discover_sessions(~/.claude/projects/)
  |     |
  |     +-- for each project dir:
  |           |
  |           +-- load_sessions_index()    <-- sessions-index.json
  |           |     { sessionId, customTitle, summary, gitBranch }
  |           |
  |           +-- for each *.jsonl (skip agent-*):
  |                 |
  |                 +-- parse_session_file()
  |                       |
  |                       +-- line by line:
  |                       |     extract_meta()  -> SessionMeta
  |                       |     parse_line()    -> SessionMessage
  |                       |
  |                       +-- return Session {
  |                             id, slug, custom_title, git_branch,
  |                             cwd, messages, tokens, file_offset
  |                           }
  |
  +-> SessionWatcher::new()    <-- notify crate, recursive watch
  |
  +-> update_sort()            <-- sort, dedup, filter
  |
  +-> select first session
```

### Event Loop (250ms tick)

```
 +------------------+
 |  Draw UI (TUI)   |
 +--------+---------+
          |
          v
 +------------------+
 |  Poll events     |  <-- crossterm (key/mouse) + 250ms timeout
 |  Batch-process   |      drain ALL pending before next draw
 +--------+---------+
          |
          v
 +------------------+
 |  tick()          |  <-- every 250ms
 |  +- watcher.poll |
 |  |   |           |
 |  |   +- FileModified(.jsonl)
 |  |   |    +-> read_new_lines(session)   <-- incremental parse
 |  |   |    +-> auto-scroll if focused
 |  |   |
 |  |   +- FileModified(sessions-index.json)
 |  |   |    +-> refresh_index_metadata()  <-- update titles
 |  |   |
 |  |   +- FileCreated(.jsonl)
 |  |        +-> discover_single_session()
 |  |
 |  +- every 10s: refresh_index_metadata()
 |
 +-> update_sort()
```

### JSONL Parsing Pipeline

```
 Raw JSONL line (one JSON object per line)
       |
       +---> serde_json::from_str::<RawMessage>
       |       { type, sessionId, message, timestamp,
       |         gitBranch, cwd, slug, summary, customTitle }
       |
       +---> extract_meta() -> SessionMeta
       |       Handles types: "user"/"assistant" (metadata fields),
       |       "summary" (summary field), "custom-title" (customTitle)
       |
       +---> parse_line() -> Option<SessionMessage>
               Handles types:
                 "user"      -> MessageType::User     (green)
                 "assistant" -> MessageType::Assistant (blue)
                                or ToolUse if has tool_use blocks
                 "progress"  -> MessageType::Progress  (skipped in chat)
                 other       -> MessageType::Other
               Skips: file-history-snapshot, queue-operation
```

## Dedup & Filter Pipeline

```
 update_sort()
   |
   1. Sort all session_ids by last_activity DESC
   |
   2. Dedup by slug:
   |    seen = {}
   |    for each session:
   |      if slug in seen:
   |        DISCARD (duplicate)
   |        but MERGE custom_title -> kept session if missing
   |      else:
   |        KEEP, record slug -> id
   |
   3. Filter: active only (mtime < 5min)
   |
   4. Filter: text search (name, id, summary)
   |
   5. Restore selection or default to first
```

## Session Metadata Priority

```
 display_name() priority:
   1. custom_title   (from /rename command)
   2. slug           (from JSONL metadata)
   3. summary        (auto-generated)
   4. short_id       (first 8 chars of UUID)
   + " (branch)" appended if git_branch set

 custom_title sources (highest priority first):
   1. sessions-index.json  (IndexEntry.customTitle)
   2. JSONL inline         (type: "custom-title", latest wins)
   3. Merged from dedup    (duplicate session had title)
```

## Key Dependencies

| Crate      | Purpose                                         |
|------------|------------------------------------------------ |
| ratatui    | TUI framework (+unstable-rendered-line-info)     |
| crossterm  | Terminal backend, raw mode, mouse, events        |
| notify     | Filesystem watcher (recursive, 1s poll)          |
| serde/json | JSONL + index deserialization                    |
| chrono     | Timestamp parsing (RFC3339 -> UTC -> local)      |
| clap       | CLI args (--path)                                |
| dirs       | Home directory resolution                        |
| anyhow     | Error handling                                   |

## Diagrams

Visual diagrams live in `docs/`:

- [`docs/architecture.excalidraw`](docs/architecture.excalidraw) — system overview
  (open with [excalidraw.com](https://excalidraw.com) or VS Code Excalidraw extension)

---

## How Claude Code Manages Sessions

Claudy is a **read-only observer** of Claude Code's on-disk session data.
Understanding Claude Code's storage format is essential.

### Directory Layout

```
~/.claude/projects/
  |
  +-- -Users-me-repo-my-project/          <-- one dir per working directory
  |     |                                      (abs path, slashes -> hyphens)
  |     +-- sessions-index.json            <-- metadata index (all sessions)
  |     +-- 237ef260-...-.jsonl            <-- session transcript (one per session)
  |     +-- 733cbd9d-...-.jsonl
  |     +-- 237ef260-.../                  <-- per-session subagent container
  |     |     +-- subagents/
  |     |     |     +-- agent-a1b2c3.jsonl <-- subagent transcript
  |     |     +-- tool-results/
  |     |           +-- toolu_01X...txt    <-- tool execution output
  |     +-- memory/
  |           +-- MEMORY.md                <-- project-scoped persistent memory
  |
  +-- -Users-me-repo-other-project/
        +-- ...
```

### sessions-index.json

Written by Claude Code. Updated on session create, rename, and periodically.

```json
{
  "version": 1,
  "originalPath": "/Users/me/repo/my-project",
  "entries": [
    {
      "sessionId":    "237ef260-dae8-4f78-9f36-dc5d8e882615",
      "fullPath":     "/Users/me/.claude/projects/.../237ef260-....jsonl",
      "fileMtime":    1769397427744,
      "firstPrompt":  "How do I fix the login bug?",
      "customTitle":  "login-fix",
      "summary":      "Fixed authentication redirect loop",
      "messageCount": 42,
      "created":      "2026-02-10T08:30:00.000Z",
      "modified":     "2026-02-10T09:15:00.000Z",
      "gitBranch":    "fix-auth",
      "projectPath":  "/Users/me/repo/my-project",
      "isSidechain":  false
    }
  ]
}
```

### JSONL File Format

Each `.jsonl` file is a session transcript. One JSON object per line.
Lines are **append-only** — Claude Code never modifies existing lines.

**Common fields on most entry types:**

```
uuid          string   Unique entry ID
parentUuid    string?  Parent message UUID (conversation tree)
timestamp     string   ISO-8601 timestamp
sessionId     string   Session UUID (same across all entries)
type          string   Entry type (see below)
slug          string   Human-readable session alias
gitBranch     string?  Git branch at time of entry
cwd           string   Working directory
version       string   Claude Code version
isSidechain   bool     true if from subagent
```

**Entry types and their purpose:**

```
type                   Purpose
----                   -------
user                   User message (prompt). content = text/tool_result array
assistant              Claude response. content = text/tool_use array + usage tokens
custom-title           /rename command result. customTitle = new name
summary                Auto-generated session summary
progress               Hook/tool progress (bash_progress, mcp_progress, etc.)
file-history-snapshot  File backup metadata before edits
queue-operation        Message queue enqueue/remove
system                 System messages
```

**Example entries:**

```jsonl
{"type":"user","uuid":"abc...","sessionId":"237ef260-...","slug":"goofy-cuddling-globe","message":{"role":"user","content":"Fix the bug"},"timestamp":"2026-02-10T08:30:00Z"}
{"type":"assistant","uuid":"def...","sessionId":"237ef260-...","message":{"role":"assistant","content":[{"type":"text","text":"Let me look..."}],"usage":{"input_tokens":1500,"output_tokens":200}},"timestamp":"2026-02-10T08:30:05Z"}
{"type":"custom-title","customTitle":"login-fix","sessionId":"237ef260-..."}
{"type":"summary","summary":"Fixed authentication redirect loop","leafUuid":"ghi..."}
{"type":"file-history-snapshot","snapshot":{"trackedFileBackups":{"/path/to/file.rs":{"backupFileName":"...","version":1}}}}
```

### Session Lifecycle

```
 User runs `claude` in terminal
   |
   v
 Claude Code creates {uuid}.jsonl
   |
   +-- Appends "user" entry for each prompt
   +-- Appends "assistant" entry for each response
   +-- Appends "progress" entries for tool execution
   +-- Appends "file-history-snapshot" before edits
   |
   User runs /rename my-title
   +-- Appends {"type":"custom-title","customTitle":"my-title"}
   +-- Updates sessions-index.json with customTitle
   |
   Session ends (user quits or idle)
   +-- Appends "summary" entry
   +-- Updates sessions-index.json (messageCount, modified)
   |
   User resumes with `claude --continue`
   +-- Appends to SAME .jsonl file
   +-- Same sessionId throughout lifetime
```

### Session Identity

A session has three identifiers:

| ID | Example | Scope |
|----|---------|-------|
| sessionId (UUID) | `237ef260-dae8-...` | Primary key, unique globally |
| Filename | `237ef260-dae8-....jsonl` | Same as sessionId |
| slug | `goofy-cuddling-globe` | Human-readable, used for dedup |

Multiple JSONL files can share a slug when Claude Code creates
separate files per working directory for the same logical session.

### Subagents

When Claude Code spawns a Task agent, it creates subagent files:

```
{sessionId}/subagents/agent-{hash}.jsonl
```

Subagent entries have `isSidechain: true`. Claudy skips files
matching `agent-*` pattern during session discovery.

---

## Key Design Decisions

- **Incremental parsing**: `file_offset` tracks last-read byte position.
  Only new lines are parsed on file change. Critical for large sessions.

- **Batch event processing**: All pending input events drained before
  redraw. Prevents scroll events from leaking across focus changes.

- **Dedup by slug**: Claude Code creates separate JSONL files per
  working directory for the same logical session. Slug-based dedup
  with custom_title merging keeps the view clean.

- **250ms tick rate**: Balances responsiveness with CPU usage.
  File watcher has 1s poll interval (macOS FSEvents).

- **ratatui `unstable-rendered-line-info`**: Required for
  `Paragraph::line_count()` which enables precise wrapped-text
  scroll calculations.
