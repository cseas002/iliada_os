//! Writes Iliada's COSMIC theme, dark and light: bronze on the navy of the
//! Iliada helmet icon (dark), deep bronze on parchment (light). COSMIC's own
//! theme code computes every colour from these few, the way Settings →
//! Appearance does, so buttons, panels and text stay consistent.
//!
//!   iliada-theme <dir>   writes <dir>/cosmic/com.system76.CosmicTheme.*/v2/…
//!
//! Used when building Iliada (the system defaults); each person gets them at
//! login unless they chose their own in Appearance.

use std::path::PathBuf;

use cosmic::cosmic_config::{Config, CosmicConfigEntry};
use cosmic::cosmic_theme::palette::{Srgb, Srgba};
use cosmic::cosmic_theme::{
    DARK_THEME_BUILDER_ID, DARK_THEME_ID, LIGHT_THEME_BUILDER_ID, LIGHT_THEME_ID, Theme, ThemeBuilder,
};

fn rgb(hex: u32) -> Srgb {
    let c = |shift: u32| ((hex >> shift) & 0xff) as f32 / 255.0;
    Srgb::new(c(16), c(8), c(0))
}

fn rgba(hex: u32) -> Srgba {
    let c = rgb(hex);
    Srgba::new(c.red, c.green, c.blue, 1.0)
}

/// Iliada's dark theme: bronze on deep navy, greys tinted towards the navy.
fn dark() -> ThemeBuilder {
    let mut b = ThemeBuilder::dark().accent(rgb(0xDDA34E)).bg_color(rgba(0x0E1830)).neutral_tint(rgb(0x2A4A8A));
    b.window_hint = Some(rgb(0xDDA34E));
    b
}

/// Iliada's light theme: a deeper bronze (readable on light) on parchment.
fn light() -> ThemeBuilder {
    let mut b = ThemeBuilder::light().accent(rgb(0x9A5F1C)).bg_color(rgba(0xEFE7D8)).neutral_tint(rgb(0xB08A4E));
    b.window_hint = Some(rgb(0x9A5F1C));
    b
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(std::env::args().nth(1).ok_or("usage: iliada-theme <dir>")?);
    for (builder, builder_id, theme_id) in
        [(dark(), DARK_THEME_BUILDER_ID, DARK_THEME_ID), (light(), LIGHT_THEME_BUILDER_ID, LIGHT_THEME_ID)]
    {
        builder.write_entry(&Config::with_custom_path(builder_id, ThemeBuilder::VERSION, dir.clone())?)?;
        builder.build().write_entry(&Config::with_custom_path(theme_id, Theme::VERSION, dir.clone())?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_accent_is_bronze_on_navy() {
        let t = dark().build();
        let a = t.accent_color();
        assert!(a.red > a.green && a.green > a.blue, "bronze: red > green > blue, got {a:?}");
        let bg = t.bg_color();
        assert!(bg.blue > bg.red, "a navy background, got {bg:?}");
        assert!(!light().build().is_dark);
    }

    #[test]
    fn writes_both_themes_in_cosmic_layout() {
        let dir = std::env::temp_dir().join(format!("iliada-theme-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (builder, builder_id, theme_id) in
            [(dark(), DARK_THEME_BUILDER_ID, DARK_THEME_ID), (light(), LIGHT_THEME_BUILDER_ID, LIGHT_THEME_ID)]
        {
            builder
                .write_entry(&Config::with_custom_path(builder_id, ThemeBuilder::VERSION, dir.clone()).unwrap())
                .unwrap();
            builder
                .build()
                .write_entry(&Config::with_custom_path(theme_id, Theme::VERSION, dir.clone()).unwrap())
                .unwrap();
        }
        let accent = std::fs::read_to_string(dir.join("cosmic/com.system76.CosmicTheme.Dark/v2/accent")).unwrap();
        assert!(accent.contains("#DDA34E"), "{accent}");
        assert!(dir.join("cosmic/com.system76.CosmicTheme.Light.Builder/v2/bg_color").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
