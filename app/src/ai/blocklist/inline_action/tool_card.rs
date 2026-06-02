//! Unified tool card rendering helper, aligned with opencode TUI's `InlineTool` / `BlockTool`.
//!
//! ## Design Philosophy
//!
//! opencode renders each ToolPart strictly according to the 4-state machine switching style:
//! - `pending` (args are still accumulating): light gray text "Writing command..." / "Reading file..."
//! - `running`(args complete, executing): BrailleSpinner + title text
//! - `completed` (successful completion): static icon + tool description, foldable
//! - `error` (failure/rejection): red error text, full text when denied STRIKETHROUGH
//!
//! All 12 built-in tools (Bash/Read/Glob/Grep/Edit/Write/...) only use InlineTool /
//! BlockTool has two components; when a new tool is connected, only the semantics are filled in and the card skeleton is not re-implemented.
//!
//! ## warp status quo
//!
//! warp's inline_action/ directory for each view (web_search.rs / web_fetch.rs /
//! requested_command.rs / requested_action.rs / ...)each
//! Self-complete rendering card (header + body + footer + permission ring + status switching),
//! Repeat boilerplate starting at ~150 lines. This is historical baggage, **Full reconstruction requires changing 12+ views at one time**,
//! The risks are high and the resistance is high.
//!
//! This module serves as the entrance to progressive reconstruction:
//! 1. Define unified API([`ToolCardState`] state machine + [`ToolCardSpec`] builder);
//! 2. Provide two helpers [`render_inline_tool_card`] / [`render_block_tool_card`];
//! 3. **The newly added inline_action will give priority to this module**; the old view will remain unchanged until the separate PR converges.
//!
//! Currently, `render_loading_header_animated` has been added to `search_results_common.rs` /
//! `render_terminal_header_strikethrough`, this module superimposes the complete spec abstraction on it.

use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::Fill;
use warpui::elements::shimmering_text::ShimmeringTextStateHandle;
use warpui::elements::{
    ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Element, Flex, MainAxisAlignment,
    ParentElement, Radius, Shrinkable,
};
use warpui::{AppContext, SingletonEntity};

use super::inline_action_header::{
    ICON_MARGIN, INLINE_ACTION_HEADER_VERTICAL_PADDING, INLINE_ACTION_HORIZONTAL_PADDING,
};
use super::inline_action_icons::icon_size;
use crate::ui_components::spinner::SpinnerStateHandle;

/// The current status of the tool card. **Strict 5-state alignment opencode TUI**:
/// Don't add intermediate states to save the graph - all rendering branches only accept these 5 cases.
///
/// 5 state instead of opencode 4 state: more [`Self::PermissionPending`], corresponding to warp
/// `AIActionStatus::Blocked` (waiting for user permission). opencode plugs this into InlineTool
/// In the whole card fg→warning color logic, we make explicit case more clear.
#[derive(Clone)]
pub enum ToolCardState {
    /// args are still being accumulated or the tool has not actually been executed yet. Visual: static icon + "Writing command..." etc.
    /// Continuous tense phrase + light gray text.
    Pending {
        /// Progressive phrases, such as "Writing command", "Reading file". No ending `...`,
        /// Automatically added during rendering.
        verb: String,
    },
    /// The tool is executing. Visual: `BrailleSpinner` (80ms frame switching) + ShimmeringText title.
    Running {
        title: String,
        spinner_handle: SpinnerStateHandle,
        shimmer_handle: ShimmeringTextStateHandle,
    },
    /// Wait for user permission to execute (`AIActionStatus::Blocked`).
    /// Visual: **header background cut warning yellow**, keep text high contrast,
    /// Align opencode's `if (permission()) return theme.warning`.
    /// detail is usually "OK if I run this command?" / "OK if I call this MCP tool?".
    PermissionPending { title: String, detail: String },
    /// The tool completed successfully. Visual: green check icon + tool description.
    Completed { title: String },
    /// Tool failed/user rejected. When `denied=true` is used, the title text has STRIKETHROUGH strikethrough
    /// Expression "rejected", aligned opencode `<text attributes={STRIKETHROUGH}>`.
    Error {
        title: String,
        denied: bool,
        detail: Option<String>,
    },
}

