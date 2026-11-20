.PHONY: check fmt lint test deny
check: fmt lint test
fmt:
	cargo fmt --all -- --check
lint:
	cargo clippy --workspace --all-targets -- -D warnings
test:
	cargo test --workspace
deny:
	cargo deny check
