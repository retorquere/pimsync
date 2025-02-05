# Changelog

This document describes changes between releases of `pimsync`. Until v1.0.0, the
public API is still subject to change. This includes commands, arguments and
configuration directives. Upon the release of v1.0.0, backwards incompatible
changes will be avoided unless absolutely necessary.

# Unreleased

- Fixed support for `keep a` and `keep b` as `conflict_resolution` parameters.

# v0.2.0

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

# v0.1.0

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

Previous development versions were named `vdirsyncer`. See [HISTORY.md] for
background on this.
