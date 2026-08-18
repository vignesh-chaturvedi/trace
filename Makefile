.PHONY: build test lint fmt check clean install bench bench-smoke

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

install:
	cargo install --path crates/trace-cli

clean:
	cargo clean
