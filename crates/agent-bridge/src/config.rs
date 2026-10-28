//! Agent 运行配置（由前台设置页的激活 profile 转换而来）。
//! Debug 手写脱敏：AgentAction 会被 eprintln!("{:?}") 打印，Key 绝不能进日志。

use std::fmt;

#[derive(Clone)]
pub struct AgentConfig {
    /// 0 Claude 1 Gemini 2 GPT 3 DeepSeek 4 其他（与 ai_config::Profile.provider 一致）
    pub provider: i32,
    pub endpoint: String,
    pub key: String,
    pub model: String,
}

impl fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentConfig")
            .field("provider", &self.provider)
            .field("endpoint", &self.endpoint)
            .field("key", &if self.key.trim().is_empty() { "<empty>" } else { "***" })
            .field("model", &self.model)
            .finish()
    }
}
