
Memory gives the agent context that persists across sessions. There are three layers: project memory (`AGENTS.md`), team memory (shared, version-controlled), and user memory (per-user).

## Project memory

Place a `AGENTS.md` file in your project root or `.agent/AGENTS.md`:

```markdown
# Project Context

This is a Rust web API using Axum and SQLx.
The database is PostgreSQL, migrations are in db/migrations/.
Run tests with `cargo test`. The CI pipeline is in .github/workflows/ci.yml.
Always run `cargo fmt` before committing.
```

This is loaded automatically at the start of every session in that project directory. Use it for project-specific instructions, conventions, and context that every session needs.

## Team memory

Team memory is project-shared memory that lives in version control under `<project>/.agent/team-memory/`. It is loaded by every session that opens the project, but the only sanctioned write path is the `/team-remember` slash command — background extraction and the model's own file-write tools cannot mutate this directory. Entries carry an `author` and `created_at` stamp, and writes are append-only by default (collisions require `--force`). On id collision across scopes, precedence is **project > team > user**.

```
> /team-remember the canary check is in dashboards/canary.tsx
> /team-remember list
> /team-remember remove <name>
```

## User memory

User-level memory lives in `~/.config/agent-code/memory/`:

- `MEMORY.md` — the index file, loaded automatically
- Individual memory files linked from the index

```markdown
<!-- ~/.config/agent-code/memory/MEMORY.md -->
- [Preferences](preferences.md) — coding style and response preferences
- [Work context](work.md) — current projects and priorities
```

```markdown
<!-- ~/.config/agent-code/memory/preferences.md -->
---
name: preferences
description: User coding style preferences
type: user
---

- I prefer explicit error handling over unwrap/expect
- Use descriptive variable names, not single letters
- Always include tests for new functions
```

## How memory is used

Memory files are injected into the system prompt at session start:

1. Project `AGENTS.md` → appears under "# Project Context"
2. User `MEMORY.md` index → appears under "# User Memory"
3. Individual memory files linked from the index → loaded and appended

The agent sees this context on every turn, so it can follow your conventions and understand your project without being told every time.

## Size limits

| Limit | Value |
|-------|-------|
| Max file size | 25KB per memory file |
| Max index lines | 200 lines |

Files exceeding these limits are truncated with a `(truncated)` marker.

## Commands

```
> /memory
Project context: loaded
User memory: loaded (2 files)
```
