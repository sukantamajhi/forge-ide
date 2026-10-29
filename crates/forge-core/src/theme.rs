use forge_syntax::HighlightClass;

/// Every color the UI uses, as 0xRRGGBB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub bg: u32,
    pub sidebar: u32,
    pub tab_active: u32,
    pub border: u32,
    pub text: u32,
    pub dim: u32,
    pub caret: u32,
    pub selection: u32,
    pub accent: u32,
    pub error: u32,
    pub warning: u32,
    pub info: u32,
    pub find_match: u32,
    pub find_current: u32,
    pub bracket_match: u32,
    pub line_highlight: u32,
    pub popup: u32,
    pub git_added: u32,
    pub git_modified: u32,
    pub git_deleted: u32,
    syntax: [u32; 14],
}

pub const THEME_NAMES: &[&str] = &["Forge Dark", "Forge Light", "Forge High Contrast"];

impl Theme {
    pub fn by_name(name: &str) -> Self {
        match name {
            "Forge Light" => Self::light(),
            "Forge High Contrast" => Self::high_contrast(),
            _ => Self::dark(),
        }
    }

    pub fn dark() -> Self {
        Self {
            name: "Forge Dark",
            bg: 0x14161b,
            sidebar: 0x1a1d24,
            tab_active: 0x20242c,
            border: 0x2a2f39,
            text: 0xd7dae0,
            dim: 0x6b7382,
            caret: 0x4fc1ff,
            selection: 0x2b4a6f,
            accent: 0xe5a14d,
            error: 0xf07178,
            warning: 0xffcb6b,
            info: 0x82aaff,
            find_match: 0x4a4130,
            find_current: 0x7a5c1e,
            bracket_match: 0x3a4a5c,
            line_highlight: 0x181b21,
            popup: 0x1a1d24,
            git_added: 0x73c991,
            git_modified: 0xe2c08d,
            git_deleted: 0xc74e39,
            syntax: [
                0xc792ea, 0x82aaff, 0xffcb6b, 0xc3e88d, 0x89ddff, 0xf78c6c, 0xf78c6c, 0x5c6773, 0x80cbc4, 0xffcb6b,
                0x89ddff, 0x8f98a8, 0xd7dae0, 0xf07178,
            ],
        }
    }

    pub fn light() -> Self {
        Self {
            name: "Forge Light",
            bg: 0xfafafa,
            sidebar: 0xeeeff2,
            tab_active: 0xfafafa,
            border: 0xd6d8de,
            text: 0x2b303b,
            dim: 0x8a909c,
            caret: 0x005fb8,
            selection: 0xb7d4f5,
            accent: 0xc56a00,
            error: 0xc62828,
            warning: 0x9a6700,
            info: 0x1565c0,
            find_match: 0xf3e3a3,
            find_current: 0xf0c04a,
            bracket_match: 0xcfe0f1,
            line_highlight: 0xf0f2f6,
            popup: 0xffffff,
            git_added: 0x2e7d32,
            git_modified: 0x8a6d00,
            git_deleted: 0xc62828,
            syntax: [
                0x7c3aed, 0x1d4ed8, 0xb45309, 0x15803d, 0x0e7490, 0xc2410c, 0xc2410c, 0x8a909c, 0x0f766e, 0xb45309,
                0x0e7490, 0x6b7280, 0x2b303b, 0xbe123c,
            ],
        }
    }

    pub fn high_contrast() -> Self {
        Self {
            name: "Forge High Contrast",
            bg: 0x000000,
            sidebar: 0x000000,
            tab_active: 0x1a1a1a,
            border: 0x6fc3df,
            text: 0xffffff,
            dim: 0xb0b0b0,
            caret: 0xffffff,
            selection: 0x264f78,
            accent: 0xffd700,
            error: 0xff6b6b,
            warning: 0xffd700,
            info: 0x75beff,
            find_match: 0x515c6a,
            find_current: 0x9a6b00,
            bracket_match: 0x1f4f6f,
            line_highlight: 0x0a0a0a,
            popup: 0x0c0c0c,
            git_added: 0x89d185,
            git_modified: 0xffd700,
            git_deleted: 0xff6b6b,
            syntax: [
                0xff80ff, 0x7fd4ff, 0xffe066, 0x9bff9b, 0x66ffff, 0xffa066, 0xffa066, 0xa0a0a0, 0x66ffe0, 0xffe066,
                0x66ffff, 0xd0d0d0, 0xffffff, 0xff8080,
            ],
        }
    }

    pub fn syntax_color(&self, class: HighlightClass) -> u32 {
        use HighlightClass::*;
        let index = match class {
            Keyword => 0,
            Function => 1,
            Type => 2,
            String => 3,
            Escape => 4,
            Number => 5,
            Constant => 6,
            Comment => 7,
            Property => 8,
            Attribute => 9,
            Operator => 10,
            Punctuation => 11,
            Variable => 12,
            Tag => 13,
        };
        self.syntax[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_unknown_falls_back_to_dark() {
        for name in THEME_NAMES {
            assert_eq!(Theme::by_name(name).name, *name);
        }
        assert_eq!(Theme::by_name("nope").name, "Forge Dark");
    }

    #[test]
    fn comments_are_dimmer_than_code_in_dark_and_brighter_in_light() {
        let luma = |c: u32| ((c >> 16) & 0xff) + ((c >> 8) & 0xff) + (c & 0xff);
        let dark = Theme::dark();
        assert!(luma(dark.syntax_color(HighlightClass::Comment)) < luma(dark.syntax_color(HighlightClass::Variable)));
        let light = Theme::light();
        assert!(luma(light.syntax_color(HighlightClass::Comment)) > luma(light.syntax_color(HighlightClass::Variable)));
    }

    #[test]
    fn every_color_fits_in_24_bits() {
        for t in [Theme::dark(), Theme::light(), Theme::high_contrast()] {
            for c in [t.bg, t.sidebar, t.tab_active, t.border, t.text, t.dim, t.caret, t.selection, t.accent, t.error, t.warning, t.info, t.find_match, t.find_current, t.bracket_match, t.line_highlight, t.popup, t.git_added, t.git_modified, t.git_deleted].into_iter().chain(t.syntax) {
                assert!(c <= 0xffffff, "{} has an out-of-range color {c:#x}", t.name);
            }
        }
    }
}
