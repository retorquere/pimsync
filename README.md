# pimsync

[Documentation](https://pimsync.whynothugo.nl/)
| [Source](https://git.sr.ht/~whynothugo/pimsync)
| [Issues](https://todo.sr.ht/~whynothugo/pimsync)
| [Patches](https://lists.sr.ht/~whynothugo/vdirsyncer-devel)
| [Chat](irc://ircs.libera.chat:6697/#pimutils)

This is the repository for `pimsync`, the rewrite and successor of [vdirsyncer].

[vdirsyncer]: https://github.com/pimutils/vdirsyncer

For documentation (including usage, compilation, installation and packaging),
see [the online documentation website][docs]. This website also includes an
HTML version of the manual pages.

[docs]: https://pimsync.whynothugo.nl/

## Building the documentation website

You can build the documentation website locally with `make site`. This
requires the following extra dependencies:

- mandoc
- py3-sphinx

Open the file `docs/build/html/index.html` to view the site locally.

The raw pages are also readable from the `docs/` directory inside this
repository.

## Developer documentation

The underlying synchronisation implementation is implemented in the
[`vstorage`] library. If you want to make a different user interface based on
pimsync, then you'll want to use this library. Pimsync itself merely parses a
configuration file and executes the algorithms implemented in `vstorage`.

[`vstorage`]: https://git.sr.ht/~whynothugo/vstorage/

The following libraries were also developed as part of this project:

- `libdav`: CalDav and CardDav client implementations.
  [Repository][libdav-repo], [documentation][libdav-docs].
- `davcli`: CalDav and CardDav command line tool. [Repository][davcli-repo].
- `vparser`: Non-validating flexible parser for iCalendar and vCard data.
  [Repository][vparser-repo], [documentation][vparser-docs].

[libdav-repo]: https://git.sr.ht/~whynothugo/libdav
[libdav-docs]: https://docs.rs/libdav/
[davcli-repo]: https://git.sr.ht/~whynothugo/davcli
[vparser-repo]: https://git.sr.ht/~whynothugo/vparser
[vparser-docs]: https://docs.rs/vparser/

# JMAP support

JMAP support is currently experimental and must be enabled at build time via:

```sh
cargo build --features jmap
```

## Contributing

See <https://pimsync.whynothugo.nl/contributing.html>.

## Thanks

Special thanks to the [NLnet foundation] that helped receive financial support from
the [NGI Zero Entrust] program of the European Commission since early 2023.

[NLnet foundation]: https://nlnet.nl/project/vdirsyncer/
[NGI Zero Entrust]: https://www.ngi.eu/ngi-projects/ngi-zero-entrust/

## Licence

Copyright 2023-2024 Hugo Osvaldo Barrera  
Licensed under the EUPL, Version 1.2 only  
SPDX-License-Identifier: EUPL-1.2
