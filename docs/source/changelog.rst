Changelog
=========

This document describes changes between releases of `pimsync`. Until v1.0.0, the
public API is still subject to change. This includes commands, arguments and
configuration directives. Upon the release of v1.0.0, backwards incompatible
changes will be avoided unless absolutely necessary.

See the dedicated page for details on :ref:`monitoring for new releases
<new-releases>`.

v0.5.3
------

- Remove relative paths for dependencies. This allows building pristine release
  tarballs without having to manually fetch dependant packages.

v0.5.2
------

- Fix release tarballs attempting to build dependencies from relative paths.

v0.5.1
------

- Fix wrong ``Content-Type`` header when updating contacts via CardDAV.

v0.5.0
------

- Various documentation improvements.
- Update to vstorage 0.2.0 and libdav 0.9.5.
- Experimental JMAP support (disabled by default).

v0.4.4
------

- Fix handling of empty responses when finding current user principal. This bug
  mostly affected discovery with DavMail.

v0.4.3
------

- Don't log item data in errors when sync fails. This could leak user data into
  logs.
- Introduce the ``tls_root``, ``tls_fingerprint`` and ``auth_cert``
  configuration parameters used for custom TLS configurations.

v0.4.2
------

- Allow using the ``cmd`` syntax for vdir ``path`` directive.
- Allow using the ``cmd`` syntax for CalDAV/CardDAV ``url`` directive.
- Implement the ``repair`` command.
- Fix deletion of vdir collections.
- Ensure that new WebDAV resources has a proper extension.
- Implement a ``read_only`` directive for storages.
- Initialise pairs concurrently, improving start-up time.
- man pages are now in ``mdoc(7)`` format, which allows cross-links and also
  produces better HTML renders. ``scdoc(1)`` is no longer required.

v0.4.1
------

- Don't build documentation website with `make build`.

v0.4.0
------

- Implement a new `on_delete` directive to protect collections from deletion.
- Normalise timezone order when comparing items. This reduces false positive
  comparisons and conflicts when servers automatically re-order timezone
  components.
- When warning about emptied collections, clarify which storage was empty.
- Fix crash if a previous execution left behind an sqlite journal file.
- Perform a few operations (including initialisation and discovery)
  concurrently, improving start-up times and plan times.
- Improve the conflict resolution UI.
- Implement a sphinx-based website, published via CI.

v0.3.0
------

- Remove unnecessary storage locks when synchronising. Previously storages were
  locked to avoid concurrent executions. Instead, ensure that individual items
  are not modified concurrently.
- Implement monitoring via `inotify` for vdir storages. Other platforms will
  continue to rely on polling.
- Fixed bug including timezones when synchronising from a Webcal storage.
- Fixed support for `keep a` and `keep b` as `conflict_resolution` parameters.
- Implemented the `shell` mechanism for directives. See `pimsync.conf(5)` for
  details.
- Skip uploading unchanged items during conflict resolution. Fixes resolving
  conflicts with read-only storages (e.g.: Webcal).
- Fix `interval` directives in storages not being parsed.

v0.2.0
------

- Several documentation improvements.
- A minimal website is now available, and the documentation is now available
  online too: <https://pimsync.whynothugo.nl/>
- Reject configuration file on superfluous parameter.
- Attempt to use the provided URL before doing service discovery. If the URL
  provided in the configuration file is a valid context URL for a DAV server,
  discovery is skipped entirely.
- Fixed some erroneous handling of percent-encoded characters in URLs.
- Prompt early for all required credentials. This speeds up start-up, since
  there's no network IO between prompts.

v0.1.0
------

- Renamed project to `pimsync`.
- Redesigned configuration file.
- Include a man page for `pimsync(1)` and `pimsync.conf(5)`.
- Several minor fixes.
- Introduce support for Dav over Unix sockets.
- Allow specifying a custom configuration file.
- Many documentation improvements.
- Collection ID was sometimes referred to as "collection name". All references
  now specify "collection id"
- The `http` storage is now named `webcal`.

Previous development versions were named `vdirsyncer`. See :doc:`history` for
background on this.
