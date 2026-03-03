Contributing
============

Issues should be reported via the `issue tracker`_. Use the search
functionality to ensure that nobody has asked similar question or had a similar
problem in the past. If no relevant issues exist, open a new one. Please try to
explain what you are trying to do and what went wrong.

.. _issue tracker: https://todo.sr.ht/~whynothugo/pimsync

Please do not send private emails with questions about pimsync; other people
with the similar questions in future can search the issue tracker but can't
read any email messages which we exchange.

Hacking
-------

Source code is available at the `project repository`_.

.. _project repository: https://git.sr.ht/~whynothugo/pimsync

Builds during development cycles track arbitrary commits of some dependencies.
Use ``git submodule update --init`` before building.

Unit tests and other basic checks can be run with `make check`. This will also
ensure that documentation has no broken links, examples build, etc.

Testing zsh completion
----------------------

Load completion with::

   fpath=($(pwd)/contrib $fpath)
   autoload -Uz compinit && compinit

If a compatible version of ``pimsync`` is not installed, add the local builds
to ``$PATH``::

   export PATH=$(pwd)/target/debug:$PATH

Sending patches
---------------

Just once, configure the patches list for this repo::

   git config sendemail.to '~whynothugo/vdirsyncer-devel@lists.sr.ht'

Make changes. Run tests. Commit. Then send patches::

   git send-email COMMIT_RANGE

See *SPECIFYING RANGES* in ``man gitrevisions`` for details on specifying a
range of commits. Common examples are described below.

To submit a single commit, use::

   git send-email -1 COMMIT_SHA

To submit all commits not present upstream, use::

   git send-email origin/main..

To submit all commits after ``18901a2a`` up to and including ``e931d793``, use::

   18901a2a..e931d793
