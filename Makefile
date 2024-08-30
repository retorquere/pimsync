DESTDIR?=/
PREFIX?=/usr/local

.PHONY: build
build: target/release/vdirsyncer vdirsyncer.1

target/release/vdirsyncer:
	cargo build -p vdirsyncer --release

vdirsyncer.1: vdirsyncer.1.scd
	scdoc < vdirsyncer.1.scd > vdirsyncer.1

.PHONY: install
install: build
	@install -Dm755 target/release/vdirsyncer 	${DESTDIR}${PREFIX}/bin/vdirsyncer
	@install -Dm644 vdirsyncer.1	${DESTDIR}${PREFIX}/share/man/man1/vdirsyncer.1

clean:
	cargo clean
	rm vdirsyncer.1

check:
	cargo build
	cargo fmt --check
	cargo clippy --all-targets
	cargo test  # includes examples and doctests
	cargo doc  # fails on broken links
