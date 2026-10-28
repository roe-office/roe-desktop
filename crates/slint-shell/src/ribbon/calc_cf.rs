//! Calc「开始」页 条件格式"突出显示单元格规则"对话框(大于/小于/介于/等于/文本包含)。
//! 下拉顺序与 CfHighlightDialog.slint 头部注释一致(Rust 端只认 index)。
//! 高亮规则走 add,不清除已有的数据条/色阶(互斥只针对可视化类规则)。

use super::calc_home::{read_selection, refresh_view};
use super::RibbonCtx;

/// 条件 index → condition 前缀。
const OPS: [&str; 10] = [
    "gt", "lt", "between", "eq", "text_contains",
    "ne", "gte", "lte", "not_between",
    "expr", // 9: 介于(不含端点),生成 expr:AND(锚点>v1,锚点<v2)
];

/// 需要两个值的条件(介于/未介于)。
fn is_range_op(op: usize) -> bool {
    op == 2 || op == 8 || op == 9
}

/// 文本类条件(值按文本处理)。
fn is_text_op(op: usize) -> bool {
    op == 4
}
/// 预设 index → (字色, 填充色)。
const PRESETS: [(Option<&str>, Option<&str>); 5] = [
    (Some("#9C0006"), Some("#FFC7CE")), // 浅红填充 + 深红文本
    (Some("#9C5700"), Some("#FFEB9C")), // 黄填充 + 深黄文本
    (Some("#006100"), Some("#C6EFCE")), // 绿填充 + 深绿文本
    (None, Some("#FFC7CE")),            // 仅浅红填充
    (Some("#C00000"), None),            // 仅红色文本
];

/// 对话框瞬时状态(存在 AppState.cf_hl_dialog)。
pub struct CfHlDialogState {
    pub sheet: u32,
    pub r1: u32,
    pub c1: u32,
    pub r2: u32,
    pub c2: u32,
    pub op: usize,
    pub preset: usize,
    pub v1: String,
    pub v2: String,
}

/// "$B$3" / "B3" 形式的单元格引用。
fn is_cell_ref(s: &str) -> bool {
    let t: String = s.chars().filter(|c| *c != '$').collect();
    let letters = t.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    let digits = t.len() - letters;
    (1..=3).contains(&letters)
        && digits > 0
        && t[letters..].chars().all(|c| c.is_ascii_digit())
}

fn num_or_ref(s: &str) -> bool {
    s.parse::<f64>().map(|v| v.is_finite()).unwrap_or(false) || is_cell_ref(s)
}

/// 文本值里会破坏 condition 切分(:)或 Rhai 反引号脚本/JSON 的字符。
fn bad_char(s: &str) -> Option<&'static str> {
    if s.contains(':') { return Some(":"); }
    if s.contains('`') { return Some("`"); }
    if s.contains('"') { return Some("\""); }
    if s.contains('\\') { return Some("\\"); }
    if s.contains("${") { return Some("${"); }
    None
}

impl CfHlDialogState {
    /// 返回 (能否确认, 提示文案)。空值只灰掉确定不显示红字;格式不对才显示红字。
    fn validate(&self) -> (bool, String) {
        let v1 = self.v1.trim();
        let v2 = self.v2.trim();
        if v1.is_empty() || (is_range_op(self.op) && v2.is_empty()) {
            return (false, String::new());
        }
        if is_text_op(self.op) {
            if let Some(c) = bad_char(v1) {
                return (false, format!("文本暂不支持字符 {}", c));
            }
        } else if !num_or_ref(v1) || (is_range_op(self.op) && !num_or_ref(v2)) {
            return (false, "请输入数字或单元格引用(如 $B$3)".to_string());
        }
        (true, String::new())
    }

