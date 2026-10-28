# DiaScan

Native Linux scanning software for **Reflecta DigitDia** magazine slide scanners
(DigitDia 4000 tested; 5000/6000 and other PIE scanners with USB ID `05e3:0142`…`014b`
use the same protocol) — a replacement for CyberView X5, written in Rust with a Qt 6/QML UI.

The scanner protocol was reverse-engineered from CyberView's `DD6000.dll` and USB captures;
see [docs/PROTOCOL.md](docs/PROTOCOL.md).

## Features

- Preview (prescan) and final scans at 300–3600 dpi, 8 or 16 bit per channel
- **Automatic magazine scanning**: scans the slide in the scanner and the following ones,
  advancing the magazine; skips empty slots and stops after three in a row
- **Infrared dust and scratch removal** (Digital ICE-style, using the scanner's IR channel)
- Automatic exposure per slide (preview-based, like CyberView), auto crop to the image area
- Automatic noise and streak removal for dark slides (normal slides untouched)
- Film types: slides, colour negatives (orange mask removal), B&W positive/negative
- Auto levels, auto colour (restores faded slides), exposure, brightness, contrast,
  saturation, cyan/red, magenta/green, yellow/blue balance, levels and gamma,
  sharpening, grain reduction, rotate and flip
- Magazine navigation: advance, back, go to slide, reload magazine
- Output: TIFF 48-bit, TIFF 24-bit, PNG 48-bit, JPEG, and JPEG XL (lossy, 16- or 8-bit;
  about 40 % smaller than JPEG at the same quality setting)
- The output folder is chosen with the desktop's native file dialog (XDG portal)

## Install (Flatpak)

```sh
flatpak install --user diascan.flatpak          # needs the KDE 6.11 runtime from Flathub
flatpak run io.github.diascan.DiaScan
```

**USB permission.** Normal users cannot access the scanner until a udev rule is installed.
If DiaScan finds the scanner but cannot open it, it offers to install the rule
(`/etc/udev/rules.d/60-diascan.rules`): click *Install access rule…* and enter your
password in the system's polkit dialog. DiaScan reloads udev and reconnects. From the
Flatpak this runs `pkexec` on the host through `flatpak-spawn --host` (hence the
`org.freedesktop.Flatpak` permission). If that is not possible (no polkit agent, dialog
cancelled), the dialog shows equivalent commands to paste into a terminal. The rule file is
also included in `dist/` and inside the Flatpak at `/app/share/diascan/60-diascan.rules`.

## Using it

1. Insert a magazine, switch the scanner on and start DiaScan; it connects automatically
   (the lamp needs about a minute to warm up after power-on).
2. **Prescan** shows the slide at the current magazine position. Adjust film type,
   dust removal and colour; the preview updates live.
3. **Scan** saves the current slide; **Scan magazine…** scans the given number of slides.
4. Files are written to the output folder as `<name>_<magazine position>.<ext>`.

If the scanner reports a *slide transport fault*, check the magazine and slide lift and
switch the scanner off and on.

After about 15 minutes without scanning the scanner switches its lamp off. DiaScan notices
this before the next scan, switches the lamp on and lets it warm up (about a minute) before
loading the slide, as CyberView does.

**Dark (dense) slides** reach the noise floor of the scanner's line sensor, which shows as
coloured noise and line streaks (CyberView has the same problem). DiaScan detects such
slides automatically and removes the sensor's line streaks and noise; normal slides are left
untouched. This can be switched off under *Image adjustment*.

Logs (every command sent to the scanner) are written to `~/.cache/diascan/diascan.log`
(`$XDG_CACHE_HOME` inside the Flatpak).

## Building

```sh
cargo build --release                      # needs Qt 6 (qmake6), libusb and libjxl
flatpak-builder --repo=repo build packaging/io.github.diascan.DiaScan.yml
flatpak build-bundle repo diascan.flatpak io.github.diascan.DiaScan
```

Crates: `crates/pie-scanner` (USB driver, usable on its own; `cargo run --example cli -p pie-scanner -- scan 600`),
`crates/imaging` (dust removal, film, colour, export), `app` (Qt UI).

License: GPL-2.0-or-later.
