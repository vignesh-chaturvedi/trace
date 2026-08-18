.PHONY: build test lint fmt check clean install

build:
	cargo build --release

test:
	cargo test --workspace

# The layout lint is a build-failing check, not a convention. Cache discipline
# erodes silently; only something that fails catches it the day it happens.
lint:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo run -q --bin trace -- lint

fmt:
	cargo fmt --all

check: fmt test lint

install:
	cargo install --path crates/trace-cli

clean:
	cargo clean
