.PHONY: check fmt lint test deny keys keys-check
check: fmt lint test
fmt:
	cargo fmt --all -- --check
lint:
	cargo clippy --workspace --all-targets -- -D warnings
test:
	cargo test --workspace
deny:
	cargo deny check

# docs/keys.md is generated from `quail keys --json` of the default build.
QUAIL_BIN = $(or $(CARGO_TARGET_DIR),target)/debug/quail
keys:
	cargo build -q -p quail-cli
	python3 tools/keys/keys_md.py --quail $(QUAIL_BIN) > docs/keys.md
keys-check:
	cargo build -q -p quail-cli
	python3 tools/keys/keys_md.py --quail $(QUAIL_BIN) --check docs/keys.md
