# Live tests

This subproject builds a single binary that runs some integration tests with
real caldav servers.

See the [initial intro] for this tool.

[initial intro]: https://whynothugo.nl/journal/2023/04/27/libdav-live-test-results/

# Status

The code isn't pretty but it works.

# Running these tests

You'll need a "profile" file with credentials an expected failures for a
server. For example:

```toml
host = "http://example.com"
username = "testuser"
password = "password"
server = "nextcloud"
```

The `server` attribute is a hint as to which server implementation is being
used. Tests that are known to fail on specific servers will soft-fail. Consider
this a kid of `xfail` feature.

Execute tests with:

```sh
cargo run -p live_tests -- example.profile
```

**DO NOT use the credentials for real/personal/work account for these tests**.
Doing so will almost definitely result in data loss.

# Running with dockerised servers

This repository includes a few sample profiles that work with dockerised caldav
servers. These can be run easily with:

```sh
# radicale
docker run --rm --publish 8001:8001 whynothugo/vdirsyncer-devkit-radicale

# xandikos
docker run --rm --publish 8000:8000 xandikos 
  xandikos -d /tmp/dav -l 0.0.0.0 -p 8000 --autocreate --dump-dav-xml

# baikal
docker run --rm --publish 8002:80 whynothugo/vdirsyncer-devkit-baikal
```

And then execute these tests with:

```sh
cargo build -p live_tests || exit

./target/debug/live_tests live_tests/xandikos.profile
./target/debug/live_tests live_tests/baikal.profile
./target/debug/live_tests live_tests/radicale.profile
```

Test clients use the discovery bootstrapping mechanism, so you can specify your
providers main site as URL as `host` and DNS discovery should resolve the real
server and port automatically.

# Licence

Copyright 2023-2024 Hugo Osvaldo Barrera  
Licensed under the EUPL, Version 1.2 only  
SPDX-License-Identifier: EUPL-1.2
