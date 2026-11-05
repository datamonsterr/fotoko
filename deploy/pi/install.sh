#!/usr/bin/env bash
# Provision a Fotoko Stand on a Raspberry Pi (Debian bookworm/trixie, aarch64). Run as root ON the Pi.
# Idempotent: safe to re-run (also used for upgrades and cert rotation).
#
#   sudo ./install.sh --certs DIR --model gutenprint.5.3://canon-selphy-cp1300/expert [options]
#
# DIR holds the provision-printer output: device.pem.crt private.pem.key AmazonRootCA1.pem agent.toml
# Find a model string with:  lpinfo -m | grep -i gutenprint | grep -i <printer name>
#
# FOTOKO_ROOT=/some/dir prefixes every file path (for tests; combine with --dry-run).
set -euo pipefail

ROOT=${FOTOKO_ROOT:-}
CONF_DIR="$ROOT/etc/fotoko"
STATE_DIR="$ROOT/var/lib/fotoko"
BIN_DST="$ROOT/usr/local/bin/fotoko-pi-agent"
UNIT_DST="$ROOT/etc/systemd/system/fotoko-agent.service"
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

BINARY="$here/fotoko-pi-agent"
[ -f "$BINARY" ] || BINARY="$here/dist/fotoko-pi-agent"
CERTS=""
MODEL=""
DEVICE_URI=""
QUEUE=fotoko
MEDIA=w288h432
SSH_FROM=""
DRY_RUN=0
SKIP_PRINTER=0
SKIP_FIREWALL=0
DISABLE_BLUETOOTH=0
DISABLE_AVAHI=0

usage() {
  cat <<'USAGE'
Usage: install.sh --certs DIR [--model PPD_URI] [options]

  --certs DIR          provision-printer output dir (required)
  --model URI          Gutenprint driver for lpadmin -m (required unless --skip-printer)
  --device-uri URI     CUPS device URI; default: the single usb:// printer found by lpinfo -v
  --queue NAME         CUPS queue name (default fotoko; must match printer.queue in agent.toml)
  --media NAME         default media (default w288h432 = 4x6 in)
  --binary PATH        agent binary (default ./fotoko-pi-agent or ./dist/fotoko-pi-agent)
  --ssh-from CIDR      allow inbound ssh from this CIDR (e.g. 192.168.1.0/24); default: no inbound at all
  --skip-firewall      do not touch the firewall
  --skip-printer       do not create the CUPS queue (agent.toml may use backend = "sim")
  --disable-bluetooth  disable bluetooth and hciuart services
  --disable-avahi      disable avahi-daemon
  --dry-run            print the actions, change nothing
  -h, --help
USAGE
}

die() { echo "install.sh: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case $1 in
    --certs) CERTS=${2:?}; shift 2 ;;
    --model) MODEL=${2:?}; shift 2 ;;
    --device-uri) DEVICE_URI=${2:?}; shift 2 ;;
    --queue) QUEUE=${2:?}; shift 2 ;;
    --media) MEDIA=${2:?}; shift 2 ;;
    --binary) BINARY=${2:?}; shift 2 ;;
    --ssh-from) SSH_FROM=${2:?}; shift 2 ;;
    --skip-firewall) SKIP_FIREWALL=1; shift ;;
    --skip-printer) SKIP_PRINTER=1; shift ;;
    --disable-bluetooth) DISABLE_BLUETOOTH=1; shift ;;
    --disable-avahi) DISABLE_AVAHI=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument: $1" ;;
  esac
done

[ -n "$CERTS" ] || { usage >&2; die "--certs is required"; }
[ "$SKIP_PRINTER" = 1 ] || [ -n "$MODEL" ] || die "--model is required (see: lpinfo -m | grep -i gutenprint), or pass --skip-printer"
[ -f "$BINARY" ] || die "agent binary not found: $BINARY (run deploy/pi/build.sh, or pass --binary)"
for f in device.pem.crt private.pem.key AmazonRootCA1.pem agent.toml; do
  [ -f "$CERTS/$f" ] || die "missing $CERTS/$f"
done
if [ -n "$SSH_FROM" ]; then
  [[ $SSH_FROM =~ ^[0-9a-fA-F:.]+(/[0-9]+)?$ ]] || die "--ssh-from must be a CIDR, got: $SSH_FROM"
fi
if [ "$DRY_RUN" = 0 ] && [ "$(id -u)" != 0 ]; then
  die "must run as root (or use --dry-run)"
fi

# run CMD...: print in dry-run mode, execute otherwise.
run() {
  if [ "$DRY_RUN" = 1 ]; then
    echo "[dry-run] $*"
  else
    echo "+ $*"
    "$@"
  fi
}
# try CMD...: like run, but a failure only warns.
try() { run "$@" || echo "install.sh: warning: failed: $*" >&2; }
step() { echo; echo "== $*"; }

step "packages"
# cups: scheduler; printer-driver-gutenprint: PPDs + rastertogutenprint (pulls cups-filters);
# cups-client: lp, lpstat, cancel, lpadmin, lpinfo (in cups-client since bookworm; pulled by cups).
# ufw / unattended-upgrades: firewall and security updates.
PKGS=(cups cups-client printer-driver-gutenprint unattended-upgrades)
[ "$SKIP_FIREWALL" = 1 ] || PKGS+=(ufw)
run env DEBIAN_FRONTEND=noninteractive apt-get update -y
run env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${PKGS[@]}"

