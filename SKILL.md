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

| Agent | Command | Notes |
|-------|---------|-------|
| Claude Code | `claude -p "<prompt>"` | Non-interactive; exits when done |
| GitHub Copilot CLI | `gh copilot` | **No prompt arg** — always launch interactively. Passing a prompt causes an immediate exit due to the folder-trust dialog. |

**Copilot CLI** (interactive — user sees trust dialog and can then chat):
```bash
PANE=$(tmux-legion spawn --name copilot --focus --cwd "$(pwd)" -- gh copilot)
```

**Claude Code** (non-interactive — exits when task is complete):
```bash
PANE=$(tmux-legion spawn --name claude -- claude -p "review the diff in $(pwd)")
```

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
