# PenTarget

Event‑driven (no polling) Wacom/WinTab tablet mapper for Windows. PenTarget dynamically
maps your tablet active area to the bounds of a chosen application window (by process name,
window class, or title substring). When that window moves, resizes, or regains foreground
focus the mapping is updated instantly via WinEvent hooks. A small GUI & tray icon now ship
with the binary so you can run it without specifying a selector up front.

## Why?

Traditional workflows either:

- Map the tablet to the whole desktop (wasting active area for a single app), or
- Manually change mappings in the Wacom control panel, or
- Use polling utilities that introduce latency or miss rapid size changes.

PenTarget listens to window events directly from the OS and applies new WinTab contexts
immediately—no busy loops, timers, or artificial delays.

## Features

- Target selection by process executable, window class, or title substring (CLI or typed into GUI).
- Selector argument is OPTIONAL: launch with no args, type your target, press Start.
- Change the selector at runtime: edit the text and toggle Stop → Start to switch targets.
- Aspect ratio preservation (crop tablet area to match window aspect; no letterboxing dead zones).
- Instant updates on move / resize / foreground / minimize / destroy (WinEvent hooks, no polling).
- Automatic context reopen on foreground changes (works around drivers resetting mapping).
- Tray icon with status coloring (Green = active & target present, Yellow = waiting / stopped).
- Visible GUI with: editable selector textbox, Keep Aspect checkbox, Start/Stop button.
- Clean shutdown: close window, Exit from tray menu, or Ctrl+C in the launching console.
- Verbose / quiet logging controls (`-v`, `-vv`, `-q`).
- Mapping + context fallback logic tested.

## Installation (Build From Source)

Requires Rust (stable) on Windows with the MSVC toolchain.

```powershell
# Clone
git clone https://github.com/freddiehaddad/pentarget.git pentarget
cd pentarget

# Build release
cargo build --release

# Binary will be at
./target/release/pentarget.exe
```

## Usage

You can (a) supply a selector on the CLI OR (b) omit it and choose later in the GUI.

CLI forms (optional now):

```text
pentarget --process photoshop.exe [options]
pentarget --win-class Chrome_WidgetWin_1 [options]
pentarget --title-contains Blender [options]
pentarget                 # no selector -> GUI opens in idle state
```

### Key Flags / Options

- `--process <NAME>`: Match by process executable (case-insensitive).
- `--win-class <CLASS>`: Match by exact top-level window class name.
- `--title-contains <SUBSTR>`: Match if window title contains substring (case-sensitive).
- `--preserve-aspect` (alias `--keep-aspect`): Crop tablet input to keep window aspect (prevents distortion; entire window remains reachable).
- `-v / --verbose`: Increase log verbosity (once = debug, twice = trace).
- `-q / --quiet`: Only warnings and errors.

Removed flag: `--full-when-unfocused` (behaviour replaced by consistent target mapping; may return later behind GUI control).

### Examples

Start with GUI only, then type a target later:

```powershell
pentarget
```

Start targeting Photoshop, keep aspect:

```powershell
pentarget --process photoshop.exe --preserve-aspect
```

Map to a Chrome window by class, stretch to fill:

```powershell
pentarget --win-class Chrome_WidgetWin_1
```

Map to any window whose title contains "Blender":

```powershell
pentarget --title-contains Blender
```

Trace-level diagnostics:

```powershell
pentarget --process krita.exe -vv
```

Quiet mode:

```powershell
pentarget --process sai.exe -q
```

### Entering / Changing the Selector in the GUI

Type one of the following patterns in the Target box, then (re)press Start:

- `process: photoshop.exe`
- `proc: krita.exe`
- `class: Chrome_WidgetWin_1`
- `title: Blender`
- Or just free text (treated as title substring)

Edits take effect when you toggle Stop → Start (this re-applies hooks and mapping).

## Tray & GUI Behaviour

Tray icon colors:

- Green: Mapping enabled AND target currently exists.
- Yellow: Waiting for target / mapping disabled / target disappeared.

