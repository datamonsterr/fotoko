# Hardware: building a Stand

One Stand = one Raspberry Pi + one USB photo printer + a QR sticker, on a venue's Wi-Fi. The software side (certificate, install, verification) is the runbook in [`deploy/pi/README.md`](../../deploy/pi/README.md); this guide covers what to buy, flashing, assembly and operation. Terms: [`CONTEXT.md`](../../CONTEXT.md).

## Bill of materials

| Item                                                                                                | Notes                                                                                                                                                                                                                     |
| --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Raspberry Pi 4 or 5, **2 GB or more**                                                               | 64-bit (aarch64) is required; the agent is cross-built for `aarch64-unknown-linux-gnu`. A Pi 3 B+ or Zero 2 W also runs 64-bit but is not tested. Wired Ethernet is more reliable than Wi-Fi when the venue has a socket. |
| Official power supply                                                                               | Pi 4: 5.1 V / 3 A USB-C. Pi 5: 5.1 V / 5 A (27 W) so it does not throttle USB. The printer has its own power brick; never power it from the Pi.                                                                           |
| microSD card, 32 GB, A1/A2, from a known brand (endurance or "high endurance" if you can)           | Keep the card sealed in a case slot; cards are the usual failure after power cuts (see overlay filesystem in `deploy/pi/wifi-and-hardening.md`). Keep one pre-flashed spare per venue.                                    |
| Case with passive or fan cooling                                                                    | Pi 5 wants the active cooler. Pick a case whose SD slot is reachable.                                                                                                                                                     |
| **Gutenprint-supported dye-sublimation or photo printer**                                           | See below. The runbook is tested against the Canon SELPHY CP1300 model string.                                                                                                                                            |
| USB cable (printer end is usually USB-B or mini/micro-B)                                            | Short, good quality, direct to a Pi USB port (not through an unpowered hub).                                                                                                                                              |
| **4x6 in media + ribbon/ink set** for that printer                                                  | Dye-sub kits bundle paper and ribbon in matching counts (for example 108 sheets); buy the kit made for the model, they are not interchangeable.                                                                           |
| Optional: UPS / power strip with surge protection, label printer, a lockable enclosure, spare cable | Power cuts are the main cause of corrupted cards and half-printed jobs.                                                                                                                                                   |

### Choosing and checking a printer

Fotoko prints through CUPS with the Gutenprint driver (`printer-driver-gutenprint`), 4x6 in media `w288h432`. Check support **before buying**:

1. Look the model up in Gutenprint's supported printers list (gutenprint.sourceforge.net, "Supported printers"). Compact dye-sub families with Gutenprint drivers include Canon SELPHY (CP series), Mitsubishi CP-D series, DNP DS series, Kodak/Sony/HiTi photo printers. "Supported" means a driver exists; reports of USB quirks on specific firmware do occur.
2. With a Pi (or any Debian machine) available, list the drivers the installed package knows: `sudo apt install cups printer-driver-gutenprint`, then `lpinfo -m | grep -i gutenprint | grep -i <model>`. A line such as `gutenprint.5.3://canon-selphy-cp1300/expert Canon SELPHY CP1300 - CUPS+Gutenprint ...` is the string you pass as `--model` to `deploy/pi/deploy.sh`/`install.sh`.
3. With the printer attached and powered: `lsusb` shows the vendor, `lpinfo -v` shows `usb://...`, and after install `lpoptions -p fotoko -l` must list `w288h432` (4x6 in) as a `PageSize`. If the media name differs, pass `--media` and set the same in `agent.toml` (`printer.media`), see runbook step 4.
4. Prefer models that report paper/ribbon status over USB: Fotoko turns a stopped queue into `paper_out`/`ribbon_out` failures, which only works when CUPS knows. Test by removing the paper and sending a job.

## Flash Raspberry Pi OS

1. On a computer, install **Raspberry Pi Imager** (raspberrypi.com/software).
2. Choose the device, OS **Raspberry Pi OS Lite (64-bit)** (Raspberry Pi OS (other) -> Lite 64-bit; bookworm or trixie both work), and the SD card.
3. In the customisation step (gear / "Edit settings") set:
   - hostname: the printer id, for example `stand-001` (reachable as `stand-001.local` on the LAN if avahi stays on; `install.sh --disable-avahi` turns it off, then use the IP);
   - username and a strong password (not `pi`/`raspberry`);
   - **Wi-Fi** SSID, passphrase and country `VN` (set the venue network; add Ethernet if available);
   - locale `Asia/Ho_Chi_Minh` and keyboard;
   - Services: enable SSH, **allow public-key authentication only**, paste your public key (`~/.ssh/id_ed25519.pub`).
