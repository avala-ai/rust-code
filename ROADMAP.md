# Roadmap

This document tracks the planned improvements for agent-code on the path to v1.0 and beyond. Items are organized by phase and priority. Each phase can be worked on independently, though some items have noted dependencies.

Status key: **Planned** | **In Progress** | **Done**

---

## Current State

| Area | Status |
|------|--------|
| Rust workspace (lib + cli) | 24K+ LOC, 95+ source files |
| Built-in tools | 32 (file ops, search, shell, web, LSP, MCP, agents, tasks) |
| Slash commands | 42 (session, context, git, agent control, config, diagnostics, history, sharing) |
| Bundled skills | 12 (commit, review, test, explain, debug, pr, refactor, init, security-review, advisor, bughunter, plan) |
| LLM providers | 12 (Anthropic, OpenAI, xAI, Google, DeepSeek, Groq, Mistral, Together, Zhipu, Ollama, Bedrock, Vertex) |
| Tests | 220+ (unit + integration + smoke) |
| Platforms | Linux x86_64/aarch64, macOS x86_64/aarch64, Windows x86_64, Docker |
| Install methods | cargo, homebrew (custom tap), curl script, prebuilt binaries, Docker |
| Docs | 22 pages (Mintlify + mdBook), troubleshooting guide, FAQ |

---

## Phase 1: Documentation

**Priority: Critical** | **Target: v0.10**

Good documentation is the highest-leverage improvement. Every other phase benefits from having clear, complete docs.

### 1.1 CHANGELOG — Done

