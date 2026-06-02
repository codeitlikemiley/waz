use pathfinder_color::ColorU;
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    Border, ConstrainedBox, Container, CornerRadius, Expanded, Flex, MainAxisAlignment,
    MainAxisSize, ParentElement as _, Percentage, Radius, Rect, Stack, Text,
};
use warpui::prelude::{Align, CrossAxisAlignment};
use warpui::text_layout::ClipConfig;
use warpui::{AppContext, Element, SingletonEntity as _};

use crate::ai::llms::LLMSpec;
use crate::appearance::Appearance;
use crate::terminal::input::inline_menu::styles as inline_styles;

const CORNER_RADIUS: f32 = 4.0;
const ROW_SPACING: f32 = 12.0;

pub enum CostRow {
    Bar { value: Option<f32> },
    BilledToApi { manage_button: Box<dyn Element> },
}

pub struct ModelSpecScoresLayout {
    pub bg_bar_color: ColorU,
}

/// Spec panel rendered for BYOP (custom provider) models.
///
/// Visually identical to [`render_model_spec_scores`] (same `render_score_row` private helper),
/// Only the semantics of the lines are different:
/// - Context — context window, bar is mapped to 4K..2M using log2 normalization
/// - Output — Single maximum output, bar is mapped to 1K..128K using log2 normalization
/// - Cost — Force the `BilledToApi` branch (BYOP users use their own keys and do not use Waz billing)
///
/// When `context_window` / `max_output_tokens` is 0 (unfilled), pass None and display the default "?" placeholder.
/// Consistent with the visual behavior of Waz's default panel when data is missing.
pub fn render_byop_spec_scores(
    context_window: Option<u32>,
    max_output_tokens: Option<u32>,
    manage_button: Box<dyn Element>,
    layout: ModelSpecScoresLayout,
    app: &AppContext,
) -> Box<dyn Element> {
    let rows = vec![
        render_score_row(
            "Context",
            ScoreRowKind::Bar {
                value: context_window.map(normalize_context_window),
            },
            layout.bg_bar_color,
            app,
        ),
        render_score_row(
            "Output",
            ScoreRowKind::Bar {
                value: max_output_tokens.map(normalize_max_output),
            },
            layout.bg_bar_color,
            app,
        ),
        render_score_row(
            "Cost",
            ScoreRowKind::BilledToApi { manage_button },
            layout.bg_bar_color,
            app,
        ),
    ];

    Flex::column()
        .with_spacing(ROW_SPACING)
        .with_children(rows)
        .finish()
}

