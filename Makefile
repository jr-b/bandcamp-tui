# Run `make` to list targets. Pass extra arguments with ARGS, e.g. `make run ARGS="--help"`.

.PHONY: help ci fmt fmt-check clippy test test-live deny run play

help: ## List targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F ':.*## ' '{printf "  %-10s %s\n", $$1, $$2}'

ci: fmt-check clippy test deny ## Run every check CI runs

fmt: ## Format code
	cargo fmt --all

fmt-check: ## Check formatting
	cargo fmt --all --check

clippy: ## Lint
	cargo clippy --all-targets --all-features --locked -- -D warnings

test: ## Run tests
	cargo test --all-targets --locked

test-live: ## Smoke tests against the real bandcamp.com (needs network)
	cargo test --test live -- --ignored

deny: ## Check dependencies with cargo-deny
	cargo deny check

run: ## Run the app
	cargo run -- $(ARGS)

play: ## Stream the first playable Discover album through the speakers (no UI)
	cargo run --release --example play_album -- $(ARGS)
