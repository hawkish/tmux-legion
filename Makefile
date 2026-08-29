# Release publishing.
#
# The release commit itself is still made by hand — bump the version in
# Cargo.toml, refresh Cargo.lock, repoint the README flake pin, and commit it as
# "📦 release: vX.Y.Z". `make release` is the step after that, the one that kept
# getting skipped: tag the commit, push the tag, and open the GitHub release.
#
# Recipes stay one command per line: macOS ships GNU Make 3.81, which has no
# .ONESHELL, so anything spanning lines has to survive a fresh shell per line.

SHELL := bash
.SHELLFLAGS := -eu -o pipefail -c
# The checks are ordered cheapest-first and report one problem at a time.
.NOTPARALLEL:

# Cargo.toml is the single source of truth for the version — flake.nix reads it
# the same way.
VERSION := $(shell awk -F'"' '/^\[package\]/{p=1;next} /^\[/{p=0} p && /^version/{print $$2; exit}' Cargo.toml)
TAG := v$(VERSION)
NOTES ?= .github/release-notes/$(TAG).md
TEMPLATE := .github/release-notes/template.md

.PHONY: help
help:
	@echo "make notes    scaffold $(NOTES) from the commits since the last tag"
	@echo "make check    say whether $(TAG) is ready to publish, changing nothing"
	@echo "make release  tag $(TAG), push it, and create the GitHub release"

# Everything that must hold before anything is published. Run it on its own for
# a dry run: every check only reads.
.PHONY: check
check: check-tools check-tree check-commit check-tag check-notes
	@echo "$(TAG) is ready to publish"

# Without gh the release would stop with the tag already pushed.
.PHONY: check-tools
check-tools:
	@command -v gh >/dev/null || { echo "check: gh CLI not found" >&2; exit 1; }

.PHONY: check-tree
check-tree:
	@[ -z "$$(git status --porcelain)" ] || { echo "check: working tree is dirty" >&2; exit 1; }

.PHONY: check-commit
check-commit:
	@git log -1 --format=%s | grep -q "release: $(TAG)$$" \
		|| { echo "check: HEAD is not the release commit for $(TAG) — it is '$$(git log -1 --format=%s)'" >&2; exit 1; }
	@git fetch --quiet origin main
	@[ "$$(git rev-parse HEAD)" = "$$(git rev-parse origin/main)" ] \
		|| { echo "check: HEAD is not what origin/main points at — push it and let CI build it first" >&2; exit 1; }

# Only the remote is worth checking: `git tag` in release refuses to overwrite a
# local tag, and it runs before anything leaves the machine.
.PHONY: check-tag
check-tag:
	@! git ls-remote --exit-code --tags origin "$(TAG)" >/dev/null 2>&1 \
		|| { echo "check: tag $(TAG) already exists on origin" >&2; exit 1; }

.PHONY: check-notes
check-notes:
	@[ -s "$(NOTES)" ] || { echo "check: no notes at $(NOTES) — run 'make notes', then write them" >&2; exit 1; }

.PHONY: release
release: check
	git tag -a "$(TAG)" -m "$(TAG)"
	git push origin "$(TAG)"
	gh release create "$(TAG)" --verify-tag --title "$(TAG)" --notes-file "$(NOTES)" --latest
	@echo "released $(TAG)"

# The commit subjects are a starting point, not the notes — rewrite them into
# prose that says what changed and why. Everything below them comes from
# $(TEMPLATE), which is plain markdown you can edit directly.
.PHONY: notes
notes: $(TEMPLATE)
	@[ ! -e "$(NOTES)" ] || { echo "notes: $(NOTES) already exists" >&2; exit 1; }
	@mkdir -p "$(dir $(NOTES))"
	@{ echo "## Features"; echo; \
	   git log --reverse --format='- %s' "$$(git describe --tags --abbrev=0)..HEAD"; echo; \
	   sed 's/{{TAG}}/$(TAG)/g' "$(TEMPLATE)"; } > "$(NOTES)"
	@echo "wrote $(NOTES) — edit it, commit it with the release commit, then 'make release'"
