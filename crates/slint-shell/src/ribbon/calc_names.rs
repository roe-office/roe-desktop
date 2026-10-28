//! 名称管理器(MVP:查看 / 新增 / 删除全局名称)。
//! 后端接口:list_names / validate_name / set_name / remove_name(roe-script)。
//! 对话框状态放 thread_local,不进 AppState。

use std::cell::{Cell, RefCell};

use slint::{ModelRc, SharedString, VecModel};

use super::calc_home::{refresh_view, sync_state_inner};
use super::RibbonCtx;

struct Item {
    name: String,
    kind: String,
    reference: String,
}

thread_local! {
    static NAMES: RefCell<Vec<String>> = RefCell::new(Vec::new());
    static SELECTED: Cell<i32> = Cell::new(-1);
}

fn rhai_str(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "")
    )
}

/// 脚本返回外壳:{"ok":..,"value"|"return_value":..} 或裸字符串,统一取出内层文本。
fn shell_value(raw: &str) -> String {
    let t = raw.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
        match &v {
            serde_json::Value::Object(o) => {
                if o.get("ok").and_then(|x| x.as_bool()) == Some(false) {
                    return format!("err:脚本执行失败: {}", t);
                }
                for k in ["return_value", "value"] {
                    if let Some(x) = o.get(k) {
                        return match x {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                    }
                }
                return t.to_string();
            }
            serde_json::Value::String(s) => return s.clone(),
            serde_json::Value::Array(_) => return t.to_string(),
            _ => {}
        }
    }
    t.trim_matches('"').to_string()
}

/// AI 面板成功分支用:从 execute_script 外壳里取 return_value 的展示文本。
/// null / 空串 / "()" / 缺字段 / 非对象 一律返回 None;超过 600 字符截断。
pub(crate) fn ai_return_text(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    let rv = v.as_object()?.get("return_value")?;
    let t = match rv {
        serde_json::Value::Null => return None,
        serde_json::Value::String(s) => s.trim().to_string(),
        other => other.to_string(),
    };
    if t.is_empty() || t == "()" {
        return None;
    }
    const MAX: usize = 600;
    if t.chars().count() > MAX {
        let cut: String = t.chars().take(MAX).collect();
        return Some(format!("{}…(已截断)", cut));
    }
    Some(t)
}

#[cfg(test)]
mod ai_return_tests {
    use super::ai_return_text;

