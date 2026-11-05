# Wi-Fi and kiosk hardening

## Wi-Fi (NetworkManager, Raspberry Pi OS bookworm and later)

Bookworm uses NetworkManager. Prefer the Imager's Wi-Fi settings; on a running Pi:

```sh
sudo nmcli radio wifi on
sudo nmcli device wifi list
sudo nmcli device wifi connect "SSID" password "PASSPHRASE" ifname wlan0   # saved, autoconnects
sudo nmcli connection modify "SSID" connection.autoconnect yes connection.autoconnect-retries 0
nmcli -p connection show "SSID" | grep -i autoconnect
```

Set the regulatory domain once (`sudo raspi-config nonint do_wifi_country VN`). Keep a fallback: a second saved network (`connection.autoconnect-priority` ranks them) or wired Ethernet. The agent needs only outbound TCP 8883 (MQTT) and 443 (image downloads from S3), so the firewall allows all outbound traffic.

## Clock

TLS fails with a wrong clock and a Pi has no battery RTC. `install.sh` enables `systemd-timesyncd`; the unit waits for `time-sync.target`. Check `timedatectl` for `System clock synchronized: yes`. Set `NTP=` in `/etc/systemd/timesyncd.conf` if the venue blocks public NTP.

## Logs

The agent logs to the journal. Cap it on an SD card, `/etc/systemd/journald.conf.d/fotoko.conf`:

```ini
[Journal]
SystemMaxUse=100M
MaxRetentionSec=2week
Storage=persistent
```

Then `sudo systemctl restart systemd-journald`. Read with `journalctl -u fotoko-agent -f`. CUPS logs to `/var/log/cups` (logrotate is preinstalled).

## Kiosk hardening

- ssh: key only (`PasswordAuthentication no`), allow from the operator CIDR only (`--ssh-from`).
- Services: `--disable-bluetooth` and `--disable-avahi` on `install.sh`. Also consider `sudo systemctl disable --now ModemManager triggerhappy`.
- HDMI console: no monitor is needed. Append `consoleblank=60` to `/boot/firmware/cmdline.txt` (single line) to blank it, and add `hdmi_blanking=1` / `dtoverlay=vc4-kms-v3d,nohdmi` to `/boot/firmware/config.txt` if you want the output off.
- Read-only root (optional): `sudo raspi-config nonint enable_overlayfs` makes the root filesystem an overlay in RAM, which protects the SD card from power cuts. `/var/lib/fotoko` (job ledger) then must live on a separate writable partition, and certs/config changes need the overlay disabled temporarily. Not done by `install.sh`.
- Updates: `unattended-upgrades` installs security updates; reboot windows are not scheduled, reboot the Stand at a quiet time.

## Watchdog

The agent does not implement the systemd watchdog protocol (`sd_notify`), so `WatchdogSec=` must not be set on the unit: it would kill a healthy agent. Liveness is covered by `Restart=always` (crashes) and the cloud heartbeat (`agent.heartbeat_secs`, the Stand shows offline when it stops). For a hung kernel or OS use the Pi hardware watchdog: add `RuntimeWatchdogSec=15` under `[Manager]` in `/etc/systemd/system.conf.d/watchdog.conf` (systemd then pets `/dev/watchdog`, and the board resets if systemd hangs).
