Usage
=====

This is an introduction to usage of pimsync. For reference documentation, see
the manual page, :doc:`pimsync.1`.

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
      type vdir/vcard
      path ~/.local/share/contacts/cards/
      fileext vcf
    }

    pair calendars {
      storage_a contacts_local
      storage_b contacts_fastmail
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

Further topics
--------------

Contributions for guides for other usage scenarios are most welcome.
