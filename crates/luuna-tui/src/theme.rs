use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// One styled role: foreground, background, bold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
}

/// Every color the client draws with. `pascal` (the default) is the navy Quick Access
/// Pascal scheme; `classic` is the original blue workbench; `terminal` keeps the
/// terminal's own background and foreground.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub base: Slot,
    pub panel: Slot,
    pub accent: Slot,
    pub selected: Slot,
    pub chrome: Slot,
    pub mnemonic: Slot,
    pub title: Slot,
    pub success: Color,
    pub active: Color,
    pub danger: Color,
    pub attention: Color,
    pub interrupted: Color,
    pub neutral: Color,
}

const fn slot(fg: Color, bg: Color, bold: bool) -> Slot {
    Slot { fg, bg, bold }
}

impl Palette {
    /// The Quick Access Pascal scheme: navy panels, pale text, cyan highlights and
    /// warm keys (owner feedback G1, 2026-11-13).
    pub const PASCAL: Palette = {
        let navy = Color::Rgb(0x0d, 0x20, 0x3b);
        let panel = Color::Rgb(0x10, 0x25, 0x41);
        let deepest = Color::Rgb(0x02, 0x0d, 0x20);
        let text = Color::Rgb(0xe0, 0xed, 0xf4);
        let muted = Color::Rgb(0x8b, 0xb3, 0xc6);
        let cyan = Color::Rgb(0x78, 0xe0, 0xe6);
        let warm = Color::Rgb(0xf3, 0xdd, 0x96);
        Palette {
            base: slot(muted, navy, false),
            panel: slot(text, panel, false),
            accent: slot(cyan, panel, false),
            selected: slot(
                Color::Rgb(0xec, 0xf7, 0xfc),
                Color::Rgb(0x1d, 0x5f, 0x75),
                true,
            ),
            chrome: slot(Color::Rgb(0xd8, 0xea, 0xf0), deepest, false),
            mnemonic: slot(warm, deepest, true),
            title: slot(Color::Rgb(0x73, 0xd8, 0xd6), panel, true),
            success: Color::Rgb(0x37, 0xf4, 0x99),
            active: cyan,
            danger: Color::Rgb(0xff, 0x7b, 0x6b),
            attention: Color::Rgb(0xf4, 0xcd, 0x81),
            interrupted: Color::Rgb(0xff, 0xa9, 0x9a),
            neutral: muted,
        }
    };
    pub const CLASSIC: Palette = {
        let blue = Color::Rgb(0, 0, 170);
        let grey = Color::Rgb(170, 170, 170);
        Palette {
            base: slot(grey, blue, false),
            panel: slot(Color::Rgb(255, 255, 255), blue, false),
            accent: slot(Color::Rgb(85, 255, 255), blue, false),
            selected: slot(Color::Rgb(255, 255, 255), Color::Rgb(0, 170, 0), true),
            chrome: slot(Color::Black, grey, false),
            mnemonic: slot(Color::Rgb(170, 0, 0), grey, true),
            title: slot(Color::Rgb(255, 255, 85), blue, true),
            success: Color::Rgb(85, 255, 85),
            active: Color::Rgb(85, 255, 255),
            danger: Color::Rgb(255, 85, 85),
            attention: Color::Rgb(255, 255, 85),
            interrupted: Color::Rgb(255, 85, 255),
            neutral: grey,
        }
    };
    pub const TERMINAL: Palette = Palette {
        base: slot(Color::Reset, Color::Reset, false),
        panel: slot(Color::Reset, Color::Reset, false),
        accent: slot(Color::Cyan, Color::Reset, false),
        selected: slot(Color::Black, Color::Cyan, true),
        chrome: slot(Color::Black, Color::Gray, false),
        mnemonic: slot(Color::Red, Color::Gray, true),
        title: slot(Color::Yellow, Color::Reset, true),
        success: Color::Green,
        active: Color::Cyan,
        danger: Color::Red,
        attention: Color::Yellow,
        interrupted: Color::Magenta,
        neutral: Color::Reset,
    };

    pub fn preset(name: &str) -> Option<Palette> {
        match name {
            "pascal" => Some(Self::PASCAL),
            "classic" => Some(Self::CLASSIC),
            "terminal" => Some(Self::TERMINAL),
            _ => None,
        }
    }

