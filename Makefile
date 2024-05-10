check:
	cargo build
	cargo fmt --check
	cargo clippy --all-targets
	cargo test  # includes examples and doctests
	cargo doc  # fails on broken links
