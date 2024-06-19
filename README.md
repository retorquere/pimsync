# vdirsyncer

[Source](https://git.sr.ht/~whynothugo/vdirsyncer-rs) |
[Issues](https://todo.sr.ht/~whynothugo/vdirsyncer-rs) |
[Patches](https://lists.sr.ht/~whynothugo/vdirsyncer-devel) |
[Chat](irc://ircs.libera.chat:6697/#pimutils)

This repository contains work-in-progress rewrite of `vdirsyncer` in Rust, as
well as crates with associated functionality.

For the original Python implementation see: https://github.com/pimutils/vdirsyncer

# User documentation

User documentation is included in the `docs` directory and can be built using
`hugo`. This will likely change in future to something easier to distribute
along with the binaries.

# Developer documentation

Documentation for libraries can be built with `cargo doc`.

The documentation for releases published to crates.io is also available
docs.rs: https://docs.rs/libdav/latest/libdav/

The documentation for the latest commits are published at:

- https://mirror.whynothugo.nl/vdirsyncer/main/vstorage/
- https://mirror.whynothugo.nl/vdirsyncer/main/libdav/


# Contributing

See [HACKING.md].

# Credits

Special thanks to the [NLnet foundation] that helped receive financial support
from the [NGI Assure] program of the European Commission in early 2023.

[NLnet foundation]: https://nlnet.nl/project/vdirsyncer/
[NGI Assure]: https://www.ngi.eu/ngi-projects/ngi-assure/

# Licence

Copyright 2023-2024 Hugo Osvaldo Barrera  
Licensed under the EUPL, Version 1.2 only  
SPDX-License-Identifier: EUPL-1.2
