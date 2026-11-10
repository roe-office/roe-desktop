//! 行列标题分隔线缩放的纯逻辑(命中、尺寸夹取、目标展开)。
//! 位置与尺寸一律为 pt;容差、缩放换算由调用方负责。
#![allow(dead_code)]

/// 鼠标位置 `pos`(内容坐标,pt)是否落在某条分隔线的 `tol` 范围内。
/// 命中某列右边界返回该列;命中某列左边界返回前一列;第 0 列的左边界不命中。
/// `index_at`:坐标到列/行号(负数表示越界);`offset_of`:列/行号到起点偏移。
pub fn hit_edge(
    pos: f32,
    tol: f32,
    index_at: impl Fn(f32) -> i32,
    offset_of: impl Fn(u32) -> f32,
) -> Option<u32> {
    if pos < 0.0 {
        return None;
    }
    let c = index_at(pos);
    if c < 0 {
        return None;
    }
    let c = c as u32;
    // 右边界:命中本列
    if (offset_of(c + 1) - pos).abs() <= tol {
        return Some(c);
    }
    // 左边界:命中前一列(第 0 列的左边界不可缩放)
    if c >= 1 && (pos - offset_of(c)).abs() <= tol {
        return Some(c - 1);
    }
    None
}

/// 新尺寸 = 按下时尺寸 + 位移(px) / 每 pt 的像素数,不小于 `min_pt`。
pub fn new_size(orig_pt: f32, delta_px: f32, px_per_pt: f32, min_pt: f32) -> f32 {
    (orig_pt + delta_px / px_per_pt).max(min_pt)
}

/// 被拖的列/行在已选整列(整行)范围内时,返回所有选中范围,否则只返回它自己。
/// `selected` 只应包含整列(整行)类型的范围,闭区间 (起, 止)。
pub fn resize_targets(dragged: u32, selected: &[(u32, u32)]) -> Vec<(u32, u32)> {
    if selected.iter().any(|&(a, b)| dragged >= a && dragged <= b) {
        selected.to_vec()
    } else {
        vec![(dragged, dragged)]
    }
}

/// 由锚点与终点反推"整列/整行选区"。
/// 交叉轴(列选区看行,行选区看列)从 0 覆盖到 `last_cross` 及以上,返回主轴的 (起, 止);否则 None。
/// 局限:空表里单击 A1 与选中整列 A 无法区分,按整列处理。
pub fn full_span(
    a_cross: u32,
    e_cross: u32,
    last_cross: u32,
    a_main: u32,
    e_main: u32,
) -> Option<(u32, u32)> {
    if a_cross.min(e_cross) == 0 && a_cross.max(e_cross) >= last_cross {
        Some((a_main.min(e_main), a_main.max(e_main)))
    } else {
        None
    }
}

/// 鼠标所在的标题区。
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Zone {
    ColHeader,
    RowHeader,
    Other,
}

/// 按逻辑 px 判断鼠标在列标题、行标题还是其他区域。左上角交叉格算 Other。
/// `header_col_w`:行标题栏宽度;`header_row_h`:列标题栏高度。
pub fn zone(x: f32, y: f32, header_col_w: f32, header_row_h: f32) -> Zone {
    if x < 0.0 || y < 0.0 {
        return Zone::Other;
    }
    if y < header_row_h && x >= header_col_w {
        Zone::ColHeader
    } else if x < header_col_w && y >= header_row_h {
        Zone::RowHeader
    } else {
        Zone::Other
    }
}

/// 一次标题缩放拖拽的状态(按下时记录,松手时使用)。
#[derive(Debug, Clone, PartialEq)]
pub struct HeaderDrag {
    pub is_col: bool,
    pub index: u32,
    pub orig_pt: f32,
    pub start_px: f32,
    pub targets: Vec<(u32, u32)>,
}

/// 标题栏鼠标位置(逻辑px,含标题栏) -> 文档 pt,用于分隔线命中。
/// 冻结区(含冻结线 tol_px 范围内,最后一个冻结列/行的右/下边界)不加滚动量。
pub fn edge_pos_pt(pos_px: f32, hdr: f32, scroll: f32, f: f32, tol_px: f32, px_per_pt: f32) -> f32 {
    let v = pos_px - hdr;
    let in_frozen = f > 0.0 && v < f + tol_px;
    (if in_frozen { v } else { v + scroll }) / px_per_pt
}

