//! Custom Agent Provider settings panel widget.
//!
//! UI Layout:
//! - Sub-header (Title on the left + `+ Add Provider` small button on top right) + brief description
//! - One card per provider, containing:
//!   · `Name` / `Base URL` / `API Key` three input boxes (edit only, no auto-save)
//!   · Models list area: header `Display Name | Model ID`, two input boxes per row + `×` delete button
//!   · Bottom button row: `+ Add Model` `Fetch from API` `Save` `Remove` (provider)
//!
//! **Save Behavior**: Clicking the "Save" button will dispatch the form state all at once to `AISettings`
//! and `AgentProviderSecrets`. Input box blur / pressing Enter will not trigger a save — this is to
//! prevent users from having their edits "implicitly submitted" while changing them. Structural operations that rebuild the page
//! (adding/deleting model rows, adding/deleting header rows, API protocol chips, model capability chips) will first submit the current card draft,
//! and then perform the original operation, preventing loss of unsaved input upon rebuilding.
//!
//! When the provider list size or the number of models for a provider changes,
//! `AISettingsPageView::rebuild_current_page` will be triggered to rebuild the entire widget,
//! so that added/deleted items get their own EditorView handles.
//! `rebuild_current_page` internally reuses the vertical scroll handle of the old PageType,
//! so the scroll position is not reset.
//!
//! Provider metadata (name/base_url/models) goes to `settings.toml`,
//! `api_key` goes to the OS keychain (`AgentProviderSecrets`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use settings::Setting;
use warpui::elements::{
    ChildView, Container, CornerRadius, CrossAxisAlignment, Expanded, Flex, MainAxisAlignment,
    MouseStateHandle, ParentElement, Radius, Text, Wrap,
};
use warpui::ui_components::{
    button::ButtonVariant,
    components::{Coords, UiComponent, UiComponentStyles},
};
use warpui::{AppContext, Element, SingletonEntity, ViewContext, ViewHandle};

use crate::ai::agent_providers::AgentProviderSecrets;
use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, SingleLineEditorOptions, TextColors, TextOptions,
};
use crate::settings::{AISettings, AgentProvider, AgentProviderApiType, AgentProviderModel};
use strum::IntoEnumIterator;

use super::ai_page::{AISettingsPageAction, AISettingsPageView, ModelCapabilityKind};
use super::settings_page::{build_sub_header, SettingsWidget, HEADER_PADDING};

const CARD_BUTTON_PADDING: f32 = 6.0;
const FIELD_LABEL_MARGIN_TOP: f32 = 6.0;
const FIELD_LABEL_MARGIN_BOTTOM: f32 = 2.0;
const MODEL_ROW_GAP: f32 = 6.0;

// ---------------------------------------------------------------------------
// Model row expansion state (process-local, thread_local single-thread UI safe; not persisted)
// ---------------------------------------------------------------------------

std::thread_local! {
    /// {provider_id => Set<model_index>} Currently expanded model items.
    /// Lost when closing the settings page; behaves similarly to `models_dev::chips_expanded()`'s AtomicBool.
    static EXPANDED_MODELS: RefCell<HashMap<String, HashSet<usize>>> = RefCell::new(HashMap::new());
}

pub(super) fn is_model_expanded(provider_id: &str, model_index: usize) -> bool {
    EXPANDED_MODELS.with(|m| {
        m.borrow()
            .get(provider_id)
            .is_some_and(|set| set.contains(&model_index))
    })
}

pub(super) fn toggle_model_expanded(provider_id: &str, model_index: usize) {
    EXPANDED_MODELS.with(|m| {
        let mut map = m.borrow_mut();
        let set = map.entry(provider_id.to_string()).or_default();
        if !set.insert(model_index) {
            set.remove(&model_index);
        }
    });
}

/// Clear expansion records for a provider when it is deleted to avoid index drift.
pub(super) fn clear_expanded_models_for_provider(provider_id: &str) {
    EXPANDED_MODELS.with(|m| {
        m.borrow_mut().remove(provider_id);
    });
}

/// Editable view handle for a model item (name + id + context + output).
struct ModelRow {
    name_editor: ViewHandle<EditorView>,
    id_editor: ViewHandle<EditorView>,
    context_editor: ViewHandle<EditorView>,
    output_editor: ViewHandle<EditorView>,
    /// Delete button inside the detail panel.
    remove_button_state: MouseStateHandle,
    /// Quick delete button to the right of the chevron at the end of the row.
    quick_remove_button_state: MouseStateHandle,
    /// Expansion/collapse chevron at the end of the row.
    expand_button_state: MouseStateHandle,
    /// Mouse state of the image/pdf/audio three-state chips inside the detail panel.
    image_chip_state: MouseStateHandle,
    pdf_chip_state: MouseStateHandle,
    audio_chip_state: MouseStateHandle,
    /// State of the reasoning / tool_call boolean toggles inside the detail panel.
    reasoning_chip_state: MouseStateHandle,
    tool_call_chip_state: MouseStateHandle,
}

struct HeaderRow {
    key_editor: ViewHandle<EditorView>,
    val_editor: ViewHandle<EditorView>,
    remove_button_state: MouseStateHandle,
}

/// All editable view handles for a provider row.
struct ProviderRow {
    name_editor: ViewHandle<EditorView>,
    base_url_editor: ViewHandle<EditorView>,
    api_key_editor: ViewHandle<EditorView>,
    fetch_button_state: MouseStateHandle,
    sync_models_dev_button_state: MouseStateHandle,
    save_button_state: MouseStateHandle,
    remove_button_state: MouseStateHandle,
    add_model_button_state: MouseStateHandle,
    header_rows: Vec<HeaderRow>,
    add_header_button_state: MouseStateHandle,
    /// Mouse states for each of the 5 ApiType chips. HashMap is mapped by chip display name.
    api_type_chip_states: RefCell<HashMap<AgentProviderApiType, MouseStateHandle>>,
    model_rows: Vec<ModelRow>,
}

type ModelDraftEditorHandles = (
    usize,
    ViewHandle<EditorView>,
    ViewHandle<EditorView>,
    ViewHandle<EditorView>,
    ViewHandle<EditorView>,
);

