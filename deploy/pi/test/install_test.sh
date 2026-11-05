#!/usr/bin/env bash
# Tests for install.sh and the service unit. No root, no apt, nothing outside a temp dir:
#   - dry-run output assertions
#   - real (non-dry-run) runs under FOTOKO_ROOT with PATH stubs for apt-get/systemctl/lpadmin/...
# shellcheck disable=SC2015 # ok/bad never fail, so A && ok || bad is safe
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
install_sh="$here/../install.sh"
unit="$here/../fotoko-agent.service"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
check() { if grep -qF -- "$3" <<<"$2"; then ok "$1"; else bad "$1: missing '$3'"; fi; }
absent() { if grep -qF -- "$3" <<<"$2"; then bad "$1: unexpected '$3'"; else ok "$1"; fi; }
expect_fail() { # name, cmd...
  local name=$1; shift
  if "$@" >/dev/null 2>&1; then bad "$name: expected failure"; else ok "$name"; fi
}

certs="$tmp/certs"
mkdir -p "$certs"
for f in device.pem.crt private.pem.key AmazonRootCA1.pem; do echo fake > "$certs/$f"; done
echo 'printer_id = "stand-001"' > "$certs/agent.toml"
echo v1 > "$tmp/bin-v1"
echo v2 > "$tmp/bin-v2"

# ---- service unit ------------------------------------------------------------------------
u=$(cat "$unit")
for field in User=fotoko Environment=USER=fotoko Restart=always NoNewPrivileges=true ProtectSystem=strict \
  ProtectHome=true PrivateTmp=true ReadWritePaths=/var/lib/fotoko CapabilityBoundingSet= \
  'RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX' SystemCallFilter=@system-service MemoryMax=256M \
  ExecStart='/usr/local/bin/fotoko-pi-agent --config /etc/fotoko/agent.toml' After=network-online.target; do
  check "unit has $field" "$u" "$field"
done
# TimeoutStopSec must exceed shutdown_grace_secs (default 20) + 10.
stop=$(sed -n 's/^TimeoutStopSec=//p' "$unit")
if [ "${stop:-0}" -gt 30 ]; then ok "TimeoutStopSec $stop > 30"; else bad "TimeoutStopSec too small: ${stop:-unset}"; fi
absent "unit has no WatchdogSec" "$u" "WatchdogSec"

# ---- dry run -----------------------------------------------------------------------------
dry() { env -u SSH_CONNECTION FOTOKO_ROOT="$tmp/root" bash "$install_sh" --dry-run --binary "$tmp/bin-v1" --certs "$certs" "$@"; }
r="$tmp/root"
out=$(dry --model 'gutenprint.5.3://canon-selphy-cp1300/expert' --device-uri 'usb://Canon/SELPHY?serial=1' \
  --ssh-from 192.168.1.0/24 --disable-bluetooth --disable-avahi)
check "installs gutenprint"  "$out" "printer-driver-gutenprint"
check "creates user"         "$out" "useradd --system --user-group --home-dir /var/lib/fotoko --no-create-home --shell /usr/sbin/nologin fotoko"
check "conf dir 0750"        "$out" "install -d -m 0750 -o root -g fotoko $r/etc/fotoko"
check "state dir 0750"       "$out" "install -d -m 0750 -o fotoko -g fotoko $r/var/lib/fotoko"
check "key 0400 fotoko"      "$out" "install -m 0400 -o fotoko -g fotoko $certs/private.pem.key $r/etc/fotoko/private.pem.key"
check "cups local only"      "$out" "cupsctl --no-remote-admin --no-share-printers --no-remote-any"
check "job history"          "$out" "cupsctl PreserveJobHistory=Yes"
check "lpadmin queue"        "$out" "lpadmin -p fotoko -E -v usb://Canon/SELPHY?serial=1 -m gutenprint.5.3://canon-selphy-cp1300/expert"
check "default media"        "$out" "media=w288h432"
check "error policy"         "$out" "printer-error-policy=stop-printer"
check "ufw deny in"          "$out" "ufw default deny incoming"
check "ufw allow out"        "$out" "ufw default allow outgoing"
check "ufw ssh from cidr"    "$out" "ufw allow from 192.168.1.0/24 to any port 22 proto tcp"
check "auto-upgrades file"   "$out" "20auto-upgrades"
check "bluetooth disabled"   "$out" "bluetooth.service"
check "avahi disabled"       "$out" "avahi-daemon.service"
check "unit installed"       "$out" "$r/etc/systemd/system/fotoko-agent.service"
if [ -e "$r" ]; then bad "dry-run created $r"; else ok "dry-run created nothing"; fi

