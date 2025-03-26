Installation
============

Whenever possible, installation via distribution packages is recommended.
Distributions packages are available on:

- Alpine Linux Edge, in the ``testing`` repository.
- ArchLinux, in the ``extra`` repository.
- Nix, via nixpkgs unstable.

pimsync itself is a single binary, but the installation process (and
distribution packages) also include :doc:`man pages <man>` as reference
documentation.

Building from source
--------------------

Requirements
............

Compiling pimsync requires:

- make
- Rust and Cargo
- libsqlite3 (development package)
- scdoc (for compiling manual pages)

Running pimsync requires:

- libc
- libsqlite3 (runtime package)

Compilation
...........

Use ``make build`` to compile pimsync. The build process uses Cargo to produce a
binary and scdoc to compile documentation.

The compiled binary is placed in ``./target/release/pimsync``. It can be executed
directly, or copied into your ``$PATH``.

Installation
............

After compiling from source, use ``make install`` to install to ``/usr/local/``.

Packaging
---------

The build and installation process is designed to ease downstream packaging and
redistribution. Efforts in packaging pimsync downstream are highly appreciated.

Version
.......

The source tarballs have the correct version injected into ``build.rs`` using
`the technique described in this article <https://whynothugo.nl/journal/2024/12/16/injecting-project-versions-into-builds/>`_.
This value the one printed by ``pimsync version`` and in logs. It can be
overridden by setting the ``PIMSYNC_VERSION`` environment variable at compile
time.

It is recommended to include the downstream package revision, in case a single
upstream release can have multiple package revisions (this is the case in most
distributions). E.g.: ``PIMSYNC_VERSION=0.4.1-r0``.

If downstream patches are applied before the build, please override the version
using the ``PIMSYNC_VERSION`` environment variable and set it to something that
reflects this. E.g.: ``PIMSYNC_VERSION=0.4.1+alpine-patches``. This is
extremely useful when receiving bug reports, since it helps us understand that
the bug report originates from a patches version of this project.

Man pages
.........

Please include man pages in distribution packages. Producing them only depends
on the ``scdoc`` command. Running ``make build`` produces the manual pages
alongside the main binary. They can be built independently by running ``make
man``.

New releases
------------

Releases are published via git tags, and published at the primary repository.
`The page listing all tags <https://git.sr.ht/~whynothugo/pimsync/refs>`_
includes a link to an RSS feed which you can monitor for new tags.

Tags are signed with the following GPG key, which is renewed yearly::

   -----BEGIN PGP PUBLIC KEY BLOCK-----

   mDMEYssd9xYJKwYBBAHaRw8BAQdAi3r4jQQVeJEBieiN9DaF52oKESMhEM4TI49m
   UMxBCaq0KUh1Z28gT3N2YWxkbyBCYXJyZXJhIDxodWdvQHdoeW5vdGh1Z28ubmw+
   iJYEExYIAD4CGwMFCwkIBwIGFQoJCAsCBBYCAwECHgECF4AWIQQSBMqfwv+t7twp
   YTZ4gHM7nQYoNwUCZpPRygUJBannUwAKCRB4gHM7nQYoN5HcAQDNCdOckfEivnHk
   qc18qC4iDYpUPfX3vJl19pjkfMhfygEAis64hmP5pS4sX27gdt6jcDEB3d8eytgg
   dd1A2UlstAC4OARiyx33EgorBgEEAZdVAQUBAQdAZ3ZuT6S6VqLadw1PlV7hG/WF
   VE7Iti8ls+8Z9FY8Ql0DAQgHiH4EGBYIACYCGwwWIQQSBMqfwv+t7twpYTZ4gHM7
   nQYoNwUCZpPSIQUJBannqgAKCRB4gHM7nQYoN13HAQC3kODrPok3QIcs8VOPG0Y3
   bEb3qdKZ2vK/qomBtr+QpwD/bzyGPSFrOzNxdxxEsmj8jiaROHvVmaH5bmyWEJCa
   3ga4MwRiyx4TFgkrBgEEAdpHDwEBB0D4AmKE4VVbqaSyoXycqi5fKMxcyExLRPcy
   1PHZnGVd34h+BBgWCAAmAhsgFiEEEgTKn8L/re7cKWE2eIBzO50GKDcFAmaT0iQF
   CQWp544ACgkQeIBzO50GKDep4QEAtEyYyBzbCxdFIWkGPqMAC4bxoJW9Lf2afNG+
   1CiU7HgBAMMOMiM21Mk0tebGLxZF3rRCCu3r54bMSEuKKSZoG2UI
   =BNyi
   -----END PGP PUBLIC KEY BLOCK-----

The key fingerprint is ``1204CA9FC2FFADEEDC2961367880733B9D062837``.

Reproducible tarballs
---------------------

Tarballs are reproducible. You may generate one from a local git checkout using
the following snippet:

.. code-block:: sh

   VERSION=0.4.1
   git archive v$VERSION --prefix 'pimsync-v$VERSION/'
