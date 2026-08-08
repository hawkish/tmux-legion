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
inputs.tmux-legion.url = "github:hawkish/tmux-legion/v0.4.0";
```

The flake exposes `packages.<system>.default` (the CLI), `packages.<system>.tmuxPlugin`
(for `programs.tmux.plugins` in home-manager), and `overlays.default` (adds
`tmux-legion` and `tmuxPlugins.tmux-legion`). Pull new revisions with
`nix flake update tmux-legion`; develop locally with
`--override-input tmux-legion /path/to/checkout`.

### After upgrading

A rebuild is not enough. A running tmux server keeps the options it read at
startup, so `@legion_bin` still points at the previous store path and the
sidebar pane is still the process launched from it — you get the old binary with
no error anywhere, which looks like the new features silently not working.

```bash
tmux source-file ~/.config/tmux/tmux.conf   # re-runs the plugin entry script
tmux kill-pane -t "$(tmux show-option -gqv @legion_pane)"  # then reopen: prefix + g
```

`tmux kill-server` does the same thing more bluntly.

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

`prefix + g` toggles the sidebar. Inside it: `-`/`+` (or `=`, arrows, mouse wheel) move,
`g`/`G` jump to top/bottom, `Enter` focuses the selected agent's pane, `x` kills it
(confirm with `y`), `r` forces a rescan, `q` closes. Clicking a row selects it and
focuses that pane too, and the highlight follows whichever agent pane you focus in tmux.
Entries that hold a keyboard slot show the key that jumps to them (see
[Keyboard LEDs](#keyboard-leds)).

### CLI

```
tmux-legion report <working|blocked|done|idle|unknown> [-m msg] [--name n] [--pane %id]
tmux-legion list [--json]
tmux-legion spawn [--name n] [--direction right|down|left|up] [--window] [--cwd p] [--focus] -- <cmd...>
tmux-legion wait [--pane %id] --status <s> [--timeout secs]    # exit 0 ok, 2 timeout, 3 pane gone
tmux-legion focus --slot <n>                                   # jump to the agent on keyboard slot n
tmux-legion toggle | open | close
```

### Keyboard LEDs

On a Keychron Q0 Max the first three agents are mirrored onto the numpad `4`, `5` and `6`
keys: the key lights up in the agent's status colour, and pressing it jumps to that agent's
pane. The sidebar shows which key belongs to which agent next to its name.

| status | key colour |
|---|---|
| blocked | red, blinking |
| working | amber, blinking |
| done | teal |
| idle | green |
| unknown | violet |
| no agent | blue (the floor) |

The two statuses that mean *something is happening right now* blink at 1 Hz, so a key
that wants you is the one that moves; the settled statuses hold steady and stay readable.
Since a single key cannot be dimmed (see below), the blink alternates the status colour
with the floor colour — half of every cycle a blinking key looks exactly like an empty
slot. Blinking is also the one thing that puts the keyboard under steady USB traffic:
two writes a second while any agent is working or blocked, and none at all when they
have all settled.

**tmux-legion takes over the keyboard's lighting while the sidebar is open.** It
switches to a per-key effect, sets the global brightness, and floors every key
that isn't a slot to one quiet colour, so the agent keys are the only ones that
differ.

Note what this cannot do: **a key cannot be individually darkened.** The
firmware ignores a per-key write whose saturation or value is zero (the key
simply keeps what it had), and treats any non-zero value as "on" rather than as
a level — so brightness is global, and every lit key is equally bright. Unused
keys are therefore a different *colour*, not dark. Switching the backlight off
does darken the board, but then nothing renders at all, agent keys included.

Two consequences worth knowing before you enable this:

- **Your stored per-key colours are overwritten and cannot be restored by
  tmux-legion** — the protocol offers no way to read them first. Re-apply your
  profile from Keychron Launcher if you want them back.
- **The lighting stays as tmux-legion left it after the sidebar closes**, for
  the same reason: there is nothing meaningful to restore it to. The slot keys
  drop back to the floor so no stale status is left showing.

The backlight level is left alone unless you set `@legion_led_brightness` — it
is global, so it is your only dimming control and the one the keyboard's own
keys adjust. Set `@legion_led_effect keep` to leave the effect alone too.

`set -g @legion_led_floor keep` skips the floor entirely: tmux-legion writes
nothing but the agent keys, and an empty slot hands its key back to whatever
your own profile stores. That is the better arrangement *if* your keyboard can
store a dark colour per key.

A Q0 Max cannot, as far as we could establish. Setting a key to `000000` in
Launcher's per-key editor renders it white, not black — a zero colour reads as
"no override" there too. The effect list's "none" entry does darken the board,
but that is the backlight switch: with it off, nothing renders at all, agent
keys included. So on this hardware the floor is the practical choice.

Slots are sticky: an agent keeps its key until its pane goes away, and a freed key is
handed to the oldest agent that hasn't got one. Agents past the third get no key.

Setup, once:

- **Wire the keyboard up by USB.** The raw-HID interface it needs is not exposed over
  Bluetooth or the 2.4 GHz dongle.
- **Remap the three keys in Keychron Launcher** so pressing one can mean "focus that agent"
  instead of typing a digit. `F17`, `F18` and `F19` are the safe choices — see below. To
  skip the remap entirely, bind something else: `set -g @legion_slot_keys 'M-4,M-5,M-6'`.
- **Quit Keychron Launcher** when you're done. It holds the keyboard exclusively, so
  tmux-legion cannot open it while Launcher is connected.
- On macOS you may need to grant your terminal **Input Monitoring** under System Settings →
  Privacy & Security.
- On Linux `/dev/hidraw*` is root-only by default, so the LEDs stay dark until a udev rule
  grants access. On NixOS, `hardware.keyboard.qmk.enable = true`, or explicitly:
  `KERNEL=="hidraw*", ATTRS{idVendor}=="3434", MODE="0660", TAG+="uaccess"`. Without it
  everything else still works — the sidebar treats it as "no keyboard attached".

#### Getting the remapped keys to actually arrive

Two things sit between the keyboard and tmux, and both bite.

**macOS eats some F-keys.** `F13` is Print Screen and `F14`/`F15` are display brightness —
they never reach the terminal at all, whatever the keyboard sends. Use `F17`–`F19`.

**Modern terminals encode them as CSI-u, which tmux does not name.** A terminal speaking the
Kitty keyboard protocol sends `F17` as `ESC [ 57380 u`; tmux has no key name for that, so the
key arrives as literal text and no binding matches. `S-F5` (tmux's xterm-era name for `F17`)
only works if your terminal sends the legacy sequence. Bind the raw sequence with `user-keys`
instead:

```tmux
# F17, F18, F19 in the Kitty keyboard protocol
set -s user-keys[0] "\033[57380u"
set -s user-keys[1] "\033[57381u"
set -s user-keys[2] "\033[57382u"
set -g @legion_slot_keys 'User0,User1,User2'
```

The codes are `57376 + (n - 13)` for `Fn`, so `F13` is `57376` and `F19` is `57382`. To check
what your terminal actually sends, run `cat -v` and press the key: `^[[57380u` means the
above applies, plain `^[[31~` means the `S-F5` naming works and you can skip `user-keys`.

The LEDs are driven by the sidebar, so they are live only while the sidebar pane is open;
closing it drops the three keys back to the floor. Everything degrades quietly — with no
keyboard attached, or on Bluetooth, the sidebar behaves exactly as before. If a keyboard was
found and then stopped responding, a red `⌨` appears in the sidebar footer.

`@legion_led_effect` defaults to the per-key effect measured on a Q0 Max. If your firmware
numbers effects differently, set it to the right id (decimal or `0x`-prefixed).

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
| `@legion_slot_keys` | `S-F2,S-F3,S-F4` | keys that jump to slots 1-3 (no prefix); empty to disable |
| `@legion_led_effect` | `23` | keyboard lighting effect to switch to, or `keep` to leave it alone |
| `@legion_led_brightness` | unset | force the global backlight to this level; unset leaves it alone |
| `@legion_led_floor` | on | `keep` skips the floor, leaving your own per-key colours in place |

## Development

```bash
nix develop        # cargo, rustc, rust-analyzer, clippy, rustfmt, tmux
cargo test
cargo build --release
```
