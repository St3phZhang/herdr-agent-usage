//! Theme-aware colors for the plugin-owned sidebar rows.
//!
//! Herdr renders its own UI from the theme in `~/.config/herdr/config.toml`.
//! A plugin cannot ask the client for the resolved palette, and sidebar token
//! styles only accept literal hex colors. This module reproduces Herdr's
//! built-in palettes, follows `theme.auto_switch` for the host appearance, and
//! applies `theme.custom` overrides so the brand/status colors move with the
//! rest of the sidebar instead of staying tuned for one dark theme.

use crate::cli::{SidebarLayout, ThemeMode};
use std::fs;
use std::process::Command;

/// The host terminal appearance Herdr uses for `theme.auto_switch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

impl Appearance {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// Every color the plugin writes into the sidebar, resolved for one theme.
///
/// Idle text is the theme's `text`; working/done/blocked mirror Herdr's
/// yellow/teal/red status palette. `safe`/`warning`/`danger` are the quota
/// severity colors, and the `gauge_*` variants are muted for meter rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePalette {
    pub idle: String,
    pub working: String,
    pub done: String,
    pub blocked: String,
    pub safe: String,
    pub warning: String,
    pub danger: String,
    pub gauge_safe: String,
    pub gauge_warning: String,
    pub gauge_danger: String,
}

impl ThemePalette {
    /// The default Herdr dark theme, also the fallback when no config exists.
    pub fn catppuccin() -> Self {
        Self::from_colors(
            ResolvedColors::from_builtin(builtin_colors("catppuccin", Appearance::Dark)),
            Appearance::Dark,
        )
    }

    /// The default Herdr light theme.
    pub fn catppuccin_latte() -> Self {
        Self::from_colors(
            ResolvedColors::from_builtin(builtin_colors("catppuccin-latte", Appearance::Light)),
            Appearance::Light,
        )
    }

    /// Severity colors for a layout. Gauges use the muted family so a bar does
    /// not read as an alarm; packed/stacked use the full-strength family.
    pub fn severity(&self, layout: SidebarLayout) -> [&str; 3] {
        match layout {
            SidebarLayout::Gauges => [
                self.gauge_safe.as_str(),
                self.gauge_warning.as_str(),
                self.gauge_danger.as_str(),
            ],
            SidebarLayout::Packed | SidebarLayout::Stacked => [
                self.safe.as_str(),
                self.warning.as_str(),
                self.danger.as_str(),
            ],
        }
    }

    fn from_colors(colors: ResolvedColors, appearance: Appearance) -> Self {
        Self {
            idle: colors.text.clone(),
            working: colors.yellow.clone(),
            done: colors.teal,
            blocked: colors.red.clone(),
            safe: colors.green.clone(),
            warning: colors.yellow.clone(),
            danger: colors.red.clone(),
            gauge_safe: muted(&colors.green, appearance),
            gauge_warning: muted(&colors.yellow, appearance),
            gauge_danger: muted(&colors.red, appearance),
        }
    }
}

/// Resolve the palette Herdr is currently drawing.
pub fn resolve(mode: ThemeMode) -> ThemePalette {
    let config = read_herdr_config();
    resolve_with(&config, mode, detect_appearance())
}

/// Resolve from an explicit config string. Used by tests and by callers that
/// already read the config before rewriting it.
pub fn resolve_with(config: &str, mode: ThemeMode, detected: Appearance) -> ThemePalette {
    let settings = parse_theme_settings(config);
    let appearance = match mode {
        ThemeMode::Auto if settings.auto_switch => detected,
        ThemeMode::Auto => infer_appearance(&settings.name).unwrap_or(detected),
        ThemeMode::Dark => Appearance::Dark,
        ThemeMode::Light => Appearance::Light,
    };
    let theme_name = select_theme_name(&settings, mode, appearance);
    let mut colors = ResolvedColors::from_builtin(builtin_colors(&theme_name, appearance));
    settings.custom.apply(&mut colors);
    // Herdr applies mode overrides only when auto_switch is on. An explicit
    // `--theme dark|light` is a plugin-level override, so it should honour
    // the matching subtable even when Herdr itself is still manual.
    if settings.auto_switch || !matches!(mode, ThemeMode::Auto) {
        match appearance {
            Appearance::Dark => {
                if let Some(dark) = settings.custom.dark.as_ref() {
                    dark.apply(&mut colors);
                }
            }
            Appearance::Light => {
                if let Some(light) = settings.custom.light.as_ref() {
                    light.apply(&mut colors);
                }
            }
        }
    }
    ThemePalette::from_colors(colors, appearance)
}

