use super::*;
use crate::ui::theme::mock_terminal_colors;
use warpui::color::ColorU;

fn mock_appearance() -> Appearance {
    use crate::ui::theme::Details;
    use super::super::theme::Fill;

    let theme = WarpTheme::new(
        Fill::Solid(ColorU::from_u32(0x000000ff)),
        ColorU::from_u32(0xffffffff),
        Fill::Solid(ColorU::new(18, 123, 156, 255)),
        None,
        Some(Details::Darker),
        mock_terminal_colors(),
        None,
        Some("Dark".to_string()),
    );
    Appearance::new(
        theme,
        FamilyId(0),
        13.0,
        Weight::Normal,
        FamilyId(1),
        1.4,
        FamilyId(0),
        FamilyId(0),
        DEFAULT_UI_FONT_SIZE,
    )
}

/// Author: logic
/// Test for UI font size semantic methods
#[test]
fn test_semantic_font_sizes_at_default() {
    let appearance = mock_appearance();
    assert_eq!(appearance.ui_font_size(), 12.0);

    assert_eq!(appearance.ui_font_overline(), 10.0);
    assert_eq!(appearance.ui_font_footnote(), 11.0);
    assert_eq!(appearance.ui_font_body(), 12.0);
    assert_eq!(appearance.ui_font_body_large(), 13.0);
    assert_eq!(appearance.ui_font_subheading(), 14.0);
    assert_eq!(appearance.ui_font_heading_3(), 16.0);
    assert_eq!(appearance.ui_font_heading_2(), 18.0);
    assert_eq!(appearance.ui_font_heading_1(), 20.0);
    assert_eq!(appearance.ui_font_display(), 24.0);
    assert_eq!(appearance.ui_font_hero(), 36.0);
}

/// Author: logic
/// Verify that header_font_size and overline_font_size also scale with ui_font_size
#[test]
fn test_header_and_overline_font_size_scaling() {
    let appearance = mock_appearance();
    assert_eq!(appearance.header_font_size(), 18.0);
    assert_eq!(appearance.overline_font_size(), 10.0);
}

/// Author: logic
/// Verify proportional scaling after setting ui_font_size to its minimum value
#[test]
fn test_semantic_font_sizes_at_minimum() {
    let mut appearance = mock_appearance();
    appearance.set_ui_font_size_test(8.0);

    assert_eq!(appearance.ui_font_size(), 8.0);
    assert_eq!(appearance.ui_font_overline(), 8.0 * 10.0 / 12.0);
    assert_eq!(appearance.ui_font_footnote(), 8.0 * 11.0 / 12.0);
    assert_eq!(appearance.ui_font_body(), 8.0);
    assert_eq!(appearance.ui_font_body_large(), 8.0 * 13.0 / 12.0);
    assert_eq!(appearance.ui_font_subheading(), 8.0 * 14.0 / 12.0);
    assert_eq!(appearance.ui_font_heading_3(), 8.0 * 16.0 / 12.0);
    assert_eq!(appearance.ui_font_heading_2(), 8.0 * 18.0 / 12.0);
    assert_eq!(appearance.ui_font_heading_1(), 8.0 * 20.0 / 12.0);
    assert_eq!(appearance.ui_font_display(), 8.0 * 24.0 / 12.0);
    assert_eq!(appearance.ui_font_hero(), 8.0 * 36.0 / 12.0);
    assert_eq!(appearance.header_font_size(), 8.0 * 18.0 / 12.0);
    assert_eq!(appearance.overline_font_size(), 8.0 * 10.0 / 12.0);
}

/// Author: logic
/// Verify scaling at the maximum value (20.0)
#[test]
fn test_semantic_font_sizes_at_maximum() {
    let mut appearance = mock_appearance();
    appearance.set_ui_font_size_test(20.0);

    assert_eq!(appearance.ui_font_size(), 20.0);
    assert_eq!(appearance.ui_font_overline(), 20.0 * 10.0 / 12.0);
    assert_eq!(appearance.ui_font_body(), 20.0);
    assert_eq!(appearance.ui_font_subheading(), 20.0 * 14.0 / 12.0);
    assert_eq!(appearance.ui_font_heading_3(), 20.0 * 16.0 / 12.0);
    assert_eq!(appearance.ui_font_display(), 20.0 * 24.0 / 12.0);
}