    #[test]
    fn string_and_number() {
        assert_eq!(ai_return_text(r#"{"ok":true,"return_value":"ok:3"}"#).as_deref(), Some("ok:3"));
        assert_eq!(ai_return_text(r#"{"ok":true,"return_value":42}"#).as_deref(), Some("42"));
    }

    #[test]
    fn empty_like_values_are_none() {
        assert_eq!(ai_return_text(r#"{"ok":true,"return_value":null}"#), None);
        assert_eq!(ai_return_text(r#"{"ok":true,"return_value":""}"#), None);
        assert_eq!(ai_return_text(r#"{"ok":true,"return_value":"()"}"#), None);
        assert_eq!(ai_return_text(r#"{"ok":true,"return_value":"  "}"#), None);
    }

    #[test]
    fn missing_or_not_object() {
        assert_eq!(ai_return_text(r#"{"ok":true,"dirty_pages":[]}"#), None);
        assert_eq!(ai_return_text("not json"), None);
        assert_eq!(ai_return_text(r#""ok""#), None);
    }

    #[test]
    fn truncates_long_text_by_chars() {
        let long = "汉".repeat(700);
        let raw = format!(r#"{{"ok":true,"return_value":"{}"}}"#, long);
        let t = ai_return_text(&raw).unwrap();
        assert!(t.ends_with("…(已截断)"));
        assert_eq!(t.chars().count(), 600 + "…(已截断)".chars().count());
    }
}

/// "ok" -> Ok;"err:xxx" -> Err(xxx);其它 -> Err(原文)
fn parse_status(raw: &str) -> Result<(), String> {
    let v = shell_value(raw);
    let v = v.trim();
    if v == "ok" {
        Ok(())
    } else if let Some(m) = v.strip_prefix("err:") {
        Err(m.to_string())
    } else {
        Err(format!("未识别的返回: {}", raw))
    }
}

fn fetch_items(sess: &mut doc_core::TileCacheRust) -> Vec<Item> {
    let raw = sess.query_script("list_names()");
    let inner = shell_value(&raw);
    match serde_json::from_str::<Vec<serde_json::Value>>(&inner) {
        Ok(arr) => arr
            .iter()
            .map(|v| Item {
                name: v["name"].as_str().unwrap_or("").to_string(),
                kind: if v["kind"].as_str() == Some("formula") { "公式".into() } else { "区域".into() },
                reference: v["ref"].as_str().unwrap_or("").to_string(),
            })
            .collect(),
        Err(e) => {
            eprintln!("[ribbon] calc.name_manager list_names 解析失败: {} raw={}", e, raw);
            Vec::new()
        }
    }
}

fn push(app: &crate::AppWindow, items: &[Item]) {
    let ss = |f: &dyn Fn(&Item) -> &str| -> ModelRc<SharedString> {
        let v: Vec<SharedString> = items.iter().map(|i| SharedString::from(f(i))).collect();
        ModelRc::new(VecModel::from(v))
    };
    app.set_name_mgr_names(ss(&|i| i.name.as_str()));
    app.set_name_mgr_kinds(ss(&|i| i.kind.as_str()));
    app.set_name_mgr_refs(ss(&|i| i.reference.as_str()));
    NAMES.with(|n| *n.borrow_mut() = items.iter().map(|i| i.name.clone()).collect());
    SELECTED.with(|s| s.set(-1));
    app.set_name_mgr_selected(-1);
}

pub fn open(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    let items = fetch_items(sess);
    eprintln!("[ribbon] calc.name_manager 打开,名称数={}", items.len());
    push(&app, &items);
    app.set_name_mgr_error("".into());
    app.set_name_mgr_input_name("".into());
    app.set_name_mgr_input_ref("".into());
    app.set_name_mgr_visible(true);
}

pub fn select(_ctx: &RibbonCtx, idx: i32) {
    SELECTED.with(|s| s.set(idx));
    if let Some(app) = _ctx.app_weak.upgrade() {
        app.set_name_mgr_selected(idx);
        app.set_name_mgr_error("".into());
    }
}

fn after_change(app: &crate::AppWindow, sess: &mut doc_core::TileCacheRust, sheet: u32) {
    let items = fetch_items(sess);
    push(app, &items);
    refresh_view(app, sess);
    sync_state_inner(app, sess, sheet);
}

pub fn add(ctx: &RibbonCtx, name: &str, reference: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };

    let v = sess.query_script(&format!("validate_name({})", rhai_str(name.trim())));
    if let Err(e) = parse_status(&v) {
        eprintln!("[ribbon] calc.name_manager validate 失败: {}", e);
        app.set_name_mgr_error(e.into());
        return;
    }
    let raw = sess.execute_script(&format!(
        "set_name({}, {})",
        rhai_str(name.trim()),
        rhai_str(reference.trim())
    ));
    eprintln!("[ribbon] calc.name_manager set_name({:?},{:?}) 返回: {}", name, reference, raw);
    if let Err(e) = parse_status(&raw) {
        app.set_name_mgr_error(e.into());
        return;
    }
    after_change(&app, sess, sheet);
    app.set_name_mgr_error("".into());
    app.set_name_mgr_input_name("".into());
    app.set_name_mgr_input_ref("".into());
}

pub fn remove(ctx: &RibbonCtx, idx: i32) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let sheet = ctx.current_sheet_idx.get();
    let name = NAMES.with(|n| n.borrow().get(idx.max(0) as usize).cloned());
    let name = match (idx >= 0, name) {
        (true, Some(n)) => n,
        _ => {
            app.set_name_mgr_error("请先在列表中选择一个名称".into());
            return;
        }
    };
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    let raw = sess.execute_script(&format!("remove_name({})", rhai_str(&name)));
    eprintln!("[ribbon] calc.name_manager remove_name({:?}) 返回: {}", name, raw);
    if let Err(e) = parse_status(&raw) {
        app.set_name_mgr_error(e.into());
        return;
    }
    after_change(&app, sess, sheet);
    app.set_name_mgr_error("".into());
}

pub fn close(_ctx: &RibbonCtx) {
    eprintln!("[ribbon] calc.name_manager 关闭");
}
