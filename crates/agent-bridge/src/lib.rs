//! agent-bridge：office-rs2 与 roe-agent 之间的桥接层。
//!
//! 依据 `agent接线设计方案_v1_讨论收敛稿.md` 四/五节：
//! - `context`：极薄的活动文档快照 Provider（Arc<RwLock<...>>，读多写少）。
//! - `event`：UI <-> Agent 之间传递的事件/动作类型定义，纯数据，无收发逻辑。
//! - `runtime`：tokio runtime 生命周期管理 + mpsc channel 建立 +
//!   slint::invoke_from_event_loop 事件泵封装。
//!
//! 本 crate 目前只服务 AgentView（主对话面板）。AiPanel（侧边栏）
//! 是完全独立的另一条任务线，本 crate 不涉及、不共享任何状态。

mod config;
mod context;
mod event;
mod runtime;
mod saved_hook;
mod turn_budget_hook;
pub mod md_plain;
mod ui_executor;
mod upload;
mod workspace_path;

pub use context::{ActiveDocumentSnapshot, AgentContextProvider};
pub use config::AgentConfig;
pub use event::{AgentAction, AgentUiEvent};
pub use runtime::AgentRuntimeHandle;
pub use workspace_path::agent_workspace_root;
pub use upload::upload_to_inputs;
pub use ui_executor::{install_ui_handler, UiExecutor, UiScriptHandler};
