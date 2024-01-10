---
title: Migration guide
date: 2024-01-10 12:46:41 +0100
type: docs
---

<!-- FIXME: templates don't render titles by default? -->
# Migration guide

This migration guide covers changes to be kept in mind when migrating from the
previous implementation (e.g.: the 0.x series).

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
vdirsycner --sync
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
discovered automatically. The `discover` command is gone.

The `-d`/`--discover` flag merely prints discovered collections as a
convenience for manually configuring collections. It does not affect
vdirsyncer's internal state.

## Custom encodings for filesystem storage

The filesystem storage saves files as UTF-8, and attempting to use the
`encoding` setting will fail. If another encoding is required for some
scenario, please [open an issue].

<!-- TODO: this should be replaced with a link to a page describing issues and lists -->

[open an issue]: https://todo.sr.ht/~whynothugo/vdirsyncer-rs

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

To specify a single collection by name, use:

```toml
collections = [
    { id = "c037725e-e4fd-4b3e-b73d-d5e27d5a90a9" }
]
```

A collection can now also be specified by href, which is useful for servers
that do not support discovery or where multiple collections have the same name:


```toml
collections = [
    { href = "/calendars/hugo/c037725e-e4fd-4b3e-b73d-d5e27d5a90a9/" }
]
```

Finally, two different collections can mapped on each side.

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