    /// Applies a configuration's preset and overrides; bad values are reported and skipped.
    pub fn configured(config: &ThemeConfig, warnings: &mut Vec<String>) -> Palette {
        let mut palette = match config.preset.as_deref() {
            None => Self::PASCAL,
            Some(name) => Self::preset(name).unwrap_or_else(|| {
                warnings.push(format!(
                    "theme.preset: unknown preset {name:?} (pascal, classic, terminal)"
                ));
                Self::PASCAL
            }),
        };
        for (name, spec) in &config.styles {
            let target = match name.as_str() {
                "base" => &mut palette.base,
                "panel" => &mut palette.panel,
                "accent" => &mut palette.accent,
                "selected" => &mut palette.selected,
                "chrome" => &mut palette.chrome,
                "mnemonic" => &mut palette.mnemonic,
                "title" => &mut palette.title,
                other => {
                    warnings.push(format!("theme.styles: unknown style {other:?}"));
                    continue;
                }
            };
            for (field, value, into) in [
                ("fg", &spec.fg, &mut target.fg),
                ("bg", &spec.bg, &mut target.bg),
            ] {
                if let Some(value) = value {
                    match parse_color(value) {
                        Ok(color) => *into = color,
                        Err(error) => {
                            warnings.push(format!("theme.styles.{name}.{field}: {error}"))
                        }
                    }
                }
            }
            if let Some(bold) = spec.bold {
                target.bold = bold;
            }
        }
        for (name, value) in &config.tones {
            let target = match name.as_str() {
                "success" => &mut palette.success,
                "active" => &mut palette.active,
                "danger" => &mut palette.danger,
                "attention" => &mut palette.attention,
                "interrupted" => &mut palette.interrupted,
                "neutral" => &mut palette.neutral,
                other => {
                    warnings.push(format!("theme.tones: unknown tone {other:?}"));
                    continue;
                }
            };
            match parse_color(value) {
                Ok(color) => *target = color,
                Err(error) => warnings.push(format!("theme.tones.{name}: {error}")),
            }
        }
        palette
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StyleSpec {
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub bold: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    pub preset: Option<String>,
    /// `false` draws without color, like NO_COLOR; `true` forces color even when NO_COLOR is set.
    pub color: Option<bool>,
    pub styles: BTreeMap<String, StyleSpec>,
    pub tones: BTreeMap<String, String>,
}

/// "#rrggbb", "#rgb", a 0–255 palette index, a color name or "reset" (the terminal's own).
pub fn parse_color(value: &str) -> Result<Color, String> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()
            .ok_or_else(|| format!("{value:?} is not a hex color"))?;
        return match digits.as_slice() {
            [r, g, b] => Ok(Color::Rgb(r * 17, g * 17, b * 17)),
            [r1, r2, g1, g2, b1, b2] => Ok(Color::Rgb(r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2)),
            _ => Err(format!("{value:?} needs 3 or 6 hex digits")),
        };
    }
    if let Ok(index) = value.parse::<u8>() {
        return Ok(Color::Indexed(index));
    }
    let name = value.to_ascii_lowercase().replace(['-', '_', ' '], "");
    Ok(match name.as_str() {
        "reset" | "default" | "none" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        _ => return Err(format!("unknown color {value:?}")),
    })
}

static PALETTE: OnceLock<Palette> = OnceLock::new();
static COLOR: OnceLock<bool> = OnceLock::new();

/// Installs the configured palette and color choice once, at startup.
pub fn install(palette: Palette, color: Option<bool>) {
    let _ = PALETTE.set(palette);
    if let Some(color) = color {
        let _ = COLOR.set(color);
    }
}