/// Author: logic
/// Verify default constants
#[test]
fn test_default_constants() {
    assert_eq!(DEFAULT_UI_FONT_SIZE, 12.0);
}

/// Author: logic
/// Verify that ui_font_body equals ui_font_size (1:1 ratio)
#[test]
fn test_ui_font_body_equals_base() {
    let mut appearance = mock_appearance();
    for size in [8.0, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0] {
        appearance.set_ui_font_size_test(size);
        assert_eq!(
            appearance.ui_font_body(),
            appearance.ui_font_size(),
            "ui_font_body should equal ui_font_size at base={}",
            size
        );
    }
}

/// Author: logic
/// Verify that each semantic method maintains a strict size hierarchy relationship
#[test]
fn test_semantic_font_size_ordering() {
    let mut appearance = mock_appearance();
    for size in [8.0, 12.0, 20.0] {
        appearance.set_ui_font_size_test(size);

        let overline = appearance.ui_font_overline();
        let footnote = appearance.ui_font_footnote();
        let body = appearance.ui_font_body();
        let body_large = appearance.ui_font_body_large();
        let subheading = appearance.ui_font_subheading();
        let h3 = appearance.ui_font_heading_3();
        let h2 = appearance.ui_font_heading_2();
        let h1 = appearance.ui_font_heading_1();
        let display = appearance.ui_font_display();
        let hero = appearance.ui_font_hero();

        assert!(overline <= footnote, "overline <= footnote at base={}", size);
        assert!(footnote <= body, "footnote <= body at base={}", size);
        assert!(body <= body_large, "body <= body_large at base={}", size);
        assert!(body_large <= subheading, "body_large <= subheading at base={}", size);
        assert!(subheading <= h3, "subheading <= h3 at base={}", size);
        assert!(h3 <= h2, "h3 <= h2 at base={}", size);
        assert!(h2 <= h1, "h2 <= h1 at base={}", size);
        assert!(h1 <= display, "h1 <= display at base={}", size);
        assert!(display <= hero, "display <= hero at base={}", size);
    }
}

/// Author: logic
/// Verify that the dropdown top bar height formula is 30.0 at the default font size (12)
#[test]
fn test_dropdown_top_bar_height_at_default() {
    let appearance = mock_appearance();
    assert_eq!(appearance.dropdown_top_bar_height(), 30.0);
}

/// Author: logic
/// Verify that the dropdown top bar height scales linearly with the font size and is not less than 30.0
#[test]
fn test_dropdown_top_bar_height_scaling() {
    let mut appearance = mock_appearance();

    appearance.set_ui_font_size_test(8.0);
    assert_eq!(appearance.dropdown_top_bar_height(), 30.0, "min size should clamp to 30.0");

    appearance.set_ui_font_size_test(10.0);
    assert_eq!(appearance.dropdown_top_bar_height(), 30.0, "size 10: 10*2.5=25, clamped to 30.0");

    appearance.set_ui_font_size_test(12.0);
    assert_eq!(appearance.dropdown_top_bar_height(), 30.0, "size 12: 12*2.5=30, exactly 30.0");

    appearance.set_ui_font_size_test(16.0);
    assert_eq!(appearance.dropdown_top_bar_height(), 40.0, "size 16: 16*2.5=40");

    appearance.set_ui_font_size_test(20.0);
    assert_eq!(appearance.dropdown_top_bar_height(), 50.0, "size 20: 20*2.5=50");
}

/// Author: logic
/// Verify that the dropdown top bar height is never less than 30.0
#[test]
fn test_dropdown_top_bar_height_never_below_minimum() {
    let mut appearance = mock_appearance();
    for size in [8.0, 9.0, 10.0, 11.0, 12.0] {
        appearance.set_ui_font_size_test(size);
        assert!(
            appearance.dropdown_top_bar_height() >= 30.0,
            "height should be >= 30.0 at size={}",
            size
        );
    }
}

/// Author: logic
/// Verify the value of the dropdown top bar height at boundary font sizes (8.0 and 20.0)
#[test]
fn test_dropdown_top_bar_height_at_boundaries() {
    let mut appearance = mock_appearance();

    appearance.set_ui_font_size_test(8.0);
    assert_eq!(appearance.dropdown_top_bar_height(), 30.0);

    appearance.set_ui_font_size_test(20.0);
    assert_eq!(appearance.dropdown_top_bar_height(), 50.0);
}
