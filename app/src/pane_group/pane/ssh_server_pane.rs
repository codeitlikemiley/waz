//! SSH Server editor pane (central pane, opened via SSH manager tree).
//!
//! Minimal structure mimicking `get_started_pane.rs`, containing no cloud sync / sharing. The pane is
//! not persisted (`LeafContents::SshServer { .. }` returns false in `is_persisted()`),
//! business data (host/user/port/...) uses `warp_ssh_manager::SshRepository` to operate on SQLite.

use warpui::{AppContext, ModelHandle, View, ViewContext, ViewHandle};

use crate::{
    app_state::LeafContents,
    pane_group::{
        pane::{ShareableLink, ShareableLinkError},
        BackingView, PaneConfiguration, PaneContent, PaneGroup, PaneView,
    },
    ssh_manager::server_view::SshServerView,
};

use super::PaneId;

pub struct SshServerPane {
    view: ViewHandle<PaneView<SshServerView>>,
    pane_configuration: ModelHandle<PaneConfiguration>,
    /// Business node ID (not the pane view ID), used for snapshot serialization.
    node_id: String,
}

impl SshServerPane {
    pub fn new<V: View>(node_id: String, ctx: &mut ViewContext<V>) -> Self {
        let id_for_view = node_id.clone();
        let server_view =
            ctx.add_typed_action_view(move |ctx| SshServerView::new(id_for_view, ctx));
        let pane_configuration = server_view.as_ref(ctx).pane_configuration();
        let pane_view = ctx.add_typed_action_view(|ctx| {
            let pane_id = PaneId::from_ssh_server_pane_ctx(ctx);
            PaneView::new(pane_id, server_view, (), pane_configuration.clone(), ctx)
        });
        Self {
            view: pane_view,
            pane_configuration,
            node_id,
        }
    }
}

impl PaneContent for SshServerPane {
    fn id(&self) -> PaneId {
        PaneId::from_ssh_server_pane_view(&self.view)
    }

    fn attach(
        &self,
        _group: &PaneGroup,
        focus_handle: crate::pane_group::focus_state::PaneFocusHandle,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        self.view
            .update(ctx, |view, ctx| view.set_focus_handle(focus_handle, ctx));
        let child = self.view.as_ref(ctx).child(ctx);

        let pane_id = self.id();
        ctx.subscribe_to_view(&child, move |pane_group, _, event, ctx| {
            pane_group.handle_pane_event(pane_id, event, ctx);
        });
    }

    fn detach(
        &self,
        _group: &PaneGroup,
        _detach_type: super::DetachType,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        let child = self.view.as_ref(ctx).child(ctx);
        ctx.unsubscribe_to_view(&child);
    }

    fn snapshot(&self, _ctx: &AppContext) -> LeafContents {
        LeafContents::SshServer {
            node_id: self.node_id.clone(),
        }
    }

    fn has_application_focus(&self, ctx: &mut ViewContext<PaneGroup>) -> bool {
        self.view.is_self_or_child_focused(ctx)
    }

    fn focus(&self, ctx: &mut ViewContext<PaneGroup>) {
        self.view
            .as_ref(ctx)
            .child(ctx)
            .update(ctx, BackingView::focus_contents)
    }

    fn shareable_link(
        &self,
        _ctx: &mut ViewContext<PaneGroup>,
    ) -> Result<ShareableLink, ShareableLinkError> {
        Ok(ShareableLink::Base)
    }

    fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    fn is_pane_being_dragged(&self, ctx: &AppContext) -> bool {
        self.view.as_ref(ctx).is_being_dragged()
    }
}
