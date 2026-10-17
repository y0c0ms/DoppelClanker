#!/bin/sh
# Back up the team server database, encrypted. Usage:
#   TEAM_BACKUP_PASSFILE=/path/to/passphrase  sh scripts/backup-team-db.sh <postgres-url> <out-dir>
#
# The dump is streamed straight into GnuPG (AES-256, symmetric). No plaintext is written to disk.
# The passphrase is read from a file the operator supplies; it is never a command-line argument
# and never stored by this script. Restore with: gpg --decrypt FILE | psql <url>
set -eu

url="${1:?usage: backup-team-db.sh <postgres-url> <out-dir>}"
outdir="${2:?usage: backup-team-db.sh <postgres-url> <out-dir>}"
passfile="${TEAM_BACKUP_PASSFILE:?set TEAM_BACKUP_PASSFILE to the passphrase file}"

[ -r "$passfile" ] || { echo "backup: passphrase file not readable" >&2; exit 2; }
command -v pg_dump >/dev/null || { echo "backup: pg_dump not found" >&2; exit 2; }
command -v gpg >/dev/null || { echo "backup: gpg not found" >&2; exit 2; }
mkdir -p "$outdir"

stamp=$(date -u +%Y%m%dT%H%M%SZ)
out="$outdir/team-db-$stamp.dump.gpg"
tmp="$out.partial"

# pg_dump -Fc writes a compressed archive; gpg encrypts it on the way out.
pg_dump --format=custom --no-owner "$url" \
  | gpg --batch --yes --quiet --symmetric --cipher-algo AES256 \
        --passphrase-file "$passfile" --output "$tmp"

# Only a complete file gets the final name, so an interrupted run never looks like a backup.
mv "$tmp" "$out"
echo "backup: wrote $out ($(wc -c < "$out") bytes)"
