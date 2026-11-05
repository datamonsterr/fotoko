# Raspberry Pi provisioning

One Stand = one Raspberry Pi (64-bit Raspberry Pi OS or Debian, bookworm or trixie, aarch64) wired by USB to a photo printer that CUPS + Gutenprint drive. The Pi runs `fotoko-pi-agent` under systemd and talks to AWS IoT Core over MQTT TLS with a per-Stand device certificate. Interfaces: `docs/contracts.md`.

| File                    | What                                                                                  |
| ----------------------- | ------------------------------------------------------------------------------------- |
| `build.sh`              | Cross-build the agent for aarch64 into `dist/` (+ sha256)                             |
| `install.sh`            | Runs on the Pi as root, idempotent: packages, user, certs, CUPS queue, firewall, unit |
| `deploy.sh`             | From the operator machine: copy + install/upgrade over ssh, health check, rollback    |
| `fotoko-agent.service`  | Hardened systemd unit                                                                 |
| `wifi-and-hardening.md` | Wi-Fi (nmcli), clock, journald, kiosk hardening, watchdog note                        |
| `test/install_test.sh`  | Dry-run test of `install.sh`                                                          |

## Runbook

### 1. Provision the device certificate (operator machine)

```sh
pnpm --filter @fotoko/infra provision-printer --printer-id stand-001 --stage prod --out ./printer-out/stand-001
```

Writes `device.pem.crt`, `private.pem.key`, `AmazonRootCA1.pem` and `agent.toml` (see `infra/cdk/README.md`). Keep the directory secret and out of git.

### 2. Build the agent

```sh
pip install ziglang && cargo install --locked cargo-zigbuild   # once; or: apt install gcc-aarch64-linux-gnu
bash deploy/pi/build.sh                                         # -> deploy/pi/dist/fotoko-pi-agent + .sha256
```

`build.sh` prefers `cargo zigbuild` (pinned to glibc 2.36, the bookworm level, so the binary also runs on trixie) and falls back to `aarch64-linux-gnu-gcc`. `ring` (rustls) needs a C compiler for the target. In a git worktree add the untracked `.cargo/config.toml` with its own `target-dir` first (see `AGENTS.md`).

### 3. Flash and boot the Pi

Raspberry Pi Imager, "Raspberry Pi OS Lite (64-bit)". In the advanced settings set hostname, a non-default user, ssh with your public key (disable password login) and the Wi-Fi network. Boot, connect the printer by USB, then check `lsusb` shows it and `ssh <user>@<ip-or-dns-name>` works (use the IP or a DNS name in all commands below; `<hostname>.local` relies on avahi, which `--disable-avahi` turns off). Details and a nmcli alternative: `wifi-and-hardening.md`.

### 4. Find the printer driver

On the Pi (CUPS is installed by `install.sh`; run `sudo apt install cups printer-driver-gutenprint` first to browse before the full install):

```sh
lpinfo -m | grep -i gutenprint | grep -i selphy      # model strings, e.g. gutenprint.5.3://canon-selphy-cp1300/expert
lpinfo -v                                            # direct usb://Canon/SELPHY%20CP1300?serial=...
```

Pass the model string as `--model`. The USB device URI is autodetected when exactly one USB printer is attached; otherwise pass `--device-uri`. After install, `lpoptions -p fotoko -l` lists the queue options: confirm `w288h432` (4x6 in) is a valid `PageSize`/media value for your driver, else pass `--media` with the right name and set the same in `agent.toml` (`printer.media`).

### 5. Install

First install (copies binary + certs, runs `install.sh`; everything after `--` goes to `install.sh`):

```sh
bash deploy/pi/deploy.sh 192.168.1.50 --user pi --certs ./printer-out/stand-001 --ssh-from 192.168.1.0/24 -- \
  --model gutenprint.5.3://canon-selphy-cp1300/expert --disable-bluetooth --disable-avahi
```

Or by hand on the Pi: copy `deploy/pi/` plus `dist/fotoko-pi-agent` and the cert directory, then `sudo ./install.sh --certs ./certs --model ... (--ssh-from CIDR | --skip-firewall) [--device-uri ...]`. Over ssh, `install.sh` refuses to enable the firewall without `--ssh-from` (it would cut your session); `deploy.sh --certs` likewise needs `--ssh-from` or `--skip-firewall`. Preview any run with `--dry-run`.

What `install.sh` sets up:

