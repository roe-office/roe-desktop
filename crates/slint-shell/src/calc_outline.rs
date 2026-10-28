//! Calc 分类汇总折叠交互:数据行区间扫描 + 折叠状态精确翻转记录。
//!
//! 设计依据: roe-ext_subtotal_collapse_design_v2.md
//! 全部是纯函数/纯数据结构,不依赖 session 与 AppWindow,便于单元测试。
//! 坐标/行号均为 0-based。

/// 找出汇总行 sum_row 统领的所有行 [start_row, end_row](闭区间)。
///
/// 语义(支持任意层级嵌套):取 sum_row 自身 level 为 L,沿明细所在方向逐行扫描,
/// level > L 的行都属于该分组(含更深层明细和中间层子汇总行),遇到第一个
/// level <= L 的行停止。
///
/// 方向由 `summary_below` 决定(对应 xlsx 的 outlinePr summaryBelow 属性):
/// - true : 汇总行在明细下方,向上扫描(Excel 默认)。
/// - false: 汇总行在明细上方,向下扫描,最多扫到 `max_row`(含,0-based,通常传 used range 最大行)。
///
/// 例(汇总在上, 0=表头/总计, 1=每日汇总, 2=明细):
/// - 折叠每日汇总行(L=1): 只覆盖该日明细。
/// - 折叠总计行(L=0): 覆盖整张表体(所有子汇总行 + 明细),仅保留总计行本身。
///
/// 返回 None: 汇总行在扫描方向上没有可折叠的行(空分组、已在表顶/表底)。
pub fn scan_group_rows(
    get_level: impl Fn(u32) -> u8,
    sum_row: u32,
    summary_below: bool,
    max_row: u32,
) -> Option<(u32, u32)> {
    let base = get_level(sum_row);
    if summary_below {
        if sum_row == 0 {
            return None;
        }
        let end_row = sum_row - 1;
        if get_level(end_row) <= base {
            return None;
        }
        let mut r = end_row;
        loop {
            if r == 0 || get_level(r - 1) <= base {
                return Some((r, end_row));
            }
            r -= 1;
        }
    } else {
        let start_row = sum_row + 1;
        if start_row > max_row || get_level(start_row) <= base {
            return None;
        }
        let mut r = start_row;
        while r < max_row && get_level(r + 1) > base {
            r += 1;
        }
        Some((start_row, r))
    }
}

/// 折叠/展开的精确翻转记录。
///
/// 设计依据: v2 §2.3。核心原则:折叠时只标记"确实由本次操作导致从可见变隐藏"的行;
/// 展开时只翻转"记录里存在"的行,不无差别覆盖整段区间,避免误伤用户手动筛选隐藏的行。
///
/// 本结构不做任何 IO/FFI 调用,只负责"要不要对某一行调用 set_row_hidden"这个决策,
/// 实际调用由持有本结构的调用方(main.rs)负责。
///
/// 内部维护两份记录,职责不同:
/// - `hidden_rows`: 哪些"数据行"是被我们的折叠操作弄隐藏的(用于展开时精确翻转,
///   不误伤筛选隐藏的行)。
/// - `collapsed_groups`: 哪些"汇总行"当前处于折叠态(用于UI画▶还是▼,是群体层面
///   的状态,不等同于hidden_rows里任何一行本身的隐藏与否)。
/// 两者一起维护、一起清除,但查询用途不同,不能合并成一份数据,否则UI状态查询要
/// 反推"这个汇总行下面有没有行被标记",既绕远又在多级嵌套场景下容易出错。
#[derive(Default, Debug, Clone)]
pub struct CollapseTracker {
    hidden_rows: std::collections::HashSet<(u32, u32)>,
    collapsed_groups: std::collections::HashSet<(u32, u32)>,
}

