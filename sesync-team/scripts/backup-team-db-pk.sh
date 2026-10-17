#!/bin/sh
# Unattended team database backup, encrypted to a PUBLIC key. Needs no secret to run:
# the public key only encrypts. The private key (and its passphrase) stay with the operator,
# so restoring is always a deliberate, manual step.
#
# Usage: backup-team-db-pk.sh <dump-command> <recipient-fingerprint> <public-key-file> <out-dir> [keep]
#   dump-command   a command that writes a pg_dump custom-format archive to stdout
#                  (e.g. "kubectl -n team exec deploy/team-server-db -- pg_dump -Fc -U team team")
#   keep           how many dumps to keep (default 14); older ones are deleted
#
# Before it writes the real dump, it proves the pipeline works with a throwaway key pair:
# a canary is encrypted and decrypted and compared. A broken pipeline stops the run here.
set -eu

dump_cmd="${1:?dump command}"
fpr="${2:?recipient fingerprint}"
pubkey="${3:?public key file}"
outdir="${4:?output directory}"
keep="${5:-14}"

command -v gpg >/dev/null || { echo "backup: gpg not found" >&2; exit 2; }
[ -r "$pubkey" ] || { echo "backup: public key not readable" >&2; exit 2; }
mkdir -p "$outdir"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export GNUPGHOME="$work/gnupg"
mkdir -p "$GNUPGHOME"; chmod 700 "$GNUPGHOME"

# 1. Canary: a throwaway key pair, so the pipeline is tested without the operator's key.
# A primary key certifies; the encryption key is a subkey. (A primary may not be "encr".)
gpg --batch --pinentry-mode loopback --passphrase '' --quick-generate-key canary ed25519 cert,sign 0 \
  || { echo "backup: canary key generation failed" >&2; exit 3; }
canary_fpr=$(gpg --list-keys --with-colons canary | awk -F: '/^fpr/{print $10; exit}')
gpg --batch --pinentry-mode loopback --passphrase '' --quick-add-key "$canary_fpr" cv25519 encr 0 \
  || { echo "backup: canary encryption subkey failed" >&2; exit 3; }
printf 'canary-%s\n' "$(date -u +%s)" > "$work/canary.plain"
gpg --batch --trust-model always --recipient "$canary_fpr" --encrypt --output "$work/canary.gpg" "$work/canary.plain"
gpg --batch --pinentry-mode loopback --passphrase '' --decrypt "$work/canary.gpg" 2>/dev/null > "$work/canary.out"
cmp -s "$work/canary.plain" "$work/canary.out" || { echo "backup: canary failed, pipeline broken" >&2; exit 3; }

# 2. Import the operator's PUBLIC key only, and check it is the expected fingerprint.
gpg --batch --import "$pubkey" >/dev/null 2>&1
gpg --batch --list-keys --with-colons "$fpr" >/dev/null 2>&1 || { echo "backup: public key does not match fingerprint" >&2; exit 4; }

# 3. Dump, compress and encrypt in one stream. Only a complete file gets the final name.
stamp=$(date -u +%Y%m%dT%H%M%SZ)
out="$outdir/team-db-$stamp.dump.gpg"
tmp="$out.partial"
sh -c "$dump_cmd" | gpg --batch --yes --trust-model always --recipient "$fpr" --encrypt --output "$tmp"
[ -s "$tmp" ] || { echo "backup: empty output, not kept" >&2; rm -f "$tmp"; exit 5; }
mv "$tmp" "$out"
echo "backup: wrote $out ($(wc -c < "$out") bytes)"

# 4. Retention: keep the newest $keep dumps, delete the rest.
ls -1t "$outdir"/team-db-*.dump.gpg 2>/dev/null | tail -n +"$((keep + 1))" | while read -r old; do rm -f "$old"; done
