# Run `just` to list recipes.
default:
    @just --list

# Run every check CI runs.
ci: fmt-check clippy test deny

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --all-targets --all-features --locked -- -D warnings

test:
    cargo test --all-targets --locked

# Smoke tests against the real bandcamp.com (needs network).
test-live:
    cargo test --test live -- --ignored

deny:
    cargo deny check

run *args:
    cargo run -- {{args}}