    fn condition(&self) -> String {
        let (v1, v2) = (self.v1.trim(), self.v2.trim());
        if self.op == 9 {
            let a = format!(
                "{}{}",
                crate::col_index_to_letter(self.c1.min(self.c2)),
                self.r1.min(self.r2) + 1
            );
            format!("expr:AND({a}>{v1},{a}<{v2})")
        } else if is_range_op(self.op) {
            format!("{}:{}:{}", OPS[self.op], v1, v2)
        } else {
            format!("{}:{}", OPS[self.op], v1)
        }
    }

    fn kind_json(&self) -> String {
        let (fg, bg) = PRESETS[self.preset];
        let mut fmt = serde_json::Map::new();
        if let Some(c) = fg { fmt.insert("fg_color".into(), c.into()); }
        if let Some(c) = bg { fmt.insert("bg_color".into(), c.into()); }
        serde_json::json!({ "CellFormat": { "condition": self.condition(), "format": fmt } }).to_string()
    }
}

fn push(app: &crate::AppWindow, st: &CfHlDialogState) {
    let (ok, warn) = st.validate();
    app.set_cfhl_op_index(st.op as i32);
    app.set_cfhl_preset_index(st.preset as i32);
    app.set_cfhl_can_confirm(ok);
    app.set_cfhl_warn(warn.into());
}

/// 菜单入口 "calc.cf:hl:N":取选区,打开对话框(不直接执行)。
pub fn hl_open(ctx: &RibbonCtx, idx: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Ok(op) = idx.parse::<usize>() else { return };
    if op >= OPS.len() { return; }
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.cf.hl: 无选中单元格");
        return;
    };
    let st = CfHlDialogState {
        sheet: ctx.current_sheet_idx.get(),
        r1, c1, r2, c2,
        op,
        preset: 0,
        v1: String::new(),
        v2: String::new(),
    };
    eprintln!("[ribbon] calc.cf.hl 打开对话框 op={} ({},{})-({},{})", OPS[op], r1, c1, r2, c2);
    app.set_cfhl_v1("".into());
    app.set_cfhl_v2("".into());
    push(&app, &st);
    ctx.state.borrow_mut().cf_hl_dialog = Some(st);
    app.set_cfhl_visible(true);
}

pub fn hl_op_selected(ctx: &RibbonCtx, idx: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.cf_hl_dialog.as_mut() else { return };
    if idx < OPS.len() { st.op = idx; }
    push(&app, st);
}

pub fn hl_preset_selected(ctx: &RibbonCtx, idx: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.cf_hl_dialog.as_mut() else { return };
    if idx < PRESETS.len() { st.preset = idx; }
    push(&app, st);
}

pub fn hl_v1_edited(ctx: &RibbonCtx, text: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.cf_hl_dialog.as_mut() else { return };
    st.v1 = text.to_string();
    push(&app, st);
}

pub fn hl_v2_edited(ctx: &RibbonCtx, text: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.cf_hl_dialog.as_mut() else { return };
    st.v2 = text.to_string();
    push(&app, st);
}

pub fn hl_cancel(ctx: &RibbonCtx) {
    ctx.state.borrow_mut().cf_hl_dialog = None;
    eprintln!("[ribbon] calc.cf.hl 对话框取消");
}

pub fn hl_confirm(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    match s.cf_hl_dialog.as_ref() {
        Some(st) if st.validate().0 => {}
        Some(_) => {
            eprintln!("[ribbon] calc.cf.hl: 校验未通过,保持对话框打开");
            app.set_cfhl_visible(true);
            return;
        }
        None => return,
    }
    let Some(st) = s.cf_hl_dialog.take() else { return };
    let kind = st.kind_json();
    let (r1, r2) = (st.r1.min(st.r2), st.r1.max(st.r2));
    let (c1, c2) = (st.c1.min(st.c2), st.c1.max(st.c2));
    let Some(sess) = s.session.as_mut() else { return };
    let ok = sess.calc_add_cf_rule(st.sheet, r1, c1, r2, c2, &kind);
    eprintln!("[ribbon] calc.cf.hl ({},{})-({},{}) {} ok={}", r1, c1, r2, c2, kind, ok);
    if !ok {
        eprintln!("[ribbon] calc.cf.hl 内核返回失败,未写入");
        return;
    }
    refresh_view(&app, sess);
}

