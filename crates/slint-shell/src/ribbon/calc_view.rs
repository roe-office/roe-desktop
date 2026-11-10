//! 「视图」页命令:冻结窗格 / 冻结首行 / 冻结首列 / 取消冻结。
//! 目标值的计算抽成纯函数(可单测),handle 只负责取状态、写文档、重绘。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeCmd {
    /// 冻结活动格上方与左侧:(活动格行, 活动格列)
    AtCell { row: u32, col: u32 },
    TopRow,
    FirstCol,
    Unfreeze,
}

/// 命令名 -> FreezeCmd。AtCell 需要活动格,由调用方传入。
pub fn parse_cmd(cmd: &str, active_row: u32, active_col: u32) -> Option<FreezeCmd> {
    match cmd {
        "freeze_panes" => Some(FreezeCmd::AtCell { row: active_row, col: active_col }),
        "freeze_top_row" => Some(FreezeCmd::TopRow),
        "freeze_first_col" => Some(FreezeCmd::FirstCol),
        "unfreeze" => Some(FreezeCmd::Unfreeze),
        _ => None,
    }
}

/// 计算目标冻结 (行数, 列数)。cur 为当前冻结 (行, 列)。
pub fn target(cmd: FreezeCmd, cur: (u32, u32)) -> (u32, u32) {
    match cmd {
        FreezeCmd::AtCell { row, col } => (row, col),
        FreezeCmd::TopRow => (1, cur.1),
        FreezeCmd::FirstCol => (cur.0, 1),
        FreezeCmd::Unfreeze => (0, 0),
    }
}

/// 网格线命令名 -> 目标开关。非网格线命令返回 None。
pub fn parse_grid_cmd(cmd: &str) -> Option<bool> {
    match cmd {
        "gridlines_on" => Some(true),
        "gridlines_off" => Some(false),
        _ => None,
    }
}

/// 「视图」页命令入口。返回 true 表示命令已识别(无论执行成败),false 让分发链继续。
pub fn handle(cmd: &str, ctx: &super::RibbonCtx) -> bool {
    if let Some(on) = parse_grid_cmd(cmd) {
        let Some(app) = ctx.app_weak.upgrade() else { return true };
        let sheet = ctx.current_sheet_idx.get();
        let mut s = ctx.state.borrow_mut();
        let Some(sess) = s.session.as_mut() else { return true };
        let cur = sess.get_gridlines(sheet);
        if cur == on {
            eprintln!("[ribbon] calc.{} 网格线未变化 {}", cmd, cur);
            return true;
        }
        sess.begin_edit();
        let ok = sess.set_gridlines(sheet, on);
        if ok { sess.commit_edit(); } else { sess.discard_edit(); }
        eprintln!("[ribbon] calc.{} sheet={} {} -> {} ok={}", cmd, sheet, cur, on, ok);
        app.set_calc_gridlines_on(sess.get_gridlines(sheet));
        if !ok {
            drop(s);
            crate::ui_lang::StatusExt::set_status_msg(&app, "修改网格显示失败".into());
            return true;
        }
        super::calc_home::refresh_view(&app, sess);
        return true;
    }
    let Some(app) = ctx.app_weak.upgrade() else { return false };
    let row = app.get_calc_selected_row().max(0) as u32;
    let col = app.get_calc_selected_col().max(0) as u32;
    let Some(fc) = parse_cmd(cmd, row, col) else { return false };
    let sheet = ctx.current_sheet_idx.get();

    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return true };
    let cur = sess.get_frozen_panes(sheet);
    let (tr, tc) = target(fc, cur);
    if (tr, tc) == cur {
        eprintln!("[ribbon] calc.{} 冻结未变化 {:?}", cmd, cur);
        return true;
    }
    sess.begin_edit();
    let ok = sess.set_frozen_panes(sheet, tr, tc);
    if ok { sess.commit_edit(); } else { sess.discard_edit(); }
    eprintln!("[ribbon] calc.{} sheet={} {:?} -> ({},{}) ok={}", cmd, sheet, cur, tr, tc, ok);
    if !ok { return true; }
    super::super::calc_frozen::refresh_extent(&app, sess, sheet);
    super::calc_home::refresh_view(&app, sess);
    super::calc_home::sync_state_inner(&app, sess, sheet);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_cell_uses_active_cell() {
        assert_eq!(target(FreezeCmd::AtCell { row: 3, col: 2 }, (0, 0)), (3, 2));
    }

    #[test]
    fn grid_cmd_parse() {
        assert_eq!(parse_grid_cmd("gridlines_on"), Some(true));
        assert_eq!(parse_grid_cmd("gridlines_off"), Some(false));
        assert_eq!(parse_grid_cmd("freeze_panes"), None);
        assert_eq!(parse_grid_cmd(""), None);
    }

    #[test]
    fn at_cell_ignores_current_freeze() {
        assert_eq!(target(FreezeCmd::AtCell { row: 5, col: 1 }, (9, 9)), (5, 1));
    }

    #[test]
    fn at_cell_a1_means_unfreeze() {
        assert_eq!(target(FreezeCmd::AtCell { row: 0, col: 0 }, (2, 2)), (0, 0));
    }

    #[test]
    fn top_row_keeps_current_cols() {
        assert_eq!(target(FreezeCmd::TopRow, (0, 0)), (1, 0));
        assert_eq!(target(FreezeCmd::TopRow, (4, 3)), (1, 3));
    }

    #[test]
    fn first_col_keeps_current_rows() {
        assert_eq!(target(FreezeCmd::FirstCol, (0, 0)), (0, 1));
        assert_eq!(target(FreezeCmd::FirstCol, (4, 3)), (4, 1));
    }

    #[test]
    fn unfreeze_is_zero() {
        assert_eq!(target(FreezeCmd::Unfreeze, (7, 8)), (0, 0));
        assert_eq!(target(FreezeCmd::Unfreeze, (0, 0)), (0, 0));
    }

    #[test]
    fn parse_known_commands() {
        assert_eq!(parse_cmd("freeze_panes", 3, 2), Some(FreezeCmd::AtCell { row: 3, col: 2 }));
        assert_eq!(parse_cmd("freeze_top_row", 3, 2), Some(FreezeCmd::TopRow));
        assert_eq!(parse_cmd("freeze_first_col", 3, 2), Some(FreezeCmd::FirstCol));
        assert_eq!(parse_cmd("unfreeze", 3, 2), Some(FreezeCmd::Unfreeze));
    }

    #[test]
    fn parse_unknown_is_none() {
        assert_eq!(parse_cmd("sort_asc", 0, 0), None);
        assert_eq!(parse_cmd("", 0, 0), None);
    }
}
