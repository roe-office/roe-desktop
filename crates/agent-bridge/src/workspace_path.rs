//! Agent 工作区物理根目录的唯一来源。
//!
//! runtime(后台 task 打开工作区)与 UI 的"项目目录"按钮必须调用同一个函数,
//! 否则按钮打开的目录和 Agent 写入的目录会不一致。

use std::path::PathBuf;

/// `$HOME/.office-rs2/agent_workspace`;`HOME` 不可用时回退到当前目录下同名相对路径。
pub fn agent_workspace_root() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".office-rs2")
        .join("agent_workspace")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_ends_with_office_rs2_agent_workspace() {
        let p = agent_workspace_root();
        assert!(p.ends_with(".office-rs2/agent_workspace"), "实际路径: {}", p.display());
    }
}
