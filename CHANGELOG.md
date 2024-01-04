# Breaking changes from the 0.1.x series

## Fetch mechanisms

The `shell` and `prompt` mechanisms for fetching passwords  have been dropped.
Only the `command` mechanism remains.

The following `shell` example:

```toml
password.fetch = ["shell", "~/.local/bin/get-my-password | head -n1"]
```

Can be replaced with:

```toml
password.fetch = ["command", "sh", "-c", "~/.local/bin/get-my-password | head -n1"]
```

For `prompt` examples, using a wrapper script is a simple approach. E.g.: the
following script:

```sh
read PASSWORD
export PASSWORD
vdirsycner --sync
```

Works with the following configuration:

```toml
password.fetch = ["command", "printenv", "PASSWORD"]
```

## Manual discovery is no longer required

Discovering collections ahead of time is no longer required. Collections are
discovered automatically. The `discover` command is gone.

## Custom encodings for filesystem storage

The filesystem storage saves files as UTF-8, and attempting to use the
`encoding` setting will fail. If another encoding is required for some
scenario, please open an issue.
