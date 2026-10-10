//! Colors and typefaces shared by every view.

use gpui::{Global, Rgba, WindowAppearance, rgb};

/// The palette and typefaces the interface is drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// Window background behind the titlebar and sidebar.
    pub ground: Rgba,
    /// Surface a note is shown on.
    pub sheet: Rgba,
    /// Hairline borders.
    pub line: Rgba,
    /// Background of a hovered control.
    pub hover: Rgba,
    /// Primary text.
    pub text: Rgba,
    /// Secondary text and icons.
    pub muted: Rgba,
    /// Labels and placeholders.
    pub faint: Rgba,
    /// Highlights that draw the eye.
    pub accent: Rgba,
    /// Destructive actions such as closing the window.
    pub danger: Rgba,
    /// Typeface for small interface labels.
    pub ui_font: &'static str,
    /// Typeface for running text.
    pub text_font: &'static str,
    /// Typeface for titles.
    pub display_font: &'static str,
    /// Whether this is the dark variant.
    pub dark: bool,
}

impl Global for Theme {}

impl Theme {
    /// Returns the variant matching the system `appearance`.
    #[must_use]
    pub fn new(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::dawn(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dusk(),
        }
    }

    /// Returns the opposite variant.
    #[must_use]
    pub fn toggled(self) -> Self {
        if self.dark {
            Self::dawn()
        } else {
            Self::dusk()
        }
    }

    /// Returns the light variant: warm paper with umber text.
    #[must_use]
    pub fn dawn() -> Self {
        Self {
            ground: rgb(0xe9e1d3),
            sheet: rgb(0xf6f1e7),
            line: rgb(0xd6ccba),
            hover: rgb(0xdfd6c6),
            text: rgb(0x2a221b),
            muted: rgb(0x6f6354),
            faint: rgb(0xa89b88),
            accent: rgb(0x2f7f6c),
            danger: rgb(0xc4502d),
            dark: false,
            ..Self::dusk()
        }
    }

    /// Returns the dark variant: umber black with bone text.
    ///
    /// The typefaces all ship with Windows.
    #[must_use]
    pub fn dusk() -> Self {
        Self {
            ground: rgb(0x13100e),
            sheet: rgb(0x1c1815),
            line: rgb(0x2e2721),
            hover: rgb(0x231e1a),
            text: rgb(0xece3d4),
            muted: rgb(0x9c8e7d),
            faint: rgb(0x5f544a),
            accent: rgb(0x7fc8b2),
            danger: rgb(0xd4623f),
            ui_font: "Bahnschrift",
            text_font: "Sitka Text",
            display_font: "Sitka Display",
            dark: true,
        }
    }
}
