DESTDIR?=/
PREFIX?=/usr/local

.PHONY: build
build: target/release/pimsync docs

target/release/pimsync:
	cargo build -p pimsync --release

docs: pimsync.1 pimsync-config.5 pimsync-migration.7

pimsync.1: pimsync.1.scd
	scdoc < pimsync.1.scd > pimsync.1

pimsync-config.5: pimsync-config.5.scd
	scdoc < pimsync-config.5.scd > pimsync-config.5

pimsync-migration.7: pimsync-migration.7.scd
	scdoc < pimsync-migration.7.scd > pimsync-migration.7

.PHONY: install
install: build
	@install -Dm755 target/release/pimsync 	${DESTDIR}${PREFIX}/bin/pimsync
	@install -Dm644 pimsync.1	${DESTDIR}${PREFIX}/share/man/man1/pimsync.1
	@install -Dm644 pimsync-config.5	${DESTDIR}${PREFIX}/share/man/man5/pimsync.5
	@install -Dm644 pimsync-migration.7	${DESTDIR}${PREFIX}/share/man/man7/pimsync-migration.7

clean:
	cargo clean
	rm pimsync.1 pimsync-config.5 pimsync-migration.7

check:
	cargo build
	cargo fmt --check
	cargo clippy --all-targets
	cargo test  # includes examples and doctests
	cargo doc  # fails on broken links
