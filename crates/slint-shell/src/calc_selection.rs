//! Calc 选区逻辑:合并区吸附/扩张/方向键跳块。
//!
//! 全部是纯函数,不依赖 session 与 AppWindow,便于单元测试。
//! 坐标均为 0-based,合并区/选区均为闭区间 (r1, c1, r2, c2)。

/// 合并区域 (r1, c1, r2, c2)。
pub type Merge = (u32, u32, u32, u32);
/// 选区矩形 (r1, c1, r2, c2),已归一化(r1<=r2, c1<=c2)。
pub type Rect = (u32, u32, u32, u32);

/// 方向键。tab/shift-tab 由调用方映射为 Right/Left。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// 返回包含 (r, c) 的合并区(若有)。
pub fn merge_at(merges: &[Merge], r: u32, c: u32) -> Option<Merge> {
    merges
        .iter()
        .copied()
        .find(|&(r1, c1, r2, c2)| r >= r1 && r <= r2 && c >= c1 && c <= c2)
}

/// 把 (r, c) 吸附到所在合并区的左上角;不在合并区内则原样返回。
pub fn snap_anchor(merges: &[Merge], r: u32, c: u32) -> (u32, u32) {
    match merge_at(merges, r, c) {
        Some((r1, c1, _, _)) => (r1, c1),
        None => (r, c),
    }
}

/// 该格所占的矩形:在合并区内返回整个合并区,否则返回 1x1 自身。
pub fn cell_rect(merges: &[Merge], r: u32, c: u32) -> Rect {
    merge_at(merges, r, c).unwrap_or((r, c, r, c))
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.0 <= b.2 && a.2 >= b.0 && a.1 <= b.3 && a.3 >= b.1
}

/// 选区扩张:凡与合并区相交,就扩张到完整包含该合并区(Excel 行为)。
/// 扩张后可能又与别的合并区相交,故循环到不动点。
pub fn expand_for_merges(merges: &[Merge], rect: Rect) -> Rect {
    let mut cur = rect;
    loop {
        let mut changed = false;
        for &m in merges {
            if intersects(cur, m) {
                let next = (cur.0.min(m.0), cur.1.min(m.1), cur.2.max(m.2), cur.3.max(m.3));
                if next != cur {
                    cur = next;
                    changed = true;
                }
            }
        }
        if !changed {
            return cur;
        }
    }
}

/// 方向键移动一步,返回新的锚点坐标(已吸附到目标合并区左上角)。
/// (r, c) 应是当前锚点。起点在合并区内时,以整块为单位移动:
/// 向右/下跳过整块,向左/上从块的边缘再走一格。
/// 向右移动保持行不变,向下移动保持列不变(与无合并区时一致)。
pub fn step(merges: &[Merge], r: u32, c: u32, dir: Dir) -> (u32, u32) {
    let (br1, bc1, br2, bc2) = cell_rect(merges, r, c);
    let (nr, nc) = match dir {
        Dir::Right => (r, bc2 + 1),
        Dir::Down => (br2 + 1, c),
        Dir::Left => (r, bc1.saturating_sub(1)),
        Dir::Up => (br1.saturating_sub(1), c),
    };
    snap_anchor(merges, nr, nc)
}

// ───────────────────────── 与 UI/session 相关的应用层 ─────────────────────────

use super::AppWindow;
use super::PT_TO_PX;

const HEADER_COL_W: f32 = 30.0; // 逻辑 px,与 slint 中 x:30px 一致
const HEADER_ROW_H: f32 = 18.0; // 逻辑 px,与 slint 中 y:18px 一致

fn col_letter(c: u32) -> String {
    super::col_index_to_letter(c)
}

/// 矩形 (r1,c1,r2,c2) 的 pt 坐标与尺寸 (x, y, w, h)。
fn rect_pt(sess: &mut doc_core::TileCacheRust, sheet: u32, rect: Rect) -> (f32, f32, f32, f32) {
    let (r1, c1, r2, c2) = rect;
    let x0 = sess.get_col_offset_pt(sheet, c1);
    let y0 = sess.get_row_offset_pt(sheet, r1);
    let x1 = sess.get_col_offset_pt(sheet, c2) + sess.get_column_width_pt(sheet, c2);
    let y1 = sess.get_row_offset_pt(sheet, r2) + sess.get_row_height_pt(sheet, r2);
    (x0, y0, x1 - x0, y1 - y0)
}

