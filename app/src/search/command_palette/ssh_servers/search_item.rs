use fuzzy_match::FuzzyMatchResult;
use ordered_float::OrderedFloat;
use warp_core::ui::theme::Fill;
use warpui::{
    elements::{Align, ConstrainedBox, Flex, Highlight, ParentElement, Shrinkable, Text},
    fonts::{Properties, Weight},
    AppContext, Element, SingletonEntity,
};

use crate::appearance::Appearance;
use crate::search::action::search_item::styles;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::command_palette::render_util;
use crate::search::item::SearchItem;
use crate::search::result_renderer::ItemHighlightState;
use crate::ui_components::icons::Icon as UiIcon;

use warp_ssh_manager::{SshNode, SshServerInfo};

#[derive(Debug)]
pub struct SshServerSearchItem {
    pub node: SshNode,
    pub server: SshServerInfo,
    /// The user@host (or just host) for display.
    pub host_user: String,
    /// The node name (used as the main label).
    pub display_name: String,
    pub match_result: FuzzyMatchResult,
}

impl SshServerSearchItem {
    pub fn new(
        node: SshNode,
        server: SshServerInfo,
        host_user: String,
        display_name: String,
    ) -> Self {
        Self {
            node,
            server,
            host_user,
            display_name,
            match_result: FuzzyMatchResult::no_match(),
        }
    }

    fn render_label(
        &self,
        item_highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        // Main label "name + gray user@host". Fuzzy match highlights the name part (the host part
        // is not highlighted and serves only as auxiliary information), so indices are only taken within the name range.
        let main_color = item_highlight_state.main_text_fill(appearance).into_solid();
        let sub_color = item_highlight_state.sub_text_fill(appearance).into_solid();

        // Note: match_result.matched_indices is relative to the index of
        // `format!("{display_name} {host_user}")` (single space). If combined is
        // separated by double spaces, the index will be offset. We re-run highlighting only on the display_name part,
        // and draw the host_user part separately as auxiliary information (which is more intuitive).
        let name_part = Text::new_inline(
            self.display_name.clone(),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(main_color)
        .with_style(Properties::default().weight(Weight::Bold))
        .with_single_highlight(
            Highlight::new()
                .with_properties(Properties::default().weight(Weight::Bold))
                .with_foreground_color(main_color),
            // Only take indices that fall within display_name (fuzzy matching matches the entire haystack,
            // but visually it is sufficient to bold and highlight only the name part; the host part style remains unchanged).
            self.match_result
                .matched_indices
                .iter()
                .copied()
                .filter(|i| *i < self.display_name.len())
                .collect(),
        )
        .finish();

        if self.host_user.is_empty() {
            return name_part;
        }

        let host_part = Text::new_inline(
            self.host_user.clone(),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(sub_color)
        .finish();

        Flex::row()
            .with_spacing(8.0)
            .with_child(name_part)
            .with_child(host_part)
            .finish()
    }

    fn render(
        &self,
        item_highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let label = self.render_label(item_highlight_state, appearance);
        let mut row = Flex::row();
        row.add_child(Shrinkable::new(1., Align::new(label).left().finish()).finish());
        ConstrainedBox::new(row.finish())
            .with_height(styles::SEARCH_ITEM_HEIGHT)
            .finish()
    }
}

impl SearchItem for SshServerSearchItem {
    type Action = CommandPaletteItemAction;

    fn render_icon(
        &self,
        highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let icon_color: Fill = appearance.theme().terminal_colors().normal.cyan.into();
        render_util::render_search_item_icon(
            appearance,
            UiIcon::Key,
            icon_color.into_solid(),
            highlight_state,
        )
    }

    fn render_item(
        &self,
        highlight_state: ItemHighlightState,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        self.render(highlight_state, appearance)
    }

    fn render_details(&self, _ctx: &AppContext) -> Option<Box<dyn Element>> {
        None
    }

    fn score(&self) -> OrderedFloat<f64> {
        OrderedFloat(self.match_result.score as f64)
    }

    fn accept_result(&self) -> CommandPaletteItemAction {
        CommandPaletteItemAction::OpenSshServer {
            node_id: self.node.id.clone(),
            server: self.server.clone(),
        }
    }

    fn execute_result(&self) -> CommandPaletteItemAction {
        self.accept_result()
    }

    fn accessibility_label(&self) -> String {
        format!("SSH server: {} {}", self.display_name, self.host_user)
    }
}