4. Write, eject, put the card in the Pi, connect the printer by USB and power both on. First boot takes a minute or two.
5. Find the Pi (`ping stand-001.local`, the venue router's client list, or `arp -a`) and log in: `ssh <user>@<ip>`. Check `lsusb` shows the printer, `timedatectl` says `System clock synchronized: yes` and `sudo apt update && sudo apt full-upgrade` once.

Detail for headless Wi-Fi with nmcli, clock, journald limits and hardening: [`deploy/pi/wifi-and-hardening.md`](../../deploy/pi/wifi-and-hardening.md).

## Install the software

Follow [`deploy/pi/README.md`](../../deploy/pi/README.md) and do not duplicate it here:

1. Seed the printer item in DynamoDB and provision the certificate: [aws.md](aws.md) steps 9 and 10 (`scripts/dev/seed-printer.sh`, `provision-printer`).
2. Build the agent (`deploy/pi/build.sh`), find the driver (`lpinfo -m`), install with `deploy/pi/deploy.sh <ip> --certs ... -- --model ...`, verify (runbook steps 2 to 6): the service is `active`, the log shows `connected to broker`, the CUPS test page prints.
3. Smoke test with a real order ([aws.md](aws.md) step 11).

## Stand assembly and QR sticker

The QR code encodes the customer URL **`https://<WebUrl>/p/<printerId>`** (CDK output `WebUrl`, the printer id from the seeded item, for example `https://d1abcd.cloudfront.net/p/stand-001`). The app also accepts `fotoko://p/<printerId>`; use the https form for stickers because any phone camera opens it in the browser without the app. A custom domain later invalidates printed stickers ([services.md](services.md)).

```sh
WEB_URL=https://d1abcd.cloudfront.net PRINTER_ID=stand-001
# qrencode (apt install qrencode / brew install qrencode): PNG, 12 px modules, 2-module quiet zone, error correction M
qrencode -t PNG -s 12 -m 2 -l M -o "$PRINTER_ID.png" "$WEB_URL/p/$PRINTER_ID"
# or SVG for a print shop:
qrencode -t SVG -m 2 -l M -o "$PRINTER_ID.svg" "$WEB_URL/p/$PRINTER_ID"
# without qrencode, via Node:
pnpm dlx qrcode -o "$PRINTER_ID.png" -w 600 --margin 2 "$WEB_URL/p/$PRINTER_ID"
```

Print at least 4 x 4 cm (bigger for a counter two metres away), black on white, matte lamination or a hard plastic sleeve (glare breaks scanning). Add the Stand name and a line of text with the short URL for people who cannot scan. **Test every sticker with two phones** and in `https://<WebUrl>/p/<printerId>` before leaving the venue.

Assembly checklist:

- Printer on a flat, dry, stable surface with room to eject paper; paper tray facing the customer's side only if staff can refill it; keep vents free.
- Pi next to the printer, case closed, SD slot covered; cable strain relief; power through a surge-protected strip.
- Sticker at eye level on the counter or a stand, plus the Stand name. No customer-reachable USB ports or power switch.
- Stick a label inside the case with printer id, hostname, ssh user and the operator contact.

## Physical security

- Customers can only reach the sticker. Mount the Pi and printer so the USB ports, SD card and power plug are not accessible (lockable box or staff-only shelf); an SD card swap defeats the device identity.
- The device certificate lives on the SD card (`/etc/fotoko/private.pem.key`, mode 0400). If a Pi or card is stolen or lost, **revoke it**: IoT Core console -> Certificates -> the printer's certificate -> Deactivate (then delete), and provision a new one with `provision-printer ... --rotate`. The device policy limits a certificate to its own printer's topics, so a stolen key cannot reach other Stands or AWS resources.
- ssh is key-only and firewalled to the operator network by `install.sh` (`--ssh-from`). Keep console access plan B (a keyboard and monitor at the venue).
- Photos are never stored on the Pi beyond the job (see the runbook); S3 deletes uploads after 7 days.
- Do not reuse one Wi-Fi password across venues; the Pi needs outbound TCP 8883 (MQTT TLS) and 443 only.

## Maintenance

- **Paper or ribbon out**: the printer raises an error, CUPS stops the queue (`printer-error-policy=stop-printer`), the agent fails the order with `paper_out`/`ribbon_out` and the Stand shows an error in the app. Refill paper and ribbon, close the cover, then re-enable the queue on the Pi:

  ```sh
  ssh <user>@<ip>
  lpstat -l -p fotoko        # reason for the stop
  sudo cupsenable fotoko     # resume after fixing the cause
  ```

  The failed job is **not** reprinted; the customer's order is failed (refund by hand in payOS if they paid, see [services.md](services.md)). Print a CUPS test page afterwards: `sudo -u fotoko lp -d fotoko /usr/share/cups/data/testprint`.

- **Routine**: log in monthly: `systemctl is-active fotoko-agent`, `journalctl -u fotoko-agent -n 50 --no-pager`, `df -h` and `timedatectl`. Upgrade the agent with `bash deploy/pi/build.sh && bash deploy/pi/deploy.sh <ip>` (automatic rollback on a failed health check). Run `sudo apt full-upgrade` at quiet hours; reboot afterwards.
- **Replace a Pi or card**: flash a spare, then `deploy.sh --certs` with the existing `printer-out/<id>` (or rotate the certificate if you cannot trust the old card).
- Dye-sub heads and rollers: follow the printer manual (dust cover on, no touching the paper path).

## Offline behaviour

- **Venue network down**: the Pi cannot reach IoT Core. The MQTT last will marks the printer `offline` within the keep-alive window (30 s default), so the app refuses new orders for that Stand (`printer must be online or busy`). A job published while the Pi is away is queued by IoT Core for its persistent MQTT session (the agent connects with `clean_session = false`, QoS 1), but only for AWS's limited retention (about an hour by default, **verify** in the IoT Core quotas), and the photo URLs inside a job expire after 15 minutes, so a late job fails with `download_failed`; the order is then failed and, if the customer paid, refunded by hand. Short outages recover by themselves; long ones are an operator task.
- **Pi reboots mid-print**: unfinished jobs found at start are reported `failed {driver_error}` and can be retried by the cloud republishing the job (`docs/contracts.md`, Pi behaviour notes); a paid order that failed is a refund case for the operator.
- **Power cut**: same as reboot, plus the card risk; use the UPS and consider the overlay filesystem option in `deploy/pi/wifi-and-hardening.md`.
- **Customer phone offline** after paying: the order still completes; they can re-open the order link when back online.
- **Clock wrong** (no network at boot, no RTC): TLS fails until NTP syncs; the agent keeps retrying. Check `timedatectl` first when a Stand will not connect.