- [x] Create `CHANGELOG.md` using [Keep a Changelog](https://keepachangelog.com/) format
- [x] Reconstruct history from git tags (Added / Changed / Fixed / Removed per version)
- [x] Add `/release-notes` command that reads and displays the current version's entry

### 1.2 Troubleshooting Guide — Done

- [x] Create `docs/troubleshooting.mdx`
- [x] Cover: API connection failures, permission denials, MCP server crashes, context window exceeded, tool execution errors, installation problems
- [x] Each issue: symptoms, cause, fix

### 1.3 Tutorials (4 pages)

- [ ] **First Project** — setting up agent-code on an existing codebase
- [ ] **Custom Skills** — writing, testing, and sharing a custom skill
- [ ] **MCP Integration** — connecting and using MCP servers
- [ ] **Multi-Provider** — switching between LLM providers, configuring fallbacks
- [ ] Add Tutorials navigation group to `docs/mint.json`

### 1.4 FAQ — Done

- [x] Create `docs/faq.mdx` with 18 questions
- [x] Categories: general, installation, usage, cost, security, extensibility

### 1.5 Security Documentation

- [ ] Expand `SECURITY.md` beyond the current template
- [ ] Create `docs/security.mdx` covering: permission model deep-dive, sandbox architecture, MCP trust boundaries, `--dangerously-skip-permissions` implications, enterprise security configuration
- [ ] Document the destructive command detection system in the Bash tool

### 1.6 Architecture Deep-Dives (4 pages)

- [ ] **Context Compaction** — microcompact, LLM summary, context collapse strategies and when each fires
- [ ] **Tool Execution** — permission flow, read-only batching vs serial mutations, streaming executor
- [ ] **Provider Abstraction** — how `normalize.rs` bridges Anthropic Messages API and OpenAI Chat Completions
- [ ] **MCP Protocol** — stdio/SSE transports, tool proxying, resource access, error handling
- [ ] Add Architecture navigation group to `docs/mint.json`

### 1.7 Performance Tuning Guide

- [ ] Create `docs/guides/performance.mdx`
- [ ] Cover: context management, token budget configuration, model selection for speed vs quality, auto-compact thresholds, deferred tool optimization

### 1.8 README Enhancements — Done

- [x] Add CI badge
- [x] Expand feature table with tool count, command count, skill count
- [x] Add platforms table with Docker
- [x] Add security section, skills table, commands section
- [x] Link to CONTRIBUTING.md and ROADMAP.md

### 1.9 Rustdoc for Library Crate

- [ ] Add top-level rustdoc to `crates/lib/src/lib.rs` with feature overview and examples
- [ ] Add `///` doc comments to ~30 key public structs and functions
- [ ] Publish rustdoc via CI (GitHub Pages or docs.rs)

---

## Phase 2: Skills and Extensibility

**Priority: High** | **Target: v0.11**

### 2.1 New Bundled Skills — Done

- [x] **`security-review`** — OWASP vulnerability scan with severity ratings
- [x] **`advisor`** — Architecture and dependency health analysis
- [x] **`bughunter`** — Systematic bug search with reproduction steps
- [x] **`plan`** — Structured implementation planning with risk flags

### 2.2 Skill Safety Setting — Done

- [x] Add `disable_skill_shell_execution: bool` to `SecurityConfig`
- [x] `Skill::expand_safe()` strips fenced shell blocks when enabled
- [x] Non-shell code blocks preserved

### 2.3 Plugin Executable Support

- [ ] Create `crates/lib/src/tools/plugin_exec.rs` implementing the `Tool` trait for plugin-provided executables
- [ ] Scan `bin/` directories inside plugin folders during plugin loading
- [ ] Register discovered executables as callable tools (JSON stdin/stdout protocol)
- [ ] Document `bin/` support in `docs/extending/plugins.mdx`

### 2.4 Remote Skill Discovery (Stretch)

- [ ] Skill index fetched from configurable URL
- [ ] `agent skill install <name>` to download to user skills directory
- [ ] Offline fallback to cached index
- [ ] Depends on: 2.1 (solid bundled skills first)

---

## Phase 3: New Commands

**Priority: High** | **Target: v0.11**

All commands are added to `crates/cli/src/commands/mod.rs`.

### 3.1 Session Commands — Done

- [x] **`/summary`** — Delegates to agent for session summary
- [x] **`/share`** — Exports session as shareable markdown with metadata
- [x] **`/feedback`** — Saves feedback to `~/.local/share/agent-code/feedback/`

### 3.2 Info Commands — Done

- [x] **`/release-notes`** — Reads CHANGELOG.md, displays current version's entry

### 3.3 Cost Enhancements — Done

- [x] Per-model token breakdown in `/cost` when multiple models used
- [x] Cache hit percentage per model
- [x] Single-model sessions show inline cache hit rate

### 3.4 Headless Mode (Stretch)

- [ ] **`agent serve`** — Start agent as a headless HTTP API server
- [ ] **`agent attach <session-id>`** — Reconnect to a running headless session
- [ ] SSE event stream for real-time updates
- [ ] Enables web UI and IDE integrations without custom bridge code

### 3.5 Self-Management Commands (Stretch)

- [ ] **`/upgrade`** — Check GitHub releases API, download and replace binary if newer version available
- [ ] **`/uninstall`** — Remove binary, config, and data directories with confirmation

---

## Phase 4: Provider Expansion

**Priority: Medium** | **Target: v0.12**

### 4.1 New Providers

- [ ] **Azure OpenAI** — separate from generic OpenAI, with Azure-specific auth (AD tokens, managed identity)
- [ ] **OpenRouter** — single API key for any model, with model routing
- [ ] **Cohere** — Command R+ models
- [ ] **Perplexity** — search-augmented generation
- [ ] **GitHub Copilot** — Copilot token-based access for GitHub users

### 4.2 Provider Features

- [ ] Interactive setup wizard for Bedrock (AWS credential chain, region, model selection)
- [ ] Provider health check in `/doctor` output
- [ ] Fallback chain configuration: try provider A, fall back to B on failure

---

## Phase 5: Testing and Quality

**Priority: Medium** | **Target: v0.11**

### 5.1 Smoke Tests — Done

- [x] `crates/cli/tests/smoke.rs` — `--version`, `--help`, unknown flags
- [x] CI-safe (no API key required)

### 5.2 Integration Tests — Done

- [x] `crates/lib/tests/skills_integration.rs` — 6 tests: bundled loading, finding by name, custom skill from temp dir, override, directory skills
- [x] `crates/lib/tests/config_integration.rs` — 5 tests: defaults, TOML parsing, security config, features, MCP entries

### 5.3 Benchmarks

- [ ] `crates/lib/benches/compaction.rs` — benchmark micro-compact on conversation histories of various sizes
- [ ] `crates/lib/benches/token_estimation.rs` — benchmark token counting on large message arrays

### 5.4 Coverage Reporting

- [ ] Add `cargo-tarpaulin` step to `.github/workflows/ci.yml`
- [ ] Upload to Codecov or Coveralls
- [ ] Add coverage badge to README

### 5.5 Per-Agent Permissions (Stretch)

- [ ] Extend permission system to support per-agent rule sets (not just global)
- [ ] Plan mode gets its own permission model (read-only by default, configurable)
- [ ] Subagents can have restricted tool access

---

## Phase 6: Distribution and CI

**Priority: Medium** | **Target: v1.0**

### 6.1 Windows Support — Done

- [x] `windows-latest` in CI test matrix
- [x] `x86_64-pc-windows-msvc` in release builds (packaged as `.zip`)
- [x] Fixed unused variable warning for Windows compilation

### 6.2 Docker Image — Done

- [x] Multi-stage `Dockerfile` (Rust builder + slim Debian runtime with git, rg, python3, node)
- [x] `.github/workflows/docker.yml` — build and push to `ghcr.io/avala-ai/agent-code` on tags
- [x] `.dockerignore` for clean build context

### 6.3 Protected Directories — Done

- [x] Built-in deny rules for `.git/`, `.husky/`, `node_modules/`
- [x] Enforced before user-configured rules
- [x] Read access unaffected
- [x] Cross-platform path handling (forward and backslash)

### 6.4 Self-Update Mechanism

- [ ] Create `crates/cli/src/update.rs`
- [ ] Check GitHub releases API for newer version on startup (with 24h cooldown)
- [ ] `agent --update` flag to download and replace binary
- [ ] Print update notification in REPL banner when available

### 6.5 npm Wrapper Package (Stretch)

- [ ] Create `npm/` directory with `package.json`, `install.js`, `index.js`
- [ ] `npm install -g agent-code` downloads appropriate prebuilt binary for platform
- [ ] Depends on: 6.1 (Windows binary needed for cross-platform npm)

### 6.6 Homebrew Core (Stretch)

- [ ] Submit formula to `homebrew/homebrew-core` (requires stable release history and passing tests)
- [ ] Depends on: v1.0 release

---

## Phase 7: Advanced Features (Post v1.0)

These are tracked for future exploration. Not committed to a timeline.

### 7.1 Agent Client Protocol (ACP)

- [ ] Implement ACP v1 for IDE integrations (Zed, VS Code, JetBrains)
- [ ] Map ACP sessions to internal agent-code sessions
- [ ] `agent acp` command to start ACP stdio server

### 7.2 Multi-Agent Orchestration

- [ ] Implement coordinator runtime in `crates/lib/src/services/coordinator.rs` (type definitions already exist)
- [ ] `Coordinator::spawn_agent()` for parallel subagent execution
- [ ] `Coordinator::run_team()` for multi-agent task decomposition with result aggregation
- [ ] Agent-to-agent messaging via `SendMessage` tool

### 7.3 Interactive Tutorial System

- [ ] `/powerup` command with step-by-step interactive lessons
- [ ] Lesson structure: explanation, try-it prompt, success verification
- [ ] Ship 5 lessons covering core features

### 7.4 Voice Mode

- [ ] STT integration for voice input
- [ ] TTS integration for spoken responses
- [ ] Push-to-talk keybinding

### 7.5 Scheduled Agents

- [ ] Cron-based agent execution (`agent cron add "0 9 * * *" --prompt "run tests"`)
- [ ] Remote trigger via webhook (`agent trigger --listen :8080`)
- [ ] Background daemon for scheduled runs

### 7.6 Web and Desktop Clients

- [ ] Headless API server mode (`agent serve`) as the foundation
- [ ] Web client connecting via HTTP + SSE
- [ ] Desktop client via Tauri v2 wrapping the web client
- [ ] All clients share the same backend

### 7.7 GitHub Integration

- [ ] Comprehensive PR lifecycle management (`/pr create`, `/pr review`, `/pr merge`)
- [ ] Issue triage and auto-labeling
- [ ] CI status monitoring and fix suggestions

---

## Performance Targets

| Metric | Current | Target (v1.0) |
|--------|---------|---------------|
| Startup time | ~200ms | <150ms |
| Tool dispatch overhead | unmeasured | <1ms per tool call |
| Binary size (release) | ~15MB | <12MB (strip + LTO) |
| Context compaction latency | unmeasured | <500ms for microcompact |
| Tests | 220+ | 400+ |
| Test coverage | unmeasured | >70% |
| Supported platforms | 5 (+ Windows + Docker) | 6 (+ npm wrapper) |

---

## Contributing

Want to help? Pick any unchecked item above and open an issue to discuss the approach before starting. See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup.

Priority areas where contributions are most welcome:
1. **Tutorials** (Phase 1.3) — step-by-step guides for common workflows
2. **Architecture deep-dives** (Phase 1.6) — explain the internals
3. **Plugin executables** (Phase 2.3) — extend the tool system
4. **Benchmarks** (Phase 5.3) — measure what matters

---

*Last updated: 2026-04-05*
