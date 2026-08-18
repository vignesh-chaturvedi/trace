.PHONY: build test lint fmt check clean install bench bench-smoke contribute preflight

build:
	cargo build --release

test:
	cargo test --workspace

# Container tests skip silently without a runtime, which makes an all-skipped
# suite indistinguishable from a passing one. CI must use this target.
test-ci:
	TRACE_REQUIRE_CONTAINER=1 cargo test --workspace

# The layout lint is a build-failing check, not a convention. Cache discipline
# erodes silently; only something that fails catches it the day it happens.
lint:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo run -q --bin trace -- lint

fmt:
	cargo fmt --all

check: fmt test lint

# A full sweep. Costs money: every task, three repeats, live provider.
bench:
	cargo run --release --bin trace -- bench run --repeats 3

# One task, three repeats. For shaking out the rig before spending on a sweep.
bench-smoke:
	cargo run --release --bin trace -- bench run --repeats 3 --limit 1

# Check everything that could waste a paid sweep, before running one.
# Costs 2 API requests.
preflight:
	@cargo build --release
	@./target/release/trace bench preflight --container

# For someone running this on the project's behalf with their own API key.
# Produces ONE file to send back. Nothing is committed, nothing is pushed.
#
# Preflight runs first and stops the build on failure, so a setup mistake
# costs two requests rather than an hour and a budget.
contribute: preflight
	./target/release/trace bench run --repeats 3 --container \
	  --bundle trace-results.md
	@echo
	@echo "=================================================================="
	@echo "  DONE."
	@echo "  Send this one file back:  trace-results.md"
	@echo "  Do not commit or push it."
	@echo "=================================================================="

install:
	cargo install --path crates/trace-cli

clean:
	cargo clean
