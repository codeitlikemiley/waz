use super::{
    settings_page::{
        render_body_item, MatchData, PageType, SettingsPageEvent, SettingsPageMeta,
        SettingsPageViewHandle, SettingsWidget,
    },
    LocalOnlyIconState, SettingsSection, ToggleState,
};
use crate::{
    appearance::Appearance,
    autoupdate::{self, github, AutoupdateStage, AutoupdateState},
    channel::ChannelState,
    report_if_error,
    settings::AutoupdateSettings,
    workspace::WorkspaceAction,
};
use settings::Setting as _;
use warp_core::{execution_mode::AppExecutionMode, settings::ToggleableSetting as _};
use warpui::ui_components::switch::SwitchStateHandle;
use warpui::{
    assets::asset_cache::AssetSource,
    elements::{
        Align, CacheOption, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, Image,
        MainAxisAlignment, MouseStateHandle, ParentElement, Wrap,
    },
    ui_components::components::UiComponent,
    AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle,
};

#[derive(Debug, Clone)]
pub enum AboutPageAction {
    ToggleAutomaticUpdates,
    /// User clicks the "Check for Updates" button: actively triggers a check (equivalent to RequestType::ManualCheck).
    CheckForUpdate,
    /// User clicks the "Go to GitHub to Download" link: opens the release page in the system's default browser.
    /// Only used in exception fallback paths (e.g., download failure / no available assets).
    OpenReleasePage(String),
    /// User clicks the "Install Now" link: dispatches to workspace, triggering an installation and restart process
    /// completely equivalent to the menu `ApplyUpdate`. See `autoupdate::apply_update` for specific platform behavior.
    InstallUpdate,
    /// User clicks the "Export Logs" link: pops up a native save-file dialog. After the user selects a save
    /// location, packages the main logs, MCP logs, autoupdater logs, and diagnostic summary into a zip
    /// and writes it directly to the path specified by the user. Feedback on success/failure is provided via workspace toast.
    /// Implemented by `WorkspaceAction::ExportLogsToPath`.
    #[cfg(not(target_family = "wasm"))]
    ExportLogs,
}

pub struct AboutPageView {
    page: PageType<Self>,
}

impl AboutPageView {
    pub fn new(ctx: &mut ViewContext<AboutPageView>) -> Self {
        // Subscribe to AutoupdateState; refreshes the UI when stage changes (checking / new version found / failed, etc.).
        let autoupdate_handle = AutoupdateState::handle(ctx);
        ctx.observe(&autoupdate_handle, |_, _, ctx| {
            ctx.notify();
        });

        AboutPageView {
            page: PageType::new_monolith(AboutPageWidget::default(), None, false),
        }
    }
}

impl Entity for AboutPageView {
    type Event = SettingsPageEvent;
}

impl TypedActionView for AboutPageView {
    type Action = AboutPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            AboutPageAction::ToggleAutomaticUpdates => {
                AutoupdateSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .automatic_updates_enabled
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AboutPageAction::CheckForUpdate => {
                AutoupdateState::handle(ctx).update(ctx, |state, ctx| {
                    state.manually_check_for_update(ctx);
                });
                ctx.notify();
            }
            AboutPageAction::OpenReleasePage(url) => {
                ctx.open_url(url);
            }
            AboutPageAction::InstallUpdate => {
                // Reuse WorkspaceAction::ApplyUpdate: it calls autoupdate::apply_update +
                // initiate_relaunch_for_update, and the platform layer determines the specific installation action inside relaunch()
                // (macOS: open dmg / Windows: non-silent installation wizard / Linux: restart new binary).
                ctx.dispatch_typed_action(&WorkspaceAction::ApplyUpdate);
            }
            #[cfg(not(target_family = "wasm"))]
            AboutPageAction::ExportLogs => {
                // Trigger workspace layer to pop up a save-file dialog, package and provide toast feedback
                // after the user chooses the save path.
                ctx.dispatch_typed_action(&WorkspaceAction::ExportLogsToPath);
            }
        }
    }
}

