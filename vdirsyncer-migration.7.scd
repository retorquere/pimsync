vdirsyncer-migration(7)

# NAME

vdirsyncer-migration - migration guide for vdirsyncer

# DESCRIPTION

This migration guide covers changes to be kept in mind when migrating from the
previous implementation (e.g.: the 0.x series). It is geared towards users with
a working configuration of the previous implementation.

# MISSING FEATURES

The following features were previously available, but are not currently
available in v2.0.0:

- HTTP Digest Auth.
- *partial_sync* configuration parameter.
- Encodings other than UTF-8.
- *post_hook* configuration parameter.
- *fileignoreext* configuration parameter.
- *start_date*, *end_date* and *item_types* configuration parameters for CalDav
storages.
- Customisation of the *User-Agent*.
- The *singlefile* storage.

# CONFIGURATION FILE FORMAT

The configuration file is now parsed as a TOML file, rather than a bespoke
format. The main difference is that sections have a dot (instead of a space)
separating the type and the name.

The following:

```
[pair contacts]
```

Becomes:

```
[pair.contacts]
```

And the following:

```
[storage contacts_local]
```

Becomes this:

```
[storage.contacts_local]
```

While the difference is quite subtle, it allows using a standard TOML parser to
read the configuration file, instead of having to implement our own parser.

# FETCH MECHANISMS

The *shell* and *prompt* mechanisms for fetching passwords  have been dropped.
Only the *command* mechanism remains, and its syntax has changed slightly.

A *fetch* definition with a *command* would previously look like this:

```
password.fetch = ["command", "hiq", "-dFpassword", "proto=carddavs", "username=..."]
```

The word *fetch* is replaced by *command* and the word *command* is no longer
required as the first parameter:

```
password.command = ["hiq", "-dFpassword", "proto=carddavs", "username=..."]
```

## MIGRATING FETCH / SHELL

The following `shell` example:

```
password.fetch = ["shell", "~/.local/bin/get-my-password | head -n1"]
```

Can be replaced with:

```
password.command = ["sh", "-c", "~/.local/bin/get-my-password | head -n1"]
```

## MIGRATING FETCH / PROMPT

Use an external prompter instead. For a GUI prompter, consider using:

```
password.command = ["zenity ", "--password"]
```

*vdirsyncer(1)* will prompt for passwords in sequence, so no special
considerations are required to prevent concurrent prompts.

# MANUAL DISCOVERY IS NO LONGER REQUIRED

Discovering collections ahead of time is no longer required. Collections are
discovered automatically. The *discover* command now has an entirely different
use.

The *discover* command merely prints discovered collections as a convenience
for manually configuring collections. It does not affect vdirsyncer's internal
state. See *vdirsyncer(1)* for details.

# AUTOMATIC CREATION OF COLLECTIONS

If a pair is configured to synchronise all collections *from a* and a new
collection is found on storage A, then the collection will automatically be
created on storage B.

For scenarios where automatic creation of collections is undesirable,
individual collection should be explicitly specified instead.

# DRY RUNS ARE NOW POSSIBLE

It is now possible to execute a dry run, which only prints the tentative plan
without executing it.

This can be used to audit new configurations and ensure that the planned
actions make sense.

# FILESYSTEM STORAGES HAVE BEEN RENAMED TO VDIR

Filesystem collections have been renamed to *vdir*. They must now specify what
type of items they contains.

A directory with icalendar files should specify:

```
type = "vdir/icalendar"
```

A directory with vcard files should specify:

```
type = "vdir/vcard"
```

# CUSTOM ENCODINGS FOR VDIR STORAGE

The vdir storage saves files as UTF-8. If another encoding is required for some
scenario, please open an issue at:

	https://todo.sr.ht/~whynothugo/vdirsyncer-rs

; TODO: this should be replaced with a link to a page describing issues and lists

# FILESYSTEM FILEEXT FIELD

The *fileext* field for filesystem storages required a leading dot. This is no
longer the case; the dot is not considered part of the extensions and should be
omitted. Omit the leading dot when porting configurations.

# COLLECTIONS ARE DECLARED IN A DIFFERENT FORMAT

The format for specifying collections from both sides remains the same:

```
collections = "all"
```

The format for specifying all collections from a single side also remains the
same:

```
collections = ["from b"]
```

Only the above usages remain unchanged.

Collections can now be specified either by *id* or by *href*:

- The *id* is the name from discovery.
- The *href* is the full path inside the storage.

Generally, using an *id* is recommended, and using an *href* is reserved for
situations where discovery is not possible or where multiple collections have
the same *id*.

To specify a single collection by name, use:

```
collections = [
    { id = "c037725e-e4fd-4b3e-b73d-d5e27d5a90a9" }
]
```

The above will find synchronise collections with the given id between both
storages.

A collection can now also be specified by *href*:

```
collections = [
    { href = "/calendars/hugo/c037725e-e4fd-4b3e-b73d-d5e27d5a90a9/" }
]
```

Note that in the above case, the collection is expected to have the same *href*
in both storages.

Two different collections can mapped on each side:

```
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

Specifying collection *null* is no longer allowed; configuration should point
to an explicitly collection instead.
