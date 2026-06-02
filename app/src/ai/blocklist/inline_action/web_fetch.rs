use warp_core::ui::appearance::Appearance;
use warpui::elements::shimmering_text::ShimmeringTextStateHandle;
use warpui::elements::{Container, CrossAxisAlignment, Element, Flex, ParentElement, Text};
use warpui::{AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext};

use super::search_results_common::{
    render_collapsible_search_results, CollapsibleSearchResultsState,
};
use crate::ai::agent::WebFetchStatus;
use crate::ai::blocklist::block::view_impl::WithContentItemSpacing;
use crate::ui_components::spinner::SpinnerStateHandle;

pub enum WebFetchViewEvent {}

#[derive(Clone, Debug)]
pub enum WebFetchViewAction {
    ToggleExpanded,
}

pub struct WebFetchView {
    pub status: WebFetchStatus,
    pub collapsible: CollapsibleSearchResultsState,
    /// Double animation state — view must be persisted. See the search_results_common annotation for details.
    shimmer_handle: ShimmeringTextStateHandle,
    spinner_handle: SpinnerStateHandle,
}

impl WebFetchView {
    pub fn new(urls: Vec<String>) -> Self {
        Self {
            status: WebFetchStatus::Fetching { urls },
            collapsible: CollapsibleSearchResultsState::new(),
            shimmer_handle: ShimmeringTextStateHandle::new(),
            spinner_handle: SpinnerStateHandle::new(),
        }
    }

    pub fn set_status(&mut self, status: &WebFetchStatus) {
        self.status = status.clone();
    }

    fn render_loading(&self, urls: &[String], app: &AppContext) -> Box<dyn Element> {
        let _ = Appearance::as_ref(app);
        // When personalization is in progress, align opencode "Fetching from the web...".
        let text = if urls.len() == 1 {
            "Fetching from the web...".to_string()
        } else {
            format!("Fetching {} web pages...", urls.len())
        };

        super::search_results_common::render_loading_header_animated(
            text,
            self.spinner_handle.clone(),
            self.shimmer_handle.clone(),
            app,
        )
    }

    fn render_success(
        &self,
        pages: &[(String, String, bool)],
        app: &AppContext,
    ) -> Box<dyn Element> {
        let successful_count = pages.iter().filter(|(_, _, success)| *success).count();
        let title_text = if successful_count == pages.len() {
            format!("Fetched {} web pages", pages.len())
        } else {
            format!("Fetched {} of {} web pages", successful_count, pages.len())
        };

        let body = if self.collapsible.is_expanded {
            Some(self.render_urls_list(pages, app))
        } else {
            None
        };

        render_collapsible_search_results(
            title_text,
            pages.len(),
            "URLs",
            &self.collapsible,
            body,
            |ctx| {
                ctx.dispatch_typed_action(WebFetchViewAction::ToggleExpanded);
            },
            app,
        )
    }

    fn render_urls_list(
        &self,
        pages: &[(String, String, bool)],
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);

        for (url, title, success) in pages {
            let display_text = if title.is_empty() {
                url.clone()
            } else {
                format!("{title} ({url})")
            };

            // Show failed URLs with a different indicator
            let display_text = if *success {
                display_text
            } else {
                format!("✗ {display_text}")
            };

            let text_color = if *success {
                appearance
                    .theme()
                    .main_text_color(appearance.theme().surface_1())
            } else {
                appearance
                    .theme()
                    .sub_text_color(appearance.theme().surface_1())
            };

            let url_text = Text::new_inline(
                display_text,
                appearance.ui_font_family(),
                appearance.monospace_font_size() - 1.0,
            )
            .with_color(text_color.into())
            .finish();

            column.add_child(Container::new(url_text).with_vertical_padding(2.).finish());
        }

        if pages.is_empty() {
            let no_results = Text::new_inline(
                "No URLs fetched".to_string(),
                appearance.ui_font_family(),
                appearance.monospace_font_size(),
            )
            .with_color(
                appearance
                    .theme()
                    .main_text_color(appearance.theme().surface_1())
                    .into(),
            )
            .finish();
            column.add_child(no_results);
        }

        column.finish()
    }
}

impl Entity for WebFetchView {
    type Event = WebFetchViewEvent;
}

impl TypedActionView for WebFetchView {
    type Action = WebFetchViewAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            WebFetchViewAction::ToggleExpanded => {
                self.collapsible.toggle_expanded();
                ctx.notify();
            }
        }
    }
}

impl View for WebFetchView {
    fn ui_name() -> &'static str {
        "WebFetchView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        match &self.status {
            WebFetchStatus::Fetching { urls } => self
                .render_loading(urls, app)
                .with_agent_output_item_spacing(app)
                .finish(),
            WebFetchStatus::Success { pages } => self
                .render_success(pages, app)
                .with_agent_output_item_spacing(app)
                .finish(),
            WebFetchStatus::Error => {
                // Render as if fetch completed with no results
                self.render_success(&[], app)
                    .with_agent_output_item_spacing(app)
                    .finish()
            }
        }
    }
}
