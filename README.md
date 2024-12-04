# pimsync

[Source](https://git.sr.ht/~whynothugo/pimsync)
| [Issues](https://todo.sr.ht/~whynothugo/vdirsyncer-rs)
| [Patches](https://lists.sr.ht/~whynothugo/vdirsyncer-devel)
| [Chat](irc://ircs.libera.chat:6697/#pimutils)

This is the repository for `pimsync`, the rewrite and successor of [vdirsyncer].

[vdirsyncer]: https://github.com/pimutils/vdirsyncer

# Requirements

Compiling pimsync requires:

- make
- Rust and Cargo
- libsqlite3 (development package)
- scdoc (for compiling manual pages)

Running pimsync requires:

- libc
- libsqlite3 (runtime package)

# Compilation

Use `make build` to compile pimsync. The build process uses Cargo to produce a
binary and scdoc to compile documentation.

The compiled binary is placed in `./target/release/pimsync`. It can be executed
directly, or copied into your `$PATH`.

# Installation

Use `make install` to install to `/usr/local/`.

# User documentation

User documentation is provided as man pages. Please see `man pimsync` and
`man pimsync.conf` as starting points.

Manual pages are built when running `make build`. They can also be built
independently using `make man`. These same man pages can be rendered as HTML
pages by using `make html`.

# Developer documentation

Documentation for libraries can be built with `cargo doc`.

The documentation for the latest commits are published at:

- https://mirror.whynothugo.nl/pimsync/main/vstorage/

The CalDav and CardDav implementations lie in a separate library, **libdav**.
See [its repository][libdav-repo] and [its documentation][libdav-docs] for
further details.

[libdav-repo]: https://git.sr.ht/~whynothugo/libdav
[libdav-docs]: https://docs.rs/libdav/

# Contributing

See [HACKING.md].

# Credits

Special thanks to the [NLnet foundation] that helped receive financial support from
the [NGI Zero Entrust] program of the European Commission since early 2023.

[NLnet foundation]: https://nlnet.nl/project/vdirsyncer/
[NGI Zero Entrust]: https://www.ngi.eu/ngi-projects/ngi-zero-entrust/

# Licence

Copyright 2023-2024 Hugo Osvaldo Barrera  
Licensed under the EUPL, Version 1.2 only  
SPDX-License-Identifier: EUPL-1.2
