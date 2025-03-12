pimsync-migration(7)

# NAME

pimsync-migration - migration guide from vdirsyncer to pimsync

# DESCRIPTION

This migration guide covers changes to be kept in mind when migrating from
vdirsyncer to pimsync. It is geared towards users with a working configuration
of the previous implementation.

# MISSING FEATURES

The following features are implemented by vdirsyncer 0.19.3, but are not
currently not available in pimsync:

- *HTTP Digest Auth*. This needs to be implemented as a _tower-service_
  middleware. Should not be too complicated.
- *partial_sync* configuration parameter. This probably won't come back in the
  same way. A *mirror* is the most likely successor. With this directive set,
  pimsync(1) would make B a mirror of A. In this mode, exactly one storage MUST be
  defined a read_only.
- Encodings other than UTF-8. There are no concrete plans for any specific
  encoding at this time.
- *post_hook* configuration parameter. Future versions might stream events, so
  external handlers can react to them instead.
- *fileignoreext* configuration parameter. There are no plans to implement this
  feature at this time.
- *start_date*, *end_date* and *item_types* configuration parameters for CalDav
  storages. These are planned, but no specific time frame.
- The *singlefile* storage. There are no concrete plans at this time. The logic
  for extracting WebCal should make implementing this (in read-only mode)
  trivial.
- All metadata is synchronised. Selecting individual fields is not currently
  possible.
- The *readonly* configuration directive is not yet implemented.

# CONFIGURATION FILE FORMAT

The configuration format has changed. The general approach is the same but the
syntax has been greatly simplified for humans to write. Technically, the format
is the *scfg* format. It is documented in detail in pimsync.conf(5).


The following:

```
[pair.contacts]
a = "contacts_local"
b = "contacts_fastmail"
collections = "all"
conflict_resolution = ["nvim", "-d"]
```

Becomes:

```
pair contacts {
	storage_a contacts_local
	storage_b contacts_fastmail
	collections all
	conflict_resolution nvim -d
}
```

And the following:

```
[storage.contacts_local]
type = "filesystem"
path = "~/.local/share/contacts/"
fileext = "vcf"
```

Becomes this:

```
storage contacts_local {
	type vdir/vcard
	path ~/.local/share/contacts/
	fileext vcf
}
```

# FETCH MECHANISMS

The *prompt* mechanism for fetching passwords has been dropped.

A *fetch* definition with a *command* would previously look like this:

```
password.fetch = ["command", "hiq", "-dFpassword", "proto=carddavs", "username=…"]
```

Now a block is provided to **password** with a single **cmd** directive
specifying the command and its arguments.

```
	password {
		cmd hiq -dFpassword proto=carddavs username=…
	}
```

A *fetch* definition with a *shell* now uses a similar syntax:

```
	password {
		shell pass show communication/migadu.com | head -1
	}
```

## MIGRATING FETCH / PROMPT

Use an external prompter instead. For a GUI prompter, consider using:

```
password {
	cmd zenity --password
}
```

pimsync(1) will request passwords in sequence, so no special considerations are
required to prevent concurrent prompts.

# MANUAL DISCOVERY IS NO LONGER REQUIRED

Discovering collections ahead of time is no longer required. Collections are
discovered automatically if necessary.

The *discover* command merely prints discovered collections as a convenience for
manually configuring collections. It does not affect pimsync's internal state.
See pimsync(1) for details.

# AUTOMATIC CREATION OF COLLECTIONS

If a pair is configured to synchronise all collections *from a* and a new
collection is found on storage A, then the collection will automatically be
created on storage B.

For scenarios where automatic creation of collections is undesirable,
individual collection should be explicitly specified instead.

# DRY RUNS ARE NOW POSSIBLE

It is now possible to execute a dry run, which only prints the tentative plan
without executing it. The output is somewhat terse at this stage and not in
its final form.

This can be used to audit new configurations and ensure that the planned
actions make sense.

# FILESYSTEM STORAGES HAVE BEEN RENAMED TO VDIR

Filesystem collections have been renamed to *vdir*. They must now specify what
type of items they contains.

A directory with icalendar files should specify:

```
type = "vdir/icalendar"
```

A directory with vcard files should specify:

```
type vdir/vcard
```

# CUSTOM ENCODINGS FOR VDIR STORAGE

The vdir storage saves files as UTF-8. If another encoding is required for some
scenario, please open an issue at:

	https://todo.sr.ht/~whynothugo/pimsync

# FILESYSTEM FILEEXT FIELD

The *fileext* field for filesystem storages required a leading dot. This is no
longer the case; the dot is not considered part of the extensions and should be
omitted. Omit the leading dot when porting configurations.

# COLLECTIONS ARE DECLARED IN A DIFFERENT FORMAT

To synchronise all collections from both sides, use:

```
collections all
```

To synchronise all collections present on a specific storage, use:

```
collections from b
```

## SPECIFYING INDIVIDUAL COLLECTIONS

Individual collections can be specified by *id* or by *href*:

- The *id* is the name from the discovery process.
- The *href* is the full path inside the storage.

Generally, using an *id* is recommended. Using *href* is intended for situations
where discovery is not possible or where multiple collections have the same
*id*.

To specify a single collection by id, use:

```
collection c037725e-e4fd-4b3e-b73d-d5e27d5a90a9
```

The above will find synchronise collections with the given id between both
storages.

Two different collections can mapped on each side:

```
pair my_pair {
	# other fields here

	# first collection
	collection {
			alias personal
			href_a /calendars/hugo/c037725e-e4fd-4b3e-b73d-d5e27d5a90a9/
			href_b /personal/
	}

	# another collection
	collection {
			alias work
			id_a work
			href_b /calendars/work/
	}
}
```

Specifying collection *null* is no longer allowed; configuration should point
to an explicitly collection instead.

# HTTP STORAGE IS NOW CALLED WEBCAL

The type of the *http* storage is now *webcal*, to better reflect its nature;
other storages also use HTTP internally.

# WEBCAL COLLECTION ID

Due to its nature, the WebCal storage has a single collection. However, this
collection has no id. A collection id must be provided for it explicitly. This
simplifies configurations where it must be synchronised to another calendar with
the same collection id.

```
storage calendars_lessons {
	type http
	collection_id lessons
	url {
		cmd hiq -dFurl proto=webcal alias=lessons
	}
}
```

# STATUS

Vdirsyncer keeps a status database in order to distinguish whether an item has
changed on one side or both. Pimsync does not import this status database in any
way. If you are migrating and have existing remote and local storages, makes
sure that they are in sync before switching over. After the first run, pimsync
will build its own status database.

Migrating with out-of-sync storages will result in conflicts during the first
run.

# ISSUES

Please report issues at:

	https://todo.sr.ht/~whynothugo/vdirsyncer-rs

If you have a working vdirsyncer configuration, and this page is missing details
necessary when upgrading, please reach out so this documentation can be
improved.

# SEE ALSO

- pimsync(1)
- pimsync.conf(5)
- *scfg*, simple configuration format, https://git.sr.ht/~emersion/scfg
- hiq(1), query a himitsu(7) key store
