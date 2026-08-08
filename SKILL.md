---
name: tmux-legion
description: Spawn and monitor AI agents in tmux panes with a live status sidebar.
  Use when running inside tmux (the TMUX env var is set) to run another agent
  (claude, copilot, ...) in a sibling pane, report your own status
  (working/blocked/done), see what other agents are doing, or wait for one to finish.
---

# tmux-legion

`tmux-legion` tracks every AI agent in the current tmux server and shows them in a
sidebar with a status: **working**, **blocked**, **done**, idle, or unknown.

## Preconditions

- Only works inside tmux: check that `$TMUX` is set. If it isn't, don't use this skill.
- The `tmux-legion` binary must be on PATH (`command -v tmux-legion`).

## Report your status

**If you are Claude Code, do NOT self-report** — your status is tracked automatically
via hooks. Self-reporting is essential for every other agent (Copilot CLI, etc.):

```bash
tmux-legion report working --message "refactoring auth module"   # when you start a task
tmux-legion report blocked --message "need decision on schema"   # BEFORE asking the user anything
tmux-legion report working                                       # when you resume
tmux-legion report done                                          # when you finish
```

Rules:
- Report `working` when you begin, and again whenever you resume after being blocked.
- Report `blocked` right before any question or permission request that waits on the user.
- Always report `done` on every exit path, including failures.
- `--name <n>` sets the display name shown in the sidebar (e.g. `--name copilot`).

## Spawn a sibling agent

Run another agent in a new pane. stdout is the new pane's id — capture it:

```bash
PANE=$(tmux-legion spawn --name reviewer -- claude -p "review the diff in $(pwd)")
```

Options: `--direction right|down|left|up` (default right), `--window` for a new
window instead of a split, `--cwd <path>`, `--focus` to move focus to the new pane.
The `--` before the command is required.

### Agent-specific invocation

Different agents require different CLI invocations — always use the right one:

| Agent | Mode | Command |
|-------|------|---------|
| Claude Code | interactive | `claude` |
| Claude Code | interactive with a seeded prompt | `claude "<prompt or /skill>"` |
| Claude Code | non-interactive | `claude -p "<prompt>"` |
| Copilot CLI | interactive | `copilot --model claude-sonnet-4.6` |
| Copilot CLI | interactive with a seeded prompt | `copilot --model claude-sonnet-4.6 -i "<prompt>"` |
| Copilot CLI | autopilot with prompt | `copilot --model gpt-5.5 --autopilot --allow-all --max-autopilot-continues 10 -p "<prompt>"` |

**Copilot CLI interactive** (user sees trust dialog and can then chat — do not pass `-p`):
```bash
PANE=$(tmux-legion spawn --name copilot --focus --cwd "$(pwd)" -- copilot --model claude-sonnet-4.6)
```

**Copilot CLI interactive with a seeded prompt** — `-i, --interactive <prompt>` *requires*
a prompt argument; it starts interactive mode and auto-submits that prompt, so the session
stays open afterwards. A bare `-i` with no argument makes copilot exit immediately and the
pane dies:
```bash
PANE=$(tmux-legion spawn --name copilot --focus --cwd "$(pwd)" -- copilot --model claude-sonnet-4.6 -i "review the diff in $(pwd)")
```

**Copilot CLI autopilot with prompt** — non-interactive, runs a task and exits:
```bash
PANE=$(tmux-legion spawn --name my-task --cwd "$(pwd)" -- copilot --model gpt-5.5 --autopilot --allow-all --max-autopilot-continues 10 -p "review the diff in $(pwd)")
```

**Claude Code interactive** — the seeded prompt is a *positional* argument (copilot uses
`-i` for the same thing). The session stays open so the user can keep chatting, and slash
commands work:
```bash
PANE=$(tmux-legion spawn --name committer --focus --cwd "$(pwd)" -- claude "/git-message")
```

**Claude Code non-interactive** (`-p`) — prints the result and exits, which closes the
pane. Set `remain-on-exit` first if you want to read what it said:
```bash
tmux set -g remain-on-exit on
PANE=$(tmux-legion spawn --name reviewer -- claude -p "review the diff in $(pwd)")
tmux-legion wait --pane "$PANE" --status done --timeout 600
tmux capture-pane -p -t "$PANE"
```

> If a spawned pane vanishes instantly, the command exited (a `-p`/`--autopilot` run
> that finished or errored). `remain-on-exit` keeps the pane around so you can read
> the error.

There is no limit on how many agents you can spawn. The first four get a keyboard key
(and its status LED, on a Keychron Q0 Max); the rest are tracked exactly the same way —
sidebar, `list`, `wait`, status hooks — just without a key. A freed key goes to the
oldest agent that hasn't got one.

## Observe and synchronize

```bash
tmux-legion list --json                                  # every tracked agent + status
tmux-legion wait --pane "$PANE" --status done --timeout 600   # block until done
tmux capture-pane -p -t "$PANE"                          # read a sibling's output
```

`wait` exit codes: `0` status reached, `2` timeout, `3` pane disappeared.

## Etiquette

- Don't kill panes you didn't spawn.
- Always `report done` before exiting (non-Claude agents).
- Prefer `wait` over polling `list` in a loop.
