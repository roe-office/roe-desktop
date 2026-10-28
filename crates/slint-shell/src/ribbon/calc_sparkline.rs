//! Calc「插入」页 迷你图:对话框(数据范围 + 位置范围 + 类型)与清除。
//! 类型下拉顺序固定(Slint 与 Rust 的隐式契约,只追加不改):0 折线 1 柱形 2 盈亏。

use super::calc_home::{read_selection, refresh_view};
use super::RibbonCtx;

const KINDS: [&str; 3] = ["line", "column", "stacked"];

pub struct SparkDialogState {
    pub sheet: u32,
    pub kind: usize,
    pub data: String,
    pub loc: String,
}

/// "$B$3" / "B3" → (row, col),0 基。
fn parse_cell(s: &str) -> Option<(u32, u32)> {
    let t: String = s.trim().chars().filter(|c| *c != '$').collect();
    let letters = t.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    if !(1..=3).contains(&letters) || letters == t.len() { return None; }
    let row: u32 = t[letters..].parse().ok()?;
    if row == 0 { return None; }
    let col = t[..letters].chars().fold(0u32, |a, c| a * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1));
    Some((row - 1, col - 1))
}

/// "A1:E3" 或 "A1" → 归一化 (r1,c1,r2,c2)。
pub(super) fn parse_range(s: &str) -> Option<(u32, u32, u32, u32)> {
    let mut it = s.trim().split(':');
    let a = parse_cell(it.next()?)?;
    let b = match it.next() { Some(x) => parse_cell(x)?, None => a };
    if it.next().is_some() { return None; }
    Some((a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)))
}

pub(super) fn fmt_range(r: (u32, u32, u32, u32)) -> String {
    let a = format!("{}{}", crate::col_index_to_letter(r.1), r.0 + 1);
    if (r.0, r.1) == (r.2, r.3) { return a; }
    format!("{}:{}{}", a, crate::col_index_to_letter(r.3), r.2 + 1)
}

impl SparkDialogState {
    /// 返回 (能否确认, 提示)。空值只灰掉确定不报红;格式/尺寸不对才报红。
    fn validate(&self) -> (bool, String) {
        if self.data.trim().is_empty() || self.loc.trim().is_empty() {
            return (false, String::new());
        }
        let (Some(d), Some(l)) = (parse_range(&self.data), parse_range(&self.loc)) else {
            return (false, "范围格式不对(如 A1:E3)".to_string());
        };
        let (dr, dc) = (d.2 - d.0 + 1, d.3 - d.1 + 1);
        let (lr, lc) = (l.2 - l.0 + 1, l.3 - l.1 + 1);
        if (lc == 1 && lr == dr) || (lr == 1 && lc == dc) {
            (true, String::new())
        } else {
            (false, "位置范围需为单列(行数同数据)或单行(列数同数据)".to_string())
        }
    }
}

fn push(app: &crate::AppWindow, st: &SparkDialogState) {
    let (ok, warn) = st.validate();
    app.set_spk_kind_index(st.kind as i32);
    app.set_spk_can_confirm(ok);
    app.set_spk_warn(warn.into());
}

/// kind: 0 折线 1 柱形 2 盈亏。数据范围默认填当前选区,位置范围留空。
pub fn open(ctx: &RibbonCtx, kind: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    if kind >= KINDS.len() { return; }
    let Some(sel) = read_selection(&app) else {
        eprintln!("[ribbon] calc.sparkline: 无选中单元格");
        return;
    };
    let st = SparkDialogState {
        sheet: ctx.current_sheet_idx.get(),
        kind,
        data: fmt_range(sel),
        loc: String::new(),
    };
    eprintln!("[ribbon] calc.sparkline 打开对话框 kind={} data={}", KINDS[kind], st.data);
    app.set_spk_data(st.data.clone().into());
    app.set_spk_loc("".into());
    push(&app, &st);
    ctx.state.borrow_mut().spark_dialog = Some(st);
    app.set_spk_visible(true);
}

pub fn kind_selected(ctx: &RibbonCtx, idx: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.spark_dialog.as_mut() else { return };
    if idx < KINDS.len() { st.kind = idx; }
    eprintln!("[ribbon] calc.sparkline kind_selected idx={}", idx);
    push(&app, st);
}

pub fn data_edited(ctx: &RibbonCtx, text: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.spark_dialog.as_mut() else { return };
    st.data = text.to_string();
    push(&app, st);
}

pub fn loc_edited(ctx: &RibbonCtx, text: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.spark_dialog.as_mut() else { return };
    st.loc = text.to_string();
    push(&app, st);
}

pub fn cancel(ctx: &RibbonCtx) {
    ctx.state.borrow_mut().spark_dialog = None;
    eprintln!("[ribbon] calc.sparkline 对话框取消");
}

pub fn confirm(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    match s.spark_dialog.as_ref() {
        Some(st) if st.validate().0 => {}
        Some(_) => { app.set_spk_visible(true); return; }
        None => return,
    }
    let Some(st) = s.spark_dialog.take() else { return };
    let (Some(d), Some(l)) = (parse_range(&st.data), parse_range(&st.loc)) else { return };
    let Some(sess) = s.session.as_mut() else { return };
    let ok = sess.calc_add_sparkline_group(st.sheet, KINDS[st.kind], d, l);
    eprintln!("[ribbon] calc.sparkline {} data={:?} loc={:?} ok={}", KINDS[st.kind], d, l, ok);
    if !ok { return; }
    refresh_view(&app, sess);
}

/// 菜单第四项:清除所选单元格内的迷你图(不开对话框)。
pub fn clear_selected(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else { return };
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    let ok = sess.calc_clear_sparklines(sheet, r1, c1, r2, c2);
    eprintln!("[ribbon] calc.sparkline.clear ({},{})-({},{}) ok={}", r1, c1, r2, c2, ok);
    if ok { refresh_view(&app, sess); }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn st(d: &str, l: &str) -> SparkDialogState {
        SparkDialogState { sheet: 0, kind: 0, data: d.into(), loc: l.into() }
    }
    #[test]
    fn parse_and_pairing() {
        assert_eq!(parse_range("A1:E3"), Some((0, 0, 2, 4)));
        assert_eq!(parse_range("$b$2"), Some((1, 1, 1, 1)));
        assert_eq!(parse_range("E3:A1"), Some((0, 0, 2, 4)));
        assert_eq!(parse_range("A0"), None);
        assert_eq!(parse_range("abc"), None);
        assert!(st("A1:E3", "G1:G3").validate().0);
        assert!(st("A1:C5", "A7:C7").validate().0);
        assert!(st("A1:E1", "G1").validate().0);
        assert!(!st("A1:E3", "G1:G2").validate().0);
        assert!(!st("A1:E3", "").validate().0);
        assert_eq!(fmt_range((0, 0, 2, 4)), "A1:E3");
        assert_eq!(fmt_range((1, 1, 1, 1)), "B2");
    }
}