// ══════════ 图标集对话框(calc.cf:icon) ══════════
// 格子下标 → 预设名见 cf_icon_styles::ICON_STYLES(与 Slint 选择器同源生成)。
// 走 calc_replace_cf_visual:与数据条/色阶互斥。阈值按 Excel 默认的百分比。

use super::cf_icon_styles::ICON_STYLES;

pub struct CfIconDialogState {
    pub sheet: u32,
    pub r1: u32,
    pub c1: u32,
    pub r2: u32,
    pub c2: u32,
    pub selected: Option<usize>,
    pub reverse: bool,
}

/// 预设名首字符即档位数(3/4/5)。
fn icon_tiers(style: &str) -> usize {
    style.chars().next().and_then(|c| c.to_digit(10)).unwrap_or(3) as usize
}

fn icon_kind_json(style: &str, reverse: bool) -> String {
    let pcts: &[u32] = match icon_tiers(style) {
        4 => &[0, 25, 50, 75],
        5 => &[0, 20, 40, 60, 80],
        _ => &[0, 33, 67],
    };
    let th: Vec<serde_json::Value> = pcts
        .iter()
        .map(|p| serde_json::json!({"kind": "percent", "val": p.to_string(), "gte": true}))
        .collect();
    serde_json::json!({ "IconSet": {
        "icon_style": style,
        "thresholds": th,
        "percent": true,
        "reverse": reverse,
        "icons_only": false,
        "tier_overrides": null,
    }})
    .to_string()
}

pub fn icon_open(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.cf.icon: 无选中单元格");
        return;
    };
    let st = CfIconDialogState {
        sheet: ctx.current_sheet_idx.get(),
        r1, c1, r2, c2,
        selected: None,
        reverse: false,
    };
    eprintln!("[ribbon] calc.cf.icon 打开对话框 ({},{})-({},{})", r1, c1, r2, c2);
    app.set_cfic_selected(-1);
    app.set_cfic_reverse(false);
    ctx.state.borrow_mut().cf_icon_dialog = Some(st);
    app.set_cfic_visible(true);
}

pub fn icon_picked(ctx: &RibbonCtx, idx: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.cf_icon_dialog.as_mut() else { return };
    if idx < ICON_STYLES.len() {
        st.selected = Some(idx);
        app.set_cfic_selected(idx as i32);
    }
}

pub fn icon_toggle_reverse(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.cf_icon_dialog.as_mut() else { return };
    st.reverse = !st.reverse;
    app.set_cfic_reverse(st.reverse);
}

pub fn icon_cancel(ctx: &RibbonCtx) {
    ctx.state.borrow_mut().cf_icon_dialog = None;
    eprintln!("[ribbon] calc.cf.icon 对话框取消");
}

pub fn icon_confirm(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let has_sel = matches!(s.cf_icon_dialog.as_ref(), Some(st) if st.selected.is_some());
    if !has_sel {
        if s.cf_icon_dialog.is_some() {
            app.set_cfic_visible(true);
        }
        return;
    }
    let Some(st) = s.cf_icon_dialog.take() else { return };
    let style = ICON_STYLES[st.selected.unwrap()];
    let kind = icon_kind_json(style, st.reverse);
    let (r1, r2) = (st.r1.min(st.r2), st.r1.max(st.r2));
    let (c1, c2) = (st.c1.min(st.c2), st.c1.max(st.c2));
    let Some(sess) = s.session.as_mut() else { return };
    let ok = sess.calc_replace_cf_visual(st.sheet, r1, c1, r2, c2, &kind);
    eprintln!("[ribbon] calc.cf.icon ({},{})-({},{}) {} ok={}", r1, c1, r2, c2, kind, ok);
    if !ok {
        eprintln!("[ribbon] calc.cf.icon 内核返回失败,未写入");
        return;
    }
    refresh_view(&app, sess);
}
