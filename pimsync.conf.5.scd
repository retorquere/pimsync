pimsync.conf(5)

# NAME

pimsync.conf - pimsync configuration file

# DESCRIPTION

The pimsync configuration file is divided into three main types of sections:

*global section*
	Global settings for pimsync.

*storage sections*
	Configuration for individual storages.

*pair sections*
	Rules for synchronising pairs of storages.

See the *FILES* section of pimsync(1) for details on the default path for the
configuration file, and how to specify an alternative path.

# GLOBAL SECTION

The following setting can be set globally:

*status_path*
	Path to a directory which will contain the status database internally
	used by pimsync.
; Defaults to *XDG_DATA_HOME/pimsync/status/*.

# STORAGE SECTIONS

A storage describes a location where calendars or contacts are stored. Each
storage is declared in its own section, and each storage section starts with a
declaration of the storage name.

*storage* _name_ { … }
	Declare a storage with name _name_.

Directives are declared inside the curly braces for each section, one per line.
The following directives apply to all storage types:

*type* _type_
	Type can be one of:

	*vdir/icalendar*
		A directory where each subdirectory represents a calendar. Each
		of these subdirectories contains individual icalendar files,
		each representing one event (or one series of recurring events),
		todo or journal entry.

	*vdir/vcard*
		A directory where each subdirectory represents an address book.
		Each of these subdirectories contains individual vcard files,
		each representing one contact.

	*caldav*
		A CalDAV server, containing calendars with events, todos and/or
		journal entries.

	*carddav*
		A CardDAV server containing address books with contacts.

	*webcal*.
		An HTTP or HTTPS URL exposing a single iCalendar file which may
		contain zero or more events, todos or journal entries. This
		storage is read-only.

*interval* _seconds_
	When monitoring a storage is no possible, check for changes every
	the specified amount of seconds. The default is 300 seconds.

*read-only*
	Do not perform any write operations on this storage.

Storages of type *vdir* take the following directives:

; TODO: should point to docs for *vdir*.

*path* _path_++
*path* { … }
	Path to a directory storing a vdir. A leading tilde will be expanded to
	match the current user's home directory.

*fileext* _ext_
	File extension for items saved into this vdir.

Storages of type *caldav* or *carddav* take the following directives:

*url* _url_++
*url* { … }
	URL of the Dav server. If this URL does not report the appropriate DAV
	capabilities, service discovery will be performed for this URL and its
	domain to find the exact location of the server and the home set
	collection.

	URLs must specify schema _http_, _https_ or _unix_. URLs starting in
	_unix://_ shall be treated as a path to a unix domain socket.

*username* _username_++
*username* { … }
	Optional. Username to use for HTTP Basic Auth.

*password* _password_++
*password* { … }
	Optional. Password to use for HTTP Basic Auth.

; TODO: directive to service discovery

Storages of type *webcal* take the following directives:

*url* _url_++
*url* { … }
	URL of an icalendar file. This file may contain multiple calendar
	items. Scheme must be _http_ or _https_.

*collection_id* _name_
	Name to be used when mapping this collection to another on the other
	storage.

; TODO: auth for webcal?

Storages of type *caldav*, *carddav* or *webcal* take the following directive:

*user_agent* _user-agent_
	Optional. Set a custom User-Agent header on outgoing HTTP requests. The
	default uses *pimsync/* followed by the current version number. E.g.:
	*pimsync/1.0.0*.

The *url*, *username*, *password* and *path* fields may be defined in the
configuration file, or may specify a command to retrieve this value from an
external storage. When specifying an external command, instead of defining a
value for the directive, specify a new block with a single *cmd* directive.

For example, to read a password via the external command
_secret-store --get-caldav-password_, use the following syntax:

```
password {
	cmd secret-store --get-caldav-password
}
```

It is also possible to specify a shell script, which is executed via _sh -c_.
For example:

```
password {
	shell pass show communication/migadu.com | head -1
}
```

It is recommended to store credentials and other sensitive values in a secret
storage service.

# PAIR SECTIONS

A pair declares how two storages shall be synchronised to each other. Each pair
section starts with a declaration of the pair name:

*pair* _name_ { … }
	Declare a pair with name _name_.

Each pair is defined by a block of directives, one per line, enclosed in curly
braces:

*storage_a* _name_
	The _name_ of the first storage to be synchronised with another.

*storage_b* _name_
	The _name_ of the second storage to be synchronised with the other.

*collections all*
	Synchronise all collections found in both storages. Collections will be
	synchronised to others with a matching id on the other side. The is the
	recommended value if you simply want to synchronise all collections from
	both storages.

*collections from a*
	Synchronise all collections found in *storage_a*. Each collection shall
	be synchronised with the collection which has the same collection id on
	*storage_b*.

*collections from b*
	Synchronise all collections found in *storage_b*. Each collection shall
	be synchronised with the collection which has the same collection id on
	*storage_a*.

*collection* _id_
	Synchronise collection with id _id_. May be specified more than once.

*collection* { … }
	Synchronise collection defined by the given rules. See *COLLECTION
	SECTIONS* below. May be specified more than once per pair.

*on_empty skip*
	When a collection is completely emptied on one side, skip it. This is
	the default.

*on_empty sync*
	When a collection is completely emptied on one side, empty its
	counterpart on the other side. Take care when using this option. If you
	completely delete a collection on one side, pimsync(1) won't restore it.
	Instead, it will delete the same collection (and all of its items) on
	the other side.

*on_delete sync*
	When a collection itself is deleted on one side, delete it on the other
	side. This only applies if the collection is already empty. This is the
	default. This is generally safe, especially when combined with *on empty
	skip*.

*on_delete skip*
	When a collection itself is deleted on one side, skip it.

The *conflict_resolution* directive specifies which action should be taken if an
item was modified on both sides:

*conflict_resolution cmd* _command_ [_args_]
	Execute _command_ to resolve conflicts between both sides. _args_ will
	be passed as arguments, followed by the path to two files; one
	containing the data on storage a, and the other containing the data on
	storage b. The conflict will be considered resolved if and only if both
	files are the same when the command exits and the command exits with
	exit code zero.

*conflict_resolution keep a*
	In case of conflict, keep the version for storage a.

*conflict_resolution keep b*
	In case of conflict, keep the version for storage a.

# COLLECTION SECTIONS

A collections block defines a mapping between two collections that are to be
synchronised; one of them in *storage_a* and the other in *storage_b*.

These sections are only used in advanced configurations which need to map
existing collections to each other without changing their location on either
side.

Each block must include an alias:

*alias* _name_
	An alias for this pair of collections that will be used for any output.

Exactly one reference to a collection in *storage_a* must be included:

*id_a* _id_
	Use a collection with _id_.

*href_a* _href_
	Use a collection with _href_.

Exactly one reference to a collection in *storage_b* must be included:

*id_b* _id_
	Use a collection with _id_.

*href_b* _href_
	Use a collection with _href_.

For example, to syncrhonise a collection with id _work_ with another at path
_/calendars/mine/work/_, use:

```
collection {
	alias work
	id_a work
	href_b /calendars/mine/work/
}
```

Specifying an id and an href for the same storage is not allowed.

# COLLECTION ID

The id of a collection is a concept specific to pimsync(1) and vdirsyncer(1).

For *vdir* storages, the id of a collection is the name of the directory that
corresponds to it.

For *caldav* or *carddav* storages, the id of a collection is the last component
of its URL.

For *webcal* storages, the id must be specified explicitly via the
*collection_id* configuration directive.

Unless configured otherwise, collections with the same id on both storages are
synchronised with each other.

# EXAMPLE

This example synchronises two storages with address books. The first is a local
set of directories which contain vcard files, the second is a remote carddav
servers. Credentials are supplied via the *hiq* command line tool.

; TODO: link vdir man page above

```
status_path "~/.local/share/pimsync/status/"

pair contacts {
	storage_a contacts_local
	storage_b contacts_remote
	collections all
	conflict_resolution cmd nvim -d
}

storage contacts_local {
	type vdir/vcard
	path ~/.local/share/contacts/cards/
	fileext vcf
	interval 30
}

storage contacts_remote {
	type carddav
	url https://carddav.example.com/
	username hugo@example.com
	password {
		cmd hiq -dFpassword proto=carddavs username=hugo@example.com
	}
	interval 30
}
```

The following example synchronises two storages with calendar data. The first is
local directories with icalendar files, the second is a remote caldav servers.

```
status_path "~/.local/share/pimsync/status/"

pair calendars {
	storage_a calendars_local
	storage_b calendars_remote
	collections from b
	conflict_resolution cmd nvim -d
}

storage calendars_local {
	type vdir/icalendar
	path ~/.local/share/calendars/
	fileext ics
	interval 30
}

storage calendars_remote {
	type caldav
	url https://caldav.example.com/
	username hugo@example.com
	password {
	 cmd hiq -dFpassword proto=caldavs username=hugo@example.com
	}
	interval 30
}
```

The following example synchronises two storages with calendar data. The first is
a local directory named "italki" with icalendar files, the second is an http
server publishing an icalendar file.

Note that the first storage is the same as the one above, and should be
specified only once if both configurations were to be merged.

The exact URL to the icalendar file exposed via http is fetched via the *hiq*
tool, used to query the *himitsu* secret store.

```
status_path "~/.local/share/pimsync/status/"

pair study_calendars {
	storage_a calendars_local
	storage_b calendars_italki
	collection italki
}

storage calendars_local {
	type vdir/icalendar
	path ~/.local/share/calendars/
	fileext ics
	interval 30
}

storage calendars_italki {
	type webcal
	collection_id italki
	url {
		cmd hiq -dFurl proto=webcal alias=italki-mine
	}
}
```

This example synchronises two storages with calendar data. The first is a carddav
server, reached via a unix domain socket. The second is storage uses a local
filesystem with vcard files.

```
status_path "~/.local/share/pimsync/status/"

pair unixtest {
	storage_a unix_a
	storage_b unix_b
	collections all
}

storage unix_a {
	type carddav
	url unix:///tmp/xandikos.sock
}

storage unix_b {
	type vdir/vcard
	path /tmp/test_storage/
	fileext vcf
}
```

# SEE ALSO

pimsync(1), pimsync-migration(7)

The configuration file follows the scfg syntax.++
See: https://git.sr.ht/~emersion/scfg
