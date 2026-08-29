# Release publishing.
#
# The release commit itself is still made by hand — bump the version in
# Cargo.toml, refresh Cargo.lock, repoint the README flake pin, and commit it as
# "📦 release: vX.Y.Z". `make release` is the step after that, the one that kept
# getting skipped: tag the commit, push the tag, and open the GitHub release.

SHELL := bash
.SHELLFLAGS := -eu -o pipefail -c

# Cargo.toml is the single source of truth for the version — flake.nix reads it
# the same way.
VERSION := $(shell awk -F'"' '/^\[package\]/{p=1;next} /^\[/{p=0} p && /^version/{print $$2; exit}' Cargo.toml)
TAG := v$(VERSION)
NOTES ?= .github/release-notes/$(TAG).md

.PHONY: help
help:
	@echo "make notes    scaffold $(NOTES) from the commits since the last tag"
	@echo "make release  tag $(TAG), push it, and create the GitHub release"

# Every guard runs before anything is published, so a failure here leaves the
# repo untouched.
.PHONY: release
release:
	@command -v gh >/dev/null || { echo "release: gh CLI not found"; exit 1; }
	@[ -z "$$(git status --porcelain)" ] || { echo "release: working tree is dirty"; exit 1; }
	@[ "$$(git rev-parse --abbrev-ref HEAD)" = main ] || { echo "release: not on main"; exit 1; }
	@git log -1 --format=%s | grep -q "release: $(TAG)$$" \
		|| { echo "release: HEAD is not the release commit for $(TAG) — it is '$$(git log -1 --format=%s)'"; exit 1; }
	@git fetch --quiet origin main
	@[ "$$(git rev-parse HEAD)" = "$$(git rev-parse origin/main)" ] \
		|| { echo "release: HEAD is not what origin/main points at — push it and let CI build it first"; exit 1; }
	@! git rev-parse -q --verify "refs/tags/$(TAG)" >/dev/null \
		|| { echo "release: tag $(TAG) already exists locally"; exit 1; }
	@! git ls-remote --exit-code --tags origin "$(TAG)" >/dev/null 2>&1 \
		|| { echo "release: tag $(TAG) already exists on origin"; exit 1; }
	@[ -s "$(NOTES)" ] || { echo "release: no notes at $(NOTES) — run 'make notes', then write them"; exit 1; }
	git tag -a "$(TAG)" -m "$(TAG)"
	git push origin "$(TAG)"
	gh release create "$(TAG)" --verify-tag --title "$(TAG)" --notes-file "$(NOTES)" --latest
	@echo "released $(TAG)"

# A starting point only — the commit subjects go under headings you then rewrite
# into prose. Releases here explain what changed and why, not just what landed.
.PHONY: notes
notes:
	@[ ! -e "$(NOTES)" ] || { echo "notes: $(NOTES) already exists"; exit 1; }
	@mkdir -p "$(dir $(NOTES))"
	@{ \
		echo "## Features"; echo; \
		git log --reverse --format='- %s' "$$(git describe --tags --abbrev=0)..HEAD"; \
		echo; \
		echo "## Install"; echo; \
		echo '### Nix flake'; echo; \
		echo '```nix'; \
		echo '# Pin to a release tag (recommended); drop the ref to track the default branch.'; \
		echo 'inputs.tmux-legion.url = "github:hawkish/tmux-legion/$(TAG)";'; \
		echo '```'; echo; \
		echo '### Manual and TPM-style'; echo; \
		echo '```bash'; \
		echo 'git clone https://github.com/hawkish/tmux-legion ~/.tmux/plugins/tmux-legion'; \
		echo 'cd ~/.tmux/plugins/tmux-legion && cargo build --release'; \
		echo "echo 'run-shell ~/.tmux/plugins/tmux-legion/tmux-legion.tmux' >> ~/.tmux.conf"; \
		echo '```'; echo; \
		echo 'Upgrading needs a tmux config reload, not just a rebuild — see [After upgrading](https://github.com/hawkish/tmux-legion#after-upgrading). The [README](https://github.com/hawkish/tmux-legion#install) covers Claude Code hooks, the agent skill, and the pi extension.'; \
	} > "$(NOTES)"
	@echo "wrote $(NOTES) — edit it, commit it with the release commit, then 'make release'"
