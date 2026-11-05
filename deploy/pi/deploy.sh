#!/usr/bin/env bash
# Deploy the agent from the operator machine to a Stand over ssh (the user needs passwordless sudo).
#
#   deploy.sh <host> --certs DIR (--ssh-from CIDR | --skip-firewall) [-- install.sh args]
#       first install or cert/config rotation: copies binary + bundle, runs install.sh
#   deploy.sh <host>
#       upgrade: replaces the binary and restarts
#
# Options: --user USER (default pi), --binary PATH, --ssh-from CIDR, --skip-firewall.
# <host> must be an IP or DNS name (not <name>.local if you pass --disable-avahi to install.sh).
#
# Safety net:
#   - the new binary is verified by sha256 after the copy;
#   - before certs/config are replaced, /etc/fotoko is archived to /var/lib/fotoko/backup-<ts>.tar
#     (0600 root); the newest 3 are kept;
#   - the health check waits up to 60 s for the agent log line "connected to broker";
#   - on failure (install.sh error or failed health check) the previous binary
#     (/usr/local/bin/fotoko-pi-agent.prev) and the /etc/fotoko backup are restored.
# Restoring old certs cannot revive a certificate that provision-printer --rotate already deleted
# in AWS: in that case issue a new one (see README).
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
HOST=""
SSH_USER=pi
CERTS=""
BINARY="$here/dist/fotoko-pi-agent"
SSH_FROM=""
SKIP_FIREWALL=0
EXTRA=()

die() { echo "deploy.sh: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case $1 in
    --user) SSH_USER=${2:?}; shift 2 ;;
    --certs) CERTS=${2:?}; shift 2 ;;
    --binary) BINARY=${2:?}; shift 2 ;;
    --ssh-from) SSH_FROM=${2:?}; shift 2 ;;
    --skip-firewall) SKIP_FIREWALL=1; shift ;;
    --) shift; EXTRA=("$@"); break ;;
    -h|--help) sed -n '2,22p' "${BASH_SOURCE[0]}"; exit 0 ;;
    -*) die "unknown option: $1 (install.sh options go after --)" ;;
    *) [ -z "$HOST" ] || die "unexpected argument: $1"; HOST=$1; shift ;;
  esac
done
[ -n "$HOST" ] || die "usage: deploy.sh <host> [--certs DIR (--ssh-from CIDR | --skip-firewall)] [--user USER] [--binary PATH] [-- install.sh args]"
[ -f "$BINARY" ] || die "missing $BINARY (run deploy/pi/build.sh)"
SHA_FILE="$BINARY.sha256"
[ -f "$SHA_FILE" ] || die "missing $SHA_FILE"
if [ -n "$CERTS" ]; then
  for f in device.pem.crt private.pem.key AmazonRootCA1.pem agent.toml; do
    [ -f "$CERTS/$f" ] || die "missing $CERTS/$f"
  done
  [ -n "$SSH_FROM" ] || [ "$SKIP_FIREWALL" = 1 ] || die "--certs needs --ssh-from CIDR or --skip-firewall (the firewall would otherwise cut off ssh)"
  [[ -z $SSH_FROM || $SSH_FROM =~ ^[0-9a-fA-F:.]+(/[0-9]+)?$ ]] || die "--ssh-from must be a CIDR, got: $SSH_FROM"
fi

TARGET="$SSH_USER@$HOST"
SSH=(ssh -o BatchMode=yes)
want=$(cut -d' ' -f1 "$SHA_FILE")
remote_dir=$("${SSH[@]}" "$TARGET" 'mktemp -d /tmp/fotoko-deploy.XXXXXX')
case $remote_dir in /tmp/fotoko-deploy.*) ;; *) die "unexpected remote temp dir: $remote_dir" ;; esac

# shellcheck disable=SC2317 # called through the EXIT trap
cleanup() {
  # One ssh call: shred the private key, then remove the directory.
  if ! "${SSH[@]}" "$TARGET" "d='$remote_dir'; shred -u \"\$d/certs/private.pem.key\" 2>/dev/null || rm -f \"\$d/certs/private.pem.key\"; rm -rf \"\$d\""; then
    echo "deploy.sh: WARNING: could not clean up $TARGET:$remote_dir; it may still hold the device private key. Delete it by hand: ssh $TARGET 'sudo shred -u $remote_dir/certs/private.pem.key; sudo rm -rf $remote_dir'" >&2
  fi
}
trap cleanup EXIT

remote() { "${SSH[@]}" "$TARGET" "$@"; }
since=$(remote "date '+%Y-%m-%d %H:%M:%S'")
backup=""

