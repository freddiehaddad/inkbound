# wacom-magnet

**Automatically map your Wacom tablet to a specific window.**

Do you like to map your tablet to a specific window? Do you find it annoying
having to keep remapping your tablet tablet whenever you move or resize the
window? If so, then **wacom-magnet** fixes this. It watches your drawing app's
window and automatically adjusts the tablet mapping to match — so the tablet
always covers exactly that window. Move the window? The mapping follows. Resize
it? The mapping adjusts. Minimize it? The tablet goes back to covering the full
screen.

## Prerequisites

wacom-magnet works with [OpenTabletDriver](https://opentabletdriver.net/) (OTD),
an open-source tablet driver that replaces the official Wacom driver. You'll
need to set it up first.

### Step 1: Remove the Wacom driver

Uninstall the official Wacom driver through Windows Settings → Apps.

### Step 2: Install OpenTabletDriver

```
winget install --id OpenTabletDriver.OpenTabletDriver
```

After installation, **restart your computer**.

### Step 3: Start the OpenTabletDriver daemon (optional)

wacom-magnet will automatically start the OTD daemon if it's not already
running, and stop it when wacom-magnet exits. If you prefer to manage the daemon
yourself, run `OpenTabletDriver.Daemon.exe` before starting wacom-magnet.

> **Tip:** To have OTD always running, add `OpenTabletDriver.Daemon.exe` to
> your Startup folder.

### Step 4: Install the VMulti driver (if needed)

Some setups require the VMulti driver for pen input to work properly. If your
pen isn't working after installing OTD, download and install VMulti from the
[OTD
wiki](https://github.com/OpenTabletDriver/OpenTabletDriver/wiki/Windows-Installation-Guide).

## Installation

Download `wacom-magnet.exe` from the
[Releases](https://github.com/freddiehaddad/wacom-magnet/releases) page, or
build from source:

```
cargo install --path .
```

## Usage

Open a terminal (Command Prompt or PowerShell) and run:

```
wacom-magnet.exe --target "krita"
```

Replace `"krita"` with the name of your drawing application. This can be:
- The **window title** (or part of it), e.g., `"krita"`, `"Photoshop"`, `"Clip
  Studio"`
- The **process name**, e.g., `"krita.exe"`, `"mspaint"`

The match is **case-insensitive** — `"krita"` will match a window titled "My
Drawing — Krita".

### What happens

1. **wacom-magnet** finds the matching window and maps your tablet to it
2. When you **move or resize** the window, the tablet mapping updates
   automatically
3. When you **minimize or close** the window, the tablet goes back to its
   original full-screen mapping
4. When the window **reappears**, the tablet locks onto it again
5. If you have **multiple windows** of the same app (e.g., two Krita windows),
   the tablet follows whichever one you click into
6. Press **Ctrl+C** in the terminal to stop wacom-magnet — your original tablet
   mapping is restored

### Options

| Option | Description |
|---|---|
| `--target <name>` | **(Required)** Window title or process name to track |
| `--rotation <degrees>` | Tablet rotation: 0, 90, 180, or 270 (default: 0). See below. |
| `--tablet <name>` | Override the tablet name (auto-detected by default) |

### Tablet rotation

If pen movements don't match your physical tablet orientation (e.g., moving the
pen right moves the cursor up), use `--rotation` to correct it. Try different
values until it feels right:

```
wacom-magnet.exe --target "krita" --rotation 90
wacom-magnet.exe --target "krita" --rotation 270
```

You only need to figure this out once — use the same value every time.

### Example

```
wacom-magnet.exe --target "clip studio"
```

### Aspect ratio

Your tablet's physical drawing area has a specific shape (aspect ratio).
wacom-magnet preserves this ratio so your strokes aren't distorted. If the
window is very wide, the tablet will use the full window height but won't
stretch to the edges horizontally — keeping your drawing natural.

## Troubleshooting

### "Failed to run OpenTabletDriver.Console.exe"

The OpenTabletDriver daemon isn't running. Start `OpenTabletDriver.Daemon.exe`
first.

### "No tablet found in OTD settings"

Your tablet isn't detected by OpenTabletDriver. Make sure it's plugged in, the
Wacom driver is fully uninstalled, and OTD is running. Open
`OpenTabletDriver.UX.Wpf.exe` to check.

### "Target window not found — waiting for it to appear..."

The application you specified isn't open yet. Open it and wacom-magnet will
detect it automatically.

### The mapping feels off or distorted

This can happen if your window is very narrow or very tall compared to your
tablet's shape. The aspect ratio preservation means the tablet won't cover the
extreme edges of oddly-shaped windows. This is intentional to prevent drawing
distortion.

## FAQ

**Q: Do I need to keep the terminal open?**
A: Yes — wacom-magnet runs in the terminal. Closing the terminal stops it and
restores your original tablet mapping.

**Q: Can I use this with the official Wacom driver?**
A: No — wacom-magnet requires OpenTabletDriver. The official Wacom driver
doesn't offer a way to change the mapping programmatically.

**Q: Will I lose pressure sensitivity or pen buttons?**
A: No — OpenTabletDriver supports pressure, tilt, and pen buttons. You can
configure them in the OTD GUI.

**Q: Does it work with multiple monitors?**
A: Yes — the mapping follows the window regardless of which monitor it's on.

**Q: What happens if I alt-tab to another app?**
A: The tablet stays mapped to the last target window. It won't change just
because you switched focus to check email or browse the web. It only updates
when a window matching your `--target` gains focus.

## License

[MIT](LICENSE)
