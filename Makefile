# Copyright 2023-2025 Hugo Osvaldo Barrera
#
# SPDX-License-Identifier: EUPL-1.2

DESTDIR?=/
PREFIX?=/usr/local

build: target/release/pimsync man

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
		-e 's,(pimsync[a-z\.\-]*)\(([0-9])\),<a href="\1.\2.html">\1(\2)</a>,g' \
	> '$@'

target/%: %.scd
	mkdir -p target
	scdoc < '$<' > '$@'

.PHONY: install
install: build
	@install -Dm755 target/release/pimsync 		-t ${DESTDIR}${PREFIX}/bin/
	@install -Dm644 target/pimsync.1		-t ${DESTDIR}${PREFIX}/share/man/man1/
	@install -Dm644 target/pimsync.conf.5		-t ${DESTDIR}${PREFIX}/share/man/man5/
	@install -Dm644 target/pimsync-migration.7	-t ${DESTDIR}${PREFIX}/share/man/man7/
	@install -Dm644 LICENCE				-t ${DESTDIR}${PREFIX}/share/licenses/pimsync/

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
watch-docs: html
	sphinx-autobuild docs/source docs/build/html