impl View for AboutPageView {
    fn ui_name() -> &'static str {
        "AboutPage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

#[derive(Default)]
struct AboutPageWidget {
    copy_version_button_mouse_state: MouseStateHandle,
    automatic_updates_switch_state: SwitchStateHandle,
    update_action_link_mouse_state: MouseStateHandle,
    /// Hover / pressed state of the "Export Logs" link.
    #[cfg(not(target_family = "wasm"))]
    export_logs_link_mouse_state: MouseStateHandle,
}

impl SettingsWidget for AboutPageWidget {
    type View = AboutPageView;

    fn search_terms(&self) -> &str {
        "about warp version automatic updates auto update check for update new version"
    }

    fn render(
        &self,
        _view: &AboutPageView,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ui_builder = appearance.ui_builder();

        // Always use pure icon logo, brand name is presented as independent text "Waz", no longer relying on svg with "warp" in it
        let image_path = "bundled/svg/warp-logo-light.svg";

        // GIT_RELEASE_TAG injection -> display tag; otherwise enter Dev development mode
        let version = ChannelState::app_version().unwrap_or("Dev");

        let version_text = ui_builder
            .span(version.to_string())
            .with_soft_wrap()
            .build()
            .with_margin_top(16.)
            .finish();

        let copy_version_icon = appearance
            .ui_builder()
            .copy_button(16., self.copy_version_button_mouse_state.clone())
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(WorkspaceAction::CopyVersion(version));
            })
            .finish();

        let version_row = Wrap::row()
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_children([
                version_text,
                Container::new(copy_version_icon)
                    .with_margin_top(16.)
                    .with_padding_left(6.)
                    .finish(),
            ]);

        let mut content = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                ConstrainedBox::new(
                    Image::new(
                        AssetSource::Bundled { path: image_path },
                        CacheOption::BySize,
                    )
                    .finish(),
                )
                .with_max_height(100.)
                .with_max_width(350.)
                .finish(),
            )
            .with_child(
                ui_builder
                    .span(crate::t!("app-name"))
                    .build()
                    .with_margin_top(12.)
                    .finish(),
            )
            .with_child(version_row.finish());

        // Update status area: displays whether a new version is currently available, and provides "Check for Updates" or "Go to GitHub to Download" link.
        // Only rendered under execution modes that can enter the autoupdate process (shares conditions with the "automatic updates" switch below).
        if AppExecutionMode::as_ref(app).can_autoupdate() {
            content.add_child(
                Container::new(self.render_update_status(appearance, app))
                    .with_margin_top(16.)
                    .finish(),
            );
        }

        content.add_child(
            ui_builder
                .span(crate::t!("settings-about-copyright"))
                .build()
                .with_margin_top(16.)
                .finish(),
        );

        // "Export Logs" link: platform-native export of zip to share with troubleshooting personnel. The WASM platform has no
        // file system logs, skip.
        #[cfg(not(target_family = "wasm"))]
        {
            let export_link = ui_builder
                .link(
                    crate::t!("settings-about-export-logs"),
                    None,
                    Some(Box::new(|ctx| {
                        ctx.dispatch_typed_action(AboutPageAction::ExportLogs);
                    })),
                    self.export_logs_link_mouse_state.clone(),
                )
                .soft_wrap(false)
                .build()
                .finish();

            // Use a vertical Flex column to simultaneously present the link and description text (explaining why it's exported and what it contains).
            let export_section = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(export_link)
                .with_child(
                    ui_builder
                        .span(crate::t!("settings-about-export-logs-description"))
                        .with_soft_wrap()
                        .build()
                        .with_margin_top(4.)
                        .finish(),
                )
                .finish();

            content.add_child(Container::new(export_section).with_margin_top(16.).finish());
        }

        if AppExecutionMode::as_ref(app).can_autoupdate() {
            content.add_child(
                Container::new(
                    ConstrainedBox::new(render_body_item::<AboutPageAction>(
                        crate::t!("settings-about-automatic-updates-label"),
                        None,
                        LocalOnlyIconState::Hidden,
                        ToggleState::Enabled,
                        appearance,
                        appearance
                            .ui_builder()
                            .switch(self.automatic_updates_switch_state.clone())
                            .check(
                                *AutoupdateSettings::as_ref(app)
                                    .automatic_updates_enabled
                                    .value(),
                            )
                            .build()
                            .on_click(move |ctx, _, _| {
                                ctx.dispatch_typed_action(AboutPageAction::ToggleAutomaticUpdates);
                            })
                            .finish(),
                        Some(crate::t!("settings-about-automatic-updates-description")),
                    ))
                    .with_max_width(520.)
                    .finish(),
                )
                .with_margin_top(24.)
                .finish(),
            );
        }

        Align::new(content.finish()).finish()
    }
}

