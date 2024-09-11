DESTDIR?=/
PREFIX?=/usr/local

.PHONY: build
build: target/release/vdirsyncer docs

target/release/vdirsyncer:
	cargo build -p vdirsyncer --release

docs: vdirsyncer.1 vdirsyncer-migration.7

vdirsyncer.1: vdirsyncer.1.scd
	scdoc < vdirsyncer.1.scd > vdirsyncer.1

vdirsyncer-migration.7: vdirsyncer-migration.7.scd
	scdoc < vdirsyncer-migration.7.scd > vdirsyncer-migration.7

.PHONY: install
install: build
	@install -Dm755 target/release/vdirsyncer 	${DESTDIR}${PREFIX}/bin/vdirsyncer
	@install -Dm644 vdirsyncer.1	${DESTDIR}${PREFIX}/share/man/man1/vdirsyncer.1
	@install -Dm644 vdirsyncer-migration.7	${DESTDIR}${PREFIX}/share/man/man7/vdirsyncer-migration.7

clean:
	cargo clean
	rm vdirsyncer.1

check:
	cargo build
	cargo fmt --check
	cargo clippy --all-targets
	cargo test  # includes examples and doctests
	cargo doc  # fails on broken links
