Note: This document is still incomplete.

# Security model

Access to creating new items in a storage should be restricted. An actor with
the ability to create new items can poison a storage in a way that other items
are overwritten (and therefore, lost).

Vdirsyncer will retain credentials in memory during its entire lifetime. This
can be improved via https://todo.sr.ht/~whynothugo/vdirsyncer-rs/44. In the
meantime, any actor with read access to vdirsyncer's memory space may extract
secret credentials from it.

# Manual tasks

The following need to be run manually and ought to be made part of some
automated process:

    cargo-audit audit
