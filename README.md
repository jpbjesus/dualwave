<p align="center">
  <img src="packaging/appimage/dualwave.svg" width="76" alt="DualWave icon">
</p>

<h1 align="center">DualWave</h1>
<p align="center">A native Linux companion for your DualSense.</p>
<p align="center">
  Bluetooth headset audio · Microphone · Adaptive triggers · Controller profiles
</p>
<p align="center">
  <a href="#getting-started">Getting started</a> ·
  <a href="#inside-the-app">Screenshots</a> ·
  <a href="#features-and-validation">Features</a> ·
  <a href="#building">Build</a> ·
  <a href="https://github.com/jpbjesus/dualwave/issues">Report an issue</a>
</p>

![DualWave controller workspace](.github/screenshots/controller.png)

DualWave brings the DualSense's controls and Bluetooth audio into one native
Linux application. Configure lighting, trigger effects, button mappings and
motion, switch profiles for games, and use headphones and a microphone through
the controller's 3.5 mm jack.

It is an independent fork of [MaximumFreak/DSZ](https://github.com/MaximumFreak/dsz).
The original controller protocol and profile format are retained; Linux backends
provide HID access, PipeWire audio, keyboard/mouse input, a desktop tray and
virtual controllers.

**Development preview.** CachyOS is the primary integration target. Bluetooth
stereo playback, headset recording and the physical Mute button were confirmed
on a standard DualSense with firmware 6.30 using the earlier CLI/kernel filter.
The complete GUI integration, current media-isolation driver and virtual
controller paths still need broader hardware validation. See the
[validation table](#features-and-validation) before relying on a particular path.

## Getting started

### AppImage on CachyOS

There is no published release yet. The AppImage can be built from this repository
using the [instructions below](#building); release artifacts will appear in
[Releases](https://github.com/jpbjesus/dualwave/releases).

1. Make `DualWave-x86_64.AppImage` executable and open it.
2. Accept the first-run system configuration if requested. One administrator
   authorization installs the bundled DKMS/device support and missing kernel
   headers. The desktop needs `kdialog` and `polkit` for this setup.
3. Reboot once after system configuration, reconnect the controller over
   Bluetooth, and open DualWave again.
4. Plug in the headset **before starting playback**, open **Audio**, and choose
   **DualWave Headphones** as the output in your desktop or application's sound
   settings.
5. To record, explicitly enable the microphone in DualWave and select
   **DualWave Microphone** in the recording application.

Everyday use runs as your normal user. System configuration is separate from
launching the application; DKMS handles rebuilds when the kernel changes.
Automatic first-run configuration currently targets CachyOS.

### Microphone controls

The microphone control combines its icon, state and recording level:

- Click the microphone to enable capture, or to mute/unmute it when enabled.
- Use its power button to disable capture independently of the saved mute state.
- The level indicator follows actual samples only while a recording application
  is using the microphone.

Recording is disabled by default. Enabling it allows capture; it does not start
recording by itself. DualWave does not save microphone recordings during normal
operation or diagnostic probes. The physical Mute button can control recording
when enabled in the profile.

## Inside the app

The main navigation groups related tasks into **Controller**, **Audio**,
**Profiles** and **Preferences**. Complete configuration pages are accessible
through visible tabs, without an Advanced menu. Controller opens directly in
Lighting; triggers, motion, mappings and virtual output sit alongside it. Preview
retains the controller drawing and quick controls; Device status contains live
information. Audio separates Playback & microphone from Haptics & audio. Each
work area remembers its last view for the current application session.

| Controller and lighting | Audio and microphone |
| --- | --- |
| ![DualWave lighting controls](.github/screenshots/lighting.png) | ![DualWave audio and microphone controls](.github/screenshots/audio.png) |
| Lighting, triggers, mappings, motion and virtual output. | Headphone playback, microphone source, gain and mute. |

| Profiles and games | Preferences |
| --- | --- |
| ![DualWave profiles](.github/screenshots/profiles.png) | ![DualWave appearance preferences](.github/screenshots/preferences.png) |
| Separate profile management and automatic game rules. | Appearance, startup, device settings, integrations and backups. |

These are captures of the native Linux UI, rendered with isolated example
profiles and no controller connected. They show the application itself, rather
than a web prototype. The interface supports English, Português (Portugal) and
Português (Brasil), light/dark/system appearance and reduced motion.

## Features and validation

“Implemented” describes the code available in this fork; it does not imply every
controller, Bluetooth adapter or game has been tested.

| Feature | Available controls | Validation |
| --- | --- | --- |
| Controller workspace | Native controller preview, contextual configuration, quick volume/microphone/output controls and actual device status | Native rendering and layout tests |
| Lighting | Static, rainbow, breathing, strobe, battery/audio response, player LEDs and mute LED | Original core retained; integrated Linux hardware checks pending |
| Adaptive triggers | Visual presets and full resistance, weapon, vibration and per-zone editors for L2/R2 | Original protocol retained; integrated hardware checks pending |
| Bluetooth headphone audio | Stereo through the jack, speaker/headset routing, hardware volume and PipeWire playback endpoint | Earlier CLI/filter confirmed on standard DualSense, firmware 6.30; current GUI/isolation path needs validation |
| Bluetooth microphone | Built-in/headset/automatic source, gain, mute, physical Mute button and PipeWire recording endpoint | Earlier CLI/filter confirmed; current integrated path needs validation |
| Haptics | Game audio, system audio, button clicks, trigger texture and rumble conversion | Original encoder/core retained; Linux integration needs hardware checks |
| Mappings and motion | Keys, mouse, controller buttons, combinations, long press, double tap, gyro, touchpad and stick modes | Native uinput backend implemented; hardware validation pending |
| Virtual controllers | Xbox 360, DualSense and DualShock 4 over native Linux USB/IP, with scoped privileged broker | Host attachment and game compatibility validation pending |
| Exclusive physical input | Scoped evdev grabs and kernel raw-state redirection; device nodes remain visible | Routing/build tests; hardware validation pending |
| Profiles and games | Create, rename, duplicate, import/export, backups and automatic per-game rules, including Wine/Proton process discovery | Native implementation and layout tests |
| Integrations | Local UDP mod API and DSU motion server for supported emulators | Implemented; game/emulator integration checks pending |
| Desktop integration | StatusNotifier tray, XDG autostart, bundled fonts, themes and motion preferences | Native implementation; desktop-dependent tray behavior |
| System support | Source-only DKMS package, device permissions and AppImage first-run setup | Package/build checks; first-install/reboot flow needs end-to-end validation |

### Compatibility and current limits

- **Primary audio target:** standard DualSense over Bluetooth (`054c:0ce6`).
  Do not assume Bluetooth audio support for Edge, DualShock 4 or clone devices
  from their controller-input support alone.
- Headphones must be connected before playback starts. Headset microphone
  selection after reinsertion, reconnect behavior and simultaneous Steam/game
  use need further validation.
- USB audio uses the controller's existing USB audio device, outside this
  Bluetooth bridge.
- AppImages are built against the build host's glibc/graphics stack. Compatibility
  with older distributions is not claimed. Other PipeWire Linux systems can
  build from source, but automatic system configuration is CachyOS-specific.
- Windows Game Bar and Windows default-audio-role management have no Linux
  counterpart. DualWave preserves PipeWire session policy.

## Building

On CachyOS/Arch, install the development dependencies and build as a normal user:

```sh
sudo pacman -S --needed base-devel rust pipewire libpipewire opus
cargo build --release --locked -p dsz -p dualwave
```

- `target/release/dsz` — native GUI.
- `target/release/dualwave` — audio diagnostic CLI.

Build an AppImage with an official
[appimagetool](https://github.com/AppImage/appimagetool) executable:

```sh
python3 packaging/appimage/build.py --appimagetool /path/to/appimagetool
```

Output: `target/appimage/DualWave-x86_64.AppImage`, with a SHA-256 manifest and
bundled source-only `dualwave-hid-dkms` package. Useful AppImage commands:

```sh
./DualWave-x86_64.AppImage --check       # Verify the bundled driver payload
./DualWave-x86_64.AppImage --plan        # Inspect setup without installing
./DualWave-x86_64.AppImage --cli list    # Read-only controller discovery
```

### Settings and profiles

Normal Linux settings live under `$XDG_CONFIG_HOME/dualwave`, or
`~/.config/dualwave` when `XDG_CONFIG_HOME` is unset. Profiles are in its
`profiles` subdirectory. **Preferences → Backup & restore** exports or restores
all profiles and application settings together; a restore first saves the
previous configuration.

Selecting a profile to edit does not silently apply it to a connected controller.
Changes to the edited profile are saved automatically. Profile names and saved
identifiers remain independent of the interface language.

### Diagnostics and system support

<details>
<summary>Audio CLI and controller isolation</summary>

```sh
cargo run -p dualwave -- list
cargo run -p dualwave -- probe
cargo run -p dualwave -- tone --seconds 10 --volume 60
cargo run -p dualwave -- mic-probe --source headset --seconds 10
cargo run -p dualwave -- run --source auto --volume 60
```

`list` reads sysfs; `probe` reads status and a CRC-checked firmware feature report.
`pipewire-probe --seconds 10` exposes endpoints without opening or writing to a
controller. `tone`, `mic-probe` and `run` explicitly start their corresponding
sessions. Source choices are `auto`, `builtin` and `headset`; `--seconds N` bounds
a session. Ctrl+C/SIGTERM stops streaming and disables microphone admission.

The current driver exposes a receive-only **DualWave Microphone Transport** HID
child. It validates the Bluetooth CRC, keeps state reports on the gamepad and
routes microphone reports to that child before HID core publishes them to raw
controller readers. The child identity and ancestry are checked before use.

Without the child, legacy audio mode requires Steam to be closed and temporarily
grabs the controller's evdev devices. It is unsuitable for gameplay during audio.
The first tested state-parser filter alone does not isolate microphone packets
from raw-HID game clients. Close other controller tools before audio probes.

No controller firmware is flashed, and the driver does not start an audio stream
on its own. Source provenance and the patch are under `packaging/kernel/`.
`dualwave_audio` defaults to off in the module; the DKMS package opts in. Its
read-only `dualwave_mic_muted` attribute preserves physical-mute synchronization.
Long-running clock drift, adapter compatibility and forced-crash cleanup still
need production validation; reconnect after forcibly terminating a session.

</details>

<details>
<summary>DKMS packaging, device access and removal</summary>

```sh
python3 packaging/build-kernel-package.py
```

The source-only package is staged in `target/packages/dualwave-hid-dkms/` and
includes scoped udev rules, a `dualwave` group for uinput/USB-IP broker access,
and systemd socket units. The AppImage's setup installs missing matching headers,
runs package hooks and enables the broker for the next boot. It does not replace
a loaded controller module during an active session.

The sources have compiled against `7.2.9-2-cachyos` and
`6.18.55-2-cachyos-lts`; future kernel compatibility still depends on a successful
DKMS build and matching headers. The USB/IP broker tracks local ownership and
releases imported virtual ports when clients exit. After a broker restart,
restart the application's virtual controller rather than adopting a busy port.

The manual development udev rule is `packaging/70-dualwave.rules`. Reconnect after
installing/reloading it. Do not run DualWave as root or add the user to the global
`input` group; access is scoped to the active local seat and supported device.

To return to the stock controller module:

```sh
sudo pacman -R dualwave-hid-dkms
sudo mkinitcpio -P
```

Reboot afterwards. A manually installed
`/etc/udev/rules.d/70-dualwave.rules` is separate from the packaged rule under
`/usr/lib/udev/rules.d/`; remove that manual copy too if undoing device access.

</details>

## Development and verification

```sh
cargo test --workspace --locked
cargo clippy -p dualwave --all-targets --locked -- -D warnings
python3 -m unittest discover -s packaging/tests -v
cc -std=c11 -Wall -Wextra -Werror packaging/kernel/test-routing.c -o /tmp/dualwave-routing-test
/tmp/dualwave-routing-test
```

Linux CI also runs the concurrent PipeWire ring-buffer tests with address and
undefined-behavior sanitizers. Upstream Windows/MSVC integrations require their
own checks; a Linux build does not validate them.

Debug builds support isolated, controller-free UI captures:

```sh
DSZ_DATA_DIR=/tmp/dualwave-preview \
DSZ_SNAPSHOT=/tmp/dualwave-controller.bmp \
DSZ_SNAPSHOT_SIZE=1240x800 \
  target/debug/dsz --page workspace
```

`DSZ_SNAPSHOT_PART` chooses `lighting`, `triggers`, `buttons` or `motion`.
`DSZ_SNAPSHOT_SECTION` selects a Preferences/Profile subpage, and
`DSZ_SNAPSHOT_BOTTOM=1` captures the end of a long editor. These switches are
for debug snapshots only; they do not open controllers or start the audio engine.

| Path | Responsibility |
| --- | --- |
| `crates/ds-proto` | Controller protocol, CRCs, reports and trigger effects |
| `crates/dsz` | Native application, profiles, device workers and Linux backends |
| `crates/dsz/src/ui` | Native egui views, navigation, widgets and theme |
| `crates/dualwave` | Standalone PipeWire/audio diagnostics |
| `packaging` | AppImage, DKMS sources, permissions, broker and system setup |
| `vendor` | Patched eframe and bundled libopus sources |
| `.github/screenshots` | Reproducible captures of the native UI |

## Credits and licenses

DualWave preserves DSZ's original history and MIT notice. Thanks to
[MaximumFreak](https://github.com/MaximumFreak/dsz),
[OpenController / Brunovncs](https://github.com/Brunovncs/OpenController), the
DS4Windows contributors and the controller-research community.

| Component | License |
| --- | --- |
| DualWave userspace and original DSZ | [MIT](LICENSE) |
| Patched Linux driver/helper | [GPL-2.0-or-later](packaging/kernel/COPYING) |
| Instrument Sans interface fonts | [SIL OFL 1.1](crates/dsz/assets/InstrumentSans-OFL.txt), [original project](https://github.com/Instrument/instrument-sans/tree/7fa22308a3d0c94ee2b3cd537a1196b65db34a3e) |
| Manrope “WAVE” signature | [SIL OFL 1.1](crates/dsz/assets/Manrope-OFL.txt) |
| OpenController drawings | [MIT](crates/dsz/assets/OpenController-LICENSE) |
| libopus | [BSD-3-Clause](vendor/opus/COPYING) |
| eframe | MIT or Apache-2.0; see `vendor/eframe` |

The interface bundles Instrument Sans Regular, Medium and SemiBold unchanged
from upstream revision `7fa22308a3d0c94ee2b3cd537a1196b65db34a3e`; the existing Manrope
ExtraBold signature is retained. Font licenses are included in the AppImage.

DualSense, DualSense Edge, DualShock and PlayStation are trademarks of Sony
Interactive Entertainment. DualWave is not affiliated with Sony. For Windows
instructions and upstream-specific measurements, consult
[DSZ](https://github.com/MaximumFreak/dsz).

## Protocol sources

The Linux port also follows the kernel's public
[vhci host ABI](https://github.com/torvalds/linux/blob/master/drivers/usb/usbip/vhci_sysfs.c),
[USB/IP host implementation](https://github.com/torvalds/linux/blob/master/tools/usb/usbip/libsrc/vhci_driver.c)
and [PipeWire stream API](https://docs.pipewire.org/group__pw__stream.html).


DSZ's protocol code was written from public documentation and code with open licenses:

| Source | License | Used for |
|---|---|---|
| Linux [`hid-playstation`](https://github.com/torvalds/linux/blob/master/drivers/hid/hid-playstation.c) | GPL-2.0, read as documentation | Input and output report layouts, valid flags, status bits, CRC prefixes, feature reports, the DualShock 4 report, speaker and headset routing (volume, output path, preamp gain) and headset detection |
| SDL [`SDL_hidapi_ps5.c`](https://github.com/libsdl-org/SDL/blob/main/src/joystick/hidapi/SDL_hidapi_ps5.c) | zlib | Edge Fn and paddle buttons, the connection-state byte, improved rumble emulation |
| [pydualsense](https://github.com/flok/pydualsense) | MIT | The Bluetooth `0x31` layout, player LED patterns and brightness, motor power, custom trigger modes |
| [SAxense](https://apps.sdore.me/SAxense) and [godot-dualsense-native](https://github.com/Lelisvaldo/godot-dualsense-native/blob/main/docs/PROTOCOL.md) | MPL-2.0, MIT | The Bluetooth haptics report `0x32`, the packet header's double-length bit, and Opus packet `0x16` for the headset jack (godot) |
| [Nielk1's PS5 Bluetooth capture](https://github.com/egormanga/SAxense/issues/1) (`packet_samples.txt`) | Captured console traffic, used as data | The speaker report `0x39`: config packet, two Opus frames (packet `0x15`), two haptic blocks, and the frame counter |
| [RFC 6716](https://www.rfc-editor.org/rfc/rfc6716) and libopus's documentation | IETF, BSD-3-Clause | Reading the capture's Opus TOC byte (CELT, fullband, 10 ms, stereo) and the encoder settings |
| Monado [`pssense_protocol.h`](https://monado.pages.freedesktop.org/monado/pssense__protocol_8h_source.html) | Boost | The motor power reduction nibbles |
| [dualsensectl](https://github.com/nowrep/dualsensectl) | GPL-2.0, read as documentation | Rumble power as an attenuation of 0–7 |
| [Nielk1's trigger effect generator](https://gist.github.com/Nielk1/6d54cc2c00d2201ccb8c2720ad7538db) | MIT | Adaptive trigger effect encoding |
| [DS4Windows](https://github.com/Ryochan7/DS4Windows) | GPL-3.0, read as documentation | The stick curve settings: dead zone, max zone, sensitivity curve, anti-dead zone, square stick, max output; rumble power in 12.5 % steps |
| Santroller's [Xbox 360 controller notes](https://santroller.github.io/guitar-configurator/controller-reverse-engineering/xbox-360-controllers.html) | GPL-3.0, read as documentation | The layout of the virtual Xbox 360 controller's capability answers (buttons, triggers, sticks, rumble motors) |
| [cemuhook protocol reference](https://v1993.github.io/cemuhook-protocol/) | public documentation | The DSU motion server: packets, messages, registration, the data layout |
| [DS4Windows](https://github.com/Ryochan7/DS4Windows) `UdpServer.cs`/`DS4Sixaxis.cs` and [evdevhook2](https://github.com/v1993/evdevhook2) `DeviceTypes.ini` | GPL-3.0, read as documentation | DSU motion axis signs and units for DualSense and DualShock 4 |
| [dogtopus's descriptor dump](https://gist.github.com/dogtopus/894da226d73afb3bdd195df41b3a26aa) and [nondebug/dualsense](https://github.com/nondebug/dualsense) | public dumps | The virtual DualSense's HID report descriptor and USB layout |
| [stealth-alex's DualSense output report notes](https://gist.github.com/stealth-alex/10a8e7cc6027b78fa18a7f48a0d3d1e4) | public notes | Microphone source selection (audio flags bits 0–1); the microphone volume and mute fields match Linux `hid-playstation` |