impl ToolCardState {
    /// Equivalent to opencode `part.state.status === "running"`. The spinner is only displayed when Running.
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    /// Equivalent to opencode `part.state.status === "completed"`. Can be hide_completed_tool_cards
    /// setting hidden.
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }

    /// Whether it is denied (denied by the user), used to cut off the strike-through visual.
    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Error { denied: true, .. })
    }

    /// Whether it is permission pending (waiting for user permission), used to switch the warning background color.
    pub fn is_permission_pending(&self) -> bool {
        matches!(self, Self::PermissionPending { .. })
    }
}

/// Tool card spec - all necessary information filled in by the caller.
pub struct ToolCardSpec {
    /// Tool icon (for the final state, select the spinner according to the state during Pending/Running).
    pub icon: warpui::elements::Icon,
    /// Current status.
    pub state: ToolCardState,
}

/// Render inline mode tool card (single line icon + text). Align opencode `InlineTool`.
///
/// Good for short descriptions: Glob "*.rs" / Grep "TODO" / WebFetch URL.
/// **Limitations**: body height is always 1 line; complex content (diff / file list) go to [`render_block_tool_card`].
pub fn render_inline_tool_card(spec: ToolCardSpec, app: &AppContext) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    // T3-6: Permission pending uses the warning yellow background, otherwise the surface_2 default background is used.
    let header_background: Fill = if spec.state.is_permission_pending() {
        Fill::Solid(theme.ui_warning_color())
    } else {
        theme.surface_2()
    };

    let mut row = Flex::row()
        .with_main_axis_alignment(MainAxisAlignment::Start)
        .with_cross_axis_alignment(CrossAxisAlignment::Center);

    // icon: Change BrailleSpinner when running, and use the passed icon in other states.
    let icon_element: Box<dyn Element> = match &spec.state {
        ToolCardState::Running { spinner_handle, .. } => {
            use warp_core::ui::theme::AnsiColorIdentifier;
            let color = AnsiColorIdentifier::Yellow.to_ansi_color(&theme.terminal_colors().normal);
            Box::new(crate::ui_components::spinner::BrailleSpinner::new(
                appearance.ui_font_family(),
                appearance.monospace_font_size(),
                color,
                spinner_handle.clone(),
            ))
        }
        _ => spec.icon.finish(),
    };
    let icon_box = ConstrainedBox::new(icon_element)
        .with_width(icon_size(app))
        .with_height(icon_size(app))
        .finish();
    row.add_child(
        Container::new(icon_box)
            .with_margin_right(ICON_MARGIN)
            .finish(),
    );

    // Text: The four states are constructed separately.
    let title_element = build_title_text(&spec.state, header_background, app);
    row.add_child(Shrinkable::new(1.0, title_element).finish());

    Container::new(row.finish())
        .with_horizontal_padding(INLINE_ACTION_HORIZONTAL_PADDING)
        .with_vertical_padding(INLINE_ACTION_HEADER_VERTICAL_PADDING)
        .with_background(header_background)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
        .finish()
}

/// Render block mode tool card (header + body). Align opencode `BlockTool`.
///
/// header is the same as inline_tool_card; body is any Element (diff, file list,
/// output preview, etc.). When Running, the header goes to the spinner, and the body is usually in-progress data.
pub fn render_block_tool_card(
    spec: ToolCardSpec,
    body: Box<dyn Element>,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    let body_background = theme.surface_1();

    let header = render_inline_tool_card(spec, app);
    let body_container = Container::new(body)
        .with_background(body_background)
        .with_horizontal_padding(INLINE_ACTION_HORIZONTAL_PADDING)
        .with_vertical_padding(INLINE_ACTION_HEADER_VERTICAL_PADDING)
        .with_corner_radius(CornerRadius::with_bottom(Radius::Pixels(8.)))
        .finish();

    let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    col.add_child(header);
    col.add_child(body_container);
    col.finish()
}

