// Pi extension (https://pi.dev/docs/latest/extensions): report this pane's
// status to the tmux-legion sidebar across the agent lifecycle, playing the
// same role claude/hooks.json plays for Claude Code.
//
// Install: copy or symlink into ~/.pi/agent/extensions/ (auto-discovered).
import { execFile } from "node:child_process";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

function report(status: string): void {
  // Outside tmux, or without the CLI on PATH, do nothing — never bother pi.
  if (!process.env.TMUX || !process.env.TMUX_PANE) return;
  execFile("tmux-legion", ["report", status, "--name", "pi"], () => {});
}

export default function (pi: ExtensionAPI) {
  pi.on("session_start", async () => report("idle"));
  pi.on("agent_start", async () => report("working"));
  pi.on("agent_end", async () => report("done"));
  // The pane flips to done/"exited" via reconcile once pi quits; this just
  // avoids a stale "working" during graceful shutdowns mid-run.
  pi.on("session_shutdown", async () => report("done"));
}
