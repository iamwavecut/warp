use enum_iterator::Sequence;

/// All [`Weight`]s that are not [`Weight::Normal`] are considered custom weights.
/// Avoid importing `CustomWeight`, and prefer using [`Weight`] throughout the codebase,
/// except in cases where you want to specifically track explicit weight overrides.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Sequence)]
pub enum CustomWeight {
    Thin,
    ExtraLight,
    Light,
    Medium,
    Semibold,
    Bold,
    ExtraBold,
    Black,
}

impl CustomWeight {
    /// Maps a numeric CSS `font-weight` value to the closest named weight.
    pub fn from_css_numeric(value: i32) -> Option<CustomWeight> {
        let value = value.clamp(1, 1000);
        let bucket = (((value + 50) / 100) * 100).clamp(100, 900);
        match bucket {
            100 => Some(CustomWeight::Thin),
            200 => Some(CustomWeight::ExtraLight),
            300 => Some(CustomWeight::Light),
            400 => None,
            500 => Some(CustomWeight::Medium),
            600 => Some(CustomWeight::Semibold),
            700 => Some(CustomWeight::Bold),
            800 => Some(CustomWeight::ExtraBold),
            900 => Some(CustomWeight::Black),
            _ => None,
        }
    }

    /// Returns true if the weight is bold or heavier.
    pub fn is_at_least_bold(&self) -> bool {
        matches!(
            self,
            CustomWeight::Bold | CustomWeight::ExtraBold | CustomWeight::Black
        )
    }

    /// We do not support nested weights at this time! The outer weight will
    /// be the only respected weight.
    pub fn merge_weights(
        first: Option<CustomWeight>,
        second: Option<CustomWeight>,
    ) -> Option<CustomWeight> {
        // We don't currently support text containing text of varying weights.
        // We will just respect the outer weight if you specify a non-Normal weight.
        first.or(second)
    }
}

#[cfg(test)]
#[path = "weight_tests.rs"]
mod tests;