#[derive(Clone)]
struct ProviderDraftEditors {
    provider_id: String,
    name_editor: ViewHandle<EditorView>,
    base_url_editor: ViewHandle<EditorView>,
    api_key_editor: ViewHandle<EditorView>,
    header_editors: Vec<(ViewHandle<EditorView>, ViewHandle<EditorView>)>,
    model_editors: Vec<ModelDraftEditorHandles>,
}

impl ProviderDraftEditors {
    fn from_row(provider_id: String, row: &ProviderRow) -> Self {
        Self {
            provider_id,
            name_editor: row.name_editor.clone(),
            base_url_editor: row.base_url_editor.clone(),
            api_key_editor: row.api_key_editor.clone(),
            header_editors: row
                .header_rows
                .iter()
                .map(|h| (h.key_editor.clone(), h.val_editor.clone()))
                .collect(),
            model_editors: row
                .model_rows
                .iter()
                .enumerate()
                .map(|(idx, m)| {
                    (
                        idx,
                        m.name_editor.clone(),
                        m.id_editor.clone(),
                        m.context_editor.clone(),
                        m.output_editor.clone(),
                    )
                })
                .collect(),
        }
    }

    fn to_save_action(&self, app: &AppContext) -> AISettingsPageAction {
        self.to_save_action_with(
            app,
            |provider_id, name, base_url, api_key, headers, models| {
                AISettingsPageAction::SaveAgentProviderEdits {
                    provider_id,
                    name,
                    base_url,
                    api_key,
                    headers,
                    models,
                }
            },
        )
    }

    fn to_save_then_action(
        &self,
        app: &AppContext,
        action: AISettingsPageAction,
    ) -> AISettingsPageAction {
        self.to_save_action_with(
            app,
            |provider_id, name, base_url, api_key, headers, models| {
                AISettingsPageAction::SaveAgentProviderEditsThen {
                    provider_id,
                    name,
                    base_url,
                    api_key,
                    headers,
                    models,
                    action: Box::new(action),
                }
            },
        )
    }

    fn to_save_action_with(
        &self,
        app: &AppContext,
        build: impl FnOnce(
            String,
            String,
            String,
            String,
            Vec<(String, String)>,
            Vec<(usize, String, String, u32, u32)>,
        ) -> AISettingsPageAction,
    ) -> AISettingsPageAction {
        let name = self.name_editor.as_ref(app).buffer_text(app);
        let base_url = self.base_url_editor.as_ref(app).buffer_text(app);
        let api_key = self.api_key_editor.as_ref(app).buffer_text(app);
        let headers: Vec<(String, String)> = self
            .header_editors
            .iter()
            .map(|(k, v)| {
                (
                    k.as_ref(app).buffer_text(app),
                    v.as_ref(app).buffer_text(app),
                )
            })
            .collect();
        let models: Vec<(usize, String, String, u32, u32)> = self
            .model_editors
            .iter()
            .map(|(idx, name_e, id_e, ctx_e, out_e)| {
                let m_name = name_e.as_ref(app).buffer_text(app);
                let m_id = id_e.as_ref(app).buffer_text(app);
                let context_window = parse_token_count(&ctx_e.as_ref(app).buffer_text(app));
                let max_output_tokens = parse_token_count(&out_e.as_ref(app).buffer_text(app));
                (*idx, m_name, m_id, context_window, max_output_tokens)
            })
            .collect();

        build(
            self.provider_id.clone(),
            name,
            base_url,
            api_key,
            headers,
            models,
        )
    }
}

/// Custom Agent Provider settings widget.
pub(super) struct AgentProvidersWidget {
    add_button_state: MouseStateHandle,
    refresh_catalog_button_state: MouseStateHandle,
    expand_chips_button_state: MouseStateHandle,
    /// Search box for the quick add chip row.
    search_editor: ViewHandle<EditorView>,
    /// One button state per catalog provider id — used by the chip row.
    quick_add_button_states: RefCell<HashMap<String, MouseStateHandle>>,
    rows: RefCell<HashMap<String, ProviderRow>>,
}