pub fn detect_appearance() -> Appearance {
    if let Some(value) = std::env::var("HERDR_AGENT_QUOTA_APPEARANCE")
        .ok()
        .and_then(|value| Appearance::parse(&value))
    {
        return value;
    }

    #[cfg(target_os = "macos")]
    {
        // `defaults` exits non-zero when AppleInterfaceStyle is unset, which
        // is macOS's way of saying "Light".
        if let Ok(output) = Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
        {
            if !output.status.success() {
                return Appearance::Light;
            }
            let value = String::from_utf8_lossy(&output.stdout);
            return if value.trim().eq_ignore_ascii_case("dark") {
                Appearance::Dark
            } else {
                Appearance::Light
            };
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
        {
            if output.status.success() {
                let value = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
                if value.contains("dark") {
                    return Appearance::Dark;
                }
                if value.contains("light") {
                    return Appearance::Light;
                }
            }
        }
        if let Ok(value) = std::env::var("GTK_THEME") {
            if value.to_ascii_lowercase().contains("dark") {
                return Appearance::Dark;
            }
        }
        if let Some(value) = std::env::var("COLORFGBG").ok().and_then(|value| {
            value
                .rsplit(';')
                .next()
                .and_then(|background| background.parse::<u8>().ok())
        }) {
            return if value >= 7 {
                Appearance::Light
            } else {
                Appearance::Dark
            };
        }
    }

    Appearance::Dark
}

fn read_herdr_config() -> String {
    crate::configure::herdr::config_path()
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_default()
}

/// Herdr's exact built-in semantic colors. Keep this table in sync with
/// `Palette` in Herdr: a theme that is not here falls back to the matching
/// Catppuccin sibling rather than to a random hue.
#[derive(Debug, Clone, Copy)]
struct BuiltinColors {
    text: &'static str,
    green: &'static str,
    yellow: &'static str,
    red: &'static str,
    teal: &'static str,
}

/// Owned copy of a built-in palette so `theme.custom` can override it.
#[derive(Debug, Clone)]
struct ResolvedColors {
    text: String,
    green: String,
    yellow: String,
    red: String,
    teal: String,
}

impl ResolvedColors {
    fn from_builtin(colors: BuiltinColors) -> Self {
        Self {
            text: colors.text.to_string(),
            green: colors.green.to_string(),
            yellow: colors.yellow.to_string(),
            red: colors.red.to_string(),
            teal: colors.teal.to_string(),
        }
    }
}

const BUILTINS: &[(&str, BuiltinColors)] = &[
    (
        "catppuccin",
        BuiltinColors {
            text: "#cdd6f4",
            green: "#a6e3a1",
            yellow: "#f9e2af",
            red: "#f38ba8",
            teal: "#94e2d5",
        },
    ),
    (
        "catppuccin-latte",
        BuiltinColors {
            text: "#4c4f69",
            green: "#40a02b",
            yellow: "#df8e1d",
            red: "#d20f39",
            teal: "#179299",
        },
    ),
    (
        "tokyo-night",
        BuiltinColors {
            text: "#c0caf5",
            green: "#9ece6a",
            yellow: "#e0af68",
            red: "#f7768e",
            teal: "#7dcfff",
        },
    ),
    (
        "tokyo-night-day",
        BuiltinColors {
            text: "#3760bf",
            green: "#587539",
            yellow: "#8c6c3e",
            red: "#f52a65",
            teal: "#118c74",
        },
    ),
    (
        "dracula",
        BuiltinColors {
            text: "#f8f8f2",
            green: "#50fa7b",
            yellow: "#f1fa8c",
            red: "#ff5555",
            teal: "#8be9fd",
        },
    ),
    (
        "nord",
        BuiltinColors {
            text: "#eceff4",
            green: "#a3be8c",
            yellow: "#ebcb8b",
            red: "#bf616a",
            teal: "#8fbcbb",
        },
    ),
    (
        "gruvbox",
        BuiltinColors {
            text: "#ebdbb2",
            green: "#b8bb26",
            yellow: "#fabd2f",
            red: "#fb4934",
            teal: "#8ec07c",
        },
    ),
    (
        "gruvbox-light",
        BuiltinColors {
            text: "#3c3836",
            green: "#79740e",
            yellow: "#b57614",
            red: "#9d0006",
            teal: "#427b58",
        },
    ),
    (
        "one-dark",
        BuiltinColors {
            text: "#abb2bf",
            green: "#98c379",
            yellow: "#e5c07b",
            red: "#e06c75",
            teal: "#56b6c2",
        },
    ),
    (
        "one-light",
        BuiltinColors {
            text: "#383a42",
            green: "#50a14f",
            yellow: "#c18401",
            red: "#e45649",
            teal: "#0184bc",
        },
    ),
    (
        "solarized",
        BuiltinColors {
            text: "#93a1a1",
            green: "#859900",
            yellow: "#b58900",
            red: "#dc322f",
            teal: "#2aa198",
        },
    ),
    (
        "solarized-light",
        BuiltinColors {
            text: "#657b83",
            green: "#859900",
            yellow: "#b58900",
            red: "#dc322f",
            teal: "#2aa198",
        },
    ),
    (
        "kanagawa",
        BuiltinColors {
            text: "#dcd7ba",
            green: "#76946a",
            yellow: "#c0a36e",
            red: "#c34043",
            teal: "#7fb4ca",
        },
    ),
    (
        "kanagawa-lotus",
        BuiltinColors {
            text: "#545464",
            green: "#6f894e",
            yellow: "#77713f",
            red: "#c84053",
            teal: "#4e8ca2",
        },
    ),
    (
        "rose-pine",
        BuiltinColors {
            text: "#e0def4",
            green: "#31748f",
            yellow: "#f6c177",
            red: "#eb6f92",
            teal: "#9ccfd8",
        },
    ),
    (
        "rose-pine-dawn",
        BuiltinColors {
            text: "#464261",
            green: "#286983",
            yellow: "#ea9d34",
            red: "#b4637a",
            teal: "#56949f",
        },
    ),
    (
        "vesper",
        BuiltinColors {
            text: "#ffffff",
            green: "#99ffe4",
            yellow: "#ffc799",
            red: "#ff8080",
            teal: "#66ddcc",
        },
    ),
];

fn builtin_colors(name: &str, appearance: Appearance) -> BuiltinColors {
    let fallback = builtin_named(fallback_name(appearance)).unwrap_or(BUILTINS[0].1);
    let Some(canonical) = canonical_theme_name(name) else {
        return fallback;
    };
    // `terminal` follows the host ANSI palette, which a plugin cannot read.
    // The matching Catppuccin sibling is the safe stand-in for each mode.
    if canonical == "terminal" {
        return fallback;
    }
    builtin_named(canonical).unwrap_or(fallback)
}

fn builtin_named(name: &str) -> Option<BuiltinColors> {
    BUILTINS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, colors)| *colors)
}

