use crate::geometry::{DisplayArea, TabletArea};
use anyhow::{Context, Result, bail};
use std::process::{Child, Command};

const SETTINGS_PATH_ENV: &str = "LOCALAPPDATA";
const SETTINGS_REL_PATH: &str = r"OpenTabletDriver\settings.json";

/// Ensures the OTD daemon is running. Returns a `DaemonGuard` that will
/// stop the daemon on drop if we started it.
pub fn ensure_daemon_running() -> Result<DaemonGuard> {
    if is_daemon_running() {
        log::info!("OTD daemon already running");
        return Ok(DaemonGuard { child: None });
    }

    log::info!("Starting OTD daemon...");
    let child = Command::new("OpenTabletDriver.Daemon.exe")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Failed to start OpenTabletDriver.Daemon.exe — is it installed?")?;

    // Give the daemon time to initialize
    std::thread::sleep(std::time::Duration::from_secs(2));

    if !is_daemon_running() {
        bail!("OTD daemon failed to start");
    }

    log::info!("OTD daemon started (PID: {})", child.id());
    Ok(DaemonGuard { child: Some(child) })
}

fn is_daemon_running() -> bool {
    Command::new("OpenTabletDriver.Console.exe")
        .args(["detect"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Stops the daemon on drop if we started it.
pub struct DaemonGuard {
    child: Option<Child>,
}

impl DaemonGuard {
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(|c| c.id())
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            log::info!("Stopping OTD daemon (PID: {})...", child.id());
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct OtdBridge {
    tablet_name: String,
    original_display_area: DisplayArea,
    original_tablet_area: TabletArea,
    tablet_aspect_ratio: f64,
}

impl OtdBridge {
    /// Create a new bridge, saving original state. `rotation_degrees` is the
    /// desired tablet rotation (from the --orientation flag).
    pub fn new(tablet_name: String, rotation_degrees: f64) -> Result<Self> {
        let (display_area, tablet_area) = get_areas(&tablet_name)?;

        // Always use the tablet's native aspect ratio (width/height as OTD
        // reports). OTD's rotation parameter handles the axis swap internally —
        // the display area is always in screen coordinates.
        let tablet_aspect_ratio = tablet_area.width / tablet_area.height;

        log::info!("Tablet: {tablet_name}");
        log::info!("Original display area: {display_area:?}");
        log::info!(
            "Tablet area: {:.1}x{:.1}, rotation: {:.0}°",
            tablet_area.width,
            tablet_area.height,
            tablet_area.rotation
        );
        log::info!("Tablet aspect ratio: {tablet_aspect_ratio:.3}");

        let bridge = Self {
            tablet_name,
            original_display_area: display_area,
            original_tablet_area: tablet_area,
            tablet_aspect_ratio,
        };

        // Apply the requested rotation
        if rotation_degrees != bridge.original_tablet_area.rotation {
            log::info!("Setting tablet rotation to {rotation_degrees:.0}°");
            bridge.set_tablet_rotation(rotation_degrees)?;
        }

        Ok(bridge)
    }

    pub fn tablet_aspect_ratio(&self) -> f64 {
        self.tablet_aspect_ratio
    }

    pub fn original_display_area(&self) -> &DisplayArea {
        &self.original_display_area
    }

    pub fn set_display_area(&self, area: &DisplayArea) -> Result<()> {
        let output = Command::new("OpenTabletDriver.Console.exe")
            .args([
                "setdisplayarea",
                &self.tablet_name,
                &area.width.to_string(),
                &area.height.to_string(),
                &area.center_x.to_string(),
                &area.center_y.to_string(),
            ])
            .output()
            .context("Failed to run OpenTabletDriver.Console.exe — is the daemon running?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("setdisplayarea failed: {}", stderr.trim());
        }

        Ok(())
    }

    fn set_tablet_rotation(&self, rotation: f64) -> Result<()> {
        let ta = &self.original_tablet_area;
        let output = Command::new("OpenTabletDriver.Console.exe")
            .args([
                "settabletarea",
                &self.tablet_name,
                &ta.width.to_string(),
                &ta.height.to_string(),
                &ta.center_x.to_string(),
                &ta.center_y.to_string(),
                &rotation.to_string(),
            ])
            .output()
            .context("Failed to set tablet rotation")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("settabletarea failed: {}", stderr.trim());
        }

        Ok(())
    }

    pub fn restore_original(&self) -> Result<()> {
        log::info!("Restoring original settings");
        self.set_display_area(&self.original_display_area)?;
        if self.original_tablet_area.rotation != 0.0 {
            // Only restore rotation if we changed it
        }
        self.set_tablet_rotation(self.original_tablet_area.rotation)?;
        Ok(())
    }
}

/// Detect tablet name from OTD settings.json.
pub fn detect_tablet_name() -> Result<String> {
    let local_app_data =
        std::env::var(SETTINGS_PATH_ENV).context("LOCALAPPDATA environment variable not set")?;
    let settings_path = std::path::Path::new(&local_app_data).join(SETTINGS_REL_PATH);

    let contents = std::fs::read_to_string(&settings_path)
        .with_context(|| format!("Failed to read {}", settings_path.display()))?;

    let json: serde_json::Value =
        serde_json::from_str(&contents).context("Failed to parse OTD settings.json")?;

    json["Profiles"]
        .as_array()
        .and_then(|profiles| profiles.first())
        .and_then(|profile| profile["Tablet"].as_str())
        .map(|s| s.to_string())
        .context("No tablet found in OTD settings — is a tablet connected?")
}

/// Get both display area and tablet area from a single `getareas` call.
fn get_areas(tablet_name: &str) -> Result<(DisplayArea, TabletArea)> {
    let output = Command::new("OpenTabletDriver.Console.exe")
        .args(["getareas", tablet_name])
        .output()
        .context("Failed to run OpenTabletDriver.Console.exe — is the daemon running?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("getareas failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let display = parse_area_from_output(&stdout, "Display area:")?;
    let tablet = parse_area_from_output(&stdout, "Tablet area:")?;

    Ok((
        DisplayArea {
            width: display.width,
            height: display.height,
            center_x: display.center_x,
            center_y: display.center_y,
        },
        tablet,
    ))
}

/// Parse an area line from getareas output.
/// Format: `Display area: [5120x2160@<2560, 1080>:0°],`
fn parse_area_from_output(output: &str, prefix: &str) -> Result<TabletArea> {
    let line = output
        .lines()
        .find(|l| l.contains(prefix))
        .with_context(|| format!("No '{prefix}' in getareas output"))?;

    let open = line.find('[').context("No '[' in area line")?;
    let close = line.find(']').context("No ']' in area line")?;
    let content = &line[open + 1..close];

    let x_pos = content.find('x').context("No 'x' in area")?;
    let at_pos = content.find('@').context("No '@' in area")?;
    let lt_pos = content.find('<').context("No '<' in area")?;
    let gt_pos = content.find('>').context("No '>' in area")?;
    let colon_pos = content.rfind(':').context("No ':' for rotation")?;

    let width: f64 = content[..x_pos].parse().context("Invalid width")?;
    let height: f64 = content[x_pos + 1..at_pos]
        .parse()
        .context("Invalid height")?;

    let coords = &content[lt_pos + 1..gt_pos];
    let comma = coords.find(',').context("No ',' in coordinates")?;
    let center_x: f64 = coords[..comma].trim().parse().context("Invalid center_x")?;
    let center_y: f64 = coords[comma + 1..]
        .trim()
        .parse()
        .context("Invalid center_y")?;

    // Parse rotation: ":0°" — strip the degree symbol
    let rotation_str = content[colon_pos + 1..].trim_end_matches('°');
    let rotation: f64 = rotation_str.parse().unwrap_or(0.0);

    Ok(TabletArea {
        width,
        height,
        center_x,
        center_y,
        rotation,
    })
}
