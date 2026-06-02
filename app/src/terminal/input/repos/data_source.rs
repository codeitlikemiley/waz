//! Async data source for the inline repos menu.
//!
//! Historically, the list of "previously opened git repositories" was pulled from `PersistedWorkspace`.
//! After the LSP + workspace history is offline, this candidate source no longer exists, so this data source
//! Only keep the connection between trait and view, and always return empty results - that is to say, the menu can still be
//! Called out but never a candidate. This avoids major changes to the upper view / suggestions mode
//! If you want to connect to the "current pane group real-time cwd" in the future, you can replenish the data source.

use warpui::{AppContext, Entity};

use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::{AsyncDataSource, BoxFuture, DataSourceRunErrorWrapper};
use crate::terminal::input::repos::AcceptRepo;

pub struct RepoMenuDataSource;

impl RepoMenuDataSource {
    pub fn new() -> Self {
        Self
    }
}

impl AsyncDataSource for RepoMenuDataSource {
    type Action = AcceptRepo;

    fn run_query(
        &self,
        _query: &Query,
        _app: &AppContext,
    ) -> BoxFuture<'static, Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
}

impl Entity for RepoMenuDataSource {
    type Event = ();
}
