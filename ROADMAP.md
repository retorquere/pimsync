This document only lists pending tasks.

# Features missing for parity with 0.1.x

- Conflict resolution UI improvements
- Protect collections from deletion
- Singlefile storage
- Ample test coverage for the synchronisation algorithm
- Advanced storage settings
  - `partial_sync`
  - `read_only` storages
  - `start_date` and `end_date`
  - `item_types`
  - `post_hook`
  - `fileignoreext`
- Google-specific storage extensions

# Features for stable release

- Security audit
- Document configuration format
- Update general documentation (including a "getting started")
- Dynamically link system TLS library
- Previous beta working solidly for a sensible period

# Non-blocking

- A design for using Google's CalDav OAuth
  - A proxy for their proprietary API might also be a good suggestion
- JSCal stuff
- JMAP lib
- vdirsycner jmap storage (requires jscal + generator)
- ical generator (blocks jmap storage)
- Fixer of invalid ics (e.g.: missing UID)
- publish vdir spec
- Protection during collection reconfiguration
