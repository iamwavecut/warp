use warpui_core::elements::Icon as WarpUiIcon;

use crate::ui::theme::Fill;

#[derive(Clone, Copy)]
pub enum ExternalProductIcon {
    Heroku,
    Notion,
    Figma,
    Github,
    Composio,
    Resend,
}

impl ExternalProductIcon {
    const PREFIXES: &'static [(&'static str, Self)] = &[
        ("heroku", Self::Heroku),
        ("notion", Self::Notion),
        ("figma", Self::Figma),
        ("github", Self::Github),
        ("composio", Self::Composio),
        ("resend", Self::Resend),
    ];

    pub fn from_string(s: &str) -> Option<Self> {
        let s_lower = s.to_ascii_lowercase();
        Self::PREFIXES
            .iter()
            .find(|(prefix, _)| s_lower.starts_with(prefix))
            .map(|(_, icon)| *icon)
    }

    pub fn get_path(&self) -> &'static str {
        match self {
            Self::Heroku => "bundled/svg/heroku.svg",
            Self::Notion => "bundled/svg/notion.svg",
            Self::Figma => "bundled/svg/figma.svg",
            Self::Github => "bundled/svg/github.svg",
            Self::Composio => "bundled/svg/composio.svg",
            Self::Resend => "bundled/svg/resend.svg",
        }
    }

    pub fn to_warpui_icon(&self, color: Fill) -> WarpUiIcon {
        let path = self.get_path();
        WarpUiIcon::new(path, color.into_solid())
    }
}

#[cfg(test)]
#[path = "external_product_icon_tests.rs"]
mod tests;