fn fallback_name(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Dark => "catppuccin",
        Appearance::Light => "catppuccin-latte",
    }
}

fn select_theme_name(settings: &ThemeSettings, mode: ThemeMode, appearance: Appearance) -> String {
    match mode {
        ThemeMode::Auto if settings.auto_switch => match appearance {
            Appearance::Dark => settings
                .dark_name
                .clone()
                .unwrap_or_else(|| sibling_theme_names(&settings.name).0),
            Appearance::Light => settings
                .light_name
                .clone()
                .unwrap_or_else(|| sibling_theme_names(&settings.name).1),
        },
        ThemeMode::Auto => settings.name.clone(),
        ThemeMode::Dark => settings
            .dark_name
            .clone()
            .unwrap_or_else(|| sibling_theme_names(&settings.name).0),
        ThemeMode::Light => settings
            .light_name
            .clone()
            .unwrap_or_else(|| sibling_theme_names(&settings.name).1),
    }
}

fn infer_appearance(name: &str) -> Option<Appearance> {
    match canonical_theme_name(name)? {
        "catppuccin" | "tokyo-night" | "dracula" | "nord" | "gruvbox" | "one-dark"
        | "solarized" | "kanagawa" | "rose-pine" | "vesper" => Some(Appearance::Dark),
        "catppuccin-latte" | "tokyo-night-day" | "gruvbox-light" | "one-light"
        | "solarized-light" | "kanagawa-lotus" | "rose-pine-dawn" => Some(Appearance::Light),
        "terminal" => None,
        _ => None,
    }
}

