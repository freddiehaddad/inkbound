//! CLI argument processing and conversion utilities.
//!
//! This module handles conversion of CLI arguments to internal types,
//! reducing duplication and clone operations in main().

use crate::gui::SelectorType;
use crate::winevent::Target;

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
pub fn cli_to_selector_config(
    process: &Option<String>,
    win_class: &Option<String>,
    title_contains: &Option<String>,
) -> SelectorConfig {
    if let Some(p) = process {
        let value = p.clone();
        SelectorConfig {
            selector_type: SelectorType::Process,
            selector_value: value.clone(),
            target: Some(Target::ProcessName(value)),
        }
    } else if let Some(c) = win_class {
        let value = c.clone();
        SelectorConfig {
            selector_type: SelectorType::WindowClass,
            selector_value: value.clone(),
            target: Some(Target::WindowClass(value)),
        }
    } else if let Some(t) = title_contains {
        let value = t.clone();
        SelectorConfig {
            selector_type: SelectorType::Title,
            selector_value: value.clone(),
            target: Some(Target::TitleSubstring(value)),
        }
    } else {
        SelectorConfig {
            selector_type: SelectorType::Process,
            selector_value: String::new(),
            target: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_selector_conversion() {
        let process = Some("notepad.exe".to_string());
        let config = cli_to_selector_config(&process, &None, &None);

        assert_eq!(config.selector_type, SelectorType::Process);
        assert_eq!(config.selector_value, "notepad.exe");
        assert!(matches!(config.target, Some(Target::ProcessName(s)) if s == "notepad.exe"));
    }

    #[test]
    fn window_class_selector_conversion() {
        let win_class = Some("Notepad".to_string());
        let config = cli_to_selector_config(&None, &win_class, &None);

        assert_eq!(config.selector_type, SelectorType::WindowClass);
        assert_eq!(config.selector_value, "Notepad");
        assert!(matches!(config.target, Some(Target::WindowClass(s)) if s == "Notepad"));
    }

    #[test]
    fn title_selector_conversion() {
        let title = Some("Document".to_string());
        let config = cli_to_selector_config(&None, &None, &title);

        assert_eq!(config.selector_type, SelectorType::Title);
        assert_eq!(config.selector_value, "Document");
        assert!(matches!(config.target, Some(Target::TitleSubstring(s)) if s == "Document"));
    }

    #[test]
    fn no_selector_defaults_to_process() {
        let config = cli_to_selector_config(&None, &None, &None);

        assert_eq!(config.selector_type, SelectorType::Process);
        assert_eq!(config.selector_value, "");
        assert!(config.target.is_none());
    }
}