/// 由 pt 矩形与当前滚动,算出屏幕框 (x, y, w, h)(逻辑 px,含 +2 边框余量)。
/// 与 slint 滚动重算公式保持一致。
fn to_screen(app: &AppWindow, pt: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let sx = app.get_calc_scroll_x();
    let sy = app.get_calc_scroll_y();
    (
        pt.0 * PT_TO_PX - sx + HEADER_COL_W - 1.0,
        pt.1 * PT_TO_PX - sy + HEADER_ROW_H - 1.0,
        pt.2 * PT_TO_PX + 2.0,
        pt.3 * PT_TO_PX + 2.0,
    )
}

fn range_address(rect: Rect) -> String {
    let (r1, c1, r2, c2) = rect;
    if r1 == r2 && c1 == c2 {
        format!("{}{}", col_letter(c1), r1 + 1)
    } else {
        format!("{}{}:{}{}", col_letter(c1), r1 + 1, col_letter(c2), r2 + 1)
    }
}

/// 只写"选区整体"相关属性(范围框、pt 矩形、sel-end、地址栏)。
fn write_range(app: &AppWindow, sess: &mut doc_core::TileCacheRust, sheet: u32, rect: Rect) {
    let pt = rect_pt(sess, sheet, rect);
    let (rx, ry, rw, rh) = to_screen(app, pt);
    app.set_calc_sel_x_pt(pt.0);
    app.set_calc_sel_y_pt(pt.1);
    app.set_calc_sel_w_pt(pt.2);
    app.set_calc_sel_h_pt(pt.3);
    app.set_calc_range_x(rx);
    app.set_calc_range_y(ry);
    app.set_calc_range_w(rw);
    app.set_calc_range_h(rh);
    app.set_calc_sel_end_row(rect.2 as i32);
    app.set_calc_sel_end_col(rect.3 as i32);
    app.set_calc_address(range_address(rect).into());
}

/// 只写"锚点格"相关属性(单格框、cell pt)。cell 为锚点格所占矩形(合并区则为整块)。
fn write_cell(app: &AppWindow, sess: &mut doc_core::TileCacheRust, sheet: u32, cell: Rect) {
    let pt = rect_pt(sess, sheet, cell);
    let (hx, hy, hw, hh) = to_screen(app, pt);
    app.set_calc_cell_x_pt(pt.0);
    app.set_calc_cell_y_pt(pt.1);
    app.set_calc_cell_w_pt(pt.2);
    app.set_calc_cell_h_pt(pt.3);
    app.set_calc_highlight_x(hx);
    app.set_calc_highlight_y(hy);
    app.set_calc_highlight_w(hw);
    app.set_calc_highlight_h(hh);
}

/// 点击/键盘导航:选中单个锚点(吸附合并区左上角),选区 = 锚点所在合并区或单格。
/// 写 selected-*、highlight-*、range-*、pt 属性、地址栏、公式栏、按钮态。
pub fn select_anchor(
    app: &AppWindow,
    sess: &mut doc_core::TileCacheRust,
    merges: &[Merge],
    sheet: u32,
    row: u32,
    col: u32,
) {
    let (ar, ac) = snap_anchor(merges, row, col);
    let cell = cell_rect(merges, ar, ac);
    app.set_calc_selected_row(ar as i32);
    app.set_calc_selected_col(ac as i32);
    write_cell(app, sess, sheet, cell);
    write_range(app, sess, sheet, cell);
    super::ribbon::calc_home::refresh_formula_bar(app, sess, sheet, ar, ac);
    super::ribbon::calc_home::sync_state_at(app, sess, sheet, ar, ac);
    super::ribbon::calc_data::dv_list_refresh(app, sess, sheet, ar, ac);
}

/// 拖选:锚点不变(selected-* 保持拖拽起点),终点为 (end_r, end_c);
/// 选区扩张到完整包含相交的合并区。只更新范围相关属性。
pub fn drag_to(
    app: &AppWindow,
    sess: &mut doc_core::TileCacheRust,
    merges: &[Merge],
    sheet: u32,
    anchor: (u32, u32),
    end_r: u32,
    end_c: u32,
) {
    let raw = (
        anchor.0.min(end_r),
        anchor.1.min(end_c),
        anchor.0.max(end_r),
        anchor.1.max(end_c),
    );
    let rect = expand_for_merges(merges, raw);
    write_range(app, sess, sheet, rect);
}