fn canonical_theme_name(name: &str) -> Option<&'static str> {
    match name.to_lowercase().replace([' ', '_'], "-").as_str() {
        "catppuccin" | "catppuccin-mocha" => Some("catppuccin"),
        "catppuccin-latte" | "latte" | "light" => Some("catppuccin-latte"),
        "terminal" => Some("terminal"),
        "tokyo-night" | "tokyonight" => Some("tokyo-night"),
        "tokyo-night-day" | "tokyo-day" | "tokyonight-day" => Some("tokyo-night-day"),
        "dracula" => Some("dracula"),
        "nord" => Some("nord"),
        "gruvbox" | "gruvbox-dark" => Some("gruvbox"),
        "gruvbox-light" => Some("gruvbox-light"),
        "one-dark" | "onedark" => Some("one-dark"),
        "one-light" | "onelight" => Some("one-light"),
        "solarized" | "solarized-dark" => Some("solarized"),
        "solarized-light" => Some("solarized-light"),
        "kanagawa" => Some("kanagawa"),
        "kanagawa-lotus" | "lotus" => Some("kanagawa-lotus"),
        "rose-pine" | "rosepine" => Some("rose-pine"),
        "rose-pine-dawn" | "rosepine-dawn" | "dawn" => Some("rose-pine-dawn"),
        "vesper" => Some("vesper"),
        _ => None,
    }
}

fn sibling_theme_names(name: &str) -> (String, String) {
    match canonical_theme_name(name) {
        Some("catppuccin" | "catppuccin-latte") => {
            ("catppuccin".to_string(), "catppuccin-latte".to_string())
        }
        Some("tokyo-night" | "tokyo-night-day") => {
            ("tokyo-night".to_string(), "tokyo-night-day".to_string())
        }
        Some("gruvbox" | "gruvbox-light") => ("gruvbox".to_string(), "gruvbox-light".to_string()),
        Some("one-dark" | "one-light") => ("one-dark".to_string(), "one-light".to_string()),
        Some("solarized" | "solarized-light") => {
            ("solarized".to_string(), "solarized-light".to_string())
        }
        Some("kanagawa" | "kanagawa-lotus") => {
            ("kanagawa".to_string(), "kanagawa-lotus".to_string())
        }
        Some("rose-pine" | "rose-pine-dawn") => {
            ("rose-pine".to_string(), "rose-pine-dawn".to_string())
        }
        _ => (name.to_string(), name.to_string()),
    }
}

#[derive(Debug, Default, Clone)]
struct ThemeSettings {
    name: String,
    auto_switch: bool,
    dark_name: Option<String>,
    light_name: Option<String>,
    custom: CustomColors,
}

#[derive(Debug, Default, Clone)]
struct CustomColors {
    text: Option<String>,
    green: Option<String>,
    yellow: Option<String>,
    red: Option<String>,
    teal: Option<String>,
    light: Option<Box<CustomColors>>,
    dark: Option<Box<CustomColors>>,
}

impl CustomColors {
    fn apply(&self, colors: &mut ResolvedColors) {
        for (value, target) in [
            (&self.text, &mut colors.text),
            (&self.green, &mut colors.green),
            (&self.yellow, &mut colors.yellow),
            (&self.red, &mut colors.red),
            (&self.teal, &mut colors.teal),
        ] {
            if let Some(normalized) = value.as_deref().and_then(normalize_color) {
                *target = normalized;
            }
        }
    }
}