echo "copy binary"
scp -q "$BINARY" "$TARGET:$remote_dir/fotoko-pi-agent"
got=$(remote "sha256sum '$remote_dir/fotoko-pi-agent'" | cut -d' ' -f1)
[ "$got" = "$want" ] || die "sha256 mismatch after copy: local $want, remote $got"
echo "sha256 ok: $want"

rollback() {
  echo "deploy.sh: rolling back" >&2
  local restored=0
  if [ -n "$backup" ]; then
    # shellcheck disable=SC2029 # $backup is expanded locally on purpose
    if remote "sudo tar -xpf '$backup' -C /etc"; then
      echo "restored /etc/fotoko from $backup" >&2
      restored=1
    else
      echo "deploy.sh: WARNING: could not restore $backup" >&2
    fi
  fi
  if remote 'test -f /usr/local/bin/fotoko-pi-agent.prev'; then
    remote 'sudo cp -p /usr/local/bin/fotoko-pi-agent.prev /usr/local/bin/fotoko-pi-agent' && restored=1
  fi
  if [ "$restored" = 0 ]; then
    die "nothing to roll back to (fresh install?); inspect the Stand: journalctl -u fotoko-agent"
  fi
  remote 'sudo systemctl restart fotoko-agent.service' || true
  sleep 5
  if remote 'systemctl is-active --quiet fotoko-agent.service'; then
    die "deploy failed; rolled back, previous version is running"
  fi
  die "deploy failed and the rolled-back version is not active either; inspect the Stand"
}

if [ -n "$CERTS" ]; then
  echo "back up /etc/fotoko"
  ts=$(date +%Y%m%d%H%M%S)
  candidate="/var/lib/fotoko/backup-$ts.tar"
  # shellcheck disable=SC2029 # $candidate is expanded locally on purpose
  if remote "if [ -d /etc/fotoko ] && [ -d /var/lib/fotoko ]; then sudo sh -c \"umask 077; tar -cpf '$candidate' -C /etc fotoko\"; echo saved; fi" | grep -q saved; then
    backup=$candidate
    echo "backup: $backup"
  else
    echo "no existing /etc/fotoko (first install): nothing to back up"
  fi
  echo "copy bundle and run install.sh"
  remote "mkdir -m 0700 '$remote_dir/certs'"
  scp -q "$CERTS/device.pem.crt" "$CERTS/private.pem.key" "$CERTS/AmazonRootCA1.pem" "$CERTS/agent.toml" "$TARGET:$remote_dir/certs/"
  scp -q "$here/install.sh" "$here/fotoko-agent.service" "$TARGET:$remote_dir/"
  args=(--certs certs --binary fotoko-pi-agent)
  [ -z "$SSH_FROM" ] || args+=(--ssh-from "$SSH_FROM")
  [ "$SKIP_FIREWALL" = 0 ] || args+=(--skip-firewall)
  args+=("${EXTRA[@]}")
  quoted=$(printf '%q ' "${args[@]}")
  # shellcheck disable=SC2029 # the remote command line is built on purpose
  if ! remote "cd '$remote_dir' && sudo bash install.sh $quoted"; then
    echo "deploy.sh: install.sh failed on the Stand" >&2
    rollback
  fi
else
  echo "replace binary and restart"
  # shellcheck disable=SC2087 # expand $remote_dir locally on purpose
  if ! remote sudo bash -s <<REMOTE
set -eu
bin=/usr/local/bin/fotoko-pi-agent
[ -f "\$bin" ] && cp -p "\$bin" "\$bin.prev"
install -m 0755 -o root -g root "$remote_dir/fotoko-pi-agent" "\$bin.new"
mv -f "\$bin.new" "\$bin"
systemctl restart fotoko-agent.service
REMOTE
  then
    echo "deploy.sh: replacing the binary failed" >&2
    rollback
  fi
fi

echo "health check (up to 60 s for 'connected to broker')"
healthy=0
for _ in $(seq 1 20); do
  sleep 3
  # shellcheck disable=SC2029 # $since is expanded locally on purpose
  if remote "systemctl is-active --quiet fotoko-agent.service && journalctl -u fotoko-agent --since '$since' --no-pager | grep -q 'connected to broker'"; then
    healthy=1
    break
  fi
done
remote 'journalctl -u fotoko-agent -n 15 --no-pager' || true
if [ "$healthy" = 1 ]; then
  # Keep the newest 3 config backups.
  remote "sudo sh -c 'ls -1t /var/lib/fotoko/backup-*.tar 2>/dev/null | tail -n +4 | xargs -r rm -f'" || true
  echo "deploy ok"
  exit 0
fi
echo "deploy.sh: agent not active or never connected to the broker within 60 s" >&2
rollback
