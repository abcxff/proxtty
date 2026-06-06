//! User configuration (Milestone 14).
//!
//! Loaded from `$PROXTTY_CONFIG`, else `~/.config/proxtty/config.toml`. A missing
//! or malformed file falls back to built-in defaults, so `proxtty` always runs.
//!
//! Example `config.toml`:
//!
//! ```toml
//! hotkey = "ctrl-space"
//! trigger = "any"          # any | option-click | ctrl-click
//! shell = "/bin/zsh"
//! border = true
//!
//! [[menu]]
//! label = "Copy screen → clipboard"
//! action = "copy_screen"
//!
//! [[menu]]
//! label = "Send: clear"
//! action = "send"
//! text = "clear\n"
//! ```

use std::path::PathBuf;

use serde::Deserialize;

/// Top-level configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Keyboard trigger, e.g. `ctrl-space`, `ctrl-]`, `ctrl-a`.
    pub hotkey: String,
    /// Mouse trigger modifier: `any`, `option-click`, or `ctrl-click`.
    pub trigger: String,
    /// Command to run when none is given on the command line.
    pub shell: Option<String>,
    /// Whether to draw a border around the menu.
    pub border: bool,
    /// Menu items, top to bottom.
    pub menu: Vec<MenuItemCfg>,
}

/// A configured menu entry: a label plus the action it performs.
#[derive(Debug, Clone, Deserialize)]
pub struct MenuItemCfg {
    pub label: String,
    #[serde(flatten)]
    pub action: ActionSpec,
}

/// What a menu item does when chosen.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ActionSpec {
    /// Send literal text to the child PTY (e.g. a command to run).
    Send { text: String },
    /// Copy the visible screen text to the system clipboard (via OSC 52).
    CopyScreen,
    /// Open the first URL visible on screen in the system browser.
    OpenUrl,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            hotkey: "ctrl-space".to_string(),
            trigger: "any".to_string(),
            shell: None,
            border: true,
            menu: default_menu(),
        }
    }
}

fn default_menu() -> Vec<MenuItemCfg> {
    vec![
        MenuItemCfg {
            label: "Copy screen → clipboard".to_string(),
            action: ActionSpec::CopyScreen,
        },
        MenuItemCfg {
            label: "Open first URL".to_string(),
            action: ActionSpec::OpenUrl,
        },
        MenuItemCfg {
            label: "Send: clear".to_string(),
            action: ActionSpec::Send {
                text: "clear\n".to_string(),
            },
        },
    ]
}

impl Config {
    /// The trigger byte parsed from [`Config::hotkey`], defaulting to Ctrl-Space.
    pub fn hotkey_byte(&self) -> u8 {
        parse_hotkey(&self.hotkey).unwrap_or(0x00)
    }

    /// Which mouse modifiers open the overlay.
    pub fn trigger_mods(&self) -> TriggerMods {
        match self.trigger.as_str() {
            "option-click" | "alt-click" => TriggerMods::Alt,
            "ctrl-click" => TriggerMods::Ctrl,
            _ => TriggerMods::Any,
        }
    }
}

/// Mouse-modifier policy for opening the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerMods {
    Alt,
    Ctrl,
    Any,
}

/// Load configuration, falling back to defaults on any problem.
pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Config::default(), // no file → defaults
    };
    match toml::from_str(&text) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("proxtty: ignoring {}: {e}", path.display());
            Config::default()
        }
    }
}

fn config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PROXTTY_CONFIG") {
        return Some(PathBuf::from(p));
    }
    dirs::config_dir().map(|d| d.join("proxtty").join("config.toml"))
}

/// Parse a hotkey spec like `ctrl-space`, `ctrl-]`, `ctrl-a` into its byte.
fn parse_hotkey(spec: &str) -> Option<u8> {
    let key = spec.trim().to_ascii_lowercase();
    let rest = key.strip_prefix("ctrl-").or_else(|| key.strip_prefix("c-"))?;
    match rest {
        "space" => Some(0x00),
        "]" => Some(0x1d),
        "\\" => Some(0x1c),
        "[" => Some(0x1b),
        _ => {
            // ctrl-a .. ctrl-z -> 0x01 .. 0x1a
            let bytes = rest.as_bytes();
            if bytes.len() == 1 && bytes[0].is_ascii_lowercase() {
                Some(bytes[0] - b'a' + 1)
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_parsing() {
        assert_eq!(parse_hotkey("ctrl-space"), Some(0x00));
        assert_eq!(parse_hotkey("ctrl-a"), Some(0x01));
        assert_eq!(parse_hotkey("ctrl-]"), Some(0x1d));
        assert_eq!(parse_hotkey("C-\\"), Some(0x1c));
        assert_eq!(parse_hotkey("shift-a"), None);
    }

    #[test]
    fn defaults_are_usable() {
        let c = Config::default();
        assert_eq!(c.hotkey_byte(), 0x00);
        assert_eq!(c.trigger_mods(), TriggerMods::Any);
        assert!(!c.menu.is_empty());
    }

    #[test]
    fn deserializes_full_config() {
        let toml = r#"
            hotkey = "ctrl-]"
            trigger = "option-click"
            shell = "/bin/zsh"
            border = false

            [[menu]]
            label = "Copy"
            action = "copy_screen"

            [[menu]]
            label = "Open"
            action = "open_url"

            [[menu]]
            label = "Clear"
            action = "send"
            text = "clear\n"
        "#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.hotkey_byte(), 0x1d);
        assert_eq!(c.trigger_mods(), TriggerMods::Alt);
        assert_eq!(c.shell.as_deref(), Some("/bin/zsh"));
        assert!(!c.border);
        assert_eq!(c.menu.len(), 3);
        assert!(matches!(c.menu[0].action, ActionSpec::CopyScreen));
        assert!(matches!(c.menu[2].action, ActionSpec::Send { .. }));
    }
}
