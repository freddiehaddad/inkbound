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

# Immediately map to Photoshop (process match; letterbox is default)
./target/release/inkbound.exe photoshop.exe

# Match a window title substring (stretch mapping)
./target/release/inkbound.exe Blender --by title --aspect stretch

# Debug logging
./target/release/inkbound.exe krita.exe --log debug
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
* Target selection by process, window class, or title substring (positional TARGET + --by, or GUI textbox + radios).
* Launch with **no arguments** → type selector → press Start.
* Change selector: edit text, Stop → Start to apply (live update roadmap item).
* Two aspect modes:
  * letterbox (default): preserve target window aspect by cropping tablet input region.
  * stretch: fill target window (may distort if aspect differs).
* Automatic context reopen on foreground to mitigate driver resets.
* Tray icon (Green = active+present, Yellow = waiting/stopped; Red only on explicit error).
* Single small GUI: selector type radios, editable textbox, aspect radios (Letterbox / Stretch), Start/Stop.
* Clean shutdown via window close, tray Exit, or Ctrl+C.
* Unified logging flag: `--log error|warn|info|debug|trace` (default info).
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

General form (all flags optional):

```text
inkbound [TARGET] [--by process|class|title] [--aspect letterbox|stretch] [--log error|warn|info|debug|trace]
```

* Omit `TARGET` to launch GUI idle.
* Default `--by` is `process` (treat TARGET as an executable name, e.g. `krita.exe`).
* Default `--aspect` is `letterbox`.
* Default `--log` is `info`.

### Flags

| Arg / Flag | Meaning |
|------------|---------|
| `TARGET` | Optional selector string (process name, class name, or title substring). |
| `--by <kind>` | Interpret TARGET as `process`, `class`, or `title` (substring). |
| `--aspect <mode>` | `letterbox` (preserve / crop) or `stretch` (fill). |
| `--log <level>` | Verbosity: `error` `warn` `info` `debug` `trace`. |

### Examples

```powershell
# Idle GUI, pick later
inkbound

# Krita (process match, default letterbox)
inkbound krita.exe

# Chrome window class (stretch mapping)
inkbound Chrome_WidgetWin_1 --by class --aspect stretch

# Any window with "Blender" in title, trace logs
inkbound Blender --by title --log trace

# Photoshop with debug logs
inkbound photoshop.exe --log debug
```

### GUI Interaction

1. Choose selector type via radio buttons (Process / Class / Title).
2. Enter selector text (e.g. `photoshop.exe`).
3. Pick aspect mode via radios: Letterbox (preserve) or Stretch (fill).
4. Press *Start*.
5. Change target later: edit text → Stop → Start (live switching planned).

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

Default level: info.

```powershell
# Debug detail
inkbound photoshop.exe --log debug

# Full trace (includes event + mapping detail)
inkbound photoshop.exe --log trace

# Override via environment (standard tracing subscriber semantics)
$env:RUST_LOG = "inkbound=debug"; inkbound photoshop.exe

# Capture a trace log to file (PowerShell)
inkbound photoshop.exe --log trace 2>&1 | Tee-Object -FilePath inkbound-trace.txt
```

## Architecture (Short Form)

1. Acquire default LOGCONTEXT via `WTInfoA`.
2. Open context with optimistic option flags (fallback list if driver rejects).
3. Install WinEvent hooks (create/show/destroy/location/foreground/minimize transitions).
4. Each relevant event recomputes geometry; letterbox => crop & possibly reopen; stretch => direct apply.
5. Foreground switches trigger a defensive reopen (driver quirk mitigation).

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Never turns green | Selector mismatch | Use `--log trace`, verify exact process (incl. `.exe`) / class / title. |
| Area shrinks unpredictably | Competing driver foreground mapping | Disable driver auto‑app mapping features. |
| Distorted mapping | Using stretch when preservation desired | Switch aspect to letterbox (GUI radio / `--aspect letterbox`). |
| Cursor offset | Driver mapped to partial display | Set driver mapping to all displays; restart. |
| Stops after alt‑tabbing | Driver reset | Heuristic reopen already applied; update driver; file issue with trace log. |
| Changed selector does nothing | Not re‑started | Press Stop then Start (live update planned). |

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
* Aspect mode (letterbox or stretch)
* Trace log (`--log trace`)

## License

MIT – see [LICENSE](./LICENSE).

---

Made with Rust, a few Win32 calls, and an aversion to polling. 🙂
