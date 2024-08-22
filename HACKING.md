# Hacking

Unit tests and other basic checks can be run with `make check`. This will also
ensure that documentation has no broken links, examples build, etc.

## Sending patches

Just once, configure the patches list for this repo:

    git config sendemail.to '~whynothugo/vdirsyncer-devel@lists.sr.ht'

Make changes. Run tests. Commit. Then send patches:

    git send-email COMMIT_RANGE
