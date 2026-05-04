---
description: Scaffold a new app in a sandboxed workspace and iterate on it turn by turn
whenToUse: when the user wants to start a new project from a description, prototype a UI, or iterate on a small app with a live preview
userInvocable: true
---

Walk the user from "I want to build X" to a working scaffold they can run and
iterate on. This skill is **prompt-only** in this release — it describes the
turn-by-turn flow the agent runs through. The full sandboxed preview workspace
(tempdir checkout + hot-reloading dev server pointer + iframe + chat) is
deferred to a follow-up; for now, do the scaffolding and dev-loop bookkeeping
in the current working tree or a user-named directory.

Steps:

1. **Clarify the brief in one paragraph.** Ask just enough questions to fill
   in: what the app does, who uses it, the platform (web / CLI / mobile /
   service), the rough size (single-page / multi-route / persistent
   storage). If the user already gave enough, skip the questions.
2. **Pick the stack.** Suggest one default (e.g. Vite + React + TypeScript
   for a web app; Rust + clap for a CLI; FastAPI for a small service)
   with a one-line reason. Offer one alternative. Wait for the user to
   confirm before scaffolding.
3. **Pick the workspace.** Default: a sibling directory next to the
   current cwd (`./<app-name>`). Confirm the path with the user before
   creating. Never scaffold inside a directory that already has files
   without explicit permission.
4. **STOP for confirmation.** Restate brief + stack + workspace path in
   four short lines. Wait for "go".
5. **Scaffold.** Run the canonical project-init command for the chosen
   stack (`npm create vite@latest`, `cargo new`, `uv init`, etc.). Add
   the minimum project-level files the user named in the brief — a
   single route, a single component, a single command. Do not over-
   scaffold; the user is going to iterate.
6. **Print the dev-loop pointer.** Tell the user the exact command to
   run for the dev server (or the equivalent `cargo run` / `python -m`),
   and the URL or expected output. If a hot-reloading dev server exists
   for the chosen stack, point at it; otherwise note the manual rebuild
   command.
7. **Hand off the iterate-by-chat loop.** From here on, each user turn
   is: "change the layout / add a route / wire this API / fix this bug."
   Each agent turn is: read what's there, make the smallest change that
   satisfies the request, summarize what changed, point at the running
   preview. Do not regenerate the whole app on every turn.
8. **STOP between turns.** After each change, wait for the user's next
   instruction or "looks good." Do not eagerly anticipate the next 5
   features.

You're done with the scaffold step when the user has run the dev command
once and seen the app load. You're done with the skill itself when the
user says they're satisfied or moves to a different task. Never delete
their work to "clean up" without an explicit request.

**Follow-up scope (not in this release):** real subprocess management
for the preview server, an iframe-bound chat surface bound to the live
app's DOM, and a persistent sandboxed tempdir lifecycle. This skill is
the prompt scaffold those will plug into.