/// log2 normalization: 4K..2M tokens → 0..1. 0 / Out of bounds controlled by caller with `Option<u32>`.
fn normalize_context_window(ctx: u32) -> f32 {
    if ctx == 0 {
        return 0.0;
    }
    let l = (ctx as f32).log2();
    let lo = 12.0; // log2(4096) = 4K
    let hi = 21.0; // log2(2 097 152) ≈ 2M
    ((l - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// log2 normalization: 1K..128K tokens → 0..1.
fn normalize_max_output(out: u32) -> f32 {
    if out == 0 {
        return 0.0;
    }
    let l = (out as f32).log2();
    let lo = 10.0; // log2(1024) = 1K
    let hi = 17.0; // log2(131 072) = 128K
    ((l - lo) / (hi - lo)).clamp(0.0, 1.0)
}

pub fn render_model_spec_scores(
    spec: Option<&LLMSpec>,
    cost_row: CostRow,
    layout: ModelSpecScoresLayout,
    app: &AppContext,
) -> Box<dyn Element> {
    let mut rows = vec![render_score_row(
        "Intelligence",
        ScoreRowKind::Bar {
            value: spec.as_ref().map(|spec| spec.quality),
        },
        layout.bg_bar_color,
        app,
    )];

    rows.push(render_score_row(
        "Speed",
        ScoreRowKind::Bar {
            value: spec.as_ref().map(|spec| spec.speed),
        },
        layout.bg_bar_color,
        app,
    ));

    match cost_row {
        CostRow::Bar { value } => {
            rows.push(render_score_row(
                "Cost",
                ScoreRowKind::Bar { value },
                layout.bg_bar_color,
                app,
            ));
        }
        CostRow::BilledToApi { manage_button } => {
            rows.push(render_score_row(
                "Cost",
                ScoreRowKind::BilledToApi { manage_button },
                layout.bg_bar_color,
                app,
            ));
        }
    }

    Flex::column()
        .with_spacing(ROW_SPACING)
        .with_children(rows)
        .finish()
}

enum ScoreRowKind {
    Bar { value: Option<f32> },
    BilledToApi { manage_button: Box<dyn Element> },
}

fn render_score_row(
    name: &str,
    kind: ScoreRowKind,
    bg_bar_color: ColorU,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();

    // Approximate the required width for the longest label ("Intelligence"), and use this as a
    // consistent width for the labels so the labels and bars are vertically aligned.
    //
    // 8 ems is enough space for Intelligence with some right margin to spare.
    let label_width = app.font_cache().em_width(
        appearance.ui_font_family(),
        appearance.monospace_font_size(),
    ) * 8.;
    let label = ConstrainedBox::new(
        Text::new(
            name.to_string(),
            appearance.ui_font_family(),
            appearance.monospace_font_size(),
        )
        .with_color(
            inline_styles::primary_text_color(
                theme,
                inline_styles::menu_background_color(app).into(),
            )
            .into_solid(),
        )
        .finish(),
    )
    .with_width(label_width)
    .finish();

    let bar_height = app.font_cache().line_height(
        appearance.monospace_font_size(),
        appearance.line_height_ratio(),
    );

    let row_content: Box<dyn Element> = match kind {
        ScoreRowKind::Bar { value: Some(value) } => {
            let background_bar = Rect::new()
                .with_background_color(bg_bar_color)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CORNER_RADIUS)))
                .finish();

            let filled_bar = Rect::new()
                .with_background_color(internal_colors::neutral_6(theme))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CORNER_RADIUS)))
                .finish();

            Expanded::new(
                1.,
                ConstrainedBox::new(
                    Stack::new()
                        .with_child(background_bar)
                        .with_child(Percentage::width(value, filled_bar).finish())
                        .finish(),
                )
                .with_height(bar_height)
                .finish(),
            )
            .finish()
        }
        ScoreRowKind::Bar { value: None } => {
            let background_bar = Rect::new()
                .with_background_color(bg_bar_color)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CORNER_RADIUS)))
                .finish();

            Expanded::new(
                1.,
                ConstrainedBox::new(
                    Stack::new()
                        .with_child(background_bar)
                        .with_child(
                            Align::new(
                                Text::new(
                                    "?",
                                    appearance.ui_font_family(),
                                    inline_styles::font_size(appearance),
                                )
                                .with_color(
                                    appearance
                                        .theme()
                                        .disabled_text_color(bg_bar_color.into())
                                        .into_solid(),
                                )
                                .finish(),
                            )
                            .finish(),
                        )
                        .finish(),
                )
                .with_height(bar_height)
                .finish(),
            )
            .finish()
        }
        ScoreRowKind::BilledToApi { manage_button } => Expanded::new(
            1.,
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Container::new(
                        Text::new(
                            "Billed to API".to_string(),
                            appearance.ui_font_family(),
                            14.,
                        )
                        .with_color(theme.disabled_ui_text_color().into())
                        .finish(),
                    )
                    .finish(),
                )
                .with_child(manage_button)
                .finish(),
        )
        .finish(),
    };

    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(label)
        .with_child(Expanded::new(1., row_content).finish())
        .finish()
}

pub fn render_model_spec_header(
    title: &str,
    description: &str,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();

    let title = Text::new(
        title.to_string(),
        appearance.ui_font_family(),
        appearance.monospace_font_size(),
    )
    .with_color(
        inline_styles::primary_text_color(theme, inline_styles::menu_background_color(app).into())
            .into_solid(),
    )
    .with_clip(ClipConfig::ellipsis())
    .finish();

    let description = Text::new(
        description.to_string(),
        appearance.ui_font_family(),
        inline_styles::font_size(appearance),
    )
    .with_color(theme.disabled_ui_text_color().into())
    .finish();

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(title)
            .with_child(Container::new(description).with_margin_top(4.).finish())
            .finish(),
    )
    .with_padding_bottom(12.)
    .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
    .finish()
}
