# rust-code

An AI-powered coding agent for the terminal, written in pure Rust.

## Features

- **Interactive REPL** with streaming responses, markdown rendering, and syntax highlighting
- **23 built-in tools** for file operations, shell commands, code search, web access, and more
- **26 slash commands** for git, session management, diagnostics, and agent control
- **Permission system** with configurable rules (allow/deny/ask per tool and pattern)
- **Agent loop** that autonomously executes multi-step coding tasks with error recovery
- **MCP support** for connecting external tool servers (stdio + SSE transports)
- **Memory system** for persistent context across sessions (project + user level)
- **Skills** for custom reusable workflows loaded from markdown files
- **Plugin system** for bundling skills, hooks, and config as packages
- **Session persistence** with save, resume, and history
- **Plan mode** for safe read-only exploration before making changes
- **Multi-agent coordination** with typed agent definitions (general, explore, plan)
- **IDE bridge protocol** for VS Code and JetBrains integration
- **Vi/Emacs editing modes** with customizable keybindings

## Architecture

```
┌───────────────────────────────────────────────────────┐
│               ENTRYPOINT (main.rs / cli)               │
│          CLI parsing, initialization, bootstrap        │
└─────────────────────────┬─────────────────────────────┘
                          │
┌─────────────────────────▼─────────────────────────────┐
│            CONFIG & BOOTSTRAP (config/)                 │
│      Settings loading, environment, MCP connect        │
└─────────────────────────┬─────────────────────────────┘
                          │
┌─────────────────────────▼─────────────────────────────┐
│             QUERY ENGINE (query/)                       │
│    Agent loop: compact → stream → tools → loop         │
└────────┬────────────┬──────────────┬──────────────────┘
         │            │              │
┌────────▼───┐  ┌─────▼─────┐  ┌────▼───────┐
│ TOOL LAYER │  │ PERMISSION │  │   HOOKS    │
│            │  │   SYSTEM   │  │            │
│ 23 tools   │  │ Rules      │  │ Pre/Post   │
│ MCP proxy  │  │ Plan mode  │  │ Shell/HTTP │
│ Executor   │  │ Ask/Deny   │  │ Lifecycle  │
└────────────┘  └────────────┘  └────────────┘
         │            │              │
┌────────▼────────────▼──────────────▼──────────────────┐
│                  SERVICES (12)                          │
│  LLM Client │ Compact │ MCP │ LSP │ Git │ Session     │
│  Tokens │ Bridge │ Coordinator │ Background │ Plugins  │
└───────────────────────────────────────────────────────┘
```

## Quick Start

```bash
# Build
cargo build --release

# Set your API key
export RC_API_KEY="your-api-key"

# Run interactive mode
./target/release/rc

# One-shot mode
./target/release/rc --prompt "explain this codebase"
```

## Configuration

Configuration is loaded from (highest to lowest priority):
1. CLI flags and environment variables
2. Project-local `.rc/settings.toml`
3. User config `~/.config/rust-code/config.toml`

```toml
[api]
base_url = "https://api.anthropic.com/v1"
model = "claude-sonnet-4-20250514"

[permissions]
default_mode = "ask"

[[permissions.rules]]
tool = "Bash"
pattern = "git *"
action = "allow"

[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path"]

[ui]
markdown = true
syntax_highlight = true
```

## Tools

| Tool | Description | Read-only |
|------|-------------|-----------|
| `Agent` | Spawn subagents for parallel tasks | No |
| `AskUserQuestion` | Interactive multi-choice prompts | Yes |
| `Bash` | Execute shell commands | No |
| `FileEdit` | Search-and-replace editing | No |
| `FileRead` | Read files with line ranges | Yes |
| `FileWrite` | Create or overwrite files | No |
| `Glob` | Find files by pattern | Yes |
| `Grep` | Regex content search (ripgrep) | Yes |
| `LSP` | Language server diagnostics | Yes |
| `McpProxy` | Bridge to MCP server tools | No |
| `NotebookEdit` | Edit Jupyter notebook cells | No |
| `EnterPlanMode` | Switch to read-only mode | Yes |
| `ExitPlanMode` | Re-enable all tools | Yes |
| `SendMessage` | Inter-agent communication | No |
| `Sleep` | Async pause with cancellation | Yes |
| `TaskCreate` | Create progress tracking tasks | Yes |
| `TaskUpdate` | Update task status | Yes |
| `TodoWrite` | Structured todo list management | Yes |
| `ToolSearch` | Find tools by keyword | Yes |
| `EnterWorktree` | Create isolated git worktree | No |
| `ExitWorktree` | Clean up git worktree | No |
| `WebFetch` | Fetch content from URLs | Yes |
| `WebSearch` | Search the web | Yes |

## Commands

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/exit` | Exit the REPL |
| `/clear` | Clear conversation history |
| `/compact` | Free context by clearing old tool results |
| `/cost` | Show session cost and token usage |
| `/model` | Show or change the current model |
| `/diff` | Show git diff |
| `/status` | Show git status |
| `/commit` | Commit current changes |
| `/review` | Review current diff for issues |
| `/branch` | Show or switch git branch |
| `/resume` | Resume a previous session |
| `/sessions` | List saved sessions |
| `/memory` | Show loaded memory context |
| `/skills` | List available skills |
| `/doctor` | Check environment health |
| `/mcp` | List MCP server connections |
| `/plan` | Toggle plan mode |
| `/init` | Initialize project config |
| `/export` | Export conversation as markdown |
| `/context` | Show context window usage |
| `/agents` | List agent types |
| `/hooks` | Show hook configuration |
| `/plugins` | List loaded plugins |
| `/verbose` | Toggle verbose output |
| `/version` | Show version |

## License

MIT
