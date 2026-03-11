# Copyright 2023-2025 Hugo Osvaldo Barrera
#
# SPDX-License-Identifier: EUPL-1.2

DESTDIR?=/
PREFIX?=/usr/local

build: target/release/pimsync

target/release/pimsync:
	cargo build -p pimsync --release

docs: html site

.PHONY: site
site: html docs/build/html/man-style.css
	make -C docs html

docs/build/html/man-style.css:
	install -D man-style.css -t docs/build/html/

.PHONY: open-site
open-site: site
	xdg-open docs/build/html/index.html

html: \
	target/pimsync.1.html \
	target/pimsync.conf.5.html \
	target/pimsync-migration.7.html

target/%.html: %
	mkdir -p target
	mandoc -T html -O style=man-style.css < '$<' | \
	sed -E \
		-e '1,20b' \
		-e 's,(pimsync[a-z\.\-]*)\(([0-9])\),<a href="\1.\2.html">\1(\2)</a>,g' \
	> '$@'

.PHONY: install
install: build
	@install -Dm755 target/release/pimsync 	-t ${DESTDIR}${PREFIX}/bin/
	@install -Dm644 pimsync.1		-t ${DESTDIR}${PREFIX}/share/man/man1/
	@install -Dm644 pimsync.conf.5		-t ${DESTDIR}${PREFIX}/share/man/man5/
	@install -Dm644 pimsync-migration.7	-t ${DESTDIR}${PREFIX}/share/man/man7/
	@install -Dm644 LICENCE			-t ${DESTDIR}${PREFIX}/share/licenses/pimsync/
	@install -Dm644 contrib/_pimsync	-t ${DESTDIR}${PREFIX}/share/zsh/site-functions/

clean:
	cargo clean
	rm -rf target docs/build/

check:
	cargo check
	cargo fmt --check
	cargo clippy --all-targets --all
	cargo test  # includes examples and doctests
	cargo doc --no-deps  # fails on broken links
	cargo-deny check
	mandoc -W error < pimsync.1 > /dev/null
	mandoc -W error < pimsync.conf.5 > /dev/null
	mandoc -W error < pimsync-migration.7 > /dev/null

	# test with optional JMAP feature
	cargo check --features jmap
	cargo clippy --all-targets --features jmap
	cargo test --workspace --features jmap
	cargo doc --features jmap

# Rebuild docs as changes occur.
watch-docs: html
	sphinx-autobuild docs/source docs/build/html
