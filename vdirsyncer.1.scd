vdirsyncer(1)

# NAME

vdirsyncer - synchronise caldav, carddav, webdav and local storages

# SYNOPSIS

*vdirsyncer* [_options_] _command_ [_arguments_]

# DESCRIPTION

*vdirsyncer* synchronises contacts or calendars between storages. For calendar,
any combination of *vdir*, *caldav* and *webdav* is supported. For contacts,
any combination of *vdir* and *carddav* is supported.

# OPTIONS

*-v* _loglevel_
	Set logger to _loglevel_, which must be one of *debug*, *info*, *warn*
	or *error*.

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

*discover*
	Discover and print collections. This command merely provides
	information, and does in any way alter *vdirsyncer*'s internal state.

*resolve-conflicts*
	Manually resolve conflicts between storages.

*sync*
	Synchronise storages once and exit.

*version*
	Print version information and exit. The *-p* option has no effect when
	using this command.

# FILES

If *$XDG_CONFIG_HOME* is define, the configuration shall be read from:

	*$XDG_DATA_HOME*/vdirsyncer/config.toml

If the above is not defined, the configuration file shall be read from:

	~/.config/vdirsyncer/config.toml

# ENVIRONMENT

*XDG_CONFIG_HOME*
	If defined, search for the configuration file relative to this
	directory. See *FILES*.

; TODO: Use XDG_DATA_HOME for status DB.

# READINESS NOTIFICATION

Readiness notification allows a service manager to monitor when vdirsyncer has
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

# SECURITY CONSIDERATIONS

When service discovery is enabled, the real hostname and port of the Dav server
is resolved using DNS records. The system's DNS resolver is used. It is expected
that the system resolver performs DNSSEC validation and will not return invalid
results. Vdirsyncer does not perform DNSSEC validation itself.

Items across storages are considered the same if they share the same UID. An
actor who can write a single item into a storage on either side can potentially
trigger an overwrite of another item on the other storage. An actor who can
write to a single entry should be considered to have access to manipulate the
entire storage.

Vdirsyncer will retain credentials in memory during its entire lifetime. Any
actor with read access to vdirsyncer's memory space may extract secret
credentials from it. Likewise, core dumps from vdirsyncer should not be shared
with untrusted parties.

Please note that our security audit is still ongoing.

# SEE ALSO

vdirsyncer-config(5), vdirsyncer-migration(7)

# DEVELOPMENT

For issues and general development inquiries, see the project home currently
hosted at SourceHut:

	https://git.sr.ht/~whynothugo/vdirsyncer-rs

# THANKS

Development of this project is possible thanks to funding and support from the
NLnet foundation and the NGI Zero Entrust program of the European Commission.
See:

- https://nlnet.nl/project/vdirsyncer/
- https://www.ngi.eu/ngi-projects/ngi-zero-entrust/

# HISTORY

The *vdirsyncer* command was first implemented by Markus Unterwaditzer in 2014.

# AUTHORS

This implementation was developed by Hugo O. Barrera <hugo@whynothugo.nl>.

vdirsyncer is an open source project,  developed for anyone to use freely. If
you would like to sponsor this project, see:

	https://whynothugo.nl/sponsor