step "user and directories"
if [ "$DRY_RUN" = 1 ] || ! id fotoko >/dev/null 2>&1; then
  # No login, no extra groups: lp/cancel/lpstat work for any local user over the CUPS socket,
  # and the agent must not be able to administer CUPS.
  run useradd --system --user-group --home-dir /var/lib/fotoko --no-create-home --shell /usr/sbin/nologin fotoko
else
  echo "user fotoko exists"
fi
run install -d -m 0750 -o root -g fotoko "$CONF_DIR"
run install -d -m 0750 -o fotoko -g fotoko "$STATE_DIR"

step "binary"
run install -d -m 0755 "$(dirname "$BIN_DST")"
if [ -f "$BIN_DST" ] && [ "$DRY_RUN" = 0 ] && ! cmp -s "$BINARY" "$BIN_DST"; then
  run cp -p "$BIN_DST" "$BIN_DST.prev"
elif [ -f "$BIN_DST" ] && [ "$DRY_RUN" = 1 ]; then
  echo "[dry-run] keep previous binary as $BIN_DST.prev if it differs"
fi
run install -m 0755 -o root -g root "$BINARY" "$BIN_DST.new"
run mv -f "$BIN_DST.new" "$BIN_DST"

step "certificates and config"
run install -m 0444 -o root -g fotoko "$CERTS/AmazonRootCA1.pem" "$CONF_DIR/AmazonRootCA1.pem"
run install -m 0444 -o root -g fotoko "$CERTS/device.pem.crt" "$CONF_DIR/device.pem.crt"
# The agent refuses a key with group/other bits; 0400 owned by the service user.
run install -m 0400 -o fotoko -g fotoko "$CERTS/private.pem.key" "$CONF_DIR/private.pem.key"
run install -m 0640 -o root -g fotoko "$CERTS/agent.toml" "$CONF_DIR/agent.toml"

if [ "$SKIP_PRINTER" = 0 ]; then
  step "cups"
  run systemctl enable --now cups.service
  # Local only: no remote admin, no sharing, no remote access.
  run cupsctl --no-remote-admin --no-share-printers --no-remote-any
  # The agent reads 'lpstat -W completed' to see how a job ended: keep job history.
  try cupsctl PreserveJobHistory=Yes
  # Network printer discovery is unwanted on a kiosk.
  try systemctl disable --now cups-browsed.service
  if [ -z "$DEVICE_URI" ]; then
    if [ "$DRY_RUN" = 1 ]; then
      DEVICE_URI="<usb:// URI autodetected by lpinfo -v>"
    else
      mapfile -t uris < <(lpinfo -v | awk '$1 == "direct" && $2 ~ /^usb:/ { print $2 }')
      [ "${#uris[@]}" -eq 1 ] || die "expected exactly one USB printer, found ${#uris[@]}; pass --device-uri (see: lpinfo -v)"
      DEVICE_URI=${uris[0]}
    fi
  fi
  # printer-error-policy=stop-printer: on a paper/ribbon/cover error CUPS disables the queue and
  # keeps the job, so the agent sees 'disabled' in lpstat -p and reports paper_out/ribbon_out.
  # abort-job would drop the job and leave the queue looking healthy. After refilling, run
  # 'cupsenable <queue>' (the job then resumes).
  run lpadmin -p "$QUEUE" -E -v "$DEVICE_URI" -m "$MODEL" \
    -o "media=$MEDIA" -o printer-error-policy=stop-printer -o printer-is-shared=false
  run cupsenable "$QUEUE"
  run cupsaccept "$QUEUE"
  run lpadmin -d "$QUEUE"
fi

if [ "$SKIP_FIREWALL" = 0 ]; then
  step "firewall (ufw)"
  run ufw default deny incoming
  run ufw default allow outgoing
  if [ -n "$SSH_FROM" ]; then
    run ufw allow from "$SSH_FROM" to any port 22 proto tcp
  else
    echo "no --ssh-from: no inbound ssh will be allowed (console access only)"
  fi
  run ufw --force enable
fi

step "unattended upgrades"
run systemctl enable --now unattended-upgrades.service
step "time sync (TLS needs a correct clock)"
try systemctl enable --now systemd-timesyncd.service

if [ "$DISABLE_BLUETOOTH" = 1 ]; then
  step "disable bluetooth"
  try systemctl disable --now bluetooth.service hciuart.service
fi
if [ "$DISABLE_AVAHI" = 1 ]; then
  step "disable avahi"
  try systemctl disable --now avahi-daemon.service avahi-daemon.socket
fi

step "systemd unit"
run install -m 0644 -o root -g root "$here/fotoko-agent.service" "$UNIT_DST"
run systemctl daemon-reload
run systemctl enable fotoko-agent.service
run systemctl restart fotoko-agent.service
echo
echo "done. check: systemctl is-active fotoko-agent; journalctl -u fotoko-agent -n 50"
