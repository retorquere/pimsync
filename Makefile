DESTDIR?=/
PREFIX?=/usr/local

build: target/release/pimsync docs

target/release/pimsync:
	cargo build -p pimsync --release

docs: man html

man: pimsync.1 pimsync.conf.5 pimsync-migration.7

html: pimsync.1.html pimsync.conf.5.html pimsync-migration.7.html

%.html: %
	mandoc -T html -O style=man-style.css < '$<' | \
	sed -E \
		-e '1,20b' \
		-e 's,(https://[^[:space:]]+),<a href="\1">\1</a>,g' \
		-e 's,(pimsync[a-z\.\-]*)\(([0-9])\),<a href="\1.\2.html">\1(\2)</a>,g' \
	> '$@'

%: %.scd
	scdoc < '$<' > '$@'

.PHONY: install
install: build
	@install -Dm755 target/release/pimsync 	${DESTDIR}${PREFIX}/bin/pimsync
	@install -Dm644 pimsync.1	${DESTDIR}${PREFIX}/share/man/man1/pimsync.1
	@install -Dm644 pimsync.conf.5	${DESTDIR}${PREFIX}/share/man/man5/pimsync.conf.5
	@install -Dm644 pimsync-migration.7	${DESTDIR}${PREFIX}/share/man/man7/pimsync-migration.7

clean:
	cargo clean
	rm pimsync.1 pimsync.conf.5 pimsync-migration.7
	rm *.html

check:
	cargo check
	cargo fmt --check
	cargo clippy --all-targets
	cargo test --workspace  # includes examples and doctests
	cargo doc  # fails on broken links
