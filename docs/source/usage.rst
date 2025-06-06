Usage
=====

This is an introduction to usage of pimsync. For reference documentation, see
the manual page, :doc:`pimsync.1`.

As mentioned in the :doc:`introduction <intro>`, pimsync is configured via a
configuration file. The default path for the configuration file is
``~/.config/pimsync/pimsync.conf``, and it can be overridden using the ``-c``
flag or by setting ``$XDG_CONFIG_HOME``.

Basic setup
-----------

The most basic setup synchronises calendar events from a remote CalDAV server
to a local directory. This requires a configuration file with three sections:

- A ``storage`` section describing the details of the CalDAV server.
- A ``storage`` section describing the details of the local directory.
- A ``pair`` section describing how these two storages should be synchronise
  with each other.

Additionally, a `status_path` must be specified. This is the path to a
directory where ``pimsync`` shall store a small database with the state of the
events on both calendar storages. This helps resolve conflicts later: when a
file changes on one storage, ``pimsync`` consults this status database to
determine which side changed and which way data needs to flow to synchronise
changes.

The corresponding configuration file for this example would looks something like:

.. code-block:: scfg

    # use a standard location for the status database:
    status_path "~/.local/share/pimsync/status/"

    storage calendars_remote {
      type caldav
      url https://caldav.example.com/
      username hugo@example.com
      password 123
    }

    storage calendars_local {
      type vdir/icalendar
      path ~/calendars/
      fileext vcf
    }

    pair calendars {
      storage_a calendars_local
      storage_b calendars_fastmail
      collections all
    }

The `collections all` directive indicates that all collections (calendars) from
both sides should be synchronised. See :doc:`pimsync.conf.5` for full details
on the configuration file syntax and available directives.

Storing passwords in configuration files like this is considered poor security
practice; one can accidentally leak them when sharing or backing up a
configuration, and it's easy for other misbehaving software to accidentally
leak it. Instead, we store credentials in a *secret storage* service, and use
some helper to retrieve credentials.

.. _secret-storage:

Usage with a secret storage service
-----------------------------------

In this case, I'll be using `himitsu <https://himitsustore.org/>`_, a secret
storage manager, which I already have set up and running locally.

First I'll store my password into the store by running ``hiq -a``, and then
providing the details for the entry, followed by ``^d``::

   proto=caldavs username=hugo@example.com password!=123

Consult the documentation for your system's secret storage service for specific
instructions on how to persist credentials. Usually non-password fields are
required in order to query the secret service and indicate *which* secret we
want.

For the above created entry, I can use the following command to query for the
password::

   hiq -dFpassword proto=caldavs username=hugo@example.com

When the above command runs, the secret storage will prompt for my
authorisation to disclose credentials, and then provide the password if I
accept the prompt.

The ``storage`` block for the above CalDAV server now changes to:

.. code-block:: scfg

    storage calendars_remote {
      type caldav
      url https://caldav.example.com/
      username hugo@example.com
      password {
       cmd hiq -dFpassword proto=caldavs username=hugo@example.com
      }
    }

Running as a deamon
-------------------

The ``pimsync daemon`` command will make pimsync run continuously and keep
calendars and contacts in sync. This is most suitable as a daemon that runs in
the background for your user session.

It is recommended to put this daemon and other which require credentials under
a non-default bundle/target/run-level, so that they don't result in credential
prompts as soon as one logs in. This target should be manually started instead.

An service script for OpenRC should look something like::

    #!/sbin/openrc-run

    description="Synchronise calendars and contacts"

    command="/usr/bin/pimsync daemon -r 3"
    ready=fd:3

    supervisor=supervise-daemon
    error_logger="logger -t '${RC_SVCNAME}' -p daemon.error"

An service unit for systemd should looks something like::

    [Unit]
    Description=Synchronise calendars and contacts
    Documentation=man:pimsync

    [Service]
    # TODO: glue for readiness notification
    ExecStart=/usr/bin/pimsync daemon
    Restart=no
    Slice=background.slice

    [Install]
    WantedBy=sync.target

Fetching a calendar via Webcal
------------------------------

Webcal refers to events published in a single iCalendar file via HTTP(s).
Pimsync can fetch events (and other iCalendar components) and synchronise them
to a CalDAV or vdir storage. Webcal storages are read-only, since they're no
more than a regular resource exposed via HTTP(s).

A typical configuration looks something like::

   pair holidays {
     storage_a calendars_local
     storage_b holidays_remote
     collection holidays
   }

   storage holidays_remote {
     type webcal
     collection_id holidays
     url https://www.thunderbird.net/media/caldata/autogen/DutchHolidays.ics
   }

   # storage calendars_local omitted for brevity; same as above.

Normally, storages have multiple calendar collections. Webcal is exception is
that it only contains a single collection. The name given to this collection is
controlled via the ``collection_id`` parameter, as shown above. In the above
example, the calendar exposed via Webcal shall be synchronised with a calendar
named ``holidays`` in the ``calendars_local`` storage.

Webcal with secret URLs
.......................

Some Webcal URLs contain secret tokens in the URL, or other sensitive material
which should not be stored in a configuration file. The ``url`` directive's
parameter can be stored in an external secret storage, and retried via the same
mechanism as used in the :ref:`usage with a secret storage service
<secret-storage>` section above::

   storage calendars_italki {
     type webcal
     collection_id italki
     url {
       cmd hiq -dFurl proto=webcal alias=italki
     }
   }

Protections against deletion
----------------------------

Pimsync features two configurable mechanisms to protect from accidental deletion of data.

Emptied collection
..................

The ``on_empty`` configuration directive controls which action to take when one
collection has been completely emptied. The default value is ``skip``, which means
that an emptied collection is skipped instead of synchronised. The other
possible value is ``sync``.

Let's assume a typical setup: a local vdir is being synchronised with a remote
CalDAV server. If we delete all items inside a local collection, then…

- ``on_empty skip`` will prevent deletion of items in the remote collection.
  Instead, a pimsync shall display a warning message. This prevents
  accidentally deleting all remote data when deleting all local data.
- ``on_empty sync`` will perform a "normal" synchronisation, which in this case
  would completely empty the remote collection.

Deleted collection
..................

The ``on_delete`` configuration directive controls what happens when a
collection itself is deleted. Only empty collections are deleted, so the value
of the ``on_empty`` setting has an indirect impact on this scenario as well.

- ``on_delete sync`` will synchronise deletion of collections. Only empty
  collecitons are ever deleted, so this setting only deletes collections if
  they were manually emptied on both sides, or when using ``on_empty sync``.
- ``on_delete skip`` will skip deletion of collections.

Wiping and starting over
------------------------

Because pimsync keeps an internal status database, simply deleting all data
from one storage won't restore all of it from the other one. Usually the
deletion protection mentioned above would kick in.

In a scenario where you want to delete all local data and have it recreated
from the remote server, the following steps should be taken:

1. Ensure that ``pimsync daemon`` is not running, and that no automated
   mechanism would trigger a synchronisation.
2. Delete all data for the local storage.
3. Delete the status database for the corresponding pair.

Further topics
--------------

Contributions for guides for other usage scenarios are most welcome.