fn parse_theme_settings(text: &str) -> ThemeSettings {
    let mut settings = ThemeSettings {
        name: "catppuccin".to_string(),
        ..ThemeSettings::default()
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return settings;
    };
    let Some(theme) = value.get("theme").and_then(toml::Value::as_table) else {
        return settings;
    };
    if let Some(name) = theme.get("name").and_then(toml::Value::as_str) {
        settings.name = name.to_string();
    }
    settings.auto_switch = theme
        .get("auto_switch")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    settings.dark_name = theme
        .get("dark_name")
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    settings.light_name = theme
        .get("light_name")
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    if let Some(custom) = theme.get("custom").and_then(toml::Value::as_table) {
        settings.custom = parse_custom_colors(custom);
    }
    settings
}

fn parse_custom_colors(table: &toml::Table) -> CustomColors {
    CustomColors {
        text: table
            .get("text")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        green: table
            .get("green")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        yellow: table
            .get("yellow")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        red: table
            .get("red")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        teal: table
            .get("teal")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        light: table
            .get("light")
            .and_then(toml::Value::as_table)
            .map(|table| Box::new(parse_custom_colors(table))),
        dark: table
            .get("dark")
            .and_then(toml::Value::as_table)
            .map(|table| Box::new(parse_custom_colors(table))),
    }
}

fn normalize_color(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        if !hex.is_ascii() {
            return None;
        }
        return match hex.len() {
            3 => Some(format!(
                "#{}{}{}{}{}{}",
                &hex[0..1],
                &hex[0..1],
                &hex[1..2],
                &hex[1..2],
                &hex[2..3],
                &hex[2..3]
            )),
            6 if hex.chars().all(|ch| ch.is_ascii_hexdigit()) => Some(format!("#{hex}")),
            _ => None,
        };
    }
    if let Some(inner) = value.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() == 3 {
            let channels = parts
                .iter()
                .map(|part| part.parse::<u8>().ok())
                .collect::<Option<Vec<_>>>()?;
            return Some(format!(
                "#{:02x}{:02x}{:02x}",
                channels[0], channels[1], channels[2]
            ));
        }
    }
    named_color(&value).map(str::to_string)
}

fn named_color(name: &str) -> Option<&'static str> {
    match name {
        "black" => Some("#000000"),
        "red" => Some("#ff0000"),
        "green" => Some("#00ff00"),
        "yellow" => Some("#ffff00"),
        "blue" => Some("#0000ff"),
        "magenta" | "purple" => Some("#ff00ff"),
        "cyan" => Some("#00ffff"),
        "white" => Some("#ffffff"),
        "gray" | "grey" => Some("#808080"),
        "darkgray" | "darkgrey" => Some("#404040"),
        "lightred" => Some("#ff8080"),
        "lightgreen" => Some("#80ff80"),
        "lightyellow" => Some("#ffff80"),
        "lightblue" => Some("#8080ff"),
        "lightmagenta" => Some("#ff80ff"),
        "lightcyan" => Some("#80ffff"),
        _ => None,
    }
}

fn muted(hex: &str, appearance: Appearance) -> String {
    let Some((r, g, b)) = parse_hex(hex) else {
        return hex.to_string();
    };
    // Blend toward a mid-tone that matches the background direction: lighter
    // on dark themes, darker on light themes. This keeps meter rows readable
    // without the full-strength alarm hue.
    let target = match appearance {
        Appearance::Dark => (176, 176, 176),
        Appearance::Light => (80, 80, 80),
    };
    let weight = 35u32;
    let mix = |channel: u8, target: u8| {
        ((u32::from(channel) * (100 - weight) + u32::from(target) * weight) / 100) as u8
    };
    format!(
        "#{:02x}{:02x}{:02x}",
        mix(r, target.0),
        mix(g, target.1),
        mix(b, target.2)
    )
}

