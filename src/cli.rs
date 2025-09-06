//! CLI argument processing and conversion utilities.
//!
//! This module handles conversion of CLI arguments to internal types,
//! reducing duplication and clone operations in main().

use crate::gui::SelectorType;
use crate::winevent::Target;
use clap::{Parser, ValueEnum};

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum SelectorKind {
    Process,
    Class,
    Title,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum AspectMode {
    Stretch,
    Letterbox,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Parser, Debug)]
#[command(
    version,
    long_about = concat!(
        "InkBound – map your tablet area to a single window.\n\n",
        "Quick examples:\n",
        "  inkbound                      # GUI only (no initial target)\n",
        "  inkbound krita.exe            # Match process (default)\n",
        "  inkbound Blender --by title   # Title contains 'Blender'\n",
        "  inkbound chrome.exe --aspect stretch\n",
        "  inkbound photoshop.exe --log debug\n\n",
        "Omit TARGET to launch GUI idle. Use --log trace for deep diagnostics.\n"
    )
)]
pub struct Cli {
    /// Optional target string (process name, class name, or title substring). Omit for GUI idle.
    pub target: Option<String>,
    /// How to interpret TARGET (default process).
    #[arg(long = "by", value_enum, default_value_t = SelectorKind::Process)]
    pub by: SelectorKind,
    /// Aspect mode: letterbox (crop / preserve) or stretch (fill window).
    #[arg(long = "aspect", value_enum, default_value_t = AspectMode::Letterbox)]
    pub aspect: AspectMode,
    /// Log verbosity level (default info).
    #[arg(long = "log", value_enum, default_value_t = LogLevel::Info)]
    pub log: LogLevel,
}

/// CLI configuration distilled into the internal selector representation.
///
/// This is produced by `cli_to_selector_config` and feeds GUI initialization for the
/// selector textbox & radio buttons while supplying an optional immediate target.
pub struct SelectorConfig {
    pub selector_type: SelectorType,
    pub selector_value: String,
    pub target: Option<Target>,
}

/// Convert CLI arguments to selector configuration.
///
/// This eliminates repetitive pattern matching and clone operations from `main` and
/// centralizes the decision logic that chooses which mutually‑exclusive selector the
/// user intended (process, class, or title substring). If none specified we default to
/// `Process` with an empty value so the GUI can be used interactively.
pub fn cli_to_selector_config(cli: &Cli) -> SelectorConfig {
    match &cli.target {
        Some(raw) => {
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                return SelectorConfig {
                    selector_type: SelectorType::Process,
                    selector_value: String::new(),
                    target: None,
                };
            }
            match cli.by {
                SelectorKind::Process => SelectorConfig {
                    selector_type: SelectorType::Process,
                    selector_value: trimmed.clone(),
                    target: Some(Target::ProcessName(trimmed)),
                },
                SelectorKind::Class => SelectorConfig {
                    selector_type: SelectorType::WindowClass,
                    selector_value: trimmed.clone(),
                    target: Some(Target::WindowClass(trimmed)),
                },
                SelectorKind::Title => SelectorConfig {
                    selector_type: SelectorType::Title,
                    selector_value: trimmed.clone(),
                    target: Some(Target::TitleSubstring(trimmed)),
                },
            }
        }
        None => SelectorConfig {
            selector_type: SelectorType::Process,
            selector_value: String::new(),
            target: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_selector_conversion() {
        let cli = Cli {
            target: Some("notepad.exe".into()),
            by: super::SelectorKind::Process,
            aspect: super::AspectMode::Letterbox,
            log: super::LogLevel::Info,
        };
        let config = cli_to_selector_config(&cli);
        assert_eq!(config.selector_type, SelectorType::Process);
        assert_eq!(config.selector_value, "notepad.exe");
        assert!(matches!(config.target, Some(Target::ProcessName(s)) if s == "notepad.exe"));
    }

    #[test]
    fn window_class_selector_conversion() {
        let cli = Cli {
            target: Some("Notepad".into()),
            by: super::SelectorKind::Class,
            aspect: super::AspectMode::Letterbox,
            log: super::LogLevel::Info,
        };
        let config = cli_to_selector_config(&cli);
        assert_eq!(config.selector_type, SelectorType::WindowClass);
        assert_eq!(config.selector_value, "Notepad");
        assert!(matches!(config.target, Some(Target::WindowClass(s)) if s == "Notepad"));
    }

    #[test]
    fn title_selector_conversion() {
        let cli = Cli {
            target: Some("Document".into()),
            by: super::SelectorKind::Title,
            aspect: super::AspectMode::Letterbox,
            log: super::LogLevel::Info,
        };
        let config = cli_to_selector_config(&cli);
        assert_eq!(config.selector_type, SelectorType::Title);
        assert_eq!(config.selector_value, "Document");
        assert!(matches!(config.target, Some(Target::TitleSubstring(s)) if s == "Document"));
    }

    #[test]
    fn no_selector_defaults_to_process() {
        let cli = Cli {
            target: None,
            by: super::SelectorKind::Process,
            aspect: super::AspectMode::Letterbox,
            log: super::LogLevel::Info,
        };
        let config = cli_to_selector_config(&cli);
        assert_eq!(config.selector_type, SelectorType::Process);
        assert_eq!(config.selector_value, "");
        assert!(config.target.is_none());
    }
}
