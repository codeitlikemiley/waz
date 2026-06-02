//! Code footer view.
//!
//! Historically, this view hosted the LSP status indicator / Enable LSP CTA / service management menu
//! and the "/update-tab-config skill" entry point in the bottom right corner of the TabConfig editor.
//! After the LSP full stack was retired, this view only has the TabConfig mode remaining — displaying
//! a static information tooltip and a button to "trigger /update-tab-config skill" when editing tab config TOML.
//!
//! In normal source code / workspace editing scenarios, `CodeFooterView` is no longer constructed (see
//! `code/local_code_editor.rs`), and the entire view completely disappears on those paths.

use std::path::{Path, PathBuf};

use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::theme::WarpTheme;
use warp_core::ui::{appearance::Appearance, Icon};
use warpui::elements::{
    ChildView, ConstrainedBox, Container, CrossAxisAlignment, Flex, MainAxisAlignment,
    MainAxisSize, ParentElement, Shrinkable,
};
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::{
    elements::{Border, Fill},
    AppContext, Element, Entity, SingletonEntity, View,
};
use warpui::{TypedActionView, ViewContext, ViewHandle};

use crate::settings::AISettings;
#[cfg(feature = "local_fs")]
use crate::user_config::is_tab_config_toml;
use crate::view_components::action_button::{
    ActionButton, ButtonSize, NakedTheme, PaneHeaderTheme,
};

const FOOTER_HEIGHT: f32 = 24.;
/// Margin for the information icon.
const ICON_MARGIN: f32 = 4.;

/// Which mode the current footer is in — now only TabConfig is left,
/// other source code / workspace scenarios no longer construct `CodeFooterView`.
enum FooterMode {
    TabConfig { path: PathBuf },
}

impl FooterMode {
    fn path(&self) -> &Path {
        match self {
            FooterMode::TabConfig { path } => path,
        }
    }
}

#[derive(Debug, Clone)]
pub enum CodeFooterViewAction {
    /// Button in the bottom right of the TabConfig editor triggers the `/update-tab-config` skill.
    RunTabConfigSkill,
}

#[derive(Debug, Clone)]
pub enum CodeFooterViewEvent {
    /// Passed through to `LocalCodeEditorView` to trigger the `/update-tab-config` skill.
    RunTabConfigSkill { path: PathBuf },
}

pub struct CodeFooterView {
    mode: FooterMode,
    tab_config_skill_button: ViewHandle<ActionButton>,
    /// Whether to draw the top separator line — reusing existing caller conventions.
    show_border: bool,
}

impl CodeFooterView {
    /// This view should only be constructed when `path` is a tab config TOML file.
    /// The caller (`LocalCodeEditorView::add_footer`) is responsible for checking beforehand using
    /// [`is_tab_config_path`](Self::is_tab_config_path).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn new(path: PathBuf, ctx: &mut ViewContext<Self>) -> Self {
        let tab_config_skill_button = Self::create_tab_config_skill_button(ctx);

        let mut footer = Self {
            mode: FooterMode::TabConfig { path },
            tab_config_skill_button,
            show_border: true,
        };
        footer.sync_tab_config_skill_button(ctx);
        ctx.subscribe_to_model(&AISettings::handle(ctx), |me, _, _, ctx| {
            me.sync_tab_config_skill_button(ctx);
        });
        footer
    }

    /// Whether the current footer corresponds to a tab config file — used by the caller for conditional construction decisions.
    #[cfg(feature = "local_fs")]
    pub fn is_tab_config_path(path: &Path) -> bool {
        is_tab_config_toml(path)
    }

    /// Under non-local_fs builds, the tab config concept is unavailable, uniformly returning false.
    #[cfg(not(feature = "local_fs"))]
    pub fn is_tab_config_path(_path: &Path) -> bool {
        false
    }

    fn create_tab_config_skill_button(ctx: &mut ViewContext<Self>) -> ViewHandle<ActionButton> {
        ctx.add_typed_action_view(|_ctx| {
            ActionButton::new("/update-tab-config", NakedTheme)
                .with_icon(Icon::Oz)
                .with_size(ButtonSize::Small)
                .with_disabled_theme(PaneHeaderTheme)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(CodeFooterViewAction::RunTabConfigSkill);
                })
        })
    }

    fn sync_tab_config_skill_button(&mut self, ctx: &mut ViewContext<Self>) {
        let is_ai_enabled = AISettings::as_ref(ctx).is_any_ai_enabled(ctx);
        self.tab_config_skill_button.update(ctx, |button, ctx| {
            button.set_disabled(!is_ai_enabled, ctx);
            button.set_tooltip(
                Some(if is_ai_enabled {
                    "Open agent input with the /update-tab-config skill"
                } else {
                    "Enable AI to use the /update-tab-config skill"
                }),
                ctx,
            );
        });
    }

    fn render_tab_config_info_icon(theme: &WarpTheme) -> Box<dyn Element> {
        Container::new(
            ConstrainedBox::new(
                Icon::Info
                    .to_warpui_icon(theme.active_ui_text_color())
                    .finish(),
            )
            .with_width(12.)
            .with_height(12.)
            .finish(),
        )
        .with_margin_left(ICON_MARGIN)
        .finish()
    }

    fn render_status_text(
        theme: &WarpTheme,
        appearance: &Appearance,
        message: String,
    ) -> Box<dyn Element> {
        let status_content = appearance
            .ui_builder()
            .span(message)
            .with_style(UiComponentStyles {
                font_family_id: Some(appearance.ui_font_family()),
                font_color: Some(internal_colors::text_sub(theme, theme.background())),
                font_size: Some(12.0),
                ..Default::default()
            })
            .build()
            .finish();

        Container::new(status_content)
            .with_margin_left(ICON_MARGIN)
            .finish()
    }

    /// Explicitly control whether to draw the top border for the host — keeping compatibility with the original signature.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn set_show_border(&mut self, show: bool) {
        self.show_border = show;
    }
}

impl Entity for CodeFooterView {
    type Event = CodeFooterViewEvent;
}

impl View for CodeFooterView {
    fn ui_name() -> &'static str {
        "CodeFooterView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let mut footer_content = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max);

        footer_content.add_child(Self::render_tab_config_info_icon(theme));
        footer_content.add_child(
            Shrinkable::new(
                1.,
                Self::render_status_text(
                    theme,
                    appearance,
                    "Use Oz to update this config".to_string(),
                ),
            )
            .finish(),
        );
        footer_content.add_child(ChildView::new(&self.tab_config_skill_button).finish());

        let mut container = Container::new(
            ConstrainedBox::new(footer_content.finish())
                .with_height(FOOTER_HEIGHT)
                .finish(),
        )
        .with_background(Fill::Solid(theme.background().into()));

        if self.show_border {
            container = container.with_border(Border::top(1.).with_border_fill(theme.outline()));
        }

        container.finish()
    }
}

impl TypedActionView for CodeFooterView {
    type Action = CodeFooterViewAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            CodeFooterViewAction::RunTabConfigSkill => {
                ctx.emit(CodeFooterViewEvent::RunTabConfigSkill {
                    path: self.mode.path().to_path_buf(),
                });
            }
        }
    }
}