fn parse_hex(value: &str) -> Option<(u8, u8, u8)> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    Some((
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme_config(name: &str, auto_switch: bool, light: &str, dark: &str) -> String {
        format!(
            "[theme]\nname = \"{name}\"\nauto_switch = {auto_switch}\nlight_name = \"{light}\"\ndark_name = \"{dark}\"\n"
        )
    }

    #[test]
    fn catppuccin_and_latte_use_herdr_status_colors() {
        let dark = resolve_with(
            &theme_config("catppuccin", false, "catppuccin-latte", "catppuccin"),
            ThemeMode::Auto,
            Appearance::Dark,
        );
        assert_eq!(dark.idle, "#cdd6f4");
        assert_eq!(dark.safe, "#a6e3a1");
        assert_eq!(dark.warning, "#f9e2af");
        assert_eq!(dark.danger, "#f38ba8");
        assert_eq!(dark.working, "#f9e2af");
        assert_eq!(dark.done, "#94e2d5");
        assert_eq!(dark.blocked, "#f38ba8");

        let light = resolve_with(
            &theme_config("catppuccin-latte", false, "catppuccin-latte", "catppuccin"),
            ThemeMode::Auto,
            Appearance::Dark,
        );
        assert_eq!(light.idle, "#4c4f69");
        assert_eq!(light.safe, "#40a02b");
        assert_eq!(light.warning, "#df8e1d");
        assert_eq!(light.danger, "#d20f39");
        assert_eq!(light.done, "#179299");
    }

    #[test]
    fn auto_switch_follows_the_host_appearance() {
        let config = theme_config("catppuccin", true, "catppuccin-latte", "catppuccin");
        let dark = resolve_with(&config, ThemeMode::Auto, Appearance::Dark);
        let light = resolve_with(&config, ThemeMode::Auto, Appearance::Light);
        assert_eq!(dark.idle, "#cdd6f4");
        assert_eq!(light.idle, "#4c4f69");
    }

    #[test]
    fn forcing_a_mode_overrides_auto_switch() {
        let config = theme_config("catppuccin", true, "catppuccin-latte", "catppuccin");
        let forced_light = resolve_with(&config, ThemeMode::Light, Appearance::Dark);
        let forced_dark = resolve_with(&config, ThemeMode::Dark, Appearance::Light);
        assert_eq!(forced_light.idle, "#4c4f69");
        assert_eq!(forced_dark.idle, "#cdd6f4");
    }

    #[test]
    fn custom_overrides_are_applied_on_top_of_the_base_theme() {
        let config = r##"
[theme]
name = "catppuccin"

[theme.custom]
green = "#112233"
yellow = "rgb(1, 2, 3)"
red = "#abc"

[theme.custom.light]
green = "#445566"
"##;
        let dark = resolve_with(config, ThemeMode::Auto, Appearance::Dark);
        assert_eq!(dark.safe, "#112233");
        assert_eq!(dark.warning, "#010203");
        assert_eq!(dark.danger, "#aabbcc");

        let light = resolve_with(config, ThemeMode::Light, Appearance::Light);
        assert_eq!(light.safe, "#445566");
    }

    #[test]
    fn unknown_themes_fall_back_to_the_matching_catppuccin_sibling() {
        let dark = resolve_with(
            &theme_config("not-a-theme", false, "catppuccin-latte", "catppuccin"),
            ThemeMode::Auto,
            Appearance::Dark,
        );
        assert_eq!(dark.safe, "#a6e3a1");
        let light = resolve_with(
            &theme_config("not-a-theme", false, "catppuccin-latte", "catppuccin"),
            ThemeMode::Light,
            Appearance::Light,
        );
        assert_eq!(light.safe, "#40a02b");
    }

    #[test]
    fn terminal_theme_uses_the_catppuccin_sibling_for_the_appearance() {
        let config = theme_config("terminal", true, "terminal", "terminal");
        let dark = resolve_with(&config, ThemeMode::Auto, Appearance::Dark);
        let light = resolve_with(&config, ThemeMode::Auto, Appearance::Light);
        assert_eq!(dark.idle, "#cdd6f4");
        assert_eq!(light.idle, "#4c4f69");
    }

    #[test]
    fn gauges_are_muted_versions_of_the_severity_family() {
        let dark = ThemePalette::catppuccin();
        let light = ThemePalette::catppuccin_latte();
        assert_ne!(dark.gauge_safe, dark.safe);
        assert_ne!(light.gauge_safe, light.safe);
        assert_eq!(dark.gauge_safe.len(), 7);
        assert_eq!(light.gauge_safe.len(), 7);
    }
}
