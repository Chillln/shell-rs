//! The window shell: title bar, dock area with its sidebars, the switch of
//! the right sidebar's tools, status bar, and the start page (recent hosts)
//! the center shows while no tab is open.

mod automatic_sync;
mod cli_changes;
mod cloud_sync;
mod credentials;
mod directories;
mod dock_skin;
mod editors;
mod forwards;
mod front;
mod links;
mod notices;
mod quick_switcher;
mod recent_hosts;
mod sidebar;
mod snippets;
mod status_bar;
mod tabs;
mod title_bar;
mod tool_sidebar;
mod tools;
mod window_state;
mod workspace_view;

pub use links::open_link_once_open;
pub use window_state::WindowState;
pub use workspace_view::{Workspace, notify_once_open, window_options};
