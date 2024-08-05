DESTDIR?=/
PREFIX?=/usr/local

build:
	cargo build -p vdirsyncer --release

.PHONY: install
install: build
	@install -Dm755 target/release/vdirsyncer 	${DESTDIR}${PREFIX}/bin/vdirsyncer

check:
	cargo build
	cargo fmt --check
	cargo clippy --all-targets
	cargo test  # includes examples and doctests
	cargo doc  # fails on broken links