fn build_title_text(
    state: &ToolCardState,
    header_background: Fill,
    app: &AppContext,
) -> Box<dyn Element> {
    use warpui::elements::shimmering_text::{ShimmerConfig, ShimmeringTextElement};
    use warpui::elements::Text;

    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();

    match state {
        ToolCardState::Pending { verb } => {
            let color = theme.sub_text_color(header_background).into_solid();
            Text::new_inline(
                format!("{verb}..."),
                appearance.ui_font_family(),
                appearance.monospace_font_size(),
            )
            .with_color(color)
            .finish()
        }
        ToolCardState::Running {
            title,
            shimmer_handle,
            ..
        } => {
            let base_color = theme.sub_text_color(header_background).into_solid();
            let shimmer_color = theme.main_text_color(header_background).into_solid();
            ShimmeringTextElement::new(
                title.clone(),
                appearance.ui_font_family(),
                appearance.monospace_font_size(),
                base_color,
                shimmer_color,
                ShimmerConfig::default(),
                shimmer_handle.clone(),
            )
            .finish()
        }
        ToolCardState::Completed { title } => {
            let color = theme.main_text_color(header_background).into();
            Text::new_inline(
                title.clone(),
                appearance.ui_font_family(),
                appearance.monospace_font_size(),
            )
            .with_color(color)
            .finish()
        }
        ToolCardState::PermissionPending { title, detail } => {
            // Main title + detail subline. The background has been cut to warn, and the text uses the main color to ensure contrast.
            let main_color = theme.main_text_color(header_background).into();
            let detail_color = theme.sub_text_color(header_background).into_solid();
            let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Start);
            col.add_child(
                Text::new_inline(
                    title.clone(),
                    appearance.ui_font_family(),
                    appearance.monospace_font_size(),
                )
                .with_color(main_color)
                .finish(),
            );
            col.add_child(
                Text::new_inline(
                    detail.clone(),
                    appearance.ui_font_family(),
                    (appearance.monospace_font_size() - 1.).max(10.),
                )
                .with_color(detail_color)
                .finish(),
            );
            col.finish()
        }
        ToolCardState::Error {
            title,
            denied,
            detail,
        } => {
            use warpui::elements::{Highlight, HighlightedRange};
            use warpui::text_layout::TextStyle;

            // Main text: hit STRIKETHROUGH when denied, do not hit error but use sub color + detail subline.
            let text_color = theme.sub_text_color(header_background).into_solid();
            let mut text_widget = Text::new_inline(
                title.clone(),
                appearance.ui_font_family(),
                appearance.monospace_font_size(),
            )
            .with_color(text_color);

            if *denied {
                let strike_style = TextStyle::new()
                    .with_show_strikethrough(true)
                    .with_foreground_color(text_color);
                let highlight = Highlight::default().with_text_style(strike_style);
                let len = title.chars().count();
                text_widget = text_widget.with_highlights(vec![HighlightedRange {
                    highlight,
                    highlight_indices: (0..len).collect(),
                }]);
            }

            // Detail row: If it is available, column splicing will be used; if not, it will be just one row.
            if let Some(detail_text) = detail {
                let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Start);
                col.add_child(text_widget.finish());
                let detail_color = theme.ui_error_color();
                col.add_child(
                    Text::new_inline(
                        detail_text.clone(),
                        appearance.ui_font_family(),
                        (appearance.monospace_font_size() - 1.).max(10.),
                    )
                    .with_color(detail_color)
                    .finish(),
                );
                col.finish()
            } else {
                text_widget.finish()
            }
        }
    }
}
