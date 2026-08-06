use super::CustomWeight;

#[test]
fn from_css_numeric_maps_named_steps() {
    let cases = [
        (100, Some(CustomWeight::Thin)),
        (200, Some(CustomWeight::ExtraLight)),
        (300, Some(CustomWeight::Light)),
        (400, None),
        (500, Some(CustomWeight::Medium)),
        (600, Some(CustomWeight::Semibold)),
        (700, Some(CustomWeight::Bold)),
        (800, Some(CustomWeight::ExtraBold)),
        (900, Some(CustomWeight::Black)),
    ];

    for (value, expected) in cases {
        assert_eq!(CustomWeight::from_css_numeric(value), expected);
    }
}

#[test]
fn from_css_numeric_rounds_to_nearest_hundred() {
    assert_eq!(
        CustomWeight::from_css_numeric(340),
        Some(CustomWeight::Light)
    );
    assert_eq!(
        CustomWeight::from_css_numeric(660),
        Some(CustomWeight::Bold)
    );
    assert_eq!(CustomWeight::from_css_numeric(380), None);
    assert_eq!(CustomWeight::from_css_numeric(449), None);
}

#[test]
fn from_css_numeric_clamps_out_of_range_without_overflow() {
    for value in [i32::MIN, -5, 0, 1, 50] {
        assert_eq!(
            CustomWeight::from_css_numeric(value),
            Some(CustomWeight::Thin)
        );
    }
    for value in [1000, 1_000_000, i32::MAX] {
        assert_eq!(
            CustomWeight::from_css_numeric(value),
            Some(CustomWeight::Black)
        );
    }
}
