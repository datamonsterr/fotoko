#!/usr/bin/env bash
# Dry-run tests for install.sh: no root, no apt, no changes outside a temp dir.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
install_sh="$here/../install.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

certs="$tmp/certs"
mkdir -p "$certs"
for f in device.pem.crt private.pem.key AmazonRootCA1.pem agent.toml; do echo fake > "$certs/$f"; done
echo fake-binary > "$tmp/fotoko-pi-agent"

fail=0
check() { # name, haystack, needle
  if grep -qF -- "$3" <<<"$2"; then echo "ok   $1"; else echo "FAIL $1: missing '$3'"; fail=1; fi
}
absent() { # name, haystack, needle
  if grep -qF -- "$3" <<<"$2"; then echo "FAIL $1: unexpected '$3'"; fail=1; else echo "ok   $1"; fi
}

run_install() { FOTOKO_ROOT="$tmp/root" bash "$install_sh" --dry-run --binary "$tmp/fotoko-pi-agent" --certs "$certs" "$@"; }

out=$(run_install --model 'gutenprint.5.3://canon-selphy-cp1300/expert' --device-uri 'usb://Canon/SELPHY?serial=1' \
  --ssh-from 192.168.1.0/24 --disable-bluetooth --disable-avahi)
r="$tmp/root"
check "installs cups"            "$out" "printer-driver-gutenprint"
check "creates user"             "$out" "useradd --system --user-group --home-dir /var/lib/fotoko --no-create-home --shell /usr/sbin/nologin fotoko"
check "conf dir 0750"            "$out" "install -d -m 0750 -o root -g fotoko $r/etc/fotoko"
check "state dir 0750"           "$out" "install -d -m 0750 -o fotoko -g fotoko $r/var/lib/fotoko"
check "binary installed"         "$out" "$r/usr/local/bin/fotoko-pi-agent"
check "key 0400 fotoko"          "$out" "install -m 0400 -o fotoko -g fotoko $certs/private.pem.key $r/etc/fotoko/private.pem.key"
check "agent.toml installed"     "$out" "$r/etc/fotoko/agent.toml"
check "cups local only"          "$out" "cupsctl --no-remote-admin --no-share-printers --no-remote-any"
check "job history"              "$out" "cupsctl PreserveJobHistory=Yes"
check "lpadmin queue"            "$out" "lpadmin -p fotoko -E -v usb://Canon/SELPHY?serial=1 -m gutenprint.5.3://canon-selphy-cp1300/expert"
check "default media"            "$out" "media=w288h432"
check "error policy"             "$out" "printer-error-policy=stop-printer"
check "ufw deny in"              "$out" "ufw default deny incoming"
check "ufw allow out"            "$out" "ufw default allow outgoing"
check "ufw ssh from cidr"        "$out" "ufw allow from 192.168.1.0/24 to any port 22 proto tcp"
check "unattended upgrades"      "$out" "unattended-upgrades.service"
check "bluetooth disabled"       "$out" "bluetooth.service"
check "avahi disabled"           "$out" "avahi-daemon.service"
check "unit installed"           "$out" "$r/etc/systemd/system/fotoko-agent.service"
check "service enabled"          "$out" "systemctl enable fotoko-agent.service"
if [ -e "$r" ]; then echo "FAIL dry-run created $r"; fail=1; else echo "ok   dry-run created nothing"; fi

out=$(run_install --model m --skip-firewall)
absent "no ufw when skipped"     "$out" "ufw default"
check "autodetect placeholder"   "$out" "<usb:// URI autodetected by lpinfo -v>"
absent "no ssh rule w/o flag"    "$out" "port 22"
absent "no bluetooth by default" "$out" "bluetooth.service"

out=$(run_install --skip-printer)
absent "skip-printer: no lpadmin" "$out" "lpadmin"

if FOTOKO_ROOT="$tmp/root" bash "$install_sh" --dry-run --binary "$tmp/fotoko-pi-agent" --certs "$certs" >/dev/null 2>&1; then
  echo "FAIL missing --model accepted"; fail=1
else echo "ok   --model required"; fi
if run_install --model m --ssh-from 'x; rm -rf /' >/dev/null 2>&1; then
  echo "FAIL bad --ssh-from accepted"; fail=1
else echo "ok   bad --ssh-from rejected"; fi
rm "$certs/agent.toml"
if run_install --model m >/dev/null 2>&1; then
  echo "FAIL missing agent.toml accepted"; fail=1
else echo "ok   missing cert file rejected"; fi

exit "$fail"