/// 整行/整列/全选的终点(末行下标, 末列下标):
/// 取"数据范围"与"当前视口末行末列"的较大者,再加余量,保证空表/数据范围之外也能铺满可见区域。
/// 视口尺寸为估计值(逻辑 px),多铺无害。
pub fn fill_extent(
    app: &AppWindow,
    sess: &mut doc_core::TileCacheRust,
    sheet: u32,
) -> (u32, u32) {
    const VIEW_W_PX: f32 = 1200.0;
    const VIEW_H_PX: f32 = 600.0;
    const ROW_MARGIN: u32 = 20;
    const COL_MARGIN: u32 = 5;
    let (used_rows, used_cols) = sess.get_used_range(sheet);
    let sx = app.get_calc_scroll_x();
    let sy = app.get_calc_scroll_y();
    let vr = sess.get_row_at_offset(sheet, (sy + VIEW_H_PX) / PT_TO_PX).max(0) as u32;
    let vc = sess.get_col_at_offset(sheet, (sx + VIEW_W_PX) / PT_TO_PX).max(0) as u32;
    let last_row = used_rows.saturating_sub(1).max(vr) + ROW_MARGIN;
    let last_col = used_cols.saturating_sub(1).max(vc) + COL_MARGIN;
    (last_row, last_col)
}

/// 点左上角交叉格:全选。锚点 (0,0),终点 = fill_extent。
pub fn select_all(
    app: &AppWindow,
    sess: &mut doc_core::TileCacheRust,
    merges: &[Merge],
    sheet: u32,
) {
    let (last_row, last_col) = fill_extent(app, sess, sheet);
    select_anchor(app, sess, merges, sheet, 0, 0);
    drag_to(app, sess, merges, sheet, (0, 0), last_row, last_col);
}

/// 点列标题:锚点落 (0,col),选区 = 整列(第 0 行到 last_row)。
/// last_row 由调用方按 used_rows-1 传入(空表为 0)。选区穿过合并区时由 drag_to 自动扩张。
pub fn select_column(
    app: &AppWindow,
    sess: &mut doc_core::TileCacheRust,
    merges: &[Merge],
    sheet: u32,
    col: u32,
    last_row: u32,
) {
    select_anchor(app, sess, merges, sheet, 0, col);
    let anchor = (
        app.get_calc_selected_row().max(0) as u32,
        app.get_calc_selected_col().max(0) as u32,
    );
    drag_to(app, sess, merges, sheet, anchor, last_row, col);
}

/// 点行标题:锚点落 (row,0),选区 = 整行(第 0 列到 last_col)。
/// last_col 由调用方按 used_cols-1 传入(空表为 0)。
pub fn select_row(
    app: &AppWindow,
    sess: &mut doc_core::TileCacheRust,
    merges: &[Merge],
    sheet: u32,
    row: u32,
    last_col: u32,
) {
    select_anchor(app, sess, merges, sheet, row, 0);
    let anchor = (
        app.get_calc_selected_row().max(0) as u32,
        app.get_calc_selected_col().max(0) as u32,
    );
    drag_to(app, sess, merges, sheet, anchor, row, last_col);
}

/// 结构/格式变化(换行、合并、插入删除行列、撤销重做)后,按当前选区重算框。
/// 读 slint 里的锚点与终点,不改变选区语义,只刷新几何。
pub fn reapply(
    app: &AppWindow,
    sess: &mut doc_core::TileCacheRust,
    merges: &[Merge],
    sheet: u32,
) {
    let ar = app.get_calc_selected_row();
    let ac = app.get_calc_selected_col();
    if ar < 0 || ac < 0 {
        return;
    }
    let (ar, ac) = snap_anchor(merges, ar as u32, ac as u32);
    let cell = cell_rect(merges, ar, ac);
    app.set_calc_selected_row(ar as i32);
    app.set_calc_selected_col(ac as i32);
    write_cell(app, sess, sheet, cell);

    let er = app.get_calc_sel_end_row();
    let ec = app.get_calc_sel_end_col();
    let raw = if er < 0 || ec < 0 {
        cell
    } else {
        (
            ar.min(er as u32),
            ac.min(ec as u32),
            ar.max(er as u32),
            ac.max(ec as u32),
        )
    };
    let rect = expand_for_merges(merges, raw);
    write_range(app, sess, sheet, rect);
}

