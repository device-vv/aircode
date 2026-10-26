use aircode_core::ThemeMode;
use gpui::{Rgba, rgb};

#[derive(Clone, Copy)]
pub struct Theme {
    pub window: Rgba,
    pub surface: Rgba,
    pub raised: Rgba,
    pub hover: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub faint: Rgba,
    pub accent: Rgba,
    pub accent_soft: Rgba,
    pub green: Rgba,
    pub yellow: Rgba,
    pub red: Rgba,
    pub blue: Rgba,
    pub purple: Rgba,
}

impl Theme {
    pub fn from_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self {
                window: rgb(0x0e1014),
                surface: rgb(0x14171d),
                raised: rgb(0x1a1e26),
                hover: rgb(0x222834),
                border: rgb(0x282e39),
                text: rgb(0xe6e9ef),
                muted: rgb(0x8b93a5),
                faint: rgb(0x596275),
                accent: rgb(0x7c8cff),
                accent_soft: rgb(0x262d4c),
                green: rgb(0x78c6a3),
                yellow: rgb(0xe5c07b),
                red: rgb(0xe06c75),
                blue: rgb(0x61afef),
                purple: rgb(0xc678dd),
            },
            ThemeMode::Light => Self {
                window: rgb(0xf5f6f8),
                surface: rgb(0xffffff),
                raised: rgb(0xedf0f5),
                hover: rgb(0xe4e8f0),
                border: rgb(0xd8dde6),
                text: rgb(0x20242d),
                muted: rgb(0x626b7d),
                faint: rgb(0x949cac),
                accent: rgb(0x5368e8),
                accent_soft: rgb(0xe2e6ff),
                green: rgb(0x25855f),
                yellow: rgb(0x9a6817),
                red: rgb(0xc84450),
                blue: rgb(0x2477bd),
                purple: rgb(0x8b49aa),
            },
        }
    }
}
