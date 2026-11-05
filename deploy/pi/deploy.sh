#!/usr/bin/env bash
# Deploy the agent from the operator machine to a Stand.
#
#   deploy.sh <host> --certs DIR -- --model URI ...  first install / cert rotation: copies the
#                                                     bundle and runs install.sh with the extra args
#   deploy.sh <host>                                  upgrade: replace the binary and restart
#
# Options: --user USER (ssh user, default pi), --binary PATH, -- (rest goes to install.sh)
# Rolls back to /usr/local/bin/fotoko-pi-agent.prev if the service is not active after the restart.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
HOST=""
SSH_USER=pi
CERTS=""
BINARY="$here/dist/fotoko-pi-agent"
EXTRA=()

die() { echo "deploy.sh: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case $1 in
    --user) SSH_USER=${2:?}; shift 2 ;;
    --certs) CERTS=${2:?}; shift 2 ;;
    --binary) BINARY=${2:?}; shift 2 ;;
    --) shift; EXTRA=("$@"); break ;;
    -h|--help) sed -n '2,10p' "${BASH_SOURCE[0]}"; exit 0 ;;
    -*) die "unknown option: $1 (install.sh options go after --)" ;;
    *) [ -z "$HOST" ] || die "unexpected argument: $1"; HOST=$1; shift ;;
  esac
done
[ -n "$HOST" ] || die "usage: deploy.sh <host> [--certs DIR] [--user USER] [--binary PATH] [-- install.sh args]"
[ -f "$BINARY" ] || die "missing $BINARY (run deploy/pi/build.sh)"
SHA_FILE="$BINARY.sha256"
[ -f "$SHA_FILE" ] || die "missing $SHA_FILE"
if [ -n "$CERTS" ]; then
  for f in device.pem.crt private.pem.key AmazonRootCA1.pem agent.toml; do
    [ -f "$CERTS/$f" ] || die "missing $CERTS/$f"
  done
fi

TARGET="$SSH_USER@$HOST"
SSH=(ssh -o BatchMode=yes)
want=$(cut -d' ' -f1 "$SHA_FILE")
remote_dir=$("${SSH[@]}" "$TARGET" 'mktemp -d /tmp/fotoko-deploy.XXXXXX')
trap '"${SSH[@]}" "$TARGET" "rm -rf \"$remote_dir\"" >/dev/null 2>&1 || true' EXIT

echo "copy binary"
scp -q "$BINARY" "$TARGET:$remote_dir/fotoko-pi-agent"
got=$("${SSH[@]}" "$TARGET" "sha256sum \"$remote_dir/fotoko-pi-agent\"" | cut -d' ' -f1)
[ "$got" = "$want" ] || die "sha256 mismatch after copy: local $want, remote $got"
echo "sha256 ok: $want"

if [ -n "$CERTS" ]; then
  echo "copy bundle and run install.sh"
  "${SSH[@]}" "$TARGET" "mkdir -m 0700 \"$remote_dir/certs\""
  scp -q "$CERTS/device.pem.crt" "$CERTS/private.pem.key" "$CERTS/AmazonRootCA1.pem" "$CERTS/agent.toml" "$TARGET:$remote_dir/certs/"
  scp -q "$here/install.sh" "$here/fotoko-agent.service" "$TARGET:$remote_dir/"
  # shellcheck disable=SC2029 # the remote command line is built on purpose
  "${SSH[@]}" "$TARGET" "cd \"$remote_dir\" && sudo bash install.sh --certs certs ${EXTRA[*]+$(printf '%q ' "${EXTRA[@]}")}"
else
  echo "replace binary and restart"
  # shellcheck disable=SC2087 # expand $remote_dir locally on purpose
  "${SSH[@]}" "$TARGET" sudo bash -s <<REMOTE
set -eu
bin=/usr/local/bin/fotoko-pi-agent
[ -f "\$bin" ] && cp -p "\$bin" "\$bin.prev"
install -m 0755 -o root -g root "$remote_dir/fotoko-pi-agent" "\$bin.new"
mv -f "\$bin.new" "\$bin"
systemctl restart fotoko-agent.service
REMOTE
fi

echo "health check"
sleep 5
if "${SSH[@]}" "$TARGET" 'systemctl is-active --quiet fotoko-agent.service'; then
  "${SSH[@]}" "$TARGET" 'journalctl -u fotoko-agent -n 15 --no-pager'
  echo "deploy ok"
  exit 0
fi

echo "deploy.sh: service not active; journal:" >&2
"${SSH[@]}" "$TARGET" 'journalctl -u fotoko-agent -n 30 --no-pager' >&2 || true
echo "rolling back" >&2
if "${SSH[@]}" "$TARGET" 'test -f /usr/local/bin/fotoko-pi-agent.prev'; then
  "${SSH[@]}" "$TARGET" 'sudo sh -c "cp -p /usr/local/bin/fotoko-pi-agent.prev /usr/local/bin/fotoko-pi-agent && systemctl restart fotoko-agent.service"'
  sleep 3
  if "${SSH[@]}" "$TARGET" 'systemctl is-active --quiet fotoko-agent.service'; then
    die "new version failed, rolled back to previous binary"
  fi
  die "rollback also failed; inspect the Stand"
fi
die "no previous binary to roll back to"
