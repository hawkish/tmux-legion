# tmux-legion

[![Release](https://img.shields.io/github/v/release/hawkish/tmux-legion?sort=semver)](https://github.com/hawkish/tmux-legion/releases/latest)
[![License](https://img.shields.io/github/license/hawkish/tmux-legion)](LICENSE)

A tmux sidebar that tracks every AI agent in your session: **blocked**, **working**,
**done**. Hooks drive the status where the agent supports them; process-tree
discovery finds the rest — including node-wrapped CLIs — with zero configuration.

Inspired by [tmux-agent-sidebar](https://github.com/hiroppy/tmux-agent-sidebar) (sidebar
mechanics, Claude Code hooks) and [herdr](https://github.com/ogulcancelik/herdr)
(explicit status reporting, agent skill, panel styling). Single Rust binary, no daemon.

![tmux-legion sidebar tracking a blocked Claude Code pane and an idle Copilot pane](screen.png)

Each agent is a two-line row — a status glyph + name, then `status · directory ·
message`, where the directory is the pane's working directory (so agents split
across repos or worktrees stay distinguishable). Styled after herdr's agents
panel (Catppuccin Mocha):

| Glyph | Status | Meaning |
|---|---|---|
| `⠋` spinner (yellow) | working | actively running |
| `◉` (red) | blocked | waiting on you (permission / input) |
| `●` (teal) | done | turn finished, still alive |
| `✓` (green) | idle | waiting for a prompt |
| `○` (gray) | unknown | discovered but unreported |

The header shows the agent count, turning into a red `● N /` badge when any are blocked.

## How it works

- **Claude Code** agents are tracked automatically via hooks: prompt/tool activity ⇒
  working, permission requests ⇒ blocked, turn finished ⇒ done, session end ⇒ removed.
- **pi** ([pi.dev](https://pi.dev)) reports via a bundled extension on its lifecycle
  events (see [Pi extension](#pi-extension) below) — pi has no shell-hook system, so
  the extension is what supplies its status.
- **Copilot CLI** has no hooks, so the reconciler reads its status from the pane's
  visible screen: an "esc to cancel" hint ⇒ working, a selection/confirm prompt
  ("enter to select") ⇒ blocked, neither ⇒ idle.
- **Any other agent** (codex, aider, ...) reports its own status with
  `tmux-legion report working|blocked|done`, guided by the bundled [SKILL.md](SKILL.md).
- A reconciler (every ~2s) **discovers** agents three ways: the foreground command
  matches `@legion_agents`, the pane carries a `@pane_agent` tag (set by hooks or
  `spawn`), or — when the foreground command is an interpreter (node, bun, deno) —
  a command in `@legion_agents` appears in the pane's process tree, so
  interpreter-wrapped CLIs are found even without hooks.
- The same reconciler **verifies liveness**: when a pane's tag no longer matches its
  foreground command, it walks the process tree (`ps`) from the pane's PID to tell
  "agent still running under a wrapper" from "agent exited" from "pane recycled",
  clearing stale tags as it goes. Rows are dropped when the pane closes, is reused,
  or the agent has been gone for ~15s. Screen content is only ever read for agents
  that need it (Copilot) — hook-driven agents are never scraped.
- State lives in a JSON file per tmux server (`~/.local/state/tmux-legion/`); writers
  take a lock and replace it atomically, the sidebar redraws on SIGUSR1 pokes.

## Install

### Nix flake

```nix
# Pin to a release tag (recommended); drop the ref to track the default branch.
inputs.tmux-legion.url = "github:hawkish/tmux-legion/v0.3.0";
```

The flake exposes `packages.<system>.default` (the CLI), `packages.<system>.tmuxPlugin`
(for `programs.tmux.plugins` in home-manager), and `overlays.default` (adds
`tmux-legion` and `tmuxPlugins.tmux-legion`). Pull new revisions with
`nix flake update tmux-legion`; develop locally with
`--override-input tmux-legion /path/to/checkout`.

### Manual / TPM-style

```bash
git clone https://github.com/hawkish/tmux-legion ~/.tmux/plugins/tmux-legion
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

[pi](https://pi.dev) has no shell-hook system, so it can't report status the way
Claude Code hooks do (adding `pi` to `@legion_agents` gets its pane discovered via
the process tree, but only with an "unknown" status). Instead, copy or symlink
[pi/tmux-legion.ts](pi/tmux-legion.ts) into `~/.pi/agent/extensions/` — it
reports idle/working/done on pi's lifecycle events.

## Usage

`prefix + g` toggles the sidebar. Inside it: `j`/`k` (or arrows / mouse wheel) move,
`g`/`G` jump to top/bottom, `Enter` focuses the selected agent's pane, `x` kills it
(confirm with `y`), `r` forces a rescan, `q` closes. Clicking a row selects it and
focuses that pane too, and the highlight follows whichever agent pane you focus in tmux.

### CLI

```
tmux-legion report <working|blocked|done|idle|unknown> [-m msg] [--name n] [--pane %id]
tmux-legion list [--json]
tmux-legion spawn [--name n] [--direction right|down|left|up] [--window] [--cwd p] [--focus] -- <cmd...>
tmux-legion wait [--pane %id] --status <s> [--timeout secs]    # exit 0 ok, 2 timeout, 3 pane gone
tmux-legion toggle | open | close
```

### Spawning agents

`spawn` splits a pane, runs the command in it, tags it, and prints the new pane id.
Whether that pane sticks around is entirely up to the command — an agent in
interactive mode keeps the pane; a headless run prints its answer and exits, which
closes the pane. The two CLIs spell that distinction in opposite ways:

| Agent | Mode | Command |
|---|---|---|
| Claude Code | interactive | `claude` |
| Claude Code | interactive, prompt seeded | `claude "<prompt or /skill>"` |
| Claude Code | headless | `claude -p "<prompt>"` |
| Copilot CLI | interactive | `copilot` (optionally `--model <m> -i`) |
| Copilot CLI | headless (autopilot) | `copilot --autopilot --allow-all -p "<prompt>"` |

Claude Code is interactive **by default** and takes its prompt as a *positional*
argument; `-p` is what makes it headless. Copilot is the mirror image: `-i` for
interactive, `-p` for autopilot (and it must not get a prompt in interactive mode —
the folder-trust dialog makes it exit immediately).

```bash
# Interactive Claude in a right-hand split, focused, running a slash command.
# The pane stays open — you can keep chatting after /git-message finishes.
tmux-legion spawn --name committer --focus --cwd "$(pwd)" -- claude "/git-message"

# Interactive Copilot on a specific model.
tmux-legion spawn --name copilot --focus --cwd "$(pwd)" -- copilot --model claude-sonnet-4.6 -i

# Headless review. remain-on-exit keeps the pane after `-p` exits, so there is still
# something to scrape; without it `wait` returns 3 (pane gone) and the output is lost.
tmux set -g remain-on-exit on
PANE=$(tmux-legion spawn --name reviewer --cwd "$(pwd)" -- claude -p "review the diff in $(pwd)")
tmux-legion wait --pane "$PANE" --status done --timeout 600
tmux capture-pane -p -t "$PANE"

# Headless Copilot autopilot, same shape.
tmux-legion spawn --name autop --cwd "$(pwd)" -- \
  copilot --model gpt-5.5 --autopilot --allow-all --max-autopilot-continues 10 -p "review the diff in $(pwd)"
```

If a spawned pane vanishes instantly, the command exited — a headless run that
finished or errored before you could read it. `tmux set -g remain-on-exit on` keeps
the pane around so you can see why.

## Options (set in tmux.conf)

| Option | Default | What |
|---|---|---|
| `@legion_key` | `g` | toggle key (with prefix) |
| `@legion_width` | `15%` | sidebar width (percent or columns) |
| `@legion_position` | `left` | `left` or `right` |
| `@legion_agents` | `claude,copilot,codex,opencode,aider,timtoo` | commands auto-detected as agents |

## Development

```bash
nix develop        # cargo, rustc, rust-analyzer, clippy, rustfmt, tmux
cargo test
cargo build --release
```
