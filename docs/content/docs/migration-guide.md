---
title: Migration guide
date: 2024-01-10 12:46:41 +0100
type: docs
---

<!-- FIXME: templates don't render titles by default? -->
# Migration guide

This migration guide covers changes to be kept in mind when migrating from the
previous implementation (e.g.: the 0.x series).

## Configuration file format

The configuration file is now parsed as a TOML file, rather than a bespoke
format. The main difference is that sections have a dot (instead of a space)
separating the type and the name.

The following:

```ini
[pair contacts]
```

Becomes:

```toml
[pair.contacts]
```

And the following:

```ini
[storage contacts_local]
```

Becomes this:

```toml
[storage.contacts_local]
```

While the difference is quite subtle, it allows using a standard TOML parser to
read the configuration file, instead of having to implement our own parser.

Other differences are named below.

## Fetch mechanisms

The `shell` and `prompt` mechanisms for fetching passwords  have been dropped.
Only the `command` mechanism remains, and its syntax has changed slightly.

### Fetch / command

A `fetch` definition with a `command` would previously look like this:

```toml
password.fetch = ["command", "hiq", "-dFpassword", "proto=carddavs", "username=..."]
```

The word `fetch` is replaced by `command` and the word `command` is no longer
required as the first parameter:

```toml
password.command = ["hiq", "-dFpassword", "proto=carddavs", "username=..."]
```

### Fetch / shell

The following `shell` example:

```toml
password.fetch = ["shell", "~/.local/bin/get-my-password | head -n1"]
```

Can be replaced with:

```toml
password.command = ["sh", "-c", "~/.local/bin/get-my-password | head -n1"]
```

### Fetch / prompt

For `prompt` examples, using a wrapper script is a simple approach. E.g.: the
following script:

```sh
read PASSWORD
export PASSWORD
vdirsycner sync
```

Works with the following configuration:

```toml
password.command = ["printenv", "PASSWORD"]
```

Keep in mind that is it possible for other processes of the same user to read
environment variables. Usage of a password manager with a `command` is
recommended for best security.

## Manual discovery is no longer required

Discovering collections ahead of time is no longer required. Collections are
discovered automatically. The `discover` command now has an entirely different
use.

The `discover` command merely prints discovered collections as a convenience
for manually configuring collections. It does not affect vdirsyncer's internal
state.

## Creation of collections is now automatic

If a pair is configured to synchronise all collections `from a` and a new
collection is found on storage A, then the collection will automatically be
created on storage B.

For scenarios where automatic creation of collections is undesirable,
individual collection should be specified instead.

## Dry runs are now possible

It is now possible to execute a dry run, which only prints the tentative plan
without executing it.

This can be used to audit new configurations and ensure that the planned
actions make sense.

## Custom encodings for filesystem storage

The filesystem storage saves files as UTF-8, and attempting to use the
`encoding` setting will fail. If another encoding is required for some
scenario, please [open an issue].

<!-- TODO: this should be replaced with a link to a page describing issues and lists -->

[open an issue]: https://todo.sr.ht/~whynothugo/vdirsyncer-rs

## Filesystem `fileext` field

The `fileext` field previously required a leading `.`. This is no longer the
case; the dot is not considered part of the extensions and should be omitted.
The current behaviour is backwards compatible and will ignore leading dots.

## Filesystem collections require an explicit subtype

Filesystem collections must now specify what type of items they contains. E.g.:

```toml
type = "filesystem/icalendar"
```

Or:

```toml
type = "filesystem/vcard"
```

## Collections are declared in a different format

The format for specifying collections from both sides remains the same:

```toml
collections = "all"
```

The format for specifying all collections from a single side also remains the
same:

```toml
collections = ["from b"]
```

Only the above usages remain unchanged.

Collections can now be specified either by `id` or by `href`. The `id` is the
name derived from discovery. The `href` is the full path inside the storage.

Generally, using an `id` is recommended, and using an `href` is reserved for
situations where discovery is not possible or where multiple collections have
the same `id`.

To specify a single collection by name, use:

```toml
collections = [
    { id = "c037725e-e4fd-4b3e-b73d-d5e27d5a90a9" }
]
```

The above will find synchronise collections with the given id between both
storages.

A collection can now also be specified by href, which is useful for servers
that do not support discovery or where multiple collections have the same name:


```toml
collections = [
    { href = "/calendars/hugo/c037725e-e4fd-4b3e-b73d-d5e27d5a90a9/" }
]
```

Note that in the above case, the collection is expected to have the same `href`
in both storages.

Two different collections can mapped on each side:

```toml
collections = [
    # first collection
    {
        # an alias, used for logging:
        "personal",
        # the description of the collection on storage a
        { href = "/calendars/hugo/c037725e-e4fd-4b3e-b73d-d5e27d5a90a9/" },
        # the description of the collection on storage b
        { href = "/personal/" },
    },
    # second collection
    {
        # an alias, used for logging:
        "work",
        # the description of the collection on storage a
        { id = "work" },
        # the description of the collection on storage b
        { href = "/work/" },
    }
]
```

Specifying collection `null` is no longer allowed; configuration should point
to an explicitly collection instead.

<!--
Note: this last item may change in future; it's just a bit non-trivial to do it
during config resolution
-->
