# tmux-legion

A tmux sidebar that tracks every AI agent in your session: **blocked**, **working**, **done**.

Inspired by [tmux-agent-sidebar](https://github.com/hiroppy/tmux-agent-sidebar) (sidebar
mechanics, Claude Code hooks) and [herdr](https://github.com/ogulcancelik/herdr)
(explicit status reporting, agent skill, panel styling). Single Rust binary, no daemon.

```
┌────────────────────┬─────────────────────────┐
│ agents      ● 1 / 3│  $ claude               │
│ ⠹ claude           │  > refactoring auth...  │
│   working · 2:api  │                         │
│ ◉ copilot          │                         │
│   blocked · 3:docs │                         │
│ ● reviewer         │                         │
│   done · 4:infra   │                         │
│                    │                         │
│ j/k ↵ jump x kill q│                         │
└────────────────────┴─────────────────────────┘
```

Each agent is a two-line row — a status glyph + name, then `status · window · message` —
styled after herdr's agents panel (Catppuccin Mocha):

| Glyph | Status | Meaning |
|---|---|---|
| `⠋` spinner (yellow) | working | actively running |
| `◉` (red) | blocked | waiting on you (permission / input) |
| `●` (teal) | done | turn finished, still alive |
| `✓` (green) | idle | |
| `○` (gray) | unknown | discovered but unreported |

The header shows the agent count, turning into a red `● N /` badge when any are blocked.

## How it works

- **Claude Code** agents are tracked automatically via hooks: prompt/tool activity ⇒
  working, permission requests ⇒ blocked, turn finished ⇒ done, session end ⇒ removed.
- **pi** ([pi.dev](https://pi.dev)) reports via a bundled extension on its lifecycle
  events (see [Pi extension](#pi-extension) below) — its panes appear as `node` to
  tmux, so process discovery can't see them.
- **Any other agent** (Copilot CLI, codex, aider, ...) reports its own status with
  `tmux-legion report working|blocked|done`, guided by the bundled [SKILL.md](SKILL.md).
- A reconciler scans panes (`pane_current_command`, `@pane_agent`) to discover agents,
  drop rows whose pane was closed or reused, and remove an agent ~15s after it exits —
  no terminal-output scraping.
- State lives in a JSON file per tmux server (`~/.local/state/tmux-legion/`); writers
  take a lock and replace it atomically, the sidebar redraws on SIGUSR1 pokes.

## Install

### Nix flake

```nix
# Private repo — fetched over SSH (reuses your GitHub SSH key, no token needed).
# Public mirrors can use github:hawkish/tmux-legion instead.
inputs.tmux-legion.url = "git+ssh://git@github.com/hawkish/tmux-legion";
```

The flake exposes `packages.<system>.default` (the CLI), `packages.<system>.tmuxPlugin`
(for `programs.tmux.plugins` in home-manager), and `overlays.default` (adds
`tmux-legion` and `tmuxPlugins.tmux-legion`). Pull new revisions with
`nix flake update tmux-legion`; develop locally with
`--override-input tmux-legion /path/to/checkout`.

### Manual / TPM-style

```bash
git clone git@github.com:hawkish/tmux-legion ~/.tmux/plugins/tmux-legion
cd ~/.tmux/plugins/tmux-legion && cargo build --release
echo 'run-shell ~/.tmux/plugins/tmux-legion/tmux-legion.tmux' >> ~/.tmux.conf
```

### Claude Code hooks

Merge [claude/hooks.json](claude/hooks.json) into `~/.claude/settings.json` (top-level
`hooks` key). The hook command uses the stable path
`~/.tmux/plugins/tmux-legion/bin/tmux-legion`; adjust it if your binary lives elsewhere.
Hook invocations are silent, fast, and always exit 0 — they never interfere with Claude.

### Agent skill

Copy or symlink `SKILL.md` to `~/.claude/skills/tmux-legion/SKILL.md` and/or
`~/.copilot/skills/tmux-legion/SKILL.md` so agents know how to spawn siblings and
report status.

### Pi extension

[pi](https://pi.dev) panes show up as `node` to tmux, so process discovery can't
see them and pi has no shell-hook system. Instead, copy or symlink
[pi/tmux-legion.ts](pi/tmux-legion.ts) into `~/.pi/agent/extensions/` — it
reports idle/working/done on pi's lifecycle events.

## Usage

`prefix + g` toggles the sidebar. Inside it: `j`/`k` (or arrows) move, `g`/`G` jump to
top/bottom, `Enter` focuses the selected agent's pane, `x` kills it (confirm with `y`),
`r` forces a rescan, `q` closes. Clicking a row selects it and focuses that pane too.

### CLI

```
tmux-legion report <working|blocked|done|idle|unknown> [-m msg] [--name n] [--pane %id]
tmux-legion list [--json]
tmux-legion spawn [--name n] [--direction right|down|left|up] [--window] [--cwd p] [--focus] -- <cmd...>
tmux-legion wait [--pane %id] --status <s> [--timeout secs]    # exit 0 ok, 2 timeout, 3 pane gone
tmux-legion toggle | open | close
```

## Options (set in tmux.conf)

| Option | Default | |
|---|---|---|
| `@legion_key` | `g` | toggle key (with prefix) |
| `@legion_width` | `15%` | sidebar width (percent or columns) |
| `@legion_position` | `left` | `left` or `right` |
| `@legion_agents` | `claude,copilot,codex,opencode,aider` | commands auto-detected as agents |

## Development

```bash
nix develop        # cargo, rustc, rust-analyzer, clippy, rustfmt, tmux
cargo test
cargo build --release
```
