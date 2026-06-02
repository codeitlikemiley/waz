//! Global SSH tree change broadcast - any view changes the tree structure (add/delete/rename/change server field)
//! After calling `notify` once, SshManagerPanel and other subscribers will refresh accordingly.
//!
//! Followed by `KeybindingChangedNotifier`(`app/src/settings_view/keybindings.rs:72`)
//! A routine: Empty struct + SingletonEntity + single Event variant.

use warpui::{Entity, SingletonEntity};

#[derive(Default)]
pub struct SshTreeChangedNotifier {}

impl SshTreeChangedNotifier {
    pub fn new() -> Self {
        Default::default()
    }
}

#[derive(Clone, Debug)]
pub enum SshTreeChangedEvent {
    /// The node list/server details have changed and you need to list_nodes again.
    TreeChanged,
}

impl Entity for SshTreeChangedNotifier {
    type Event = SshTreeChangedEvent;
}

impl SingletonEntity for SshTreeChangedNotifier {}
