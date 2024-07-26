Note: This document is still incomplete.

# Security model

Access to creating new items in a storage should be restricted. An actor with
the ability to create new items can poison a storage in a way that other items
are overwritten (and therefore, lost).

Vdirsyncer will retain credentials in memory during its entire lifetime. This
can be improved via https://todo.sr.ht/~whynothugo/vdirsyncer-rs/44. In the
meantime, any actor with read access to vdirsyncer's memory space may extract
secret credentials from it.

Vdirsyncer discovers the server's real hostname and path using DNS-based
discovery. For this, the system resolver is used. It is expected that the
system resolver performs DNSSEC validation and will not return invalid results.
Vdirsyncer does not perform DNSSEC validation itself.

# Manual tasks

The following need to be run manually and ought to be made part of some
automated process:

    cargo-audit audit
