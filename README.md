# PenTarget

Event‑driven (no polling) Wacom/WinTab tablet mapper for Windows. PenTarget dynamically
re-maps your tablet to the exact bounds of a chosen window (by process name, window class,
or title substring). When that window moves, resizes, or regains foreground focus the
mapping is updated instantly via WinEvent hooks. Optionally the tablet reverts to the full
area whenever another window is foreground.

## Why?

Traditional workflows either:

- Map the tablet to the whole desktop (wasting active area for a single app), or
- Manually change mappings in the Wacom control panel, or
- Use polling utilities that introduce latency or miss rapid size changes.

PenTarget listens to window events directly from the OS and applies new WinTab contexts
immediately—no busy loops, timers, or artificial delays.

## Features

- Target selection by process executable, window class, or title substring (exactly one required).
- Optional aspect ratio preservation (letter/pillar boxing) or full stretch to window.
- Automatic context reopen on foreground changes (improves reliability with some drivers).
- Optional full-tablet mapping while target is unfocused (`--full-when-unfocused`).
- Clean shutdown with Ctrl+C.
- Verbose / quiet logging controls (`-v`, `-vv`, `-q`).
- Unit-tested core logic (mapping + context fallback).

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

Exactly one selector flag is required.

```text
pentarget --process photoshop.exe [options]
pentarget --win-class Chrome_WidgetWin_1 [options]
pentarget --title-contains Sketch [options]
```

### Key Flags

- `--process <NAME>`: Match by process executable (case-insensitive).
- `--win-class <CLASS>`: Match by exact top-level window class name.
- `--title-contains <SUBSTR>`: Match if window title contains substring (case-sensitive).
- `--preserve-aspect` (alias `--keep-aspect`): Maintain tablet aspect; center inside window.
- `--full-when-unfocused`: While another window is foreground, temporarily revert to full tablet.
- `-v / --verbose`: Increase log verbosity (once = debug, twice = trace).
- `-q / --quiet`: Only warnings and errors.

### Examples

Preserve aspect mapping for Photoshop:

```powershell
pentarget --process photoshop.exe --preserve-aspect
```

Map to a Chrome window by class, stretch to fill (ignore aspect):

```powershell
pentarget --win-class Chrome_WidgetWin_1
```

Map to a window whose title contains "Blender" and revert to full tablet when unfocused:

```powershell
pentarget --title-contains Blender --full-when-unfocused
```

Ultra-verbose tracing for debugging:

```powershell
pentarget --process krita.exe -vv
```

Quiet mode (only problems):

```powershell
pentarget --process sai.exe -q
```

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

- `WTInfoA` fetches the default LOGCONTEXT template.
- We attempt `WTOpenA` with a prioritized option sequence (messages + system cursor integration).
- SetWinEvent hooks (location change, foreground, create, destroy, minimize) drive recalculations.
- On foreground regain we close & reopen the context (some drivers reset mapping otherwise).
- Mapping math optionally preserves aspect and centers the output extents.

## Troubleshooting

| Symptom | Potential Cause | Resolution |
|---------|-----------------|------------|
| No mapping change | Wrong selector / window not matched | Use `-vv` to see window events; verify process/class/title. |
| Intermittent loss after alt-tabbing | Driver resets context | Automatic reopen should mitigate; ensure latest driver. |
| Cursor offset / mismatch | Tablet not mapped to full desktop in driver | Set Wacom mapping to all displays and retry. |
| App ignores pen input | Driver rejected preferred flags | Check logs for fallback option; ensure `WTOpen succeeded`. |
| High CPU usage | Unexpected; event storm | Verify no other tool is toggling contexts. |

Enable trace logs and capture a run:

```powershell
pentarget --process photoshop.exe -vv 2>&1 | Tee-Object -FilePath pentarget-trace.txt
```

## Limitations / Notes

- Windows only (Win32 + WinTab).
- Only a subset of context flags exercised; pressure / tilt packets unaffected.
- No rotation / inversion switches (set those in the Wacom driver).
- Hook installation failures are logged but not fatal (partial coverage possible).

## Contributing

Issues and pull requests welcome. Please include driver version, tablet model, and a trace log
(`-vv` with `WINTAB_DUMP=1`) when reporting mapping anomalies.

## License

MIT – see [LICENSE](./LICENSE).