impl AgentProvidersWidget {
    pub(super) fn new(ctx: &mut ViewContext<AISettingsPageView>) -> Self {
        let providers = AISettings::as_ref(ctx).agent_providers.value().clone();
        let mut rows = HashMap::with_capacity(providers.len());
        for provider in &providers {
            let row = Self::build_row(provider, ctx);
            rows.insert(provider.id.clone(), row);
        }

        // Trigger a catalog load (disk cache + network if necessary) upon entering the page.
        ctx.dispatch_typed_action_deferred(AISettingsPageAction::EnsureModelsDevLoaded);

        // ---- Search Box ----
        let initial_query = crate::ai::agent_providers::models_dev::search_query();
        let search_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text(
                crate::t!("settings-agent-providers-search-placeholder"),
                ctx,
            );
            if !initial_query.is_empty() {
                editor.set_buffer_text(&initial_query, ctx);
            }
            editor
        });
        ctx.subscribe_to_view(&search_editor, move |_, editor, event, ctx| {
            if matches!(event, EditorEvent::Edited(_)) {
                let buffer_text = editor.as_ref(ctx).buffer_text(ctx);
                ctx.dispatch_typed_action_deferred(AISettingsPageAction::SetModelsDevSearchQuery(
                    buffer_text,
                ));
            }
        });

        Self {
            add_button_state: MouseStateHandle::default(),
            refresh_catalog_button_state: MouseStateHandle::default(),
            expand_chips_button_state: MouseStateHandle::default(),
            search_editor,
            quick_add_button_states: RefCell::new(HashMap::new()),
            rows: RefCell::new(rows),
        }
    }

    /// Construct the EditorView and subscriptions for a single model row.
    fn build_model_row(
        model: &AgentProviderModel,
        ctx: &mut ViewContext<AISettingsPageView>,
    ) -> ModelRow {
        // ---- name editor ----
        let initial_name = model.name.clone();
        let name_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text(
                crate::t!("settings-agent-providers-model-name-placeholder"),
                ctx,
            );
            if !initial_name.is_empty() {
                editor.set_buffer_text(&initial_name, ctx);
            }
            editor
        });
        // Only responsible for collapsing selection on blur; no longer saves implicitly, saving goes through the bottom "Save" button.
        ctx.subscribe_to_view(&name_editor, move |_, editor, event, ctx| {
            collapse_selection_if_blurred(&editor, event, ctx);
        });

        // ---- id editor ----
        let initial_id = model.id.clone();
        let id_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text(
                crate::t!("settings-agent-providers-model-id-placeholder"),
                ctx,
            );
            if !initial_id.is_empty() {
                editor.set_buffer_text(&initial_id, ctx);
            }
            editor
        });
        ctx.subscribe_to_view(&id_editor, move |_, editor, event, ctx| {
            collapse_selection_if_blurred(&editor, event, ctx);
        });

        // ---- context_window editor (number, empty = 0 = unspecified) ----
        let initial_context = if model.context_window == 0 {
            String::new()
        } else {
            model.context_window.to_string()
        };
        let context_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text(
                crate::t!("settings-agent-providers-model-context-placeholder"),
                ctx,
            );
            if !initial_context.is_empty() {
                editor.set_buffer_text(&initial_context, ctx);
            }
            editor
        });
        ctx.subscribe_to_view(&context_editor, move |_, editor, event, ctx| {
            collapse_selection_if_blurred(&editor, event, ctx);
        });

        // ---- max_output_tokens editor ----
        let initial_output = if model.max_output_tokens == 0 {
            String::new()
        } else {
            model.max_output_tokens.to_string()
        };
        let output_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text(
                crate::t!("settings-agent-providers-model-output-placeholder"),
                ctx,
            );
            if !initial_output.is_empty() {
                editor.set_buffer_text(&initial_output, ctx);
            }
            editor
        });
        ctx.subscribe_to_view(&output_editor, move |_, editor, event, ctx| {
            collapse_selection_if_blurred(&editor, event, ctx);
        });

        ModelRow {
            name_editor,
            id_editor,
            context_editor,
            output_editor,
            remove_button_state: MouseStateHandle::default(),
            quick_remove_button_state: MouseStateHandle::default(),
            expand_button_state: MouseStateHandle::default(),
            image_chip_state: MouseStateHandle::default(),
            pdf_chip_state: MouseStateHandle::default(),
            audio_chip_state: MouseStateHandle::default(),
            reasoning_chip_state: MouseStateHandle::default(),
            tool_call_chip_state: MouseStateHandle::default(),
        }
    }

    /// Construct the HeaderRow's EditorView and subscriptions.
    fn build_header_row(
        key: &str,
        value: &str,
        ctx: &mut ViewContext<AISettingsPageView>,
    ) -> HeaderRow {
        let initial_key = key.to_owned();
        let key_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("x-portkey-provider", ctx);
            if !initial_key.is_empty() {
                editor.set_buffer_text(&initial_key, ctx);
            }
            editor
        });

        let initial_value = value.to_owned();
        let val_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("openai", ctx);
            if !initial_value.is_empty() {
                editor.set_buffer_text(&initial_value, ctx);
            }
            editor
        });

        // Saving header rows also goes through the bottom "Save" button; this only handles collapsing selection on blur.
        // (header_index / provider_id / val_editor will still be read dynamically as `HeaderRow` in build_row.)
        ctx.subscribe_to_view(&key_editor, move |_, editor, event, ctx| {
            collapse_selection_if_blurred(&editor, event, ctx);
        });

        ctx.subscribe_to_view(&val_editor, move |_, editor, event, ctx| {
            collapse_selection_if_blurred(&editor, event, ctx);
        });

        HeaderRow {
            key_editor,
            val_editor,
            remove_button_state: MouseStateHandle::default(),
        }
    }

    /// Construct all view handles and button mouse states for a provider.
    fn build_row(
        provider: &AgentProvider,
        ctx: &mut ViewContext<AISettingsPageView>,
    ) -> ProviderRow {
        let provider_id = provider.id.clone();

        // ---- Name Editor ----
        let initial_name = provider.name.clone();
        let name_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor
                .set_placeholder_text(crate::t!("settings-agent-providers-name-placeholder"), ctx);
            if !initial_name.is_empty() {
                editor.set_buffer_text(&initial_name, ctx);
            }
            editor
        });
        // Only responsible for collapsing selection on blur; saving goes through the bottom "Save" button.
        ctx.subscribe_to_view(&name_editor, move |_, editor, event, ctx| {
            collapse_selection_if_blurred(&editor, event, ctx);
        });

        // ---- Base URL Editor ----
        let initial_base_url = provider.base_url.clone();
        let base_url_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text(
                crate::t!("settings-agent-providers-base-url-placeholder"),
                ctx,
            );
            if !initial_base_url.is_empty() {
                editor.set_buffer_text(&initial_base_url, ctx);
            }
            editor
        });
        ctx.subscribe_to_view(&base_url_editor, move |_, editor, event, ctx| {
            collapse_selection_if_blurred(&editor, event, ctx);
        });

        // ---- API Key Editor (password mode) ----
        let initial_api_key = AgentProviderSecrets::as_ref(ctx)
            .get(&provider_id)
            .map(str::to_owned)
            .unwrap_or_default();
        let api_key_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, true);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text(
                crate::t!("settings-agent-providers-api-key-placeholder"),
                ctx,
            );
            if !initial_api_key.is_empty() {
                editor.set_buffer_text(&initial_api_key, ctx);
            }
            editor
        });
        ctx.subscribe_to_view(&api_key_editor, move |_, editor, event, ctx| {
            collapse_selection_if_blurred(&editor, event, ctx);
        });

        // ---- Model Rows ----
        let model_rows: Vec<ModelRow> = provider
            .models
            .iter()
            .map(|m| Self::build_model_row(m, ctx))
            .collect();

        let header_rows: Vec<HeaderRow> = provider
            .extra_headers
            .iter()
            .map(|(k, v)| Self::build_header_row(k, v, ctx))
            .collect();
        let add_header_button_state = MouseStateHandle::default();

        ProviderRow {
            name_editor,
            base_url_editor,
            api_key_editor,
            fetch_button_state: MouseStateHandle::default(),
            sync_models_dev_button_state: MouseStateHandle::default(),
            save_button_state: MouseStateHandle::default(),
            remove_button_state: MouseStateHandle::default(),
            add_model_button_state: MouseStateHandle::default(),
            header_rows,
            add_header_button_state,
            api_type_chip_states: RefCell::new(HashMap::new()),
            model_rows,
        }
    }

    /// Renders the "API Type" row: 5 chips arranged horizontally, with the currently selected one highlighted.
    /// Clicking a chip dispatches `SetAgentProviderApiType`, and the backend will pre-fill the default endpoint.
    fn render_api_type_field(
        &self,
        provider: &AgentProvider,
        row: &ProviderRow,
        draft_editors: ProviderDraftEditors,
        label_color: warp_core::ui::theme::Fill,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let label_text = Container::new(
            Text::new(
                crate::t!("settings-agent-providers-field-api-type"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(label_color.into())
            .finish(),
        )
        .with_margin_top(FIELD_LABEL_MARGIN_TOP)
        .with_margin_bottom(FIELD_LABEL_MARGIN_BOTTOM)
        .finish();

        let mut chip_row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        {
            let mut states = row.api_type_chip_states.borrow_mut();
            for variant in AgentProviderApiType::iter() {
                let state = states.entry(variant).or_default().clone();
                let is_selected = provider.api_type == variant;
                let label = if is_selected {
                    format!("● {}", variant.display_name())
                } else {
                    variant.display_name().to_owned()
                };
                let chip = Self::render_card_button_preserving_draft(
                    label,
                    state,
                    draft_editors.clone(),
                    AISettingsPageAction::SetAgentProviderApiType {
                        provider_id: provider.id.clone(),
                        api_type: variant,
                    },
                    appearance,
                );
                chip_row = chip_row.with_child(Container::new(chip).with_margin_right(6.).finish());
            }
        }

        let hint_text = Container::new(
            Text::new(
                crate::t!(
                    "settings-agent-providers-api-type-hint",
                    url = provider.api_type.default_base_url()
                ),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(appearance.theme().disabled_ui_text_color().into())
            .soft_wrap(true)
            .finish(),
        )
        .with_margin_top(2.)
        .finish();

        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(label_text)
            .with_child(chip_row.finish())
            .with_child(hint_text)
            .finish()
    }

    fn render_card_button(
        label: impl Into<String>,
        mouse_state: MouseStateHandle,
        action: AISettingsPageAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        appearance
            .ui_builder()
            .button(ButtonVariant::Secondary, mouse_state)
            .with_style(UiComponentStyles {
                font_size: Some(appearance.ui_font_body()),
                padding: Some(Coords::uniform(CARD_BUTTON_PADDING)),
                ..Default::default()
            })
            .with_centered_text_label(label.into())
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(action.clone());
            })
            .finish()
    }

    fn render_card_button_preserving_draft(
        label: impl Into<String>,
        mouse_state: MouseStateHandle,
        draft_editors: ProviderDraftEditors,
        action: AISettingsPageAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        appearance
            .ui_builder()
            .button(ButtonVariant::Secondary, mouse_state)
            .with_style(UiComponentStyles {
                font_size: Some(appearance.ui_font_body()),
                padding: Some(Coords::uniform(CARD_BUTTON_PADDING)),
                ..Default::default()
            })
            .with_centered_text_label(label.into())
            .build()
            .on_click(move |ctx, app, _| {
                ctx.dispatch_typed_action(draft_editors.to_save_then_action(app, action.clone()));
            })
            .finish()
    }

    fn render_model_row(
        provider: &AgentProvider,
        index: usize,
        model: &AgentProviderModel,
        row: &ModelRow,
        draft_editors: ProviderDraftEditors,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let provider_id = provider.id.as_str();
        let is_expanded = is_model_expanded(provider_id, index);

        // chevron: expand ▾ / collapse ▸. Reuses the visual style of render_card_button.
        let chevron_label = if is_expanded { "▾" } else { "▸" };
        let chevron_button = Self::render_card_button_preserving_draft(
            chevron_label,
            row.expand_button_state.clone(),
            draft_editors.clone(),
            AISettingsPageAction::ToggleAgentProviderModelExpanded {
                provider_id: provider.id.clone(),
                model_index: index,
            },
            appearance,
        );
        let quick_remove_button = Self::render_card_button_preserving_draft(
            "×",
            row.quick_remove_button_state.clone(),
            draft_editors.clone(),
            AISettingsPageAction::RemoveAgentProviderModel {
                provider_id: provider.id.clone(),
                model_index: index,
            },
            appearance,
        );
        let row_controls = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Container::new(chevron_button)
                    .with_margin_right(MODEL_ROW_GAP)
                    .finish(),
            )
            .with_child(quick_remove_button)
            .finish();

        let cell = |flex: f32, view: &ViewHandle<EditorView>| -> Box<dyn Element> {
            Expanded::new(
                flex,
                Container::new(ChildView::new(view).finish())
                    .with_margin_right(MODEL_ROW_GAP)
                    .finish(),
            )
            .finish()
        };

        let header_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(cell(2., &row.name_editor))
            .with_child(cell(2., &row.id_editor))
            .with_child(cell(1., &row.context_editor))
            .with_child(cell(1., &row.output_editor))
            .with_child(row_controls)
            .finish();

        let mut col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header_row);

        if is_expanded {
            col = col.with_child(Self::render_model_detail_panel(
                provider,
                index,
                model,
                row,
                draft_editors,
                appearance,
            ));
        }

        Container::new(col.finish())
            .with_margin_bottom(MODEL_ROW_GAP)
            .finish()
    }

    /// Expanded detail panel for a single model:
    /// - Modalities: image / pdf / audio three-state chips (Auto / On / Off)
    /// - Capabilities: reasoning / tool_call two boolean chips
    /// - Bottom Remove button
    fn render_model_detail_panel(
        provider: &AgentProvider,
        index: usize,
        model: &AgentProviderModel,
        row: &ModelRow,
        draft_editors: ProviderDraftEditors,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let label_color = theme.active_ui_text_color();

        // ---- Modalities Section ----
        let modalities_label = Container::new(
            Text::new(
                "Modalities".to_string(),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(label_color.into())
            .finish(),
        )
        .with_margin_top(FIELD_LABEL_MARGIN_TOP)
        .with_margin_bottom(FIELD_LABEL_MARGIN_BOTTOM)
        .finish();

        let modality_chip = |label: &str,
                             slot: Option<bool>,
                             state: MouseStateHandle,
                             kind: ModelCapabilityKind|
         -> Box<dyn Element> {
            // Three-state visuals: Auto = bare label / On = `● label` / Off = `○ label`.
            // Follows the `● {label}` selected style of existing ApiType / ReasoningEffort chips,
            // Off uses an empty circle ○ to contrast with the solid ●, and Auto has no prefix (same as unselected state).
            let chip_label = match slot {
                None => label.to_string(),
                Some(true) => format!("● {label}"),
                Some(false) => format!("○ {label}"),
            };
            Self::render_card_button_preserving_draft(
                chip_label,
                state,
                draft_editors.clone(),
                AISettingsPageAction::CycleAgentProviderModelCapability {
                    provider_id: provider.id.clone(),
                    model_index: index,
                    kind,
                },
                appearance,
            )
        };

        let modalities_row = Wrap::row()
            .with_spacing(6.)
            .with_run_spacing(4.)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(modality_chip(
                "Image",
                model.image,
                row.image_chip_state.clone(),
                ModelCapabilityKind::Image,
            ))
            .with_child(modality_chip(
                "PDF",
                model.pdf,
                row.pdf_chip_state.clone(),
                ModelCapabilityKind::Pdf,
            ))
            .with_child(modality_chip(
                "Audio",
                model.audio,
                row.audio_chip_state.clone(),
                ModelCapabilityKind::Audio,
            ))
            .finish();

        // ---- Capabilities Section (reasoning / tool_call) ----
        let capabilities_label = Container::new(
            Text::new(
                "Capabilities".to_string(),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(label_color.into())
            .finish(),
        )
        .with_margin_top(FIELD_LABEL_MARGIN_TOP)
        .with_margin_bottom(FIELD_LABEL_MARGIN_BOTTOM)
        .finish();

        let bool_chip = |label: &str,
                         on: bool,
                         state: MouseStateHandle,
                         action: AISettingsPageAction|
         -> Box<dyn Element> {
            let chip_label = if on {
                format!("● {label}")
            } else {
                format!("○ {label}")
            };
            Self::render_card_button_preserving_draft(
                chip_label,
                state,
                draft_editors.clone(),
                action,
                appearance,
            )
        };

        let capabilities_row = Wrap::row()
            .with_spacing(6.)
            .with_run_spacing(4.)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(bool_chip(
                "Reasoning",
                model.reasoning,
                row.reasoning_chip_state.clone(),
                AISettingsPageAction::ToggleAgentProviderModelReasoning {
                    provider_id: provider.id.clone(),
                    model_index: index,
                },
            ))
            .with_child(bool_chip(
                "Tool Calling",
                model.tool_call,
                row.tool_call_chip_state.clone(),
                AISettingsPageAction::ToggleAgentProviderModelToolCall {
                    provider_id: provider.id.clone(),
                    model_index: index,
                },
            ))
            .finish();

        // ---- Remove Button (only appears after expansion to avoid accidental deletion in collapsed state) ----
        let remove_button = Self::render_card_button_preserving_draft(
            "Remove model",
            row.remove_button_state.clone(),
            draft_editors,
            AISettingsPageAction::RemoveAgentProviderModel {
                provider_id: provider.id.clone(),
                model_index: index,
            },
            appearance,
        );

        let remove_row = Container::new(
            Flex::row()
                .with_main_axis_alignment(MainAxisAlignment::End)
                .with_child(remove_button)
                .finish(),
        )
        .with_margin_top(FIELD_LABEL_MARGIN_TOP)
        .finish();

        // The overall detail panel uses a slightly indented + bordered style to distinguish it from the main row.
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(modalities_label)
                .with_child(modalities_row)
                .with_child(capabilities_label)
                .with_child(capabilities_row)
                .with_child(remove_row)
                .finish(),
        )
        .with_margin_top(4.)
        .with_margin_left(12.)
        .with_margin_bottom(8.)
        .finish()
    }

    fn render_provider_card(
        &self,
        provider: &AgentProvider,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_any_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
        let label_color = if is_any_ai_enabled {
            appearance.theme().active_ui_text_color()
        } else {
            appearance.theme().disabled_ui_text_color()
        };
        let detail_color = if is_any_ai_enabled {
            appearance.theme().foreground()
        } else {
            appearance.theme().disabled_ui_text_color()
        };

        let rows = self.rows.borrow();
        let row = match rows.get(&provider.id) {
            Some(row) => row,
            None => {
                return Container::new(
                    Text::new(
                        crate::t!(
                            "settings-agent-providers-row-missing",
                            id = provider.id.as_str()
                        ),
                        appearance.ui_font_family(),
                        appearance.ui_font_size(),
                    )
                    .with_color(detail_color.into())
                    .finish(),
                )
                .with_margin_bottom(8.)
                .finish();
            }
        };
        let draft_editors = ProviderDraftEditors::from_row(provider.id.clone(), row);

        let name_field = field_block(
            &crate::t!("settings-agent-providers-field-name"),
            ChildView::new(&row.name_editor).finish(),
            label_color,
            appearance,
        );
        let api_type_field = self.render_api_type_field(
            provider,
            row,
            draft_editors.clone(),
            label_color,
            appearance,
        );
        let base_url_field = field_block(
            &crate::t!("settings-agent-providers-field-base-url"),
            ChildView::new(&row.base_url_editor).finish(),
            label_color,
            appearance,
        );
        let api_key_field = field_block(
            &crate::t!("settings-agent-providers-field-api-key"),
            ChildView::new(&row.api_key_editor).finish(),
            label_color,
            appearance,
        );

        let headers_label = Container::new(
            Text::new(
                "Extra Headers".to_string(),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(label_color.into())
            .finish(),
        )
        .with_margin_top(FIELD_LABEL_MARGIN_TOP)
        .with_margin_bottom(FIELD_LABEL_MARGIN_BOTTOM)
        .finish();
        let mut headers_column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(headers_label);

        for (idx, h_row) in row.header_rows.iter().enumerate() {
            let remove_header_button = Self::render_card_button_preserving_draft(
                "×",
                h_row.remove_button_state.clone(),
                draft_editors.clone(),
                AISettingsPageAction::RemoveAgentProviderHeader {
                    provider_id: provider.id.clone(),
                    header_index: idx,
                },
                appearance,
            );
            let header_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Expanded::new(
                        1.,
                        Container::new(ChildView::new(&h_row.key_editor).finish())
                            .with_margin_right(MODEL_ROW_GAP)
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.,
                        Container::new(ChildView::new(&h_row.val_editor).finish())
                            .with_margin_right(MODEL_ROW_GAP)
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(remove_header_button)
                .finish();
            headers_column.add_child(
                Container::new(header_row)
                    .with_margin_bottom(MODEL_ROW_GAP)
                    .finish(),
            );
        }

        let add_header_button = Self::render_card_button_preserving_draft(
            "+ Add Header",
            row.add_header_button_state.clone(),
            draft_editors.clone(),
            AISettingsPageAction::AddAgentProviderHeader {
                provider_id: provider.id.clone(),
            },
            appearance,
        );
        headers_column.add_child(add_header_button);

        // ---- Model List Section ----
        let models_label = Container::new(
            Text::new(
                crate::t!(
                    "settings-agent-providers-models-label",
                    count = provider.models.len()
                ),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(label_color.into())
            .finish(),
        )
        .with_margin_top(FIELD_LABEL_MARGIN_TOP)
        .with_margin_bottom(FIELD_LABEL_MARGIN_BOTTOM)
        .finish();

        let mut models_column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(models_label);

        if provider.models.is_empty() {
            let empty_hint = Container::new(
                Text::new(
                    crate::t!("settings-agent-providers-models-empty-hint"),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(appearance.theme().disabled_ui_text_color().into())
                .soft_wrap(true)
                .finish(),
            )
            .with_margin_bottom(MODEL_ROW_GAP)
            .finish();
            models_column.add_child(empty_hint);
        } else {
            // Header: Display Name | Model ID | Context | Output
            let dim = appearance.theme().disabled_ui_text_color();
            let header_cell = |flex: f32, label: &str| -> Box<dyn Element> {
                Expanded::new(
                    flex,
                    Container::new(
                        Text::new(
                            label.to_string(),
                            appearance.ui_font_family(),
                            appearance.ui_font_size(),
                        )
                        .with_color(dim.into())
                        .finish(),
                    )
                    .with_margin_right(MODEL_ROW_GAP)
                    .finish(),
                )
                .finish()
            };
            let header = Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(header_cell(
                        2.,
                        &crate::t!("settings-agent-providers-models-header-name"),
                    ))
                    .with_child(header_cell(
                        2.,
                        &crate::t!("settings-agent-providers-models-header-id"),
                    ))
                    .with_child(header_cell(
                        1.,
                        &crate::t!("settings-agent-providers-models-header-context"),
                    ))
                    .with_child(header_cell(
                        1.,
                        &crate::t!("settings-agent-providers-models-header-output"),
                    ))
                    // Placeholder, aligned with the expand/delete buttons below.
                    .with_child(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_child(
                                Container::new(
                                    Text::new(
                                        "  ".to_string(),
                                        appearance.ui_font_family(),
                                        appearance.ui_font_size(),
                                    )
                                    .with_color(dim.into())
                                    .finish(),
                                )
                                .with_margin_right(MODEL_ROW_GAP)
                                .finish(),
                            )
                            .with_child(
                                Text::new(
                                    "  ".to_string(),
                                    appearance.ui_font_family(),
                                    appearance.ui_font_size(),
                                )
                                .with_color(dim.into())
                                .finish(),
                            )
                            .finish(),
                    )
                    .finish(),
            )
            .with_margin_bottom(2.)
            .finish();
            models_column.add_child(header);

            for (idx, m_row) in row.model_rows.iter().enumerate() {
                let model = match provider.models.get(idx) {
                    Some(m) => m,
                    // Edge case: settings modified during rebuild gap, model_rows and provider.models
                    // temporarily differ in length; skip to avoid panic, it will naturally self-correct in the next frame.
                    None => continue,
                };
                models_column.add_child(Self::render_model_row(
                    provider,
                    idx,
                    model,
                    m_row,
                    draft_editors.clone(),
                    appearance,
                ));
            }
        }

        // ---- Bottom Button Row ----
        let add_model_button = Self::render_card_button_preserving_draft(
            crate::t!("settings-agent-providers-add-model"),
            row.add_model_button_state.clone(),
            draft_editors.clone(),
            AISettingsPageAction::AddAgentProviderModel {
                provider_id: provider.id.clone(),
            },
            appearance,
        );
        let fetch_button = Self::render_card_button_preserving_draft(
            crate::t!("settings-agent-providers-fetch-from-api"),
            row.fetch_button_state.clone(),
            draft_editors.clone(),
            AISettingsPageAction::FetchAgentProviderModels {
                provider_id: provider.id.clone(),
            },
            appearance,
        );
        let sync_models_dev_button = Self::render_card_button_preserving_draft(
            crate::t!("settings-agent-providers-sync-models-dev"),
            row.sync_models_dev_button_state.clone(),
            draft_editors.clone(),
            AISettingsPageAction::SyncProviderModelsFromModelsDev {
                provider_id: provider.id.clone(),
            },
            appearance,
        );
        let remove_button = Self::render_card_button(
            crate::t!("settings-agent-providers-remove"),
            row.remove_button_state.clone(),
            AISettingsPageAction::RemoveAgentProvider {
                provider_id: provider.id.clone(),
            },
            appearance,
        );

        // ---- Save button: dynamically read all form buffers in the on_click closure.
        // The action cannot be pre-built here (as form values change with input), so draft editor handles
        // are captured in the closure, and SaveAgentProviderEdits is dispatched upon clicking.
        let save_button = {
            let draft_editors = draft_editors.clone();

            appearance
                .ui_builder()
                .button(ButtonVariant::Accent, row.save_button_state.clone())
                .with_style(UiComponentStyles {
                    font_size: Some(appearance.ui_font_body()),
                    padding: Some(Coords::uniform(CARD_BUTTON_PADDING)),
                    ..Default::default()
                })
                .with_centered_text_label(crate::t!("settings-agent-providers-save"))
                .build()
                .on_click(move |ctx, app, _| {
                    ctx.dispatch_typed_action(draft_editors.to_save_action(app));
                })
                .finish()
        };

        let bottom_row = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Container::new(add_model_button)
                            .with_margin_right(8.)
                            .finish(),
                    )
                    .with_child(Container::new(fetch_button).with_margin_right(8.).finish())
                    .with_child(sync_models_dev_button)
                    .finish(),
            )
            .with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(Container::new(save_button).with_margin_right(8.).finish())
                        .with_child(remove_button)
                        .finish(),
                )
                // Substantially spaced from the left main action group (add model / fetch / sync),
                // to prevent the two groups from sticking together under SpaceBetween when the card width is insufficient.
                .with_margin_left(16.)
                .finish(),
            )
            .finish();

        // Trigger reading of detail_color (using transparent) to avoid unused warnings; only used for potential coloring.
        let _ = detail_color;

        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(name_field)
                .with_child(api_type_field)
                .with_child(base_url_field)
                .with_child(api_key_field)
                .with_child(
                    Container::new(headers_column.finish())
                        .with_margin_top(8.)
                        .finish(),
                )
                .with_child(
                    Container::new(models_column.finish())
                        .with_margin_top(8.)
                        .finish(),
                )
                .with_child(Container::new(bottom_row).with_margin_top(10.).finish())
                .finish(),
        )
        .with_background(appearance.theme().surface_1())
        .with_uniform_padding(12.)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .with_margin_bottom(8.)
        .finish()
    }
}

