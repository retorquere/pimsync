# Copyright 2023-2025 Hugo Osvaldo Barrera
#
# SPDX-License-Identifier: EUPL-1.2

DESTDIR?=/
PREFIX?=/usr/local

build: target/release/pimsync docs

target/release/pimsync:
	cargo build -p pimsync --release

docs: man html site

.PHONY: site
site: html
	make -C docs html

.PHONY: open-site
open-site: site
	xdg-open docs/build/html/index.html

man: \
	target/pimsync.1 \
	target/pimsync.conf.5 \
	target/pimsync-migration.7

html: \
	target/pimsync.1.html \
	target/pimsync.conf.5.html \
	target/pimsync-migration.7.html

target/%.html: target/%
	mandoc -T html -O style=man-style.css < '$<' | \
	sed -E \
		-e '1,20b' \
		-e 's,(https://[^[:space:]]+),<a href="\1">\1</a>,g' \
		-e 's,(pimsync[a-z\.\-]*)\(([0-9])\),<a href="\1.\2.html">\1(\2)</a>,g' \
	> '$@'

target/%: %.scd
	mkdir -p target
	scdoc < '$<' > '$@'

.PHONY: install
install: build
	@install -Dm755 target/release/pimsync 		${DESTDIR}${PREFIX}/bin/pimsync
	@install -Dm644 target/pimsync.1		${DESTDIR}${PREFIX}/share/man/man1/pimsync.1
	@install -Dm644 target/pimsync.conf.5		${DESTDIR}${PREFIX}/share/man/man5/pimsync.conf.5
	@install -Dm644 target/pimsync-migration.7	${DESTDIR}${PREFIX}/share/man/man7/pimsync-migration.7

clean:
	cargo clean
	rm -rf target docs/build/

check:
	cargo check
	cargo fmt --check
	cargo clippy --all-targets
	cargo test --workspace  # includes examples and doctests
	cargo doc  # fails on broken links

# Rebuild docs as changes occur.
watch-docs:
	sphinx-autobuild docs/source docs/build/html
