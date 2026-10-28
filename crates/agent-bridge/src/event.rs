//! UI <-> Agent 之间传递的事件/动作类型定义。
//!
//! 纯数据结构，不含任何收发逻辑（收发逻辑在 runtime.rs）。
//! 对应设计方案"四、总体架构"中的事件类型草案，具体字段以后续
//! 实现时为准，这里只定方向。

use crate::config::AgentConfig;

/// UI -> Agent：slint 回调内部通过 mpsc 发送给后台 task 的动作。
/// 对应 AgentView.slint 的 send-message / script-confirm /
/// script-discard 三个用户发起的回调。
#[derive(Clone, Debug)]
pub enum AgentAction {
    /// 用户在输入框发送了一条新消息。
    UserQuery(String),
    /// 用户在某条 pending 的 script 消息上点击了 Confirm。
    /// 参数是 ShadowRegistry::take() 认的 shadow_handle（u64），
    /// 不是消息列表下标——AgentMessage 需要携带这个字段，
    /// 由 main.rs 的回调从对应消息里取出后再发送 Action。
    UserConfirm(u64),
    /// 用户在某条 pending 的 script 消息上点击了 Discard。
    /// 参数同上，是 shadow_handle。
    UserDiscard(u64),
    /// 设置页保存/切换激活项后，用新配置重建 agent（失败不退出后台 task）。
    Reconfigure(AgentConfig),
    /// 清空对话历史(换文档时由面板发送)。
    ResetHistory,
}

/// Agent -> UI：后台通过 slint::invoke_from_event_loop 送回 UI
/// 线程、更新 agent_messages 模型的事件。
#[derive(Clone, Debug)]
pub enum AgentUiEvent {
    /// 流式文本增量。
    TextDelta(String),
    /// 思考状态切换。
    ThinkingStatus(bool),
    /// 工具调用产生了待确认的脚本，触发 Confirm 挂起。
    ToolExecutionPending {
        shadow_handle: String,
        script_preview: String,
    },
    ExecutionSuccess(String),
    ExecutionError(String),
    /// 设置页保存/激活配置后 Agent 重建失败（不属于对话流，UI 就地提示）。
    ConfigError(String),
    /// save_document 成功后由 hook 发出,携带虚拟路径(如 /exports/xxx.pptx),UI 显示提示行。
    FileSaved(String),
}