/// Parses user input into a token count. Tolerates `128k` / `128K` / `128 000` / `128,000` / whitespace,
/// returns 0 upon parsing failure (semantics: unspecified).
fn parse_token_count(input: &str) -> u32 {
    let cleaned: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',' && *c != '_')
        .collect();
    if cleaned.is_empty() {
        return 0;
    }
    let lower = cleaned.to_lowercase();
    let (num_part, multiplier): (&str, u64) = if let Some(stripped) = lower.strip_suffix('k') {
        (stripped, 1_000)
    } else if let Some(stripped) = lower.strip_suffix('m') {
        (stripped, 1_000_000)
    } else {
        (lower.as_str(), 1)
    };
    num_part
        .parse::<f64>()
        .ok()
        .map(|n| (n * multiplier as f64).round() as u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0)
}

/// Collapses the editor selection to the end on blur.
///
/// Each input box is an independent `EditorView` maintaining its own selection range.
/// Drawing of the selection highlight is unaffected by focus state (see `app/src/editor/view/element.rs:1091`),
/// so after double-clicking/triple-clicking/dragging selection and blurring, the old selection will remain in the buffer,
/// displaying simultaneously with selections of other editors, resembling "multiple selected states".
/// Here, on Blurred, both head/tail are collapsed to the end to visually release the selection.
fn collapse_selection_if_blurred(
    editor: &ViewHandle<EditorView>,
    event: &EditorEvent,
    ctx: &mut ViewContext<AISettingsPageView>,
) {
    if matches!(event, EditorEvent::Blurred) {
        editor.update(ctx, |editor, ctx| editor.move_to_buffer_end(ctx));
    }
}