out=$(dry --model m --skip-firewall)
absent "no ufw when skipped"     "$out" "ufw default"
check "autodetect placeholder"   "$out" "<usb:// URI autodetected by lpinfo -v>"
out=$(dry --model m --ssh-from 10.0.0.0/8)
absent "no bluetooth by default" "$out" "bluetooth.service"
out=$(dry --skip-printer --skip-firewall)
absent "skip-printer: no lpadmin" "$out" "lpadmin"

expect_fail "--model required"          dry --skip-firewall
expect_fail "bad --ssh-from rejected"   dry --model m --ssh-from 'x; rm -rf /'
mv "$certs/agent.toml" "$tmp/agent.toml.bak"
expect_fail "missing cert file rejected" dry --model m --skip-firewall
mv "$tmp/agent.toml.bak" "$certs/agent.toml"

# ---- ssh lock-out guard -----------------------------------------------------------------
ssh_run() { SSH_CONNECTION='10.0.0.2 5555 10.0.0.9 22' FOTOKO_ROOT="$tmp/root" bash "$install_sh" --dry-run --binary "$tmp/bin-v1" --certs "$certs" --model m "$@"; }
msg=$(ssh_run 2>&1 || true)
check "ssh without --ssh-from refused" "$msg" "lock you out"
expect_fail "ssh guard exit code" ssh_run
ssh_run --ssh-from 10.0.0.0/8 >/dev/null 2>&1 && ok "ssh + --ssh-from allowed" || bad "ssh + --ssh-from refused"
ssh_run --skip-firewall >/dev/null 2>&1 && ok "ssh + --skip-firewall allowed" || bad "ssh + --skip-firewall refused"

# ---- real run with stubs -----------------------------------------------------------------
stubs="$tmp/stubs"
mkdir -p "$stubs"
export STUB_LOG="$tmp/stub.log" STUB_STATE="$tmp/stubstate"
mkdir -p "$STUB_STATE"
: > "$STUB_LOG"
for cmd in apt-get systemctl lpadmin cupsctl cupsenable cupsaccept ufw useradd lpinfo lpoptions; do
  cat > "$stubs/$cmd" <<STUB
#!/bin/sh
echo "$cmd \$*" >> "\$STUB_LOG"
case "$cmd" in
  useradd) touch "\$STUB_STATE/user" ;;
  lpinfo) echo "direct usb://Canon/SELPHY" ;;
  lpoptions) echo "PageSize/Media Size: *w288h432 w144h432" ;;
esac
exit 0
STUB
  chmod +x "$stubs/$cmd"
done
cat > "$stubs/id" <<'STUB'
#!/bin/sh
[ "$1" = fotoko ] && [ -e "$STUB_STATE/user" ]
STUB
chmod +x "$stubs/id"

real_root="$tmp/real"
mkdir -p "$real_root/etc/cups"
printf 'Listen localhost:631\nListen /run/cups/cups.sock\nPreserveJobHistory Yes\n' > "$real_root/etc/cups/cupsd.conf"
real() { # binary, extra args...
  local bin=$1; shift
  env -u SSH_CONNECTION PATH="$stubs:$PATH" FOTOKO_ROOT="$real_root" bash "$install_sh" \
    --binary "$bin" --certs "$certs" --model m --ssh-from 10.0.0.0/8 "$@"
}

