---
title: Changelog
date: 2024-01-10 15:38:06 +0100
type: docs
---

# Changelog

The 2.x series is a complete rewrite of vdirsyncer. For details on vdirsyncer
0.x series, see its separate [changelog][old-changelog].

[old-changelog]: https://vdirsyncer.pimutils.org/en/stable/changelog.html

For details on how to migrate a configuration file from vdirsyncer 0.x, see
the [migration guide](/docs/migration-guide/).

## New features in v2.0.0

- `daemon` will continuously keep storages in sync.
- `sync` requires no user intervention.
- Use `resolve-conflicts` to manually resolve conflicts. Conflict resolution is
  no longer applied automatically during sync.
- If a single file fails or results in conflict, the rest of the synchronisation
  process will continue.
