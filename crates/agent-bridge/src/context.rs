//! 活动文档快照 Provider —— re-export 自 roe_agent::context。
//!
//! ============================================================
//! 归属变更说明（v202 会话：搬到 roe-agent）
//! ============================================================
//! 这个类型原本在这里（office-rs2/agent-bridge）直接定义。现在
//! 改为 re-export roe_agent::context 的类型，不再自己定义。
//!
//! 原因：ExecuteScriptTool/QueryScriptTool 需要在 Tool::call()
//! 内部主动查询"当前活动文档是哪个 handle"（设计方案"二节·2.3"
//! 拍板的方案 B：Tool 自己持有 context，LLM 不需要知道/传递
//! handle）。这意味着 roe-agent 必须能持有这个类型。但 roe-agent
//! 不能反过来依赖 agent-bridge（agent-bridge 依赖 slint，会让
//! 纯后端仓库 roe-agent 间接沾上 GUI 框架依赖，违反既定仓库边界
//! 原则）。
//!
//! 因此改为：类型定义搬到 roe-agent，agent-bridge 反过来依赖
//! roe-agent 导出的类型（agent-bridge 本来就已经依赖 roe-agent，
//! 不引入新的依赖边，依赖链保持单向：
//! GUI/agent-bridge -> roe-agent -> 内核）。
//!
//! 语义上也更贴切："Agent 工具调用时该操作哪个文档"本来就是
//! Agent 运行时契约的一部分，不是 GUI 专属概念。
//!
//! 完整设计说明（Arc<RwLock<...>> 选型理由、UI/Agent 两端如何
//! 调用等）见 roe_agent::context 模块本身的文档注释。

pub use roe_agent::context::{ActiveDocumentSnapshot, AgentContextProvider};
