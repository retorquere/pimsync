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
leave sensitive credentials in your shell history. Some terminals don't record
commands into history if they're preceded by an empty <kbd>Space</kbd>.

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
- Base url: https://fastmail.com/
- Resolved context path: https://d277161.caldav.fastmail.com/dav/calendars
- Current user principal: https://d277161.caldav.fastmail.com/dav/principals/user/vdirsyncer@fastmail.com/
- Calendar home set: https://d277161.caldav.fastmail.com/dav/calendars/user/vdirsyncer@fastmail.com/
```

Errors should generally be useful (please report an issue if you find an
obscure error where the underlying root cause is not clear):

```console
> DAVCLI_PASSWORD=wrong_password davcli --caldav discover
- Base url: https://fastmail.com/
- Resolved context path: https://d277161.caldav.fastmail.com/dav/calendars
Error: error querying server

Caused by:
    http request returned 401 Unauthorized
```

[The introductory article for davcli][intro] for more details.

[intro]: https://whynothugo.nl/journal/2023/05/01/introducing-davcli/

# Listing and reading items

In order to perform other operations, the context path (obtained via discovery
as shown above) should be speficied as `DAVCLI_BASE_URL`.

Following the example above, this would be:

```console
> export DAVCLI_BASE_URL=https://d277161.caldav.fastmail.com/dav/calendars
```

Previous versions of davcli performed discovery/bootstrap sequence
automatically on each execution. This resulted in slow operations. Discovery
now needs to be done once, manually (it can only be skipped in cases where the
BASE_URL points directly to the final server).

With the above variable set, `find-collections` will list collections belonging
to the current user:

```console
> davcli --caldav find-collections
/dav/calendars/user/vdirsyncer@fastmail.com/00fsWMCvxPHGbMWw/
/dav/calendars/user/vdirsyncer@fastmail.com/031sSQFuFJZXhZ8E/
/dav/calendars/user/vdirsyncer@fastmail.com/07pqbROw8tUg9xcZ/
/dav/calendars/user/vdirsyncer@fastmail.com/0BzTEecb6x69Igdj/
/dav/calendars/user/vdirsyncer@fastmail.com/0CGYH7P3bClmkf3C/
...
```

Items inside these collections can be listed with:

```sh
davcli --caldav list-items /dav/calendars/user/vdirsyncer@fastmail.com/00fsWMCvxPHGbMWw/
```

See `davcli --help` for further details.

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

Copyright 2023-2024 Hugo Osvaldo Barrera  
Licensed under the EUPL, Version 1.2 only  
SPDX-License-Identifier: EUPL-1.2
