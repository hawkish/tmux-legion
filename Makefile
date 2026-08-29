# Local development tasks.
#
# Releasing is not one of them: CI publishes. Bump the version in Cargo.toml,
# refresh Cargo.lock, repoint the README flake pin, run `make notes` and write
# them, then commit the lot as "📦 release: vX.Y.Z" and push. The release job in
# .github/workflows/ci.yml tags and publishes once the build is green.
#
# Recipes stay one command per line: macOS ships GNU Make 3.81, which has no
# .ONESHELL, so anything spanning lines has to survive a fresh shell per line.

SHELL := bash
.SHELLFLAGS := -eu -o pipefail -c

# Cargo.toml is the single source of truth for the version — flake.nix and the
# release job both read it the same way.
VERSION := $(shell awk -F'"' '/^\[package\]/{p=1;next} /^\[/{p=0} p && /^version/{print $$2; exit}' Cargo.toml)
TAG := v$(VERSION)
NOTES ?= .github/release-notes/$(TAG).md
TEMPLATE := .github/release-notes/template.md

.PHONY: help
help:
	@echo "make build    cargo build --release, into target/release/tmux-legion"
	@echo "make notes    scaffold $(NOTES) from the commits since the last tag"

# The release build, same as the README's. CI builds through the flake instead,
# so this says nothing about whether `nix build` will succeed.
.PHONY: build
build:
	@command -v cargo >/dev/null || { echo "build: cargo not found — run 'direnv allow', or 'nix develop' first" >&2; exit 1; }
	cargo build --release

# The commit subjects are a starting point, not the notes — rewrite them into
# prose that says what changed and why. Everything below them comes from
# $(TEMPLATE), which is plain markdown you can edit directly.
#
# Commit the result with the release commit: the release job refuses to publish
# without it.
.PHONY: notes
notes: $(TEMPLATE)
	@[ ! -e "$(NOTES)" ] || { echo "notes: $(NOTES) already exists" >&2; exit 1; }
	@mkdir -p "$(dir $(NOTES))"
	@{ echo "## Features"; echo; \
	   git log --reverse --format='- %s' "$$(git describe --tags --abbrev=0)..HEAD"; echo; \
	   sed 's/{{TAG}}/$(TAG)/g' "$(TEMPLATE)"; } > "$(NOTES)"
	@echo "wrote $(NOTES) — edit it, then commit it with the release commit"
