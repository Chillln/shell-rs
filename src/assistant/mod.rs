//! 自然语言助手：模型仅提出工具调用，应用审批后在固定 SSH 连接上执行。

mod client;
mod markdown;
mod model;
mod panel;
mod settings_panel;

pub use client::{
    AssistantProvider, HttpAssistantProvider, ModelEvent, ModelRequest, ModelTurn, ToolCall,
};
pub use model::{ApiProtocol, AssistantSettings, PermissionMode};
pub use panel::{AssistantPanel, AssistantTarget};
pub use settings_panel::AssistantSettingsPanel;
