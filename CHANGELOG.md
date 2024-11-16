# v2.0.0-beta2

- Renamed project to `pimsync`.
- Redesigned configuration file.
- Include a man page for `pimsync(1)` and `pimsync.conf(5)`.
- Several minor fixes.
- Introduce support for Dav over Unix sockets.
- Allow specifying a custom configuration file.
- Many documentation improvements.
- Collection ID was sometimes referred to as "collection name". All references
  now specify "collection id"

# v2.0.0-beta1

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

New features in this series:

- `daemon` will continuously keep storages in sync.
- `sync` requires no user intervention.
- Use `resolve-conflicts` to manually resolve conflicts. Conflict resolution is
  no longer applied automatically during sync.
- If a single file fails or results in conflict, the rest of the synchronisation
  process will continue.
