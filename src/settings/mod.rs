//! 设置: the settings tab, the settings it edits and how they are applied.

mod app_theme;
mod apply;
mod highlight_rules;
mod local_shell_editor;
mod model;
mod settings_panel;
mod shortcuts_editor;
mod store;
mod terminal_themes;

pub use apply::apply;
pub use model::{
    AppSettings, Appearance, Choice, ExternalCliSettings, InterfaceLanguage, LocalTerminalSettings,
    NotificationSettings, TerminalFontSettings, TerminalHighlightSettings, TerminalThemeSettings,
    WindowSettings,
};
pub use settings_panel::{SettingsPanel, SettingsPanelEvent};
pub use store::{SettingsProblem, SettingsStore, SettingsStoreEvent};
