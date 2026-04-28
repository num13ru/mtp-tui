use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

const DEFAULT_TEMPLATE: &str = "\
# mtp-tui configuration
# https://github.com/num13ru/mtp-tui

# Host pane opens here instead of the current working directory.
# Supports ~ for home directory.
# default_host_dir = \"~/Downloads\"

# Navigate to this device folder after connecting (default: root).
# default_device_dir = \"/Download\"
";

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub default_host_dir: Option<String>,
    pub default_device_dir: Option<String>,
}

impl Config {
    pub fn load() -> Self {
        Self::try_load().unwrap_or_default()
    }

    fn try_load() -> Option<Self> {
        let path = config_path()?;
        if !path.exists() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&path, DEFAULT_TEMPLATE);
            return Some(Self::default());
        }
        let contents = fs::read_to_string(&path).ok()?;
        toml::from_str(&contents).ok()
    }

    pub fn host_dir(&self) -> Option<PathBuf> {
        let path = self.host_dir_expanded()?;
        if path.is_absolute() && path.is_dir() {
            Some(path)
        } else {
            None
        }
    }

    /// Returns the tilde-expanded path without checking existence.
    /// `None` only when `default_host_dir` is unset.
    pub fn host_dir_expanded(&self) -> Option<PathBuf> {
        let raw = self.default_host_dir.as_deref()?;
        Some(expand_tilde(raw))
    }

    pub fn device_dir(&self) -> Option<&str> {
        self.default_device_dir.as_deref()
    }
}

fn config_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return Some(p.join("mtp-tui").join("config.toml"));
        }
    }
    Some(
        home_dir()?
            .join(".config")
            .join("mtp-tui")
            .join("config.toml"),
    )
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    if path == "~"
        && let Some(home) = home_dir()
    {
        return home;
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_home() {
        let expanded = expand_tilde("~/Documents");
        let home = home_dir().unwrap();
        assert_eq!(expanded, home.join("Documents"));
    }

    #[test]
    fn expand_tilde_bare() {
        let expanded = expand_tilde("~");
        let home = home_dir().unwrap();
        assert_eq!(expanded, home);
    }

    #[test]
    fn expand_tilde_no_prefix() {
        let expanded = expand_tilde("/absolute/path");
        assert_eq!(expanded, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn expand_tilde_mid_path_untouched() {
        let expanded = expand_tilde("/foo/~bar");
        assert_eq!(expanded, PathBuf::from("/foo/~bar"));
    }

    #[test]
    fn parse_empty_toml() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.default_host_dir.is_none());
        assert!(config.default_device_dir.is_none());
    }

    #[test]
    fn parse_with_values() {
        let config: Config = toml::from_str(
            r#"
            default_host_dir = "~/Downloads"
            default_device_dir = "/Download"
            "#,
        )
        .unwrap();
        assert_eq!(config.default_host_dir.as_deref(), Some("~/Downloads"));
        assert_eq!(config.default_device_dir.as_deref(), Some("/Download"));
    }

    #[test]
    fn parse_ignores_unknown_keys() {
        let config: Config = toml::from_str(
            r#"
            default_host_dir = "~/Downloads"
            unknown_key = "value"
            "#,
        )
        .unwrap();
        assert_eq!(config.default_host_dir.as_deref(), Some("~/Downloads"));
    }

    #[test]
    fn default_template_is_valid_toml() {
        let config: Config = toml::from_str(DEFAULT_TEMPLATE).unwrap();
        assert!(config.default_host_dir.is_none());
        assert!(config.default_device_dir.is_none());
    }

    #[test]
    fn host_dir_expanded_returns_none_when_unset() {
        let config = Config::default();
        assert!(config.host_dir_expanded().is_none());
    }

    #[test]
    fn host_dir_expanded_returns_path_even_if_missing() {
        let config = Config {
            default_host_dir: Some("~/nonexistent_dir_abc123".into()),
            default_device_dir: None,
        };
        let expanded = config.host_dir_expanded().unwrap();
        assert!(expanded.is_absolute());
        assert!(expanded.ends_with("nonexistent_dir_abc123"));
        assert!(config.host_dir().is_none(), "host_dir() should be None for missing dir");
    }

    #[test]
    fn host_dir_expanded_relative_path() {
        let config = Config {
            default_host_dir: Some("relative/path".into()),
            default_device_dir: None,
        };
        let expanded = config.host_dir_expanded().unwrap();
        assert!(!expanded.is_absolute());
        assert!(config.host_dir().is_none(), "host_dir() should be None for relative path");
    }
}
