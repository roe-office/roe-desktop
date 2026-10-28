//! UiExecutor:把脚本请求投递到 UI 线程执行(roe_api 的 HANDLES 是 thread_local)。
//!
//! 前台(slint-shell)在 UI 线程调用 `install_ui_handler` 注册 handler;
//! Tool 在 tokio 线程调用 `UiExecutor::execute/query`,经
//! `slint::invoke_from_event_loop` 进入 UI 线程,结果经 oneshot 回传。
//! handler 返回内核原始 JSON,由本文件解析成 ExecuteOutcome / JsonValue。

use std::cell::RefCell;
use std::rc::Rc;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::oneshot;

use roe_agent::executor::{ExecuteOutcome, ScriptExecError, ScriptExecutor};

/// UI 线程侧的执行入口(不要求 Send)。
/// Ok(raw) = 内核返回的成功 JSON;Err(msg) = 失败信息(可为内核的错误 JSON)。
pub trait UiScriptHandler {
    fn execute_raw(&self, script: &str) -> Result<String, String>;
    fn query_raw(&self, script: &str) -> Result<String, String>;
}

thread_local! {
    static HANDLER: RefCell<Option<Rc<dyn UiScriptHandler>>> = RefCell::new(None);
}

/// 必须在 UI 线程调用。
pub fn install_ui_handler(h: Rc<dyn UiScriptHandler>) {
    HANDLER.with(|c| *c.borrow_mut() = Some(h));
}

#[derive(Default)]
pub struct UiExecutor;

impl UiExecutor {
    pub fn new() -> Self {
        Self
    }
}

fn on_ui<T, F>(f: F) -> BoxFuture<'static, Result<T, ScriptExecError>>
where
    T: Send + 'static,
    F: FnOnce(Rc<dyn UiScriptHandler>) -> Result<T, ScriptExecError> + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    let posted = slint::invoke_from_event_loop(move || {
        let r = HANDLER
            .with(|c| c.borrow().clone())
            .ok_or_else(|| ScriptExecError::Script("UI 脚本入口尚未注册".into()))
            .and_then(f);
        let _ = tx.send(r);
    });
    Box::pin(async move {
        if posted.is_err() {
            return Err(ScriptExecError::Script("UI 事件循环已退出".into()));
        }
        rx.await
            .unwrap_or_else(|_| Err(ScriptExecError::Script("UI 线程未返回结果".into())))
    })
}

fn extract_error(raw: &str) -> String {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| raw.to_string())
}

fn parse_execute(raw: &str) -> ExecuteOutcome {
    let v: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let dirty_pages = v
        .get("dirty_pages")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_u64().map(|n| n as usize)).collect())
        .unwrap_or_default();
    let script_result = v.get("return_value").cloned().unwrap_or(Value::Null);
    ExecuteOutcome { dirty_pages, dirty_regions: Vec::new(), script_result }
}

fn parse_query(raw: &str) -> Result<Value, ScriptExecError> {
    let v: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    if v.get("ok").and_then(|b| b.as_bool()) == Some(false) {
        return Err(ScriptExecError::Script(extract_error(raw)));
    }
    Ok(v.get("value").cloned().unwrap_or(Value::Null))
}

impl ScriptExecutor for UiExecutor {
    fn execute(&self, script: String) -> BoxFuture<'static, Result<ExecuteOutcome, ScriptExecError>> {
        on_ui(move |h| match h.execute_raw(&script) {
            Ok(raw) => Ok(parse_execute(&raw)),
            Err(raw) => Err(ScriptExecError::Script(extract_error(&raw))),
        })
    }

    fn query(&self, script: String) -> BoxFuture<'static, Result<Value, ScriptExecError>> {
        on_ui(move |h| match h.query_raw(&script) {
            Ok(raw) => parse_query(&raw),
            Err(raw) => Err(ScriptExecError::Script(extract_error(&raw))),
        })
    }
}