impl CollapseTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 折叠一个分组。调用方需先用 scan_group_rows 算出 [start_row, end_row],
    /// 再对区间内每一行调用 `currently_hidden_fn(row)` 查询当前真实隐藏状态并传入,
    /// 本方法返回"需要执行 set_row_hidden(row, true) 的行列表"(即那些当前可见、
    /// 需要被我们隐藏的行;已经隐藏的行——多半是筛选造成的——跳过,不重复设置也不记录)。
    ///
    /// sum_row 会被记入 collapsed_groups,供UI查询展开/折叠态。
    pub fn collapse_group(
        &mut self,
        sheet_idx: u32,
        sum_row: u32,
        start_row: u32,
        end_row: u32,
        mut currently_hidden_fn: impl FnMut(u32) -> bool,
    ) -> Vec<u32> {
        let mut need_hide = Vec::new();
        if end_row >= start_row {
            for row in start_row..=end_row {
                if !currently_hidden_fn(row) {
                    self.hidden_rows.insert((sheet_idx, row));
                    need_hide.push(row);
                }
            }
        }
        self.collapsed_groups.insert((sheet_idx, sum_row));
        need_hide
    }

    /// 展开一个分组,同一区间 [start_row, end_row]。
    /// 返回"需要执行 set_row_hidden(row, false) 的行列表"——只包含当初被我们标记为
    /// hidden_rows 的行,不在记录里的行(比如折叠期间被用户筛选隐藏的)不动。
    pub fn expand_group(&mut self, sheet_idx: u32, sum_row: u32, start_row: u32, end_row: u32) -> Vec<u32> {
        let mut need_show = Vec::new();
        if end_row >= start_row {
            for row in start_row..=end_row {
                if self.hidden_rows.remove(&(sheet_idx, row)) {
                    need_show.push(row);
                }
            }
        }
        self.collapsed_groups.remove(&(sheet_idx, sum_row));
        need_show
    }

    /// 查询某个汇总行对应的分组当前是否处于折叠态(true=折叠/画▶, false=展开/画▼)。
    pub fn group_is_collapsed(&self, sheet_idx: u32, sum_row: u32) -> bool {
        self.collapsed_groups.contains(&(sheet_idx, sum_row))
    }

    /// 清空某个 sheet 的所有折叠记录(该 sheet 行结构被整体重建时调用,如分类汇总清除/新建;
    /// 记录 key 本身带 sheet_idx,切 sheet 无需清;换文档由 finish_open 整体重置 tracker)。
    pub fn clear_sheet(&mut self, sheet_idx: u32) {
        self.hidden_rows.retain(|(s, _)| *s != sheet_idx);
        self.collapsed_groups.retain(|(s, _)| *s != sheet_idx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- scan_group_rows ----
    // 汇总在下(summary_below=true)

    #[test]
    fn below_stops_at_header_row_above() {
        let levels = [0u8, 1, 0];
        assert_eq!(scan_group_rows(|r| levels[r as usize], 2, true, 2), Some((1, 1)));
    }

    #[test]
    fn below_multi_group_stops_at_previous_summary_row() {
        let levels = [0u8, 1, 1, 0, 1, 1, 0];
        let g = |r: u32| levels[r as usize];
        assert_eq!(scan_group_rows(g, 6, true, 6), Some((4, 5)));
        assert_eq!(scan_group_rows(g, 3, true, 6), Some((1, 2)));
    }

    #[test]
    fn below_top_boundary_first_group_touches_sheet_top() {
        let levels = [1u8, 1, 0];
        assert_eq!(scan_group_rows(|r| levels[r as usize], 2, true, 2), Some((0, 1)));
    }

    #[test]
    fn below_single_row_group_at_top() {
        let levels = [1u8, 0];
        assert_eq!(scan_group_rows(|r| levels[r as usize], 1, true, 1), Some((0, 0)));
    }

    #[test]
    fn below_sum_row_zero_returns_none() {
        assert_eq!(scan_group_rows(|_| 0u8, 0, true, 0), None);
    }

    #[test]
    fn below_three_level() {
        // 0表头 1明细 2明细 3周一汇总 4明细 5明细 6周二汇总 7总计
        let levels = [0u8, 2, 2, 1, 2, 2, 1, 0];
        let g = |r: u32| levels[r as usize];
        assert_eq!(scan_group_rows(g, 3, true, 7), Some((1, 2)));
        assert_eq!(scan_group_rows(g, 6, true, 7), Some((4, 5)));
        assert_eq!(scan_group_rows(g, 7, true, 7), Some((1, 6)));
    }

    #[test]
    fn below_empty_group_returns_none() {
        let levels = [0u8, 1, 1];
        assert_eq!(scan_group_rows(|r| levels[r as usize], 2, true, 2), None);
    }

    // 汇总在上(summary_below=false),真机样本 subtotal-above.xlsx 的前 8 行结构
    // 0表头 1总计 2周一汇总 3明细 4明细 5周二汇总 6明细 7明细
    const ABOVE: [u8; 8] = [0, 0, 1, 2, 2, 1, 2, 2];

    #[test]
    fn above_daily_summary_covers_only_its_details() {
        let g = |r: u32| ABOVE[r as usize];
        assert_eq!(scan_group_rows(g, 2, false, 7), Some((3, 4)));
        // 关键回归: 周二汇总(row5)上下都是明细,必须向下取自己的 6..7,不能取周一的 3..4
        assert_eq!(scan_group_rows(g, 5, false, 7), Some((6, 7)));
    }

    #[test]
    fn above_grand_total_covers_whole_body() {
        let g = |r: u32| ABOVE[r as usize];
        assert_eq!(scan_group_rows(g, 1, false, 7), Some((2, 7)));
    }

    #[test]
    fn above_group_at_sheet_bottom_stops_at_max_row() {
        // 最后一组紧贴表尾,不能越界
        let levels = [0u8, 1, 2, 2];
        let g = |r: u32| levels[r as usize];
        assert_eq!(scan_group_rows(g, 1, false, 3), Some((2, 3)));
    }

    #[test]
    fn above_empty_group_or_last_row_returns_none() {
        let levels = [0u8, 1, 1];
        let g = |r: u32| levels[r as usize];
        assert_eq!(scan_group_rows(g, 1, false, 2), None); // 下一行同级
        assert_eq!(scan_group_rows(g, 2, false, 2), None); // 已是最后一行
    }

    // ---- CollapseTracker ----

    #[test]
    fn collapse_marks_visible_rows_and_returns_need_hide_list() {
        let mut t = CollapseTracker::new();
        // 区间[3,5]全部当前可见(currently_hidden始终false)
        let need_hide = t.collapse_group(0, 6, 3, 5, |_row| false);
        assert_eq!(need_hide, vec![3, 4, 5]);
        assert!(t.group_is_collapsed(0, 6));
    }

    #[test]
    fn collapse_skips_rows_already_hidden_by_filter() {
        let mut t = CollapseTracker::new();
        // row4 已经被筛选隐藏,不应出现在need_hide里,也不应被我们记录
        let need_hide = t.collapse_group(0, 6, 3, 5, |row| row == 4);
        assert_eq!(need_hide, vec![3, 5]);
    }

    #[test]
    fn expand_only_returns_rows_we_hid_ourselves() {
        let mut t = CollapseTracker::new();
        t.collapse_group(0, 6, 3, 5, |row| row == 4); // row4是筛选隐藏的,不记录
        let need_show = t.expand_group(0, 6, 3, 5);
        // 只有3和5是我们隐藏的,row4不在里面(不应被强制翻回可见)
        assert_eq!(need_show, vec![3, 5]);
        assert!(!t.group_is_collapsed(0, 6));
    }

    #[test]
    fn group_is_collapsed_false_before_any_collapse() {
        let t = CollapseTracker::new();
        assert!(!t.group_is_collapsed(0, 6));
    }

    #[test]
    fn clear_sheet_only_affects_that_sheet() {
        let mut t = CollapseTracker::new();
        t.collapse_group(0, 6, 3, 5, |_| false);
        t.collapse_group(1, 6, 3, 5, |_| false);
        t.clear_sheet(0);
        assert!(!t.group_is_collapsed(0, 6));
        assert!(t.group_is_collapsed(1, 6));
        // sheet0展开时应该查不到任何需要翻转的行了(记录已清空)
        assert_eq!(t.clone().expand_group(0, 6, 3, 5), Vec::<u32>::new());
    }

    #[test]
    fn collapse_then_expand_then_collapse_again_is_consistent() {
        let mut t = CollapseTracker::new();
        t.collapse_group(0, 6, 3, 5, |_| false);
        t.expand_group(0, 6, 3, 5);
        // 再次折叠,应该和第一次行为一致(不会因为"曾经折叠过"而产生残留状态)
        let need_hide = t.collapse_group(0, 6, 3, 5, |_| false);
        assert_eq!(need_hide, vec![3, 4, 5]);
        assert!(t.group_is_collapsed(0, 6));
    }
}
