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

Each storage section starts with a declaration of the storage name.

*storage* _name_ { … }
	Declare a storage with name _name_.

Each storage is defined by a block of directives, one per line, enclosed in
curly braces:

*type* _type_
	Type can be one of *vdir/icalendar*, *vdir/vcard*, *caldav*, *carddav*
	or *webcal*.

*interval* _seconds_
	When monitoring a storage is no possible, check for changes every
	the specified amount of seconds. The default is 300 seconds.

; TODO: should point to docs for *vdir*.

Storages of type *vdir* take the following directives:

*path* _path_
	Path to a directory storing a vdir. A leading tilde will be expanded to
	match the current user's home directory.

*fileext* _ext_
	File extension for items saved into this vdir.

Storages of type *caldav* or *carddav* take the following directives:

*url* _url_++
*url* { … }
	URL of the Dav server. Service discovery will be performed on this URL
	to find the exact location of the home set collection. URLs must specify
	schema _http_, _https_ or _unix_. URLs starting in _unix://_ will be
	treated as a path to a unix domain socket.

*username* _username_++
*username* { … }
	Optional. Username to use for HTTP Basic Auth.

*password* _password_++
*password* { … }
	Optional. Password to use for HTTP Basic Auth.

Storages of type *webcal* take the following directives:

*url* _url_++
*url* { … }
	URL of an icalendar file. This file may contain multiple calendar
	items. Scheme must be _http_ or _https_.

*collection_id* _name_
	Name to be used when mapping this collection to another on the other
	storage.

; TODO: auth for webcal?

The *url*, *username* and *password* fields may be defined in the configuration
file, or may specify a command to retrieve this value from an external store. To
read a password via the external command _secret-store --get-caldav-password_,
use the following syntax:

```
password {
	cmd secret-store --get-caldav-password
}
```

# PAIR SECTIONS

Each pair section starts with a declaration of the pair name.

*pair* _name_ { … }
	Declare a pair with name _name_.

Each pair is defined by a block of directives, one per line, enclosed in
curly braces:

*storage_a* _name_
	The _name_ of the first storage to be synchronised with another.

*storage_b* _name_
	The _name_ of the second storage to be synchronised with the other.

*collections all*
	Synchronise all collections found in both storages. Collections will be
	synchronised to others with a matching id on the other side.

*collections from a*
	Synchronise all collections found in storage a. Collections will be
	synchronised to others with a matching id on the other side.

*collections from b*
	Synchronise all collections found in storage b. Collections will be
	synchronised to others with a matching id on the other side.

*collection* _id_
	Synchronise collection with id _id_. May be specified more than once.

*collection* { … }
	Synchronise collection defined by the given rules. See *COLLECTION
	SECTIONS* below. May be specified more than once.

*on_empty skip*
	When a collection is entire empty on one side, skip it. This is the
	default.

*on_empty sync*
	When a collection is entire empty on one side, empty its counterpart on
	the other side.

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

The following is a complete example of a configuration file:

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

pair study_calendars {
	storage_a calendars_local
	storage_b calendars_italki
	collection italki
}

storage calendars_italki {
	type webcal
	collection_id italki
	url {
		cmd hiq -dFurl proto=webcal alias=italki-mine
	}
}

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