impl AboutPageWidget {
    /// Renders the "Update Status" row: status text + action link (check updates / progress display / install now / GitHub fallback).
    fn render_update_status(&self, appearance: &Appearance, app: &AppContext) -> Box<dyn Element> {
        let ui_builder = appearance.ui_builder();

        // Current stage determines text and action:
        // - NoUpdateAvailable / Unknown Error: up to date + "Check Now"
        // - CheckingForUpdate: Checking... (No action)
        // - DownloadingUpdate: Downloading X% (X MB / Y MB) (No action)
        // - UpdateReady / UpdatedPendingRestart: Ready to install + "Install Now" button
        // - UnableTo*: Auto-install failed + "Go to GitHub to Download" fallback link
        let stage = autoupdate::get_update_state(app);
        let progress = autoupdate::AutoupdateState::as_ref(app).download_progress().cloned();

        let (status_text, action) = match &stage {
            AutoupdateStage::CheckingForUpdate => (
                crate::t!("settings-about-update-checking"),
                UpdateAction::None,
            ),
            AutoupdateStage::DownloadingUpdate => {
                // Shared across three platforms: get downloaded bytes from AutoupdateState.download_progress,
                // format into "X.X MB / Y.Y MB (P%)"; when total size is unknown, only display downloaded bytes.
                let new_version = stage
                    .available_new_version()
                    .map(|v| v.version.as_str())
                    .unwrap_or("");
                let text = match &progress {
                    Some(p) => {
                        // i18n_embed_fl::fl! requires parameters to be references and have lifetime, so
                        // bind the progress string to a let binding first, do not insert temporary expressions.
                        let progress_str = format_download_progress(p);
                        crate::t!(
                            "settings-about-update-downloading",
                            version = new_version,
                            progress = progress_str.as_str()
                        )
                    }
                    None => crate::t!(
                        "settings-about-update-downloading-init",
                        version = new_version
                    ),
                };
                (text, UpdateAction::None)
            }
            AutoupdateStage::NoUpdateAvailable => (
                crate::t!("settings-about-update-up-to-date"),
                UpdateAction::Check,
            ),
            AutoupdateStage::UpdateReady { new_version, .. }
            | AutoupdateStage::UpdatedPendingRestart { new_version } => {
                let text = crate::t!(
                    "settings-about-update-ready",
                    version = new_version.version.as_str()
                );
                (text, UpdateAction::Install)
            }
            stage if stage.available_new_version().is_some() => {
                // UnableToUpdateToNewVersion / UnableToLaunchNewVersion / Updating (remnant):
                // Auto-installation error or interrupted -> give user a manual download fallback.
                let new_version = stage.available_new_version().unwrap();
                let text = crate::t!(
                    "settings-about-update-available",
                    version = new_version.version.as_str()
                );
                let url = github::cached_release()
                    .map(|r| r.html_url)
                    .unwrap_or_else(|| {
                        "https://github.com/codeitlikemiley/waz/releases/latest".to_owned()
                    });
                (text, UpdateAction::OpenReleasePage(url))
            }
            // Fallback (theoretically unreachable): any remaining stage is treated as "up to date".
            _ => (
                crate::t!("settings-about-update-up-to-date"),
                UpdateAction::Check,
            ),
        };

        let mut row = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(ui_builder.span(status_text).build().finish());

        match action {
            UpdateAction::None => {}
            UpdateAction::Check => {
                row.add_child(
                    Container::new(
                        ui_builder
                            .link(
                                crate::t!("settings-about-update-check-now"),
                                None,
                                Some(Box::new(|ctx| {
                                    ctx.dispatch_typed_action(AboutPageAction::CheckForUpdate);
                                })),
                                self.update_action_link_mouse_state.clone(),
                            )
                            .soft_wrap(false)
                            .build()
                            .finish(),
                    )
                    .with_padding_left(8.)
                    .finish(),
                );
            }
            UpdateAction::OpenReleasePage(url) => {
                let url_clone = url.clone();
                row.add_child(
                    Container::new(
                        ui_builder
                            .link(
                                crate::t!("settings-about-update-open-release"),
                                None,
                                Some(Box::new(move |ctx| {
                                    ctx.dispatch_typed_action(AboutPageAction::OpenReleasePage(
                                        url_clone.clone(),
                                    ));
                                })),
                                self.update_action_link_mouse_state.clone(),
                            )
                            .soft_wrap(false)
                            .build()
                            .finish(),
                    )
                    .with_padding_left(8.)
                    .finish(),
                );
            }
            UpdateAction::Install => {
                row.add_child(
                    Container::new(
                        ui_builder
                            .link(
                                crate::t!("settings-about-update-install-now"),
                                None,
                                Some(Box::new(|ctx| {
                                    ctx.dispatch_typed_action(AboutPageAction::InstallUpdate);
                                })),
                                self.update_action_link_mouse_state.clone(),
                            )
                            .soft_wrap(false)
                            .build()
                            .finish(),
                    )
                    .with_padding_left(8.)
                    .finish(),
                );
            }
        }

        // Installation hint: only displayed in UpdateReady/UpdatedPendingRestart status (Install action),
        // letting the user know in advance what they will see after clicking (opening dmg / starting installation wizard / restarting AppImage).
        if matches!(
            autoupdate::get_update_state(app),
            AutoupdateStage::UpdateReady { .. } | AutoupdateStage::UpdatedPendingRestart { .. }
        ) {
            // t! is a macro, must pass a literal, cannot use variables. Pick specific key according to cfg branch.
            #[cfg(target_os = "macos")]
            let hint = crate::t!("settings-about-update-install-hint-macos");
            #[cfg(windows)]
            let hint = crate::t!("settings-about-update-install-hint-windows");
            #[cfg(all(not(target_os = "macos"), not(windows)))]
            let hint = crate::t!("settings-about-update-install-hint-linux");

            return Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(row.finish())
                .with_child(
                    ui_builder
                        .span(hint)
                        .with_soft_wrap()
                        .build()
                        .with_margin_top(4.)
                        .finish(),
                )
                .finish();
        }

        row.finish()
    }
}

/// Formats bytes as "X.X MB" / "X KB", used for download progress text.
fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Renders DownloadProgress as "1.2 MB / 3.4 MB (35%)"; only displays downloaded if total is unknown.
fn format_download_progress(p: &autoupdate::DownloadProgress) -> String {
    let downloaded = format_bytes(p.downloaded);
    match p.total {
        Some(total) if total > 0 => {
            let pct = ((p.downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0);
            format!("{} / {} ({:.0}%)", downloaded, format_bytes(total), pct)
        }
        _ => downloaded,
    }
}

/// Actions to display in the update status area: None / Check / Open GitHub Release / Install.
enum UpdateAction {
    None,
    Check,
    OpenReleasePage(String),
    Install,
}

impl SettingsPageMeta for AboutPageView {
    fn section() -> SettingsSection {
        SettingsSection::About
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        true
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<AboutPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<AboutPageView>) -> Self {
        SettingsPageViewHandle::About(view_handle)
    }
}
