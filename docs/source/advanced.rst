Advanced usage
==============

Using CalDAV via a Unix Domain Socket
-------------------------------------

Aside from connecting to servers via HTTP, pimsync can connect to servers via a
Unix domain socket (see ``unix(7)``). This feature is considered an "escape
hatch" for connecting to servers which require any transport feature not
natively supported by pimsync.

This feature allows tunnelling the connection to a server which is not directly
reachable. Some example usages are:

* Using ``ssh -L path/to/local/socket:…``, to tunnel a remote socket or port.
* Using an HTTP proxy server (e.g.: ``nginx``) to perform any rewrites (or
  either requests and/or responses), perform TLS client authentication, or any
  other custom feature.

Using a Unix socket instead of a TCP socket facilitates exposing a service to a
single local user (or group of users) without having to reconfigure a local
firewall.

If you are running your local CalDAV on a Unix domain socket, you can configure
pimsync to communicate with it directly without having to expose the server on
a TCP port.

To use CalDAV or CardDAV with a Unix domain socket, use configure a ``url`` in
the style of ``unix:///path/to/socket``. See ``STORAGE SECTIONS`` in the
:doc:`pimsync.conf.5` manual page.

One instance per pair
---------------------

If ``pimsync daemon`` encounters a fatal error when synchronising a pair (i.e.:
cannot read or write from the status database), it will stop synchronising that
pair and continue with others. Pimsync will exit only if all pairs have failed.

As a result, it might not be entirely obvious when a single pair has
encountered a fatal error.

Running one pimsync process per pair provides better status indication when
executed via a service manager. When a single pair fails, that process will
exit, and the service's status would reflect this failure.

Running one instance per pair also yields cleaner logging, since each pair logs
into a separate the stderr of separate processes.

This is considered an advanced usage because it requires that you configure
your service manager to run multiple instances of the pimsync service.

Logging
-------

All logging is done to `stderr`. By default, `pimsync` logs warnings and
errors. This can be controlled with the `-v` parameter, which takes a log
level. E.g.: `-v debug`.

Take care when using `-v trace`: it is likely to output sensitive data. Do not
share with others the output produced with trace logging.

CalDAV and CardDAV collection IDs
---------------------------------

By default, pimsync uses the last segment of a collection's URL path as its
collection ID. For example, for a collection at ``…/user/calendar/``, the
collection ID would be ``calendar``.

However, some servers use URL structures where the collection name is not
the last segment. For example, collections may be exposed under URLs such as::

   …/dav/user@example.com/personal/events/
   …/dav/user@example.com/work/events/

In such cases, you may want the collection ID to be ``personal`` and ``work``
respectively, and having both named ``events`` would be problematic. This can
be achieved by setting the ``collection_id_segment`` directive to
``second-last`` in your storage configuration:

.. code-block:: scfg

    storage my_caldav {
        type caldav
        url https://dav.example.com/
        collection_id_segment second-last
        username user@example.com
        password mypassword
    }

Valid values for ``collection_id_segment`` are:

* ``last`` (the default) - Use the last URL segment as the collection ID.
* ``second-last`` - Use the second-to-last URL segment as the collection ID.
