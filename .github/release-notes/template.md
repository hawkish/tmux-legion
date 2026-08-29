## Install

### Nix flake

```nix
# Pin to a release tag (recommended); drop the ref to track the default branch.
inputs.tmux-legion.url = "github:hawkish/tmux-legion/{{TAG}}";
```

### Manual and TPM-style

```bash
git clone https://github.com/hawkish/tmux-legion ~/.tmux/plugins/tmux-legion
cd ~/.tmux/plugins/tmux-legion && cargo build --release
echo 'run-shell ~/.tmux/plugins/tmux-legion/tmux-legion.tmux' >> ~/.tmux.conf
```

Upgrading needs a tmux config reload, not just a rebuild — see [After upgrading](https://github.com/hawkish/tmux-legion#after-upgrading). The [README](https://github.com/hawkish/tmux-legion#install) covers Claude Code hooks, the agent skill, and the pi extension.