fn single_line_editor_options(
    appearance: &Appearance,
    is_password: bool,
) -> SingleLineEditorOptions {
    SingleLineEditorOptions {
        is_password,
        clear_selections_on_blur: true,
        text: TextOptions {
            font_size_override: Some(appearance.ui_font_size()),
            font_family_override: Some(appearance.monospace_font_family()),
            text_colors_override: Some(TextColors {
                default_color: appearance.theme().active_ui_text_color(),
                disabled_color: appearance.theme().disabled_ui_text_color(),
                hint_color: appearance.theme().disabled_ui_text_color(),
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn field_block(
    label: &str,
    editor_element: Box<dyn Element>,
    label_color: warp_core::ui::theme::Fill,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let label_text = Container::new(
        Text::new(
            label.to_string(),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(label_color.into())
        .finish(),
    )
    .with_margin_top(FIELD_LABEL_MARGIN_TOP)
    .with_margin_bottom(FIELD_LABEL_MARGIN_BOTTOM)
    .finish();

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(label_text)
        .with_child(editor_element)
        .finish()
}

impl AgentProvidersWidget {
    /// Renders the "Quick add known provider from models.dev" section:
    /// - Title + "Refresh Catalog" button
    /// - A row of chips (each corresponding to a catalog provider id), clicking creates a new local provider and pre-fills models
    /// - Displays "Fetching..." when the catalog is not yet loaded
    fn render_models_dev_section(
        &self,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        use crate::ai::agent_providers::models_dev;

        let label_color = appearance.theme().active_ui_text_color();
        let dim_color = appearance.theme().disabled_ui_text_color();

        let title = Text::new(
            crate::t!("settings-agent-providers-quick-add-title"),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(label_color.into())
        .finish();

        let refresh_button = Self::render_card_button(
            crate::t!("settings-agent-providers-refresh-catalog"),
            self.refresh_catalog_button_state.clone(),
            AISettingsPageAction::RefreshModelsDev,
            appearance,
        );

        let search_box = Container::new(ChildView::new(&self.search_editor).finish())
            .with_margin_left(8.)
            .with_margin_right(8.)
            .finish();

        let header_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(title)
            .with_child(Expanded::new(1., search_box).finish())
            .with_child(refresh_button)
            .finish();

        let mut body = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        body.add_child(header_row);

        // Displays the first N items when collapsed (enough to fill about 1 row — actual line wrapping is handled by the Wrap layout).
        const COLLAPSED_LIMIT: usize = 8;
        let expanded = models_dev::chips_expanded();

        match models_dev::cached() {
            None => {
                body.add_child(
                    Container::new(
                        Text::new(
                            crate::t!("settings-agent-providers-loading-catalog"),
                            appearance.ui_font_family(),
                            appearance.ui_font_size(),
                        )
                        .with_color(dim_color.into())
                        .finish(),
                    )
                    .with_margin_top(4.)
                    .finish(),
                );
            }
            Some(catalog) if catalog.is_empty() => {
                body.add_child(
                    Container::new(
                        Text::new(
                            crate::t!("settings-agent-providers-catalog-empty"),
                            appearance.ui_font_family(),
                            appearance.ui_font_size(),
                        )
                        .with_color(dim_color.into())
                        .finish(),
                    )
                    .with_margin_top(4.)
                    .finish(),
                );
            }
            Some(catalog) => {
                // Filter by search query; empty query -> keep all items in order.
                let query = models_dev::search_query();
                let filtered = models_dev::filter_catalog(&catalog, &query);
                let total = filtered.len();
                let has_query = !query.trim().is_empty();
                // Always expand all matches when search is active, do not collapse (otherwise, matching results ≤ collapse limit won't be fully visible).
                let visible_count = if expanded || has_query {
                    total
                } else {
                    COLLAPSED_LIMIT.min(total)
                };

                let mut wrap = Wrap::row()
                    .with_spacing(6.)
                    .with_run_spacing(6.)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center);
                {
                    let mut states = self.quick_add_button_states.borrow_mut();
                    for (cat_id, cat_provider) in filtered.iter().take(visible_count) {
                        let label = if cat_provider.name.is_empty() {
                            cat_id.clone()
                        } else {
                            cat_provider.name.clone()
                        };
                        let state = states.entry(cat_id.clone()).or_default().clone();
                        let model_count = cat_provider.models.len();
                        let display_label = format!("+ {label} ({model_count})");
                        let chip = Self::render_card_button(
                            display_label,
                            state,
                            AISettingsPageAction::AddProviderFromModelsDev {
                                catalog_provider_id: cat_id.clone(),
                            },
                            appearance,
                        );
                        wrap = wrap.with_child(chip);
                    }
                }
                body.add_child(Container::new(wrap.finish()).with_margin_top(4.).finish());

                if has_query && total == 0 {
                    body.add_child(
                        Container::new(
                            Text::new(
                                crate::t!(
                                    "settings-agent-providers-no-match",
                                    query = query.as_str()
                                ),
                                appearance.ui_font_family(),
                                appearance.ui_font_size(),
                            )
                            .with_color(dim_color.into())
                            .finish(),
                        )
                        .with_margin_top(4.)
                        .finish(),
                    );
                }

                // Expand/collapse button (only displayed when there is no search and catalog exceeds the collapse limit).
                if !has_query && total > COLLAPSED_LIMIT {
                    let toggle_label = if expanded {
                        crate::t!("settings-agent-providers-collapse")
                    } else {
                        let count: i64 = (total - COLLAPSED_LIMIT) as i64;
                        crate::t!("settings-agent-providers-expand-remaining", count = count)
                    };
                    let toggle_button = Self::render_card_button(
                        toggle_label,
                        self.expand_chips_button_state.clone(),
                        AISettingsPageAction::ToggleModelsDevChipsExpanded,
                        appearance,
                    );
                    body.add_child(
                        Container::new(
                            Flex::row()
                                .with_main_axis_alignment(MainAxisAlignment::Start)
                                .with_child(toggle_button)
                                .finish(),
                        )
                        .with_margin_top(6.)
                        .finish(),
                    );
                }
            }
        }

        Container::new(body.finish())
            .with_background(appearance.theme().surface_1())
            .with_uniform_padding(10.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .with_margin_bottom(10.)
            .finish()
    }
}

impl SettingsWidget for AgentProvidersWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "agent provider providers custom openai compatible deepseek glm moonshot dashscope qwen ollama base url api key models save provider custom model save"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_any_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
        let providers = AISettings::as_ref(app).agent_providers.value().clone();

        let title_node = build_sub_header(
            appearance,
            crate::t!("settings-agent-providers-title"),
            Some(if is_any_ai_enabled {
                appearance.theme().active_ui_text_color()
            } else {
                appearance.theme().disabled_ui_text_color()
            }),
        )
        .finish();

        let header_add_button = Self::render_card_button(
            crate::t!("settings-agent-providers-add-button"),
            self.add_button_state.clone(),
            AISettingsPageAction::AddAgentProvider,
            appearance,
        );

        let header = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(Expanded::new(1., title_node).finish())
                .with_child(header_add_button)
                .finish(),
        )
        .with_padding_bottom(HEADER_PADDING)
        .finish();

        let description_text = crate::t!("settings-agent-providers-description");
        let description = Container::new(
            Text::new(
                description_text,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(if is_any_ai_enabled {
                appearance.theme().foreground().into()
            } else {
                appearance.theme().disabled_ui_text_color().into()
            })
            .soft_wrap(true)
            .finish(),
        )
        .with_margin_bottom(12.)
        .finish();

        let mut column = Flex::column().with_child(header).with_child(description);

        // ---- Quick add chip row from models.dev ----
        column.add_child(self.render_models_dev_section(appearance, app));

        if providers.is_empty() {
            let empty = Container::new(
                Text::new(
                    crate::t!("settings-agent-providers-empty"),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(appearance.theme().disabled_ui_text_color().into())
                .finish(),
            )
            .with_margin_bottom(12.)
            .finish();
            column.add_child(empty);
        } else {
            for provider in &providers {
                column.add_child(self.render_provider_card(provider, appearance, app));
            }
        }

        Container::new(column.finish())
            .with_margin_bottom(HEADER_PADDING)
            .finish()
    }
}
