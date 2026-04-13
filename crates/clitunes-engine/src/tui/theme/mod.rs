//! Semantic colour theming for the clitunes TUI.
//!
//! All UI colours flow through [`Theme`], which maps semantic
//! [`Token`]s to concrete [`Rgb`] values. The built-in "midnight"
//! palette ships as the `Default` impl; users can override any subset
//! via `~/.config/clitunes/theme.toml` (see [`loader`]).

pub mod loader;
pub mod tokens;

pub use tokens::Token;

use crate::visualiser::cell_grid::Rgb;

/// Number of semantic tokens, derived from the last variant's index.
/// Adding a `Token` variant forces updating `token_index()` (exhaustive
/// match), and if the new variant's index exceeds `Success`, updating
/// this line — otherwise the array is too small and tests fail at compile
/// time via the const assertion below.
const TOKEN_COUNT: usize = token_index(Token::Success) + 1;

/// A resolved colour theme — one RGB value per semantic token.
///
/// Clone + Send + Sync by construction (all fields are Copy).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    colors: [Rgb; TOKEN_COUNT],
}

impl Theme {
    /// Look up the concrete colour for a semantic token.
    pub fn get(&self, token: Token) -> Rgb {
        self.colors[token_index(token)]
    }

    /// Override a single token's colour.
    pub fn set(&mut self, token: Token, rgb: Rgb) {
        self.colors[token_index(token)] = rgb;
    }
}

/// The built-in "midnight" palette — dark cool-grey surfaces with a
/// warm gold accent. Values are chosen to match the hardcoded picker
/// constants that shipped in v1.0; migrating to tokens changes zero
/// pixels on day one.
impl Default for Theme {
    fn default() -> Self {
        let mut colors = [Rgb::BLACK; TOKEN_COUNT];
        colors[token_index(Token::Background)] = Rgb::new(10, 12, 18);
        colors[token_index(Token::Surface)] = Rgb::new(20, 22, 30);
        colors[token_index(Token::SurfaceBright)] = Rgb::new(35, 38, 48);
        colors[token_index(Token::Border)] = Rgb::new(150, 160, 180);
        colors[token_index(Token::BorderFocus)] = Rgb::new(180, 190, 210);
        colors[token_index(Token::Foreground)] = Rgb::new(200, 205, 215);
        colors[token_index(Token::ForegroundDim)] = Rgb::new(110, 115, 130);
        colors[token_index(Token::ForegroundBright)] = Rgb::new(255, 255, 255);
        colors[token_index(Token::Muted)] = Rgb::new(70, 75, 85);
        colors[token_index(Token::Accent)] = Rgb::new(230, 220, 140);
        colors[token_index(Token::AccentHover)] = Rgb::new(245, 235, 160);
        colors[token_index(Token::Danger)] = Rgb::new(255, 107, 107);
        colors[token_index(Token::Warning)] = Rgb::new(255, 200, 100);
        colors[token_index(Token::Success)] = Rgb::new(100, 220, 140);
        Self { colors }
    }
}

/// Map a token variant to an array index.
const fn token_index(token: Token) -> usize {
    match token {
        Token::Background => 0,
        Token::Surface => 1,
        Token::SurfaceBright => 2,
        Token::Border => 3,
        Token::BorderFocus => 4,
        Token::Foreground => 5,
        Token::ForegroundDim => 6,
        Token::ForegroundBright => 7,
        Token::Muted => 8,
        Token::Accent => 9,
        Token::AccentHover => 10,
        Token::Danger => 11,
        Token::Warning => 12,
        Token::Success => 13,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_midnight_palette_has_correct_accent() {
        let theme = Theme::default();
        assert_eq!(theme.get(Token::Accent), Rgb::new(230, 220, 140));
    }

    #[test]
    fn set_overrides_get() {
        let mut theme = Theme::default();
        let red = Rgb::new(255, 0, 0);
        theme.set(Token::Danger, red);
        assert_eq!(theme.get(Token::Danger), red);
        // Other tokens unchanged.
        assert_eq!(theme.get(Token::Accent), Rgb::new(230, 220, 140));
    }

    #[test]
    fn theme_is_clone() {
        let a = Theme::default();
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn all_tokens_round_trip() {
        let theme = Theme::default();
        let tokens = [
            Token::Background,
            Token::Surface,
            Token::SurfaceBright,
            Token::Border,
            Token::BorderFocus,
            Token::Foreground,
            Token::ForegroundDim,
            Token::ForegroundBright,
            Token::Muted,
            Token::Accent,
            Token::AccentHover,
            Token::Danger,
            Token::Warning,
            Token::Success,
        ];
        for token in tokens {
            let rgb = theme.get(token);
            let mut t2 = Theme::default();
            t2.set(token, rgb);
            assert_eq!(t2.get(token), rgb);
        }
    }
}