/// 拖动引导线的标题栏坐标(逻辑px)。按被拖列/行的起点判断区域:
/// 起点在冻结区则不减滚动量,否则减。
pub fn guide_px(hdr: f32, off_pt: f32, new_pt: f32, scroll: f32, f: f32, px_per_pt: f32) -> f32 {
    let start_px = off_pt * px_per_pt;
    let in_frozen = f > 0.0 && start_px < f;
    let end_px = (off_pt + new_pt) * px_per_pt;
    hdr + if in_frozen { end_px } else { end_px - scroll }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 每列 64pt 的均匀网格
    fn idx(x: f32) -> i32 { (x / 64.0).floor() as i32 }
    fn off(c: u32) -> f32 { c as f32 * 64.0 }
    fn close(a: f32, b: f32) -> bool { (a - b).abs() < 1e-3 }

    #[test]
    fn edge_pos_no_freeze_equals_original() {
        assert!(close(edge_pos_pt(130.0, 30.0, 50.0, 0.0, 4.0, 2.0), 75.0));
    }

    #[test]
    fn edge_pos_in_frozen_ignores_scroll() {
        assert!(close(edge_pos_pt(90.0, 30.0, 500.0, 100.0, 4.0, 2.0), 30.0));
    }

    #[test]
    fn edge_pos_in_scroll_area_adds_scroll() {
        assert!(close(edge_pos_pt(230.0, 30.0, 500.0, 100.0, 4.0, 2.0), 350.0));
    }

    #[test]
    fn edge_pos_near_frozen_line_belongs_to_frozen() {
        assert!(close(edge_pos_pt(132.0, 30.0, 500.0, 100.0, 4.0, 2.0), 51.0));
    }

    #[test]
    fn guide_no_freeze_equals_original() {
        assert!(close(guide_px(30.0, 40.0, 10.0, 5.0, 0.0, 2.0), 125.0));
    }

    #[test]
    fn guide_frozen_col_ignores_scroll() {
        assert!(close(guide_px(30.0, 40.0, 10.0, 500.0, 100.0, 2.0), 130.0));
    }

    #[test]
    fn guide_last_frozen_col_end_on_line_ignores_scroll() {
        // 起点 80px < f=100,右缘 = 100px 恰在冻结线上
        assert!(close(guide_px(30.0, 40.0, 10.0, 500.0, 100.0, 2.0), 130.0));
    }

    #[test]
    fn guide_scroll_col_subtracts_scroll() {
        assert!(close(guide_px(30.0, 100.0, 10.0, 500.0, 100.0, 2.0), -250.0));
    }


    #[test]
    fn zone_column_header() {
        assert_eq!(zone(100.0, 5.0, 30.0, 18.0), Zone::ColHeader);
    }

    #[test]
    fn zone_row_header() {
        assert_eq!(zone(10.0, 100.0, 30.0, 18.0), Zone::RowHeader);
    }

    #[test]
    fn zone_corner_is_other() {
        assert_eq!(zone(10.0, 5.0, 30.0, 18.0), Zone::Other);
    }

    #[test]
    fn zone_data_area_is_other() {
        assert_eq!(zone(100.0, 100.0, 30.0, 18.0), Zone::Other);
    }

    #[test]
    fn zone_boundary_belongs_to_data_side() {
        assert_eq!(zone(100.0, 18.0, 30.0, 18.0), Zone::Other);
        assert_eq!(zone(30.0, 100.0, 30.0, 18.0), Zone::Other);
    }

    #[test]
    fn full_span_whole_columns() {
        assert_eq!(full_span(0, 10, 10, 2, 4), Some((2, 4)));
    }

    #[test]
    fn full_span_handles_reversed_ends() {
        assert_eq!(full_span(10, 0, 10, 4, 2), Some((2, 4)));
    }

    #[test]
    fn full_span_single_whole_column() {
        assert_eq!(full_span(0, 10, 10, 3, 3), Some((3, 3)));
    }

    #[test]
    fn full_span_rejects_partial_cross_axis() {
        assert_eq!(full_span(1, 10, 10, 2, 4), None);
        assert_eq!(full_span(0, 5, 10, 2, 4), None);
    }

    #[test]
    fn hit_right_edge_of_col0() {
        assert_eq!(hit_edge(63.0, 3.0, idx, off), Some(0));
        assert_eq!(hit_edge(62.0, 3.0, idx, off), Some(0));
    }

    #[test]
    fn hit_just_past_edge_still_belongs_to_left_col() {
        assert_eq!(hit_edge(64.5, 3.0, idx, off), Some(0));
        assert_eq!(hit_edge(129.0, 3.0, idx, off), Some(1));
    }

    #[test]
    fn middle_of_cell_is_not_an_edge() {
        assert_eq!(hit_edge(32.0, 3.0, idx, off), None);
        assert_eq!(hit_edge(100.0, 3.0, idx, off), None);
    }

    #[test]
    fn left_edge_of_first_col_is_not_resizable() {
        assert_eq!(hit_edge(1.0, 3.0, idx, off), None);
    }

    #[test]
    fn out_of_range_is_none() {
        assert_eq!(hit_edge(-5.0, 3.0, idx, off), None);
    }

    #[test]
    fn new_size_adds_scaled_delta() {
        assert!(close(new_size(64.0, 10.0, 1.3333334, 3.0), 71.5));
        assert!(close(new_size(64.0, -8.0, 1.3333334, 3.0), 58.0));
    }

    #[test]
    fn new_size_is_clamped_to_min() {
        assert!(close(new_size(64.0, -9999.0, 1.3333334, 3.0), 3.0));
    }

    #[test]
    fn new_size_respects_zoom() {
        // 缩放 2 倍时,同样的像素位移对应一半的 pt
        assert!(close(new_size(64.0, 20.0, 2.6666667, 3.0), 71.5));
    }

    #[test]
    fn targets_expand_to_all_selected_ranges() {
        assert_eq!(resize_targets(2, &[(1, 3), (7, 8)]), vec![(1, 3), (7, 8)]);
    }

    #[test]
    fn targets_only_dragged_when_outside_selection() {
        assert_eq!(resize_targets(5, &[(1, 3), (7, 8)]), vec![(5, 5)]);
    }

    #[test]
    fn targets_without_selection_is_dragged_only() {
        assert_eq!(resize_targets(4, &[]), vec![(4, 4)]);
    }
}
