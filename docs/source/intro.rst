.. Copyright 2025 Hugo Osvaldo Barrera
..
.. SPDX-License-Identifier: ISC

Introduction
============

**pimsync** synchronises documentation across storages. Currently supported
storages are:

- **CalDAV**: HTTP extension which provides read and write access to calendars
  on servers.
- **CardDAV**: HTTP extension which provides read and write access to address
  books on servers.
- **Vdir**: Convention for storing calendars and address books in local
  directories using standardised and well-documented formats.
- **WebCal**: Convention for exposing a calendar or event read-only using HTTP.

Common use cases include:

- Synchronising a CalDAV or CardDAV server with a local directory. Local data
  can then be accessed and manipulated by a variety of programs, none of which
  have to know or worry about synchronisation, network connectivity, or remote
  servers. The changes are then synced back to the server periodically.
- Synchronising from server to a local directory with the intent of keeping
  back-ups using a tool that backs up local directory trees.
- Synchronising data between two different CalDAV or CardDAV servers.

pimsync is configured via a configuration file. It can then be used to
synchronise data once (via the ``pimsync sync`` command) or to keep data
synchronised continuously (via the ``pimsync daemon`` command).

.. TODO: need a section on "running as a daemon"

Philosophy
----------

User own their data
   Pimsync enables users allows to synchronise data from other servers onto any
   chosen destination, including the local filesystem, where other tools can
   intercut with this data.

Synchronise items regardless of whether they are valid or not
   If another program created a "technically invalid" calendar event, the data
   needs to be synchronised anyway in order for it to be accessible. Once the
   data is synchronised it can be accessed by some other tool which can fix it,
   or simply accessed by a program which can handle the "technically invalid"
   quirks properly. See also `the robustness principle
   <https://en.wikipedia.org/wiki/Robustness_principle>`_

Use standards protocols, formats and conventions whenever possible.
   Calendar components are stored using `the iCalendar format`_. Contacts are
   stored using `the vCard format`_. CalDAV and CardDAV are well established
   protocols which many implementations (including ones which anyone can
   self-host) and service providers.

.. _the iCalendar format: https://www.rfc-editor.org/rfc/rfc5545
.. _the vCard format: https://www.rfc-editor.org/rfc/rfc6350

Do one thing, do it well
   Don't focus on seemingly related functionality; let external tools implement
   other features and provide the means for them to interoperate. Doing less
   leads to greater flexibility.

Provide clear instructions
   Focus on clear documentation and explaining how things work. Avoid guessing
   what the user wanted when it can lead to unexpected outcomes. Allow anyone
   to understand how things work to the level of detail with which they are
   comfortable.

Comparison to vdirsyncer
------------------------

**pimsync** grew out of lessons learnt from **vdirsyncer**, a previous
incarnation of the same tool. Key differences are:

Focuses on good error management and recovery
   If synchronising a single item fails (e.g.: because a server rejects it),
   the overall operation continues, and other items in the same collection are
   synchronised properly.

   This is made possible in great deal due to using Rust and it's stricter
   focus on error handling.

The ``daemon`` subcommand
   Runs continuously in the keeping collections in sync. Ideal for usage as a
   service.

Synchronise collections
   Automatically create (and, optionally, delete) collections which are created
   (and deleted) on one storage into the other.

Simplified setup
   Being built in Rust, **pimsync** can be compiled into a single binary. This
   substantially simplifies the installation process, and also provide improved
   performance.

A few features present in **vdirsyncer** are still missing from pimsync. For
details on these and changes required when migrating, see :doc:`the migration
manual page <pimsync-migration.7>`