real "$tmp/bin-v1" >"$tmp/run1.out" 2>"$tmp/run1.err" && ok "real run 1" || { bad "real run 1"; cat "$tmp/run1.err"; }
bin_dst="$real_root/usr/local/bin/fotoko-pi-agent"
[ "$(cat "$bin_dst")" = v1 ] && ok "binary installed" || bad "binary content"
[ ! -e "$bin_dst.prev" ] && ok "no .prev on first install" || bad ".prev on first install"
[ "$(stat -c %a "$real_root/etc/fotoko/private.pem.key")" = 400 ] && ok "key mode 0400" || bad "key mode"
[ "$(stat -c %a "$real_root/etc/fotoko/agent.toml")" = 640 ] && ok "agent.toml mode 0640" || bad "agent.toml mode"
[ "$(stat -c %a "$real_root/etc/fotoko")" = 750 ] && ok "conf dir mode 0750" || bad "conf dir mode"
cmp -s "$unit" "$real_root/etc/systemd/system/fotoko-agent.service" && ok "unit installed" || bad "unit content"
grep -q 'Unattended-Upgrade "1"' "$real_root/etc/apt/apt.conf.d/20auto-upgrades" && ok "20auto-upgrades written" || bad "20auto-upgrades"
log=$(cat "$STUB_LOG")
check "stub lpadmin ran"   "$log" "lpadmin -p fotoko -E -v usb://Canon/SELPHY -m m"
check "stub ufw enabled"   "$log" "ufw --force enable"
check "stub unit restart"  "$log" "systemctl restart fotoko-agent.service"
absent "no media warning"  "$(cat "$tmp/run1.err")" "not listed"

real "$tmp/bin-v2" >/dev/null 2>&1 && ok "real run 2 (upgrade)" || bad "real run 2"
[ "$(cat "$bin_dst")" = v2 ] && ok "binary upgraded" || bad "binary not upgraded"
[ "$(cat "$bin_dst.prev")" = v1 ] && ok ".prev holds previous binary" || bad ".prev content"
real "$tmp/bin-v2" >/dev/null 2>&1 && ok "real run 3 (idempotent)" || bad "real run 3"
[ "$(cat "$bin_dst")" = v2 ] && [ "$(cat "$bin_dst.prev")" = v1 ] && ok "same binary leaves .prev alone" || bad "idempotent run changed .prev"
[ "$(grep -c '^useradd' "$STUB_LOG")" = 1 ] && ok "user created once" || bad "useradd count $(grep -c '^useradd' "$STUB_LOG")"
[ "$(stat -c %a "$real_root/etc/fotoko/private.pem.key")" = 400 ] && ok "key mode after re-run" || bad "key mode after re-run"

# warnings and hard failures
mismatch="$tmp/mismatch"
mkdir -p "$mismatch"
cp "$certs"/*.pem* "$mismatch/" 2>/dev/null || true
printf '[printer]\nqueue = "other"\nmedia = "w100h100"\n' > "$mismatch/agent.toml"
err=$(env -u SSH_CONNECTION PATH="$stubs:$PATH" FOTOKO_ROOT="$real_root" bash "$install_sh" --binary "$tmp/bin-v2" \
  --certs "$mismatch" --model m --skip-firewall 2>&1 >/dev/null || true)
check "queue mismatch warned" "$err" "differs from printer.queue"
check "media mismatch warned" "$err" "differs from printer.media"

printf 'Listen 0.0.0.0:631\nPreserveJobHistory Yes\n' > "$real_root/etc/cups/cupsd.conf"
err=$(real "$tmp/bin-v2" 2>&1 >/dev/null || true)
check "non-local Listen warned" "$err" "Listen/Port directive"
printf 'Listen localhost:631\nPreserveJobHistory No\n' > "$real_root/etc/cups/cupsd.conf"
expect_fail "PreserveJobHistory No is fatal" real "$tmp/bin-v2"

# media missing from the driver options -> warning
printf '#!/bin/sh\necho "PageSize/Media Size: *a4 letter"\n' > "$stubs/lpoptions"
printf 'Listen localhost:631\n' > "$real_root/etc/cups/cupsd.conf"
err=$(real "$tmp/bin-v2" 2>&1 >/dev/null || true)
check "media post-check warns" "$err" "not listed"

exit "$fail"