pub fn palette() -> &'static Palette {
    PALETTE.get_or_init(|| Palette::PASCAL)
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub color: bool,
}
impl Theme {
    pub fn from_env() -> Self {
        Self {
            color: COLOR.get().copied().unwrap_or_else(|| {
                std::env::var("NO_COLOR").unwrap_or_default().is_empty()
                    && std::env::var("TERM").unwrap_or_default() != "dumb"
            }),
        }
    }
    pub fn base(self) -> Style {
        self.slot(palette().base)
    }
    pub fn panel(self) -> Style {
        self.slot(palette().panel)
    }
    pub fn accent(self) -> Style {
        self.slot(palette().accent)
    }
    pub fn selected(self) -> Style {
        self.slot(palette().selected)
    }
    pub fn muted(self) -> Style {
        self.base()
    }
    pub fn chrome(self) -> Style {
        self.slot(palette().chrome)
    }
    pub fn mnemonic(self) -> Style {
        self.slot(palette().mnemonic)
    }
    pub fn title(self) -> Style {
        self.slot(palette().title)
    }
    pub fn focused(self) -> Style {
        self.panel()
    }
    pub fn tone(self, tone: &str) -> Style {
        let p = palette();
        let color = match tone {
            "success" => p.success,
            "active" => p.active,
            "danger" => p.danger,
            "attention" => p.attention,
            "interrupted" => p.interrupted,
            _ => p.neutral,
        };
        if self.color {
            self.panel().fg(color)
        } else {
            Style::default()
        }
    }
    /// Without color every role is the terminal's default style, as it always was.
    fn slot(self, slot: Slot) -> Style {
        if !self.color {
            return Style::default();
        }
        let style = Style::default().fg(slot.fg).bg(slot.bg);
        if slot.bold {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    }
}

pub fn state_tone(state: &str) -> &'static str {
    match state {
        "passed" | "accepted" | "connected" | "reviewed" => "success",
        "running" | "exploring" | "implementing" | "validating" | "reviewing" | "active" => {
            "active"
        }
        "failed" | "rejected" | "blocked" | "testing_blocked" | "changes_required" | "blocking" => {
            "danger"
        }
        "unconfirmed" | "approved" | "plan_ready" | "paused" | "warning" => "attention",
        "interrupted" | "reconnecting" | "abandoned" => "interrupted",
        _ => "neutral",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_parse_in_every_supported_form() {
        assert_eq!(parse_color("#0000aa"), Ok(Color::Rgb(0, 0, 170)));
        assert_eq!(parse_color("#fff"), Ok(Color::Rgb(255, 255, 255)));
        assert_eq!(parse_color("202"), Ok(Color::Indexed(202)));
        assert_eq!(parse_color("Light-Cyan"), Ok(Color::LightCyan));
        assert_eq!(parse_color("reset"), Ok(Color::Reset));
        assert!(parse_color("#12").is_err());
        assert!(parse_color("chartreuse").is_err());
    }

    #[test]
    fn overrides_apply_and_bad_values_warn() {
        let config: ThemeConfig = serde_json::from_value(serde_json::json!({
            "preset": "terminal",
            "styles": {"title": {"fg": "#ff8800", "bold": false}, "bogus": {}},
            "tones": {"danger": "196", "success": "nope"}
        }))
        .unwrap();
        let mut warnings = Vec::new();
        let palette = Palette::configured(&config, &mut warnings);
        assert_eq!(
            palette.title,
            slot(Color::Rgb(255, 136, 0), Color::Reset, false)
        );
        assert_eq!(palette.danger, Color::Indexed(196));
        assert_eq!(palette.success, Palette::TERMINAL.success);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn classic_matches_the_original_styles() {
        assert_eq!(
            Palette::CLASSIC.selected,
            slot(Color::Rgb(255, 255, 255), Color::Rgb(0, 170, 0), true)
        );
        assert_eq!(Palette::preset("classic"), Some(Palette::CLASSIC));
        assert_eq!(Theme { color: false }.title(), Style::default());
    }

    #[test]
    fn pascal_is_the_default_and_uses_the_quick_access_navy() {
        let mut warnings = Vec::new();
        assert_eq!(
            Palette::configured(&ThemeConfig::default(), &mut warnings),
            Palette::PASCAL
        );
        assert!(warnings.is_empty());
        assert_eq!(Palette::PASCAL.base.bg, Color::Rgb(0x0d, 0x20, 0x3b));
        assert_eq!(Palette::PASCAL.accent.fg, Color::Rgb(0x78, 0xe0, 0xe6));
        assert_eq!(Palette::PASCAL.success, Color::Rgb(0x37, 0xf4, 0x99));
        let theme = Theme { color: true };
        assert_eq!(
            theme.selected(),
            Style::default()
                .fg(Color::Rgb(0xec, 0xf7, 0xfc))
                .bg(Color::Rgb(0x1d, 0x5f, 0x75))
                .add_modifier(Modifier::BOLD)
        );
    }
}
