---
title: Changelog
date: 2024-01-10 15:38:06 +0100
type: docs
---

# Changelog

Given that this is a complete rewrite from the 0.x series, the changelog for
previous changes is out-of-scope here.

For details on vdirsyncer 0.x, see its separate [changelog][old-changelog].

[old-changelog]: https://vdirsyncer.pimutils.org/en/stable/changelog.html

For details on how to migration a configuration file from vdirsyncer 0.x, see
the [migration guide](/docs/migration-guide/).

## New features in v2.0.0

- `sync --continuous` will continuously synchronise storages.
- If a single file fails or results in conflict, the rest of the
  synchronisation process will continue.