Tray menu: Right-click → Restore / Start|Stop / Exit. Double‑click tray icon to show window.

Window controls:

- Target textbox: Editable anytime (press Start to apply changes).
- Keep tablet aspect: When checked, the tablet area is cropped to match window aspect to avoid distortion.
- Start/Stop: Enables/disables dynamic mapping (Stop returns tablet to previous full-tablet context extents).

## Logging & Diagnostics

Logging uses `tracing`:

- Default level: INFO (unless `RUST_LOG` is set or `-v/-q` overrides).
- `-v` => DEBUG, `-vv` => TRACE.
- `-q` overrides all and sets WARN.

To force a custom filter:

```powershell
$env:RUST_LOG = "pentarget=debug"; pentarget --process photoshop.exe
```

To inspect applied LOGCONTEXT values set:

```powershell
$env:WINTAB_DUMP = "1"
pentarget --process photoshop.exe -v
```

## Wacom Tablet Settings Guidance

PenTarget adjusts only the WinTab context extents/origins; it does *not* rotate or invert the
hardware. Ensure these control panel settings are consistent:

1. Orientation: Set your desired tablet orientation (e.g., standard landscape) **in the Wacom
   Settings**. PenTarget assumes that orientation and does not perform rotation.
2. Mapping: Leave the tablet mapped to the **full display area** (do not confine to a single
   monitor in the driver) for best accuracy. PenTarget constrains virtually.
3. Screen Area: Prefer "All Displays" or the unified desktop—PenTarget will create a window-
   scoped logical output region inside that space.
4. Pen Buttons / Pressure: Unaffected; PenTarget only changes coordinate scaling.
5. Disable any vendor features that continuously remap the tablet to a focused app if they cause
   interference (avoid competing context changes).

If you physically rotate the tablet (e.g., portrait), change it in Wacom Settings first; restart
PenTarget to pick up the new input aspect ratio for accurate letterboxing with `--preserve-aspect`.

## How It Works (Brief)

1. `WTInfoA` fetches a baseline LOGCONTEXT.
2. Context opened with preferred option set (falls back gracefully if driver rejects flags).
3. WinEvent hooks (create / show / destroy / location change / foreground / minimize) fire; each relevant event recomputes a LOGCONTEXT geometry.
4. Aspect ON: build a new geometry template and reopen context (driver-friendly way to apply cropping consistently).
5. Aspect OFF: modify extents in-place via a lightweight update call.
6. Foreground transitions explicitly reopen to counter driver resets.

## Troubleshooting

| Symptom | Likely Cause | Resolution |
|---------|--------------|-----------|
| Target never turns green | Selector mismatch | Use `-vv`; verify process/class/title text. Try exact process name incl. `.exe`. |
| Mapping stops after some alt-tabs | Driver reset context | Reopen already attempted; update driver; leave GUI running. |
| Tablet area distorted | Aspect not preserved | Enable Keep tablet aspect (cropping). |
| Cursor offset | Driver not set to full desktop mapping | Set Wacom mapping to all displays; restart PenTarget. |
| High CPU | Unexpected event storm | Use `-vv` to inspect; ensure no other remap utilities active. |
| Changing selector has no effect | Not toggled after edit | Press Stop then Start to apply new selector. |

Enable trace logs and capture a run:

```powershell
pentarget --process photoshop.exe -vv 2>&1 | Tee-Object -FilePath pentarget-trace.txt
```

## Limitations / Notes

- Windows only (Win32 + WinTab APIs).
- Pressure / tilt packets pass through untouched.
- Orientation / rotation handled only by the tablet driver.
- Partial hook installation (rare) is logged but not fatal.
- Selector changes apply on next Start (could become live in future).

## Contributing

Issues and pull requests welcome. Please include driver version, tablet model, and a trace log
(`-vv` with `WINTAB_DUMP=1`) when reporting mapping anomalies.

## License

MIT – see [LICENSE](./LICENSE).
