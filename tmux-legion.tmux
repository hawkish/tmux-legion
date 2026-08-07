#!/usr/bin/env bash
# tmux-legion plugin entry point. Resolves the binary, binds the toggle key,
# and installs refresh hooks. Starts no processes: the sidebar only exists
# while its pane does.

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ -x "$CURRENT_DIR/bin/tmux-legion" ]; then
  BIN="$CURRENT_DIR/bin/tmux-legion"
elif [ -x "$CURRENT_DIR/target/release/tmux-legion" ]; then
  BIN="$CURRENT_DIR/target/release/tmux-legion"
else
  BIN="$(command -v tmux-legion || true)"
fi
if [ -z "$BIN" ]; then
  tmux display-message "tmux-legion: binary not found (build with cargo, or add to PATH)"
  exit 0
fi

tmux set-option -g @legion_bin "$BIN"

get_opt() {
  local value
  value="$(tmux show-option -gqv "$1")"
  echo "${value:-$2}"
}

tmux bind-key "$(get_opt @legion_key g)" run-shell -b "\"$BIN\" toggle"

# Slot keys: jump straight to the agent whose status is on that keyboard LED.
# Root-table bindings, so no prefix. The defaults assume the Keychron numpad
# 4/5/6 keys are remapped to F14/F15/F16 in firmware -- tmux has no F13+ names
# and spells those S-F2/S-F3/S-F4 (it follows xterm, where F13-F24 are S-F1
# onwards). Position in the list picks the slot.
#   set -g @legion_slot_keys ''                # disable
#   set -g @legion_slot_keys 'M-4,M-5,M-6'     # unmodified keyboard
slot=1
for key in $(get_opt @legion_slot_keys 'S-F2,S-F3,S-F4' | tr ',' ' '); do
  [ "$slot" -le 3 ] || break
  tmux bind-key -n "$key" run-shell -b "\"$BIN\" focus --slot $slot"
  slot=$((slot + 1))
done

# Indexed hook slots ([71]) so user-defined hooks are not clobbered.
# poke is a cheap no-op while the sidebar is closed.
for hook in after-new-window after-select-window after-select-pane pane-exited session-closed; do
  tmux set-hook -g "${hook}[71]" "run-shell -b \"\\\"$BIN\\\" poke\""
done