- Packages: `cups`, `cups-client` (`lp`, `lpstat`, `cancel`, `lpadmin`), `printer-driver-gutenprint`, `unattended-upgrades`, `ufw`.
- System user `fotoko` (no login, no extra groups). `lp`/`lpstat`/`cancel` work for any local user over the CUPS socket, so the agent gets no `lpadmin`/`lp` group.
- `/etc/fotoko` 0750 root:fotoko (`agent.toml` 0640; CA and cert 0444; `private.pem.key` 0400 fotoko, the agent refuses anything looser), `/var/lib/fotoko` 0750 fotoko, `/usr/local/bin/fotoko-pi-agent`.
- CUPS queue `fotoko` (`lpadmin -E`), default media `w288h432`, local only (`cupsctl --no-remote-admin --no-share-printers --no-remote-any`, `cups-browsed` disabled), `PreserveJobHistory Yes` (the agent reads `lpstat -W completed` to learn how a job ended).
- `printer-error-policy=stop-printer`: on a paper or ribbon error CUPS stops the queue and holds the job. The agent sees the stopped queue in `lpstat -p`, cancels the failed job and reports it failed (`paper_out`/`ribbon_out`), so the customer's order is reported failed rather than lost silently. `abort-job` would drop the print and leave the queue looking healthy. After fixing the cause the operator re-enables the queue with `sudo cupsenable fotoko`; the failed job is not reprinted.
- `install.sh` also checks `cupsd.conf` (job history on, `Listen` loopback only), warns when `--queue`/`--media` differ from `agent.toml` or the driver does not list the media, and writes `/etc/apt/apt.conf.d/20auto-upgrades`.
- ufw: deny incoming, allow outgoing, ssh only from `--ssh-from` (no inbound ssh without it, so keep console access). Without ufw, or with `--skip-firewall`, set an equivalent nftables policy yourself.
- `unattended-upgrades`, `systemd-timesyncd`, and the `fotoko-agent` unit (enabled and started).

Upgrade (binary only, keeps the old one as `fotoko-pi-agent.prev`):

```sh
bash deploy/pi/build.sh && bash deploy/pi/deploy.sh 192.168.1.50
```

`deploy.sh` verifies the sha256 after copying, restarts, then waits up to 60 s for `systemctl is-active` and the agent log line `connected to broker`. On failure it restores `.prev`. With `--certs` it first archives `/etc/fotoko` to `/var/lib/fotoko/backup-<ts>.tar` (0600 root, newest 3 kept) and restores it, plus the previous binary, if `install.sh` fails or the health check fails. A failed `install.sh` may already have changed CUPS or ufw settings; those are not rolled back (re-run after fixing). The temp dir on the Stand, including the private key, is shredded afterwards; if that cleanup fails `deploy.sh` prints the path loudly.

In `agent.toml` with `backend = "sim"` (no printer, for bench tests) `sim.out_dir` must be under `/var/lib/fotoko`: the unit is `ProtectSystem=strict` and only that path is writable.

### 6. Verify

```sh
ssh pi@192.168.1.50
systemctl is-active fotoko-agent            # active
journalctl -u fotoko-agent -n 50 --no-pager # "connected" / heartbeat lines, no TLS errors
lpstat -p fotoko                            # printer fotoko is idle.  enabled since ...
sudo -u fotoko lp -d fotoko /usr/share/cups/data/testprint   # physical test print (CUPS test page)
lpstat -W completed -o fotoko               # test job listed in history
```

End-to-end: create an order for the Stand in the app (or publish a test job as in `crates/e2e`) and watch `journalctl -fu fotoko-agent` for the job, `printing`, `printed`. `systemd-analyze security fotoko-agent` shows the unit's exposure score (offline estimate: 1.2 OK).

### 7. Rotate the certificate

```sh
pnpm --filter @fotoko/infra provision-printer --printer-id stand-001 --stage prod --out ./printer-out/stand-001-new --rotate
```

Rotation retires the old certificate immediately, so the Stand is offline until the new files are installed. `deploy.sh` backs up `/etc/fotoko` and restores it if the new files fail, but that only helps when the old certificate is still valid: `--rotate` has already deleted it in AWS, so after a failed install fix the cause and re-run with the new files (or provision again). Run `deploy.sh 192.168.1.50 --certs ./printer-out/stand-001-new -- --model <same model>` (re-running `install.sh` is idempotent; the existing CUPS queue is reconfigured in place) and check `journalctl -u fotoko-agent`.

## Troubleshooting

| Symptom                                   | Check                                                                                                                                                                |
| ----------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Service restarts in a loop                | `journalctl -u fotoko-agent -n 100`: config error (paths, key mode must be 0400/0600), TLS, DNS                                                                      |
| TLS handshake / certificate not yet valid | Wrong clock: `timedatectl` must say `System clock synchronized: yes`                                                                                                 |
| Stand shows offline in the app            | `systemctl is-active fotoko-agent`, Wi-Fi (`nmcli device`), `ufw status` (outbound 8883 open)                                                                        |
| `lpstat -p fotoko` says disabled          | Paper, ribbon or cover: fix the cause, then `sudo cupsenable fotoko`; reason in `lpstat -l -p fotoko`                                                                |
| Printer not found at install              | `lsusb`, `lpinfo -v`; pass `--device-uri`; try another cable/port; Selphy models need a powered printer                                                              |
| Wrong print size / margins                | `lpoptions -p fotoko -l`; adjust `--media` and `printer.media`                                                                                                       |
| Jobs reported printed but nothing printed | `lpstat -W completed -o fotoko`, `/var/log/cups/error_log`; `PreserveJobHistory` must be Yes                                                                         |
| Upgrade failed                            | `deploy.sh` already restored `.prev`; manual: `sudo cp -p /usr/local/bin/fotoko-pi-agent.prev /usr/local/bin/fotoko-pi-agent && sudo systemctl restart fotoko-agent` |

## Tests

```sh
shellcheck -x deploy/pi/*.sh deploy/pi/test/*.sh
bash deploy/pi/test/install_test.sh
systemd-analyze verify deploy/pi/fotoko-agent.service   # only complains when the binary is not installed on this machine
```
