//! TOML theme loader with partial-override merging.
//!
//! Reads `~/.config/clitunes/theme.toml` (or the XDG equivalent) and
//! layers user overrides on top of the built-in defaults. Unknown keys
//! are silently ignored so forward-compatible themes don't break on
//! older builds.

use std::path::Path;

use crate::visualiser::cell_grid::Rgb;

use super::Theme;
use super::Token;

/// Intermediate TOML representation. All fields are optional so partial
/// overrides work: the user only specifies what they want to change.
#[derive(Debug, Default, serde::Deserialize)]
struct ThemeFile {
    #[serde(default)]
    colors: Colors,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct Colors {
    background: Option<String>,
    surface: Option<String>,
    surface_bright: Option<String>,
    border: Option<String>,
    border_focus: Option<String>,
    foreground: Option<String>,
    foreground_dim: Option<String>,
    foreground_bright: Option<String>,
    muted: Option<String>,
    accent: Option<String>,
    accent_hover: Option<String>,
    danger: Option<String>,
    warning: Option<String>,
    success: Option<String>,
}

/// Parse a `#RRGGBB` hex string into an [`Rgb`].
///
/// Returns `None` for malformed input (wrong length, invalid hex digits,
/// missing `#` prefix).
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Rgb::new(r, g, b))
}

/// Format an [`Rgb`] as a `#RRGGBB` hex string.
pub fn rgb_to_hex(c: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
}

/// Load a theme from a TOML file, merging overrides onto defaults.
///
/// If the file does not exist or cannot be parsed, returns
/// `Theme::default()` with no error — the user should never be blocked
/// from starting because of a broken theme file.
pub fn load(path: &Path) -> Theme {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Theme::default(),
    };

    let file: ThemeFile = match toml::from_str(&content) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "theme: failed to parse, using defaults");
            return Theme::default();
        }
    };

    let mut theme = Theme::default();
    let c = &file.colors;

    macro_rules! apply {
        ($field:ident, $token:ident) => {
            if let Some(hex) = &c.$field {
                if let Some(rgb) = parse_hex(hex) {
                    theme.set(Token::$token, rgb);
                } else {
                    tracing::warn!(
                        key = stringify!($field),
                        value = %hex,
                        "theme: invalid hex colour, skipping"
                    );
                }
            }
        };
    }

    apply!(background, Background);
    apply!(surface, Surface);
    apply!(surface_bright, SurfaceBright);
    apply!(border, Border);
    apply!(border_focus, BorderFocus);
    apply!(foreground, Foreground);
    apply!(foreground_dim, ForegroundDim);
    apply!(foreground_bright, ForegroundBright);
    apply!(muted, Muted);
    apply!(accent, Accent);
    apply!(accent_hover, AccentHover);
    apply!(danger, Danger);
    apply!(warning, Warning);
    apply!(success, Success);

    theme
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_valid() {
        assert_eq!(parse_hex("#FF8000"), Some(Rgb::new(255, 128, 0)));
        assert_eq!(parse_hex("#000000"), Some(Rgb::new(0, 0, 0)));
        assert_eq!(parse_hex("#ffffff"), Some(Rgb::new(255, 255, 255)));
    }

    #[test]
    fn parse_hex_invalid() {
        assert_eq!(parse_hex("#ZZZZZZ"), None);
        assert_eq!(parse_hex("#12"), None);
        assert_eq!(parse_hex(""), None);
        assert_eq!(parse_hex("FF8000"), None); // missing #
        assert_eq!(parse_hex("#FF80001"), None); // too long
    }

    #[test]
    fn rgb_to_hex_roundtrip() {
        let original = Rgb::new(150, 160, 180);
        let hex = rgb_to_hex(original);
        let parsed = parse_hex(&hex).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let theme = load(Path::new("/nonexistent/theme.toml"));
        assert_eq!(
            theme.get(Token::Background),
            Theme::default().get(Token::Background)
        );
    }

    #[test]
    fn load_partial_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.toml");
        std::fs::write(
            &path,
            r##"
[colors]
accent = "#FF0000"
"##,
        )
        .unwrap();

        let theme = load(&path);
        assert_eq!(theme.get(Token::Accent), Rgb::new(255, 0, 0));
        // Non-overridden tokens retain defaults.
        assert_eq!(
            theme.get(Token::Background),
            Theme::default().get(Token::Background)
        );
    }

    #[test]
    fn load_invalid_hex_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.toml");
        std::fs::write(
            &path,
            r##"
[colors]
accent = "#ZZZZZZ"
border = "#96A0B4"
"##,
        )
        .unwrap();

        let theme = load(&path);
        // Invalid accent skipped, retains default.
        assert_eq!(
            theme.get(Token::Accent),
            Theme::default().get(Token::Accent)
        );
        // Valid border applied.
        assert_eq!(theme.get(Token::Border), Rgb::new(0x96, 0xA0, 0xB4));
    }
}
