#!/bin/sh
# Interactively test the conflict resolution UI.

set -eu

TMPDIR=$(mktemp -d -t pimsync-testing.XXXXXXXXXX)
export TMPDIR

echo "Test directory is $TMPDIR"

mkdir -p "$TMPDIR/a/events"
mkdir -p "$TMPDIR/b/events"

cat > "$TMPDIR/pimsync.conf" <<EOF
status_path "$TMPDIR/status/"

pair contacts {
  storage_a a
  storage_b b
  collections all
  conflict_resolution cmd nvim -d
}

storage a {
  type vdir/icalendar
  path "$TMPDIR/a"
  fileext ics
}

storage b {
  type vdir/icalendar
  path "$TMPDIR/b"
  fileext ics
}
EOF

# Events are in conflict: they have the same UID but different SUMMARY.
cat > "$TMPDIR/a/events/test.ics" <<EOF
BEGIN:VCALENDAR
BEGIN:VEVENT
DTSTART:20250319T121000Z
DTEND:20250419T121000Z
SUMMARY:Fun party!
UID:123
END:VEVENT
END:VCALENDAR
EOF

cat > "$TMPDIR/b/events/test.ics" <<EOF
BEGIN:VCALENDAR
BEGIN:VEVENT
DTSTART:20250319T121000Z
DTEND:20250419T121000Z
SUMMARY:Serious party.
UID:123
END:VEVENT
END:VCALENDAR
EOF

#cargo run -- -v info -c "$TMPDIR/pimsync.conf" resolve-conflicts
cargo run -- -c "$TMPDIR/pimsync.conf" resolve-conflicts
