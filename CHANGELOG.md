# Breaking changes from the 0.1.x series

## Fetch mechanisms

The `shell` and `prompt` mechanisms for fetching passwords  have been dropped.
Only the `command` mechanism remains, and its syntax has changed slightly.

### Fetch / command

A `fetch` definition with a `command` would previously look like this:

```toml
password.fetch = ["command", "hiq", "-dFpassword", "proto=carddavs", "username=whynothugo@fastmail.com"]
```

The word `fetch` is replaced by `command` and the word `command` is no longer
required as the first parameter:

```toml
password.command = ["hiq", "-dFpassword", "proto=carddavs", "username=whynothugo@fastmail.com"]
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

## Custom encodings for filesystem storage

The filesystem storage saves files as UTF-8, and attempting to use the
`encoding` setting will fail. If another encoding is required for some
scenario, please open an issue.

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

Only the above usages remain the same. To specify a single collection by name,
use:

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
    { # first collection
        "my calendar", # this is an alias, used for logging.
        { href = "/calendars/hugo/c037725e-e4fd-4b3e-b73d-d5e27d5a90a9/" },
        { href = "/work/" },
    },
    { # second collection
        "another calendar", # this is an alias, used for logging.
        { id = "personal" },
        { href = "/personal/" },
    }
]
```
