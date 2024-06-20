# Hacking

Unit tests and other basic checks can be run with `make check`. This will also
ensure that documentation has no broken links, examples build, etc.

## Integration tests

A small integration tests helper program is available as part of this project.
It runs a sequence of tests on a real `CalDav` server. See
`live_tests/README.md` for full details.

## Other test servers

Radicale:

    docker run --rm --publish 8001:8001 whynothugo/vdirsyncer-devkit-radicale


Baikal:

    docker run --rm --publish 8002:80 whynothugo/vdirsyncer-devkit-baikal

- Cyrus IMAP: Hosted test account by Fastmail.com.
- Nextcloud: Hosted test account.

## Sending patches

Just once, configure the patches list for this repo:

    git config sendemail.to '~whynothugo/vdirsyncer-devel@lists.sr.ht'

Make changes. Run tests. Commit. Then send patches:

    git send-email COMMIT_RANGE
