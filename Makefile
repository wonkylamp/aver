.PHONY: up ci-local test test-custody test-action test-e2e-local test-onboarding test-batch test-upgrade test-install test-eval plugin-check test-native test-docker bench bench-native eval schemas-check vectors lint site site-deps

# The ignored tests run Node natively; two at a time keeps an 8-core laptop
# from timing out their runs.
NATIVE_TEST_THREADS ?= 2

# Xcode's /usr/bin/python3 lacks PyYAML and pytest, which the action tests
# and the Python fixtures need; Homebrew's has them.
ifeq ($(shell uname),Darwin)
ifneq ($(wildcard /opt/homebrew/bin),)
export PATH := /opt/homebrew/bin:$(PATH)
endif
endif

up:
	scripts/up.sh

# What CI runs, on this machine (the --run-as test needs Linux).
ci-local: lint test test-native

test: site-deps test-action test-install test-eval plugin-check
	cargo test --workspace --no-fail-fast
	AVER=$${CARGO_TARGET_DIR:-target}/debug/aver action/test/e2e-local.sh
	$(MAKE) test-custody
	cd site && npm test

# The crash matrix: every operation cut at each fault::point, audited, run
# again and audited again (crates/aver/tests/custody; shell checks only).
test-custody:
	cargo test -p aver --features faults --test custody

test-action:
	action/test/run.sh
	action/test/workflows.sh

# The action end to end with no network: action.yml's steps, the real
# scripts and a release aver against a local origin and a mock GitHub API.
# make test runs it on the debug aver cargo test built.
test-e2e-local:
	cargo build --release --quiet -p aver
	action/test/e2e-local.sh

test-install:
	packaging/test/run.sh

# A new user's Claude Code onboarding, offline and with no model call: the
# plugin installed from the local marketplace, its hooks without aver,
# install.sh, doctor and init, a worktree session, claim dedup and a
# teammate without aver. Uses aver from PATH (or AVER=<binary>).
test-onboarding:
	tests/e2e/plugin-onboarding.sh

# The team flows of `aver run` with scripted fake agents, no model: ranking,
# accept, batches with `after:`, retries and budgets, combine, leases and
# notices, rerun --onto and Ctrl-C. Uses aver from PATH (or AVER=<binary>).
test-batch:
	tests/e2e/run-batch.sh

# Other agent tools and uninstalling, offline and with no model call: aver
# integrate claude twice on a fresh repository, Codex, Cursor, Gemini and
# Copilot set up once around the user's text, the tools aver init picks,
# setup --remove byte for byte (torn record and torn file too), doctor
# without node, go or cargo. Uses aver from PATH (or AVER=<binary>).
test-upgrade:
	tests/e2e/upgrade-tools.sh

# The Claude Code plugin and what `aver init` writes are one set of files:
# valid manifests and frontmatter, byte-identical with the templates, and
# Claude Code's own validator where `claude` is installed.
plugin-check:
	cargo test -p aver --test plugin --test integrate_golden --test integrate_text
	@if command -v claude >/dev/null 2>&1; then \
		claude plugin validate integrations/claude-code && claude plugin validate .; \
	else \
		echo "plugin-check: claude is not on PATH; skipped claude plugin validate"; \
	fi

# The evaluation harness with scripted agents and a stub aver (no Docker,
# no claude), and every scenario validated against its reference solution.
test-eval:
	node --test 'bench/eval/test/*.test.ts'

test-native:
	cargo test -p aver -- --ignored --test-threads=$(NATIVE_TEST_THREADS)

# The old name, from before Aver 0.4 deleted the Docker sandbox; remove it
# in the release after 0.4.
test-docker: test-native

bench:
	bench/run.sh

# The A/B evaluation with real Claude Code agents (spends tokens; run it from
# your own terminal). Pass flags with ARGS, e.g. make eval ARGS="--scenarios vague-edge"
# or make eval ARGS="--set hard --agents 3".
eval:
	cargo build --release --quiet -p aver
	node bench/eval/run.ts $(ARGS)

# Native verify against the tests themselves: fixtures/calc and a one-line
# edit in this repository's aver-core (docs/BENCHMARKS.md). CASES=calc
# runs one case.
bench-native:
	cargo build --release --quiet -p aver
	bench/native/run.sh

schemas-check:
	cargo test -p aver-core --test schemas

vectors:
	AVER_REGEN_VECTORS=1 cargo test -p aver-core --test vectors

lint: site-deps
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cd site && npm run --silent typecheck
	site/node_modules/.bin/tsc -p bench/eval/tsconfig.json

site: site-deps
	cd site && npm run --silent build

# Reinstalls when package-lock.json is newer than the last install.
site-deps:
	@cd site && { [ node_modules/.package-lock.json -nt package-lock.json ] || npm ci --no-audit --no-fund; }
