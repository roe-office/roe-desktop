//! 保存成功后的 UI 通知 hook:工具结果里取出 save_document 的虚拟路径并回调。
//! 只观察、不改变工具结果(始终 Keep)。始终挂载(add_hook 按值传入,不能按条件挂);
//! 非 save_document 或保存失败时 extract_saved_path 返回 None,hook 什么都不做。

use std::sync::Arc;

use rig_agent::agent::{AgentHook, HookContext, ToolResultAction, ToolResultEvent};
use rig_core::tool::ToolResult;

/// 若这次工具结果是一次成功的保存,调用 `on_saved(虚拟路径)` 并返回 true;否则返回 false。
pub(crate) fn notify_if_saved(
    tool_name: &str,
    result: &ToolResult,
    on_saved: &dyn Fn(String),
) -> bool {
    let mut hit = false;
    if let Some(path) = roe_agent::saved_path::extract_saved_path(tool_name, result) {
        on_saved(path);
        hit = true;
    }
    // 脚本里 export_image 导出的图片:每张也算一次"产物落盘",触发同样的回调。
    for path in roe_agent::saved_path::extract_export_folders(tool_name, result) {
        on_saved(path);
        hit = true;
    }
    hit
}

/// 把"保存成功"转成回调的 hook。
pub struct SavedHook {
    on_saved: Arc<dyn Fn(String) + Send + Sync>,
}

impl SavedHook {
    pub fn new<F>(on_saved: F) -> Self
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        Self { on_saved: Arc::new(on_saved) }
    }
}

impl AgentHook for SavedHook {
    async fn on_tool_result(
        &self,
        _ctx: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> ToolResultAction {
        notify_if_saved(event.tool_name, event.raw_result, &*self.on_saved);
        ToolResultAction::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_core::tool::{ToolExecutionError, ToolOutput};
    use std::sync::Mutex;

    fn ok_json(v: serde_json::Value) -> ToolResult {
        ToolResult::success(ToolOutput::json(v))
    }

    fn collect(tool: &str, r: &ToolResult) -> (bool, Vec<String>) {
        let got = Mutex::new(Vec::new());
        let hit = notify_if_saved(tool, r, &|p| got.lock().unwrap().push(p));
        (hit, got.into_inner().unwrap())
    }

    #[test]
    fn successful_save_calls_back_with_virtual_path() {
        let r = ok_json(serde_json::json!({"virtual_path": "/exports/报表.xlsx"}));
        let (hit, got) = collect("save_document", &r);
        assert!(hit);
        assert_eq!(got, vec!["/exports/报表.xlsx".to_string()]);
    }

    #[test]
    fn other_tool_does_not_call_back() {
        let r = ok_json(serde_json::json!({"virtual_path": "/exports/a.xlsx"}));
        let (hit, got) = collect("list_workspace", &r);
        assert!(!hit);
        assert!(got.is_empty());
    }

    #[test]
    fn failed_save_does_not_call_back() {
        let r = ToolResult::failed(ToolExecutionError::other("boom"));
        let (hit, got) = collect("save_document", &r);
        assert!(!hit);
        assert!(got.is_empty());
    }
}

#[cfg(test)]
mod export_hook_tests {
    use super::*;
    use rig_core::tool::ToolOutput;
    use std::sync::Mutex;

    #[test]
    fn export_images_trigger_callback_per_file() {
        let r = ToolResult::success(ToolOutput::json(serde_json::json!({
            "script_result": null,
            "exports": [
                {"path": "/exports/a_1108_v1/page1.png", "width": 1, "height": 1},
                {"path": "/exports/a_1108_v1/page2.png", "width": 1, "height": 1}
            ]
        })));
        let got = Mutex::new(Vec::new());
        let hit = notify_if_saved("execute_script_preview", &r, &|p| got.lock().unwrap().push(p));
        assert!(hit);
        assert_eq!(got.into_inner().unwrap(), vec!["/exports/a_1108_v1".to_string()]);
    }

    #[test]
    fn plain_script_result_does_not_trigger() {
        let r = ToolResult::success(ToolOutput::json(serde_json::json!({"script_result": 1})));
        let got = Mutex::new(Vec::new());
        assert!(!notify_if_saved("execute_script_preview", &r, &|p| got.lock().unwrap().push(p)));
        assert!(got.into_inner().unwrap().is_empty());
    }
}
