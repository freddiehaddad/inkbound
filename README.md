# InkBound

> Event‑driven WinTab tablet *window mapper* for Windows, written in Rust.

InkBound dynamically remaps your Wacom tablet's active area to a single application window
— selected by process name, window class, or title substring — and follows it instantly as it
moves, resizes, gains focus, minimizes, or is destroyed. No busy loops, no timers: just WinEvent
hooks and WinTab context updates.

## TL;DR (Quick Start)

```powershell
# Clone and build (MSVC toolchain required)
git clone https://github.com/freddiehaddad/inkbound.git
cd inkbound
cargo build --release

# Run with GUI only (choose target later)
./target/release/inkbound.exe

# Or immediately map to Photoshop keeping aspect
./target/release/inkbound.exe --process photoshop.exe --preserve-aspect
```

## Why Another Mapper?

Typical workflows either:

* Map the tablet to the whole desktop (wasted precision for a single app),
* Constantly visit the Wacom control panel, or
* Use polling tools that miss fast resizes or add latency.

InkBound fuses lightweight OS notifications (WinEvent) with direct WinTab context geometry
updates for immediate remapping.

## Features

* Zero polling: WinEvent hooks (move, size, foreground, minimize, destroy, create, show).
* Target selection by process, window class, or title substring (CLI or GUI textbox).
* Launch with **no arguments** → type selector → press Start.
* Change selector: edit text, Stop → Start to apply (live update roadmap item).
* Aspect ratio preservation (crops INPUT extents; always fills window pixels—no letterbox gaps).
* Automatic context reopen on foreground to mitigate driver resets.
* Tray icon (Green = active+present, Yellow = waiting/stopped; Red only on explicit error).
* Single small GUI: radios (selector type), editable textbox, Keep Aspect checkbox, Start/Stop.
* Clean shutdown via window close, tray Exit, or Ctrl+C.
* Verbose logging (`-v` debug, `-vv` trace) & quiet mode (`-q`).
* Tested fallback for finicky driver option bit combinations.

## Requirements

| Component | Requirement |
|-----------|-------------|
| OS        | Windows 10 / 11 (x64) |
| Rust      | Stable toolchain with MSVC (`rustup default stable-x86_64-pc-windows-msvc`) |
| Build Tools | Visual Studio Build Tools / Desktop C++ (for MSVC linker) |
| Tablet Driver | Official **Wacom** driver (WinTab API exposed) – install before running |
| Hardware  | Wacom tablet (other WinTab devices may work, untested) |

### Install / Verify Toolchain

```powershell
# Install Rust (if missing)
winget install Rustlang.Rustup -e  # or download from https://rustup.rs

# Ensure MSVC host
rustup toolchain install stable-x86_64-pc-windows-msvc
rustup default stable-x86_64-pc-windows-msvc

# Confirm
rustc -V
cargo -V
```

### Wacom Driver

Download / update from: <https://www.wacom.com/support/product-support/drivers>

InkBound relies on the WinTab (wintab32.dll) interface the driver provides. If the driver is
missing or incompatible you will see early errors opening the context.

## Building

```powershell
git clone https://github.com/freddiehaddad/inkbound.git
cd inkbound
cargo build --release
```

Binary path: `target\release\inkbound.exe`

Optionally install to Cargo bin dir:

```powershell
cargo install --path .
inkbound.exe --help
```

## Usage Overview

You may specify a selector up front OR launch idle and choose in the GUI.

```text
inkbound --process photoshop.exe [options]
inkbound --win-class Chrome_WidgetWin_1 [options]
inkbound --title-contains Blender [options]
inkbound  # no selector => GUI idle
```

### Key Flags

| Flag | Meaning |
|------|---------|
| `--process <NAME>` | Match process executable (case‑insensitive, include `.exe`). |
| `--win-class <CLASS>` | Match exact top‑level window class. |
| `--title-contains <SUBSTR>` | Match if window title contains substring (case‑sensitive). |
| `--preserve-aspect` | Crop tablet input to preserve window aspect (no distortion). |
| `-v / -vv` | Increase verbosity (debug / trace). |
| `-q` | Quiet (warnings + errors only). |

