pimsync(1)

# NAME

pimsync - synchronise caldav, carddav, webcal and local directories

# SYNOPSIS

*pimsync* [_options_] _command_ [_arguments_]

# DESCRIPTION

*pimsync* synchronizes calendars and contacts between two storages. The most
popular usage is to synchronize a CalDAV/CardDAV server with a local directory.
The local data can then be accessed by a variety of programs, none of which have
to know or worry about synchronisation, network connectivity, or remote servers.

Supported calendar storages are *vdir*, *caldav* and *webcal*. With the
exception of *webcal* to *webcal*, all permutations of these are supported. The
*webcal* storage is read-only, and two read-only storages cannot be synchronised
with each other.

Supported contacts storages are *vdir* and *carddav*. Any permutation of these
is supported.

# OPTIONS

*-c* _configfile_
	Read the configuration file from the specified path, instead of the
	default. See FILES below.

*-v* _loglevel_
	Set logger to _loglevel_, which must be one of *trace*, *debug*, *info*,
	*warn* or *error*. The default is *warn*. When specifying *trace*, logs
	may contain sensitive data, see *SECURITY CONSIDERATIONS* below.

*-p* _pair_
	Only operate on the pair named _pair_.

# COMMANDS

*check*
	Check configuration and exit. Validates the configuration itself and
	ensures that the storage is valid. E.g.: a *vdir* must be an existing
	directory, and *caldav* server must report CalDav capabilities, etc.

*daemon*
	Continuously keep storages in sync. The *-r* _READY_FD_ argument may be
	used to provide a readiness notification file descriptor.

	Once it has reached its ready state, the demon will monitor storages for
	changes and synchronise them. In case of non-fatal errors, pimsync will
	continue running the background and try again later.

*discover*
	Discover and print collections. This command merely provides
	information, and does in any way alter pimsync's internal state.

*resolve-conflicts*
	Manually resolve conflicts between storages.

*sync*
	Synchronise storages once and exit.

*version*
	Print version information and exit. The *-p* option has no effect when
	using this command.

# CONFLICT RESOLUTION

When synchronising two storages, if a single item has been modified on both
sides, it is said to be in conflict. This needs to be resolved manually.

The *resolve-conflicts* command shall prompt to reconcile conflicting items
using an external command. At this time, generic text diff

# FILES

If *$XDG_CONFIG_HOME* is defined, the configuration shall be read from:

TODO: rename to pimsync.conf (also man page)
	*$XDG_CONFIG_HOME*/pimsync/pimsync.conf

If the above is not defined, the configuration file shall be read from:

	~/.config/pimsync/pimsync.conf

See pimsync.conf(5) for details on the configuration file format.

# ENVIRONMENT

*XDG_CONFIG_HOME*
	If defined, search for the configuration file relative to this
	directory. See *FILES* above.

; TODO: Use XDG_DATA_HOME for status DB.

# READINESS NOTIFICATION

Readiness notification allows a service manager to monitor when pimsync has
successfully started. It is considered to have successfully started when the
configuration file has been parsed and all credentials have been resolved. At
this point, no further human interaction is required.

Once the service is ready, it will write *READY=1* followed by a newline
character to the file descriptor _READY_FD_, and then close it. This is
compatible with the readiness notification mechanism implemented by s6, dinit
and systemd.

See: https://skarnet.org/software/s6/notifywhenup.html

# PRIVACY

The only network communications shall be with the servers for configured
storages, and the locally configured DNS resolver.

The status database file contains metadata of configured storages, their
collections and items.

# DATA VALIDATION

Data synchronised is not validated. This is an intentional design choice: if
your calendar application produces events that are technically invalid,
pimsync will synchronise, and you will be able to access the exact same data
on other hosts. You may also need to synchronise invalid data in order to fix
the invalidity.

In order to map items between storage, pimsync does some minor parsing of
items to extract their UID. Items must be syntactically valid components, even
if semantically invalid.

In order to interact with CalDav/CardDav resources where a UID cannot be
extracted, consider using davcli(1). In future, a *repair* command shall be
included to address this scenario.

# SECURITY CONSIDERATIONS

When service discovery is enabled, the real hostname and port of the Dav server
is resolved using DNS records. The system's DNS resolver is used. It is expected
that the system resolver performs DNSSEC validation and will not return invalid
results. Pimsync does not perform DNSSEC validation itself.

Pimsync will follow symlinks when reading items. Care should be taken when a
user has write permission to a storage, but another user runs pimsync on that
same storage. This may change at a future date.

Items across storages are considered the same if they share the same UID. An
actor who can write a single item into a storage on either side can potentially
trigger an overwrite of another item on the other storage. An actor who can
write to a single entry should be considered to have access to manipulate the
entire storage.

Pimsync will retain credentials in memory during its entire lifetime. Any actor
with read access to pimsync's memory space may extract secret credentials from
it. Likewise, core dumps from pimsync should not be shared with untrusted
parties.

When logging with *-v trace*, logs MAY contain credentials, secrets, data inside
of storage items and other sensitive information.

# CAVEATS

For vdir storages, checking the etag is subject to race conditions. Pimsync will
first check that an item has not been modified, and then update it. If an
external program updates the same item between the check and the modification,
those changes will be lost. It is unlikely that this issue can be resolved with
existing filesystem primitives.


# SEE ALSO

pimsync.conf(5), pimsync-migration(7), khal(1), todo(1)

# DEVELOPMENT

For issues and general development inquiries, see the project home currently
hosted at SourceHut:

	https://sr.ht/~whynothugo/vdirsyncer-rs

# THANKS

Development of this project is possible thanks to funding and support from the
NLnet foundation and the NGI Zero Entrust program of the European Commission.
See:

- https://nlnet.nl/project/vdirsyncer/
- https://www.ngi.eu/ngi-projects/ngi-zero-entrust/

A security audit was completed by Radically Open Security:

- https://www.radicallyopensecurity.com/

# HISTORY

The *vdirsyncer* command was first implemented by Markus Unterwaditzer in 2014.

Work on the *pimsync* command, strongly inspired by *vdirsyncer*, was started by
Hugo Barrera in 2023.

The acronym *pim* refers to *personal information manager*.

# AUTHORS

This implementation was developed by Hugo O. Barrera <hugo@whynothugo.nl>.

pimsync is an open source project,  developed for anyone to use freely. If
you would like to sponsor this project, see:

	https://whynothugo.nl/sponsor
