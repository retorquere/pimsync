# davcli

[Source](https://git.sr.ht/~whynothugo/vdirsyncer-rs) |
[Issues](https://todo.sr.ht/~whynothugo/vdirsyncer-rs) |
[Patches](https://lists.sr.ht/~whynothugo/vdirsyncer-devel) |
[Chat](irc://ircs.libera.chat:6697/#pimutils)

**`davcli`** is a command line tool to interact with CalDav and CardDav
servers. It is simple interface to `libdav`, and part of the vdirsyncer
project.

# Goals

The main goal of this project is to provide a simple command line interface to
expose simple caldav and carddav operations; essentially equivalents to `ls`,
`cat`, `mkdir`, etc (plus discovery).

Output is printed to `stdout` in a clean format (e.g.: so you can use this in
shell scripts) and all logging is printed to `stderr`.

The output of `--help` should be sufficient to find the basic subcommands, and
appending `--help` to any of these should provide enough information to
understand their usage. If anything is not clear, please open a ticket.

# Configuration

Server details are provided via environment variables. Please take care not to
leave passwords in your shell history.

```console
> export DAVCLI_BASE_URL=https://fastmail.com
> export DAVCLI_USERNAME=vdirsyncer@fastmail.com
> export DAVCLI_PASSWORD=XXX
```

The examples below assume that these variables are properly set.

# Discovery

The `discover` subcommand can be used to test if a server publishes its context
path and calendar home set correctly.

This uses DNS-based discovery as described in [rfc6764], so can be used to test
if DNS is correctly configured for a publicly hosted service:

[rfc6764]: https://www.rfc-editor.org/rfc/rfc6764

```console
> davcli --caldav discover
Discovery successful.
- Context path: https://d277161.caldav.fastmail.com/dav/calendars
- Calendar home set: https://d277161.caldav.fastmail.com/dav/calendars/user/vdirsyncer@fastmail.com/
```

Errors should generally be useful (please report an issue if you find an
obscure error where the underlying root cause is not clear):

```console
> DAVCLI_PASSWORD=wrong_password davcli --caldav discover
Error: error querying current user principal

Caused by:
    0: error during http request
    1: http request returned 401 Unauthorized
```

[The introductory article for davcli][intro] for more details.

[intro]: https://whynothugo.nl/journal/2023/05/01/introducing-davcli/

# Authentication

Passwords must be provided as the environment variable `DAVCLI_PASSWORD`. Only
password-based authentication is implemented at this time.

# Limitations

Nothing is cached. Ever. Performance is basically the worst possible, so
there's enormous room for improvement. A caching mechanism needs to be exposed
by `libdav`.

# Building from source

```console
$ git clone https://git.sr.ht/~whynothugo/vdirsyncer-rs
$ cd vdirsyncer-rs
$ cargo build --release --package davcli
```

The resulting binary will be located in `./target/release/davcli`.

# Todo

This documentation should move to a man page which can be published.

# Licence

<!--
Copyright 2023 Hugo Osvaldo Barrera

SPDX-License-Identifier: EUPL-1.2
-->

Copyright 2023 Hugo Osvaldo Barrera  
Licensed under the EUPL, Version 1.2 only