### Examples

```powershell
# Idle GUI, pick later
inkbound

# Krita with aspect preserved
inkbound --process krita.exe --preserve-aspect

# Chrome window by class (no aspect crop)
inkbound --win-class Chrome_WidgetWin_1

# Any window with "Blender" in title, trace logs
inkbound --title-contains Blender -vv

# Quiet mapping to SAI
inkbound --process sai.exe -q
```

### GUI Interaction

1. Choose selector type via radio buttons (Process / Class / Title).
2. Enter selector text (e.g. `photoshop.exe`).
3. (Optional) Check *Keep tablet aspect* to crop input extents.
4. Press *Start*.
5. Change target later: edit text → Stop → Start.

Tray menu: Right‑click → Restore / Start|Stop / Exit. Double‑click icon to restore.

Colors:

* Green – run enabled & target window currently exists.
* Yellow – waiting / stopped / target missing.
* Red – explicit error path (failed context reopen / mapping failure).

## Tablet Driver Configuration

InkBound only adjusts WinTab **context geometry**; it does *not* rotate, flip, or calibrate hardware.

Recommended driver settings (Wacom Desktop Center / Settings):

1. Orientation: Set the physical orientation you use (landscape / portrait). InkBound assumes it.
2. Screen Area: "All Displays" or the unified desktop. (Let InkBound carve a sub‑region virtually.)
3. Tablet Area: Full tablet. (Cropping is done logically when aspect is preserved.)
4. Disable any driver feature that auto‑remaps to the foreground app (to avoid conflicts).
5. If you rotate the tablet later, change it in the driver UI then restart InkBound.

## Logging & Diagnostics

Defaults to INFO. Override:

```powershell
# Debug
inkbound --process photoshop.exe -v

# Trace
inkbound --process photoshop.exe -vv

# Environment filter
$env:RUST_LOG = "inkbound=debug"; inkbound --process photoshop.exe

# Dump applied LOGCONTEXT state each update
$env:WINTAB_DUMP = "1"; inkbound --process photoshop.exe -v
```

Capture a trace log:

```powershell
inkbound --process photoshop.exe -vv 2>&1 | Tee-Object -FilePath inkbound-trace.txt
```

## Architecture (Short Form)

1. Acquire default LOGCONTEXT via `WTInfoA`.
2. Open context with optimistic option flags (fallback list if driver rejects).
3. Install WinEvent hooks (create/show/destroy/location/foreground/minimize transitions).
4. Each relevant event recomputes geometry; aspect ON => build template & reopen; aspect OFF => apply in place.
5. Foreground switches trigger a defensive reopen (driver quirk mitigation).

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Never turns green | Selector mismatch | Use `-vv`, verify exact process (incl. `.exe`) / class / title. |
| Area shrinks unpredictably | Competing driver foreground mapping | Disable driver auto‑app mapping features. |
| Distorted / stretched mapping | Aspect not preserved | Enable `--preserve-aspect` / checkbox. |
| Cursor offset | Driver mapped to partial display | Set driver mapping to all displays; restart. |
| Stops after alt‑tabbing | Driver reset | Reopen heuristic already applied; update driver; file issue with logs. |
| Changed selector does nothing | Not re‑started | Press Stop then Start to reapply hooks. |

## Limitations

* Windows only.
* Pen pressure / tilt untouched (pass through).
* No rotation logic; rely on driver orientation.
* Selector edits require a restart toggle (live update planned).
* Partial hook install tolerated (logged at trace level).

## Contributing

PRs and issues welcome. Please include:

* Tablet model & driver version
* Windows version (build number)
* Whether aspect mode was on
* Trace log (`-vv`) and, if geometry related, `WINTAB_DUMP=1`

## License

MIT – see [LICENSE](./LICENSE).

---

Made with Rust, a few Win32 calls, and an aversion to polling. 🙂
