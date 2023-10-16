check:
	cargo build
	cargo fmt --check
	cargo clippy
	cargo test  # includes examples and doctests
	cargo doc  # fails on broken links