/// 从内核刷新合并区缓存。
pub fn refresh_merges(merges: &mut Vec<Merge>, sess: &mut doc_core::TileCacheRust, sheet: u32) {
    *merges = sess.calc_get_merged_cells(sheet);
}

#[cfg(test)]
mod tests {
    use super::*;

    // 合并区 B2:C3  => (1,1,2,2)
    fn one() -> Vec<Merge> {
        vec![(1, 1, 2, 2)]
    }

    #[test]
    fn snap_inside_merge_goes_to_top_left() {
        assert_eq!(snap_anchor(&one(), 2, 2), (1, 1));
        assert_eq!(snap_anchor(&one(), 1, 2), (1, 1));
        assert_eq!(snap_anchor(&one(), 2, 1), (1, 1));
    }

    #[test]
    fn snap_outside_merge_unchanged() {
        assert_eq!(snap_anchor(&one(), 0, 0), (0, 0));
        assert_eq!(snap_anchor(&one(), 3, 3), (3, 3));
    }

    #[test]
    fn cell_rect_returns_whole_merge_or_self() {
        assert_eq!(cell_rect(&one(), 2, 2), (1, 1, 2, 2));
        assert_eq!(cell_rect(&one(), 5, 5), (5, 5, 5, 5));
    }

    #[test]
    fn expand_includes_partially_overlapping_merge() {
        // 选 A1:B2 与合并区 B2:C3 相交 => 扩张到 A1:C3
        assert_eq!(expand_for_merges(&one(), (0, 0, 1, 1)), (0, 0, 2, 2));
    }

    #[test]
    fn expand_leaves_disjoint_rect_alone() {
        assert_eq!(expand_for_merges(&one(), (5, 5, 6, 6)), (5, 5, 6, 6));
    }

    #[test]
    fn expand_chains_through_multiple_merges() {
        // B2:C3 与 D3:E4;选 A1:B2 扩张后含 B2:C3 => (0,0,2,2),
        // 该矩形与 D3:E4 不相交(列 3>2),不再扩张
        let m = vec![(1, 1, 2, 2), (2, 3, 3, 4)];
        assert_eq!(expand_for_merges(&m, (0, 0, 1, 1)), (0, 0, 2, 2));
        // 选 A1:D3:与 B2:C3 相交并含 D3,与 D3:E4 相交 => 连锁扩张到 (0,0,3,4)
        assert_eq!(expand_for_merges(&m, (0, 0, 2, 3)), (0, 0, 3, 4));
    }

    #[test]
    fn step_right_from_merge_skips_whole_block() {
        // 锚点 B2(1,1),向右应落到 D2(1,3),不是 C2
        assert_eq!(step(&one(), 1, 1, Dir::Right), (1, 3));
    }

    #[test]
    fn step_down_from_merge_skips_whole_block() {
        assert_eq!(step(&one(), 1, 1, Dir::Down), (3, 1));
    }

    #[test]
    fn step_left_into_merge_lands_on_top_left() {
        // 从 D3(2,3) 向左,目标 C3(2,2) 在合并区内 => 吸附到 B2(1,1)
        assert_eq!(step(&one(), 2, 3, Dir::Left), (1, 1));
    }

    #[test]
    fn step_up_into_merge_lands_on_top_left() {
        // 从 C4(3,2) 向上,目标 C3(2,2) 在合并区内 => 吸附到 B2(1,1)
        assert_eq!(step(&one(), 3, 2, Dir::Up), (1, 1));
    }

    #[test]
    fn step_left_from_merge_leaves_block() {
        // 锚点 B2(1,1) 向左 => A2(1,0)
        assert_eq!(step(&one(), 1, 1, Dir::Left), (1, 0));
    }

    #[test]
    fn step_up_from_merge_leaves_block() {
        assert_eq!(step(&one(), 1, 1, Dir::Up), (0, 1));
    }

    #[test]
    fn step_clamps_at_origin() {
        assert_eq!(step(&[], 0, 0, Dir::Left), (0, 0));
        assert_eq!(step(&[], 0, 0, Dir::Up), (0, 0));
    }

    #[test]
    fn step_without_merges_is_plain_move() {
        assert_eq!(step(&[], 3, 3, Dir::Right), (3, 4));
        assert_eq!(step(&[], 3, 3, Dir::Down), (4, 3));
        assert_eq!(step(&[], 3, 3, Dir::Left), (3, 2));
        assert_eq!(step(&[], 3, 3, Dir::Up), (2, 3));
    }
}
