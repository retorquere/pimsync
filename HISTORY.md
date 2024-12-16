Initial development versions of `pimsync` were named `vdirsyncer`, given that
this project sought to be a replacement for the latter.

While both projects fit the same use cases, they diverged in configuration,
interface, and exact features. It became clear that re-using the same name would
be prone to confusion. `pimsync` gained its own identity at this point, but
previous versions were tagged experting as `v2.0.0-betaX`.

With its new name, `pimsync` has started versioning from zero, rather than
continuing where `vdirsyncer` left off.

This document describes the original versions with the previous version scheme.
These are all pre-release tags, which were not packaged by any downstream
distribution. Tags corresponding with these releases have been deleted, to avoid
confusion due to their unaligned version scheme.

# v2.0.0-beta1

This was commit 67850c57b1212f9d2d7d73597620cc331f30e094.

Documentation improvements:

- Introduce this CHANGELOG.
- Include a man page for the vdirsyncer command. See `vdirsyncer(1)`.
- Move the migration guide into a man page. See `vdirsyncer-migration(7)`.
- The migration guide now mentions all missing features that were available in
  the previous implementation.

Fixes and improvements:

- Fixed failure when readiness file descriptor is not a regular file.
- Only use default configuration path if `XDG_CONFIG_HOME` is undefined.
- Improve error output when configuration file is missing or invalid.
- Make username and password optional for CalDav and CardDav.

# v2.0.0-beta0

This was commit 6eb4d3ca2ba41a181f34eabe6fa1d1379ce15373.

- Command line arguments have changed, see `vdirsyncer -h`.
- Implemente readiness notification.
- Improve handling of items that are moved within a collection.
- Fix unnecessary fetching of unchanged items in some edge cases.
- Implement sanitisation of inputs for vdir storages.
- Ensure that vdir files are written atomically.
- Document some security considerations, including DNSSEC limitations.
- Handle home sets with more than one entry.
- Split out libdav and davcli into separate repositories.
- Implement protection of emptied collections.
- Don't load disabled disabled storages and pairs.
- Prevent local concurrent access to the same storage.

# v2.0.0-alpha0

This was commit 40d0154dfd39258c7d7173b3fdb86f00ea03cf59.

New features in this series:

- `daemon` will continuously keep storages in sync.
- `sync` requires no user intervention.
- Use `resolve-conflicts` to manually resolve conflicts. Conflict resolution is
  no longer applied automatically during sync.
- If a single file fails or results in conflict, the rest of the synchronisation
  process will continue.
