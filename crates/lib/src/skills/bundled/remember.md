---
description: Save a specific insight to long-term memory with type and scope prompting
whenToUse: when the user shares a preference, fact about themselves, project context, or external pointer worth retaining across sessions
userInvocable: true
---

Capture an insight or preference the user just shared as a memory entry,
following the two-step write discipline. This skill walks the user through
choosing the right scope so memories don't sprawl.

Steps:

1. **Restate the insight in one sentence** — confirm with the user that this
   is what should be saved. If the insight is derivable from the codebase
   (architecture, file paths, git history, debug fixes the code already
   shows), STOP — don't save it; the agent can rediscover it.
2. **Classify the type:**
   - `user` — role, preference, or knowledge about the person.
   - `feedback` — a rule about how to approach work ("always run tests
     before push").
   - `project` — in-flight context for the current codebase (current branch
     plan, in-flight refactor).
   - `reference` — a pointer to an external system (a Slack thread, a doc
     URL, an issue number).
3. **Pick the scope:**
   - `user` (`~/.config/agent-code/memory/`) — applies in every project.
   - `project` (`.agent/memory/`) — applies only here.
   - `team` — only when the project has an established shared-memory
     directory under version control. If unclear, ask before assuming team
     scope.
4. **STOP and confirm** the type + scope + filename with the user before
   writing. The filename is short, kebab-case, descriptive
   (`prefer-conventional-commits`, not `note-1`). Confirm it doesn't
   collide with an existing memory.
5. **Write the memory file** with frontmatter (`name`, `description`, `type`)
   and the body containing only the insight itself — no preamble, no
   recap of how the insight came up.
6. **Append one index line** to the appropriate `MEMORY.md` (under ~150
   chars): `- [Title](file.md) — one-line hook`. Do not dump content into
   the index. Do not duplicate an existing entry.
7. **Confirm in one line**: `saved <scope>/<filename>`.

You're done when the file exists, the index entry exists, and the user has
seen what was saved. Never save anything the user didn't explicitly approve.
