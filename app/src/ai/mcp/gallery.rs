use std::collections::HashMap;

use crate::ai::mcp::templatable::{GalleryData, JsonTemplate, TemplatableMCPServer};
use crate::server::datetime_ext::DateTimeExt;
use chrono::DateTime;
use uuid::Uuid;
use warpui::{Entity, ModelContext, SingletonEntity};

#[derive(Clone, Debug)]
pub struct GalleryMCPServer {
    uuid: Uuid,
    title: String,
    description: String,
    #[allow(dead_code)]
    version: i32,
    #[allow(dead_code)]
    instructions_in_markdown: Option<String>,
    json_template: JsonTemplate,
}

impl GalleryMCPServer {
    pub fn new(
        uuid: Uuid,
        title: String,
        description: String,
        version: i32,
        instructions_in_markdown: Option<String>,
        json_template: JsonTemplate,
    ) -> Self {
        Self {
            uuid,
            title,
            description,
            version,
            instructions_in_markdown,
            json_template,
        }
    }

    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    pub fn title(&self) -> String {
        self.title.clone()
    }

    pub fn description(&self) -> String {
        self.description.clone()
    }

    pub fn version(&self) -> i32 {
        self.version
    }

    pub fn json_template(&self) -> &JsonTemplate {
        &self.json_template
    }

    pub fn instructions_in_markdown(&self) -> Option<&String> {
        self.instructions_in_markdown.as_ref()
    }
}

impl TryFrom<GalleryMCPServer> for TemplatableMCPServer {
    type Error = String;

    fn try_from(gallery_server: GalleryMCPServer) -> Result<Self, Self::Error> {
        let GalleryMCPServer {
            uuid: gallery_uuid,
            title,
            description,
            version: gallery_version,
            instructions_in_markdown: _,
            json_template,
        } = gallery_server;

        Ok(TemplatableMCPServer {
            uuid: Uuid::new_v4(),
            name: title,
            description: Some(description),
            template: json_template,
            version: DateTime::now().timestamp(),
            gallery_data: Some(GalleryData {
                gallery_item_id: gallery_uuid,
                version: gallery_version,
            }),
        })
    }
}

pub struct MCPGalleryManager {
    gallery_items: HashMap<Uuid, GalleryMCPServer>,
    templatable_mcp_servers: HashMap<Uuid, TemplatableMCPServer>,
}

impl MCPGalleryManager {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        // Waz (localization, Phase 2d-2): The original subscription to `UpdateManager`'s `MCPGalleryUpdated` event
        // distributed gallery items after cloud fetching. After localization, the cloud object fetch/fan-in was removed.
        // In this Phase, the gallery remains empty and the subscription is cancelled:
        // the gallery is always empty locally and rendered as an empty canvas by `MCPServersListPageView`. Local MCP relies on `file_based_manager`
        // which reads `~/.warp/mcp.json` and `~/.claude/...`, and is unaffected.
        Self {
            gallery_items: Default::default(),
            templatable_mcp_servers: Default::default(),
        }
    }

    pub fn get_gallery(&self) -> Vec<GalleryMCPServer> {
        self.gallery_items.values().cloned().collect()
    }

    pub fn get_gallery_item(&self, gallery_uuid: Uuid) -> Option<&GalleryMCPServer> {
        self.gallery_items.get(&gallery_uuid)
    }

    pub fn get_templatable_mcp_server(&self, gallery_uuid: Uuid) -> Option<&TemplatableMCPServer> {
        self.templatable_mcp_servers.get(&gallery_uuid)
    }
}

pub enum MCPGalleryManagerEvent {
    ItemsRefreshed,
}

impl Entity for MCPGalleryManager {
    type Event = MCPGalleryManagerEvent;
}

impl SingletonEntity for MCPGalleryManager {}
