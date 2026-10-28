//! Calc「数据」页按钮。

use super::calc_home::{read_selection, reflow_selection, refresh_view, sync_state_inner};
use super::RibbonCtx;

/// 处理「数据」页命令。返回 true 表示已处理,false 交给其它页继续分发。
pub fn handle(cmd: &str, ctx: &RibbonCtx) -> bool {
    if cmd == "name_manager" {
        super::calc_names::open(ctx);
        return true;
    }
    if let Some(k) = cmd.strip_prefix("stat:") {
        super::calc_stat::open(ctx, k.parse().unwrap_or(usize::MAX));
        return true;
    }
    match cmd {
        "sort_asc" => {
            sort_selection(ctx, true);
            true
        }
        "sort_desc" => {
            sort_selection(ctx, false);
            true
        }
        "autosum" => {
            autosum(ctx);
            true
        }
        "delete_dups" => {
            delete_dups(ctx);
            true
        }
        "subtotal" => {
            subtotal_open(ctx);
            true
        }
        "text_to_columns" => {
            text_split_open(ctx);
            true
        }
        "df_open" => {
            super::calc_df::open(ctx);
            true
        }
        "df_open_parquet" => {
            super::calc_df::open_file(ctx, "parquet");
            true
        }
        "df_open_csv" => {
            super::calc_df::open_file(ctx, "csv");
            true
        }
        "df_open_xlsx" => {
            super::calc_df::open_file(ctx, "xlsx");
            true
        }
        "df_sql_open" => {
            super::calc_df::open_sql(ctx);
            true
        }
        "df_chain_open" => {
            super::calc_df::open_chain(ctx);
            true
        }
        "data_validation" => {
            dv_open(ctx);
            true
        }
        "filter" => {
            filter_open(ctx);
            true
        }
        c if c.starts_with("fn:") => {
            insert_function(ctx, &c[3..]);
            true
        }
        "filter_toggle" | "filter_clear" | "filter_reapply" => {
            filter_cmd(cmd, ctx);
            true
        }
        _ => false,
    }
}

/// 函数库:把 `NAME(` 写入编辑框。
/// - 非编辑态:选中格进入编辑,内容替换为 `=NAME(`(Esc 可恢复原值);
/// - 编辑态且是公式:先去掉末尾正在键入的字母前缀(如 `=SU` → `=`),再追加 `NAME(`;
/// - 编辑态但是非公式的普通文本:不处理,避免覆盖用户输入。
fn insert_function(ctx: &RibbonCtx, name: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    if app.get_calc_selected_col() < 0 || app.get_calc_selected_row() < 0 {
        eprintln!("[ribbon] calc.fn: 无选中单元格");
        return;
    }
    let new_text = if app.get_calc_editing() {
        let cur = app.get_calc_display_value().to_string();
        if cur.is_empty() {
            format!("={}(", name)
        } else if let Some(rest) = cur.strip_prefix('=') {
            let keep = rest.trim_end_matches(|c: char| c.is_ascii_alphabetic());
            format!("={}{}(", keep, name)
        } else {
            eprintln!("[ribbon] calc.fn: 正在编辑普通文本,忽略");
            return;
        }
    } else {
        format!("={}(", name)
    };
    let cursor = new_text.len() as i32;
    eprintln!("[ribbon] calc.fn {} -> {:?}", name, new_text);
    app.invoke_calc_set_edit_text(new_text.into(), cursor);
}

/// 对选区排序。规则:
/// - 单格选区:忽略(自动扩展到连续数据区记入遗留);
/// - 选区与任一合并区相交:拒绝(内核 sort_range 不处理合并区);
/// - 键列 = 锚点所在列;
/// - 整个选区都参与排序(表头判断记入遗留)。
fn sort_selection(ctx: &RibbonCtx, ascending: bool) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.sort: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();

    let mut s = ctx.state.borrow_mut();
    let merges = s.calc_merges.clone();
    let Some(sess) = s.session.as_mut() else { return };

    // 单格 / 单列 / 单行选区:扩展到连续数据区(与删除重复值一致)。
    // 多行多列选区视为用户明确框定,不扩展、也不判断表头。
    let expanded = r1 == r2 || c1 == c2;
    let (r1, c1, r2, c2) = if expanded {
        match expand_to_region(sess, sheet, r1, c1, "calc.sort") {
            Some((er1, ec1, er2, ec2)) => {
                eprintln!(
                    "[ribbon] calc.sort expanded=true ({},{})-({},{}) -> ({},{})-({},{})",
                    r1, c1, r2, c2, er1, ec1, er2, ec2
                );
                (er1, ec1, er2, ec2)
            }
            None => {
                eprintln!("[ribbon] calc.sort: 未探测到连续数据区,忽略");
                return;
            }
        }
    } else {
        (r1, c1, r2, c2)
    };
    if r1 == r2 && c1 == c2 {
        eprintln!("[ribbon] calc.sort: 连续数据区只有一个单元格,忽略");
        return;
    }

    // 仅在"自动扩展"时判断表头:是表头则排序范围从第二行起,表头保持不动。
    let r1 = if expanded {
        let raw = sess.query_script(&format!(
            "detect_header_row({}, {}, {}, {}, {})",
            sheet, r1, c1, r2, c2
        ));
        match parse_header(&raw) {
            Some(true) => {
                eprintln!("[ribbon] calc.sort header=true, 排序范围起点 {} -> {}", r1, r1 + 1);
                r1 + 1
            }
            Some(false) => {
                eprintln!("[ribbon] calc.sort header=false, 整个区域参与排序");
                r1
            }
            None => {
                eprintln!("[ribbon] calc.sort: detect_header_row 无法解析,按无表头处理: {}", raw);
                r1
            }
        }
    } else {
        r1
    };
    if r1 >= r2 {
        eprintln!("[ribbon] calc.sort: 跳过表头后只剩一行,无需排序");
        return;
    }

    // 合并区相交检查(闭区间相交),用最终排序范围(已跳过表头)
    let hit_merge = merges
        .iter()
        .any(|&(mr1, mc1, mr2, mc2)| mr1 <= r2 && mr2 >= r1 && mc1 <= c2 && mc2 >= c1);
    if hit_merge {
        eprintln!("[ribbon] calc.sort: 选区与合并单元格相交,已拒绝");
        return;
    }

    // 键列 = 锚点列相对(扩展后)选区起始列的偏移
    let anchor_col = app.get_calc_selected_col();
    let key_col: i64 = if anchor_col >= c1 as i32 && anchor_col <= c2 as i32 {
        (anchor_col - c1 as i32) as i64
    } else {
        0
    };

    // sheet 名动态查询(2026-09-22修复:此前写死"Sheet1",UI目前虽无重命名sheet
    // 入口、sheet_idx<->名字稳定绑定,但写死表名遇到非默认sheet名会直接排错表/
    // 排序静默失败,属于随时会踩的真实坑,顺手一次性修掉)。
    let sheet_name = sess.get_sheet_name(sheet);
    let script = format!(
        "sort_range({}, `{{\"sheet\":\"{}\",\"start\":[{},{}],\"end\":[{},{}]}}`, {}, {});",
        sheet, sheet_name, r1, c1, r2, c2, key_col, ascending
    );
    let res = sess.execute_script(&script);
    eprintln!(
        "[ribbon] calc.sort ({},{})-({},{}) key_col={} asc={} 返回: {}",
        r1, c1, r2, c2, key_col, ascending, res
    );
    if !res.contains("\"ok\":true") {
        return;
    }
    refresh_view(&app, sess);
    sync_state_inner(&app, sess, sheet);
    // 排序会改变自动行高,按当前选区重算高亮框
    reflow_selection(&app, &mut s, sheet);
}

/// 解析 detect_autosum_range 的返回。query_script 的外壳实测为
/// {"ok":true,"value":"<字符串化 JSON>"};同时兼容:直接是 {"direction":..};
/// 外壳字段 value / return_value 为对象或字符串化 JSON。解析失败返回 None。
fn parse_autosum(raw: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    if v.get("direction").is_some() {
        return Some(v);
    }
    for key in ["value", "return_value"] {
        let Some(inner) = v.get(key) else { continue };
        if inner.get("direction").is_some() {
            return Some(inner.clone());
        }
        if let Some(t) = inner.as_str() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(t) {
                if parsed.get("direction").is_some() {
                    return Some(parsed);
                }
            }
        }
    }
    None
}

fn a1(r: u32, c: u32) -> String {
    format!("{}{}", crate::col_index_to_letter(c), r + 1)
}

/// 自动求和(仅 SUM)。规则:
/// - 单格选区:向上/向左探测连续数字,把 =SUM(范围) 写到该格;探测不到则忽略;
/// - 单列多格选区:把 =SUM(选区) 写到选区下方一格;
/// - 单行多格选区:把 =SUM(选区) 写到选区右侧一格;
/// - 多行多列选区:本轮忽略(记入遗留)。
/// 写入后选区移到结果格。
fn autosum(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.autosum: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();

    let mut s = ctx.state.borrow_mut();
    let merges = s.calc_merges.clone();
    let Some(sess) = s.session.as_mut() else { return };

    // (结果格行, 结果格列, 公式)
    let (tr, tc, formula) = if r1 == r2 && c1 == c2 {
        let raw = sess.query_script(&format!("detect_autosum_range({}, {}, {})", sheet, r1, c1));
        let Some(v) = parse_autosum(&raw) else {
            eprintln!("[ribbon] calc.autosum: 无法解析探测结果: {}", raw);
            return;
        };
        let dir = v["direction"].as_str().unwrap_or("none");
        if dir == "none" {
            eprintln!("[ribbon] calc.autosum: 上方/左侧没有可求和的连续数字,忽略");
            return;
        }
        let sr = v["start"][0].as_u64().unwrap_or(0) as u32;
        let sc = v["start"][1].as_u64().unwrap_or(0) as u32;
        let er = v["end"][0].as_u64().unwrap_or(0) as u32;
        let ec = v["end"][1].as_u64().unwrap_or(0) as u32;
        (r1, c1, format!("=SUM({}:{})", a1(sr, sc), a1(er, ec)))
    } else if c1 == c2 {
        // 单列多格:结果写到选区下方一格
        (r2 + 1, c1, format!("=SUM({}:{})", a1(r1, c1), a1(r2, c2)))
    } else if r1 == r2 {
        // 单行多格:结果写到选区右侧一格
        (r1, c2 + 1, format!("=SUM({}:{})", a1(r1, c1), a1(r2, c2)))
    } else {
        eprintln!("[ribbon] calc.autosum: 暂不支持多行多列选区,已忽略");
        return;
    };

    if !sess.calc_set_cell(sheet, tr, tc, &formula) {
        eprintln!("[ribbon] calc.autosum 写入 {} 失败: {}", a1(tr, tc), formula);
        return;
    }
    eprintln!("[ribbon] calc.autosum {} <- {}", a1(tr, tc), formula);
    refresh_view(&app, sess);
    // 选区移到结果格(内含公式栏/按钮态/高亮刷新)
    crate::calc_selection::select_anchor(&app, sess, &merges, sheet, tr, tc);
}

/// 解析 delete_duplicates 的返回。约定 "ok:N" / "err:msg";
/// 外壳为 execute_script 的 {"ok":true,...,"return_value":"ok:N"} 或直接字符串。
/// 返回 Ok(删除行数) / Err(原因)。解析失败把原文放进 Err,由调用方打日志。
fn parse_delete_result(raw: &str) -> Result<u32, String> {
    let mut text = raw.to_string();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if v.get("ok").and_then(|x| x.as_bool()) == Some(false) {
            return Err(format!("脚本执行失败: {}", raw));
        }
        for key in ["return_value", "value"] {
            if let Some(t) = v.get(key).and_then(|x| x.as_str()) {
                text = t.to_string();
                break;
            }
        }
    }
    let t = text.trim().trim_matches('"');
    if let Some(n) = t.strip_prefix("ok:") {
        n.trim().parse::<u32>().map_err(|_| format!("无法解析删除行数: {}", raw))
    } else if let Some(m) = t.strip_prefix("err:") {
        Err(m.to_string())
    } else {
        Err(format!("未识别的返回格式: {}", raw))
    }
}

/// 删除重复项。规则:
/// - 单格选区:忽略;
/// - 选区与任一合并区相交:拒绝;
/// - has_header 固定 false(整个选区参与判重),判重列 = 选区全部列;
/// - 每组重复保留最靠上的一行;删除的是整行(选区之外同行的数据也会上移)。
/// 删除后选区落回选区左上角。
/// 删除重复值对话框的瞬时状态(存在 AppState.dedup_dialog)。
pub struct DedupDialogState {
    pub sheet: u32,
    /// 扩展后的范围(闭区间)
    pub r1: u32,
    pub c1: u32,
    pub r2: u32,
    pub c2: u32,
    /// 首行文字(与列一一对应,可能为空串);用于"数据包含标题"时的列名
    pub header_texts: Vec<String>,
    pub checked: Vec<bool>,
    pub has_header: bool,
}

impl DedupDialogState {
    fn col_names(&self) -> Vec<String> {
        (0..self.checked.len())
            .map(|i| {
                let letter = format!("列 {}", crate::col_index_to_letter(self.c1 + i as u32));
                if self.has_header {
                    match self.header_texts.get(i) {
                        Some(t) if !t.is_empty() => t.clone(),
                        _ => letter,
                    }
                } else {
                    letter
                }
            })
            .collect()
    }
}

/// 把对话框状态灌回 Slint(整体替换模型,与 pivot 对话框同一范式)。
fn push_dedup_dialog(app: &crate::AppWindow, st: &DedupDialogState) {
    let names: Vec<slint::SharedString> =
        st.col_names().into_iter().map(slint::SharedString::from).collect();
    app.set_column_pick_columns(slint::ModelRc::new(slint::VecModel::from(names)));
    app.set_column_pick_checked(slint::ModelRc::new(slint::VecModel::from(st.checked.clone())));
    app.set_column_pick_has_header(st.has_header);
    app.set_column_pick_all_checked(!st.checked.is_empty() && st.checked.iter().all(|&b| b));
}

/// 解析 get_row_texts 的返回(JSON 字符串数组)。外壳同其它 query_script:
/// {"ok":true,"value":"<字符串化 JSON>"};兼容 return_value、直接数组。
pub(super) fn parse_row_texts(raw: &str) -> Option<Vec<String>> {
    fn pick(v: &serde_json::Value) -> Option<Vec<String>> {
        let arr = v.as_array()?;
        Some(arr.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect())
    }
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    if let Some(r) = pick(&v) {
        return Some(r);
    }
    for key in ["value", "return_value"] {
        let Some(inner) = v.get(key) else { continue };
        if let Some(r) = pick(inner) {
            return Some(r);
        }
        if let Some(t) = inner.as_str() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(t) {
                if let Some(r) = pick(&parsed) {
                    return Some(r);
                }
            }
        }
    }
    None
}

/// 数据页"删除重复值"入口:校验/扩展选区后打开对话框,**不直接执行**。
fn delete_dups(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.delete_dups: 无选中单元格");
        return;
    };
    if r1 == r2 && c1 == c2 {
        eprintln!("[ribbon] calc.delete_dups: 单格选区,请先选择要去重的区域");
        return;
    }
    let sheet = ctx.current_sheet_idx.get();

    let mut s = ctx.state.borrow_mut();
    let merges = s.calc_merges.clone();
    let Some(sess) = s.session.as_mut() else { return };

    // 单列/单行选区:扩展到连续数据区;多行多列选区视为用户明确框定,不扩展。
    let (r1, c1, r2, c2) = if r1 == r2 || c1 == c2 {
        match expand_to_region(sess, sheet, r1, c1, "calc.delete_dups") {
            Some((er1, ec1, er2, ec2)) => {
                eprintln!(
                    "[ribbon] calc.delete_dups expanded=true ({},{})-({},{}) -> ({},{})-({},{})",
                    r1, c1, r2, c2, er1, ec1, er2, ec2
                );
                (er1, ec1, er2, ec2)
            }
            None => {
                eprintln!("[ribbon] calc.delete_dups: 未探测到连续数据区,忽略");
                return;
            }
        }
    } else {
        (r1, c1, r2, c2)
    };
    if r1 == r2 && c1 == c2 {
        eprintln!("[ribbon] calc.delete_dups: 连续数据区只有一个单元格,忽略");
        return;
    }

    let hit_merge = merges
        .iter()
        .any(|&(mr1, mc1, mr2, mc2)| mr1 <= r2 && mr2 >= r1 && mc1 <= c2 && mc2 >= c1);
    if hit_merge {
        eprintln!("[ribbon] calc.delete_dups: 选区与合并单元格相交,已拒绝");
        return;
    }

    // 表头初值:启发式;取首行文字作列名
    let has_header = {
        let raw = sess.query_script(&format!(
            "detect_header_row({}, {}, {}, {}, {})",
            sheet, r1, c1, r2, c2
        ));
        parse_header(&raw).unwrap_or(false)
    };
    let header_texts = {
        let raw = sess.query_script(&format!("get_row_texts({}, {}, {}, {})", sheet, r1, c1, c2));
        parse_row_texts(&raw).unwrap_or_default()
    };
    let ncols = (c2 - c1 + 1) as usize;
    let st = DedupDialogState {
        sheet,
        r1,
        c1,
        r2,
        c2,
        header_texts,
        checked: vec![true; ncols],
        has_header,
    };
    eprintln!(
        "[ribbon] calc.delete_dups 打开对话框 ({},{})-({},{}) has_header={} ncols={}",
        r1, c1, r2, c2, has_header, ncols
    );

    app.set_column_pick_title("删除重复值".into());
    app.set_column_pick_range(
        format!("范围:{}:{}", a1(r1, c1), a1(r2, c2)).into(),
    );
    push_dedup_dialog(&app, &st);
    s.dedup_dialog = Some(st);
    app.set_column_pick_visible(true);
}

pub fn dedup_toggle_column(ctx: &RibbonCtx, idx: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.dedup_dialog.as_mut() else { return };
    if idx < st.checked.len() {
        st.checked[idx] = !st.checked[idx];
    }
    eprintln!("[ribbon] calc.delete_dups toggle_column idx={} checked={:?}", idx, st.checked);
    push_dedup_dialog(&app, st);
}

pub fn dedup_toggle_all(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.dedup_dialog.as_mut() else { return };
    // 全部已勾选 → 全部取消;否则全部勾选
    let all = st.checked.iter().all(|&b| b);
    for b in st.checked.iter_mut() {
        *b = !all;
    }
    eprintln!("[ribbon] calc.delete_dups toggle_all -> {}", !all);
    push_dedup_dialog(&app, st);
}

pub fn dedup_toggle_header(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.dedup_dialog.as_mut() else { return };
    st.has_header = !st.has_header;
    eprintln!("[ribbon] calc.delete_dups toggle_header -> {}", st.has_header);
    push_dedup_dialog(&app, st);
}

pub fn dedup_cancel(ctx: &RibbonCtx) {
    ctx.state.borrow_mut().dedup_dialog = None;
    eprintln!("[ribbon] calc.delete_dups 对话框取消");
}

pub fn dedup_confirm(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    // 先只读校验:未勾选任何列时保持对话框打开(Slint 侧 confirmed 已把它关了,这里重新打开)
    let any_checked = match s.dedup_dialog.as_ref() {
        Some(st) => st.checked.iter().any(|&b| b),
        None => return,
    };
    if !any_checked {
        eprintln!("[ribbon] calc.delete_dups: 未勾选任何列,保持对话框打开");
        app.set_column_pick_visible(true);
        return;
    }
    let Some(st) = s.dedup_dialog.take() else { return };
    let cols: Vec<usize> = st
        .checked
        .iter()
        .enumerate()
        .filter(|(_, &b)| b)
        .map(|(i, _)| i)
        .collect();
    // 全选时传 "[]"(内核语义:全列判重),否则传勾选列相对 c1 的偏移
    let cols_json = if cols.len() == st.checked.len() {
        "[]".to_string()
    } else {
        serde_json::to_string(&cols).unwrap_or_else(|_| "[]".to_string())
    };
    let merges = s.calc_merges.clone();
    let Some(sess) = s.session.as_mut() else { return };
    let script = format!(
        "delete_duplicates({}, {}, {}, {}, {}, \"{}\", {})",
        st.sheet,
        st.r1,
        st.c1,
        st.r2,
        st.c2,
        cols_json.replace('"', "\\\""),
        st.has_header
    );
    let raw = sess.execute_script(&script);
    eprintln!(
        "[ribbon] calc.delete_dups ({},{})-({},{}) cols={} has_header={} 返回: {}",
        st.r1, st.c1, st.r2, st.c2, cols_json, st.has_header, raw
    );
    match parse_delete_result(&raw) {
        Ok(n) => eprintln!("[ribbon] calc.delete_dups 删除 {} 行", n),
        Err(e) => {
            eprintln!("[ribbon] calc.delete_dups 失败: {}", e);
            return;
        }
    }
    refresh_view(&app, sess);
    sync_state_inner(&app, sess, st.sheet);
    // 行结构变了:选区落回左上角,并按新行高重算高亮
    crate::calc_selection::select_anchor(&app, sess, &merges, st.sheet, st.r1, st.c1);
    reflow_selection(&app, &mut s, st.sheet);
}

/// 解析 detect_current_region 的返回。外壳同 detect_autosum_range:
/// {"ok":true,"value":"<字符串化 JSON>"};兼容 return_value、对象/字符串化 JSON、
/// 顶层直接含 start/end。{"direction":"none"} 或解析失败返回 None。
fn parse_region(raw: &str) -> Option<(u32, u32, u32, u32)> {
    fn pick(v: &serde_json::Value) -> Option<(u32, u32, u32, u32)> {
        let st = v.get("start")?;
        let en = v.get("end")?;
        Some((
            st.get(0)?.as_u64()? as u32,
            st.get(1)?.as_u64()? as u32,
            en.get(0)?.as_u64()? as u32,
            en.get(1)?.as_u64()? as u32,
        ))
    }
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    if let Some(r) = pick(&v) {
        return Some(r);
    }
    for key in ["value", "return_value"] {
        let Some(inner) = v.get(key) else { continue };
        if let Some(r) = pick(inner) {
            return Some(r);
        }
        if let Some(t) = inner.as_str() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(t) {
                if let Some(r) = pick(&parsed) {
                    return Some(r);
                }
            }
        }
    }
    None
}

/// 以 (row,col) 为起点探测连续数据区。探测不到或解析失败返回 None(并打印原文)。
pub(crate) fn expand_to_region(
    sess: &mut doc_core::TileCacheRust,
    sheet: u32,
    row: u32,
    col: u32,
    tag: &str,
) -> Option<(u32, u32, u32, u32)> {
    let raw = sess.query_script(&format!("detect_current_region({}, {}, {})", sheet, row, col));
    let r = parse_region(&raw);
    if r.is_none() {
        eprintln!("[ribbon] {}: detect_current_region 无结果/无法解析: {}", tag, raw);
    }
    r
}

// ══════════ 分类汇总对话框(insert_subtotals) ══════════

/// 汇总方式:(显示名, Excel SUBTOTAL 函数编号)。顺序即对话框列表顺序。
const SUBTOTAL_FUNCS: [(&str, i64); 11] = [
    ("求和", 9),
    ("计数", 3),
    ("平均值", 1),
    ("最大值", 4),
    ("最小值", 5),
    ("乘积", 6),
    ("数值计数", 2),
    ("标准偏差(样本)", 7),
    ("标准偏差(总体)", 8),
    ("方差(样本)", 10),
    ("方差(总体)", 11),
];

/// 分类汇总对话框的瞬时状态(存在 AppState.subtotal_dialog)。
pub struct SubtotalDialogState {
    pub sheet: u32,
    /// 数据区(闭区间)。首行 r1 固定视为表头,数据从 r1+1 开始。
    pub r1: u32,
    pub c1: u32,
    pub r2: u32,
    pub c2: u32,
    /// 各列显示名(首行文字,为空则"列 X")
    pub col_names: Vec<String>,
    /// 分类字段:相对 c1 的列偏移
    pub group_idx: usize,
    /// 汇总方式:SUBTOTAL_FUNCS 下标
    pub func_idx: usize,
    /// 各列是否勾选为汇总项(与 col_names 同序)
    pub agg_checked: Vec<bool>,
    /// "清除所有分类汇总":勾选后确定 = 只清除不新建,其余控件置灰
    pub clear_existing: bool,
}

impl SubtotalDialogState {
    fn any_agg_checked(&self) -> bool {
        self.agg_checked
            .iter()
            .enumerate()
            .any(|(i, &b)| b && i != self.group_idx)
    }

    /// 清除模式恒可确定;新建模式要求至少一个汇总项。
    fn can_confirm(&self) -> bool {
        self.clear_existing || self.any_agg_checked()
    }
}

/// 把对话框状态灌回 Slint(整体替换模型,与 dedup/pivot 同一范式)。
fn push_subtotal_dialog(app: &crate::AppWindow, st: &SubtotalDialogState) {
    let names: Vec<slint::SharedString> =
        st.col_names.iter().map(|n| slint::SharedString::from(n.as_str())).collect();
    app.set_subtotal_group_columns(slint::ModelRc::new(slint::VecModel::from(names)));
    app.set_subtotal_group_selected(st.group_idx as i32);
    let funcs: Vec<slint::SharedString> =
        SUBTOTAL_FUNCS.iter().map(|(n, _)| slint::SharedString::from(*n)).collect();
    app.set_subtotal_func_names(slint::ModelRc::new(slint::VecModel::from(funcs)));
    app.set_subtotal_func_selected(st.func_idx as i32);
    app.set_subtotal_agg_checked(slint::ModelRc::new(slint::VecModel::from(st.agg_checked.clone())));
    app.set_subtotal_clear_existing(st.clear_existing);
    let ok = st.can_confirm();
    app.set_subtotal_can_confirm(ok);
    app.set_subtotal_warn(if ok { "".into() } else { "请至少选择一个汇总项".into() });
}

/// 数据页"分类汇总"入口:校验/扩展选区后打开对话框,**不直接执行**。
fn subtotal_open(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.subtotal: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();

    let mut s = ctx.state.borrow_mut();
    let merges = s.calc_merges.clone();
    let Some(sess) = s.session.as_mut() else { return };

    // 单格/单行/单列选区:扩展到连续数据区;多行多列选区视为用户明确框定。
    let (r1, c1, r2, c2) = if r1 == r2 || c1 == c2 {
        match expand_to_region(sess, sheet, r1, c1, "calc.subtotal") {
            Some(r) => r,
            None => {
                eprintln!("[ribbon] calc.subtotal: 未探测到连续数据区,忽略");
                return;
            }
        }
    } else {
        (r1, c1, r2, c2)
    };
    // 首行是表头,至少还要有一行数据、至少两列(分类列 + 一个汇总列)
    if r2 <= r1 || c2 <= c1 {
        eprintln!("[ribbon] calc.subtotal: 数据区需至少2行2列(表头+数据),忽略");
        return;
    }
    let hit_merge = merges
        .iter()
        .any(|&(mr1, mc1, mr2, mc2)| mr1 <= r2 && mr2 >= r1 && mc1 <= c2 && mc2 >= c1);
    if hit_merge {
        eprintln!("[ribbon] calc.subtotal: 选区与合并单元格相交,已拒绝");
        return;
    }

    let header_texts = {
        let raw = sess.query_script(&format!("get_row_texts({}, {}, {}, {})", sheet, r1, c1, c2));
        parse_row_texts(&raw).unwrap_or_default()
    };
    let ncols = (c2 - c1 + 1) as usize;
    let col_names: Vec<String> = (0..ncols)
        .map(|i| match header_texts.get(i) {
            Some(t) if !t.is_empty() => t.clone(),
            _ => format!("列 {}", crate::col_index_to_letter(c1 + i as u32)),
        })
        .collect();
    // 默认:分类字段=第一列,汇总项=除分类列外全部勾选,方式=求和
    let mut agg_checked = vec![true; ncols];
    agg_checked[0] = false;
    let st = SubtotalDialogState {
        sheet,
        r1,
        c1,
        r2,
        c2,
        col_names,
        group_idx: 0,
        func_idx: 0,
        agg_checked,
        clear_existing: false,
    };
    eprintln!(
        "[ribbon] calc.subtotal 打开对话框 ({},{})-({},{}) ncols={}",
        r1, c1, r2, c2, ncols
    );

    app.set_subtotal_range(format!("范围:{}:{}", a1(r1, c1), a1(r2, c2)).into());
    push_subtotal_dialog(&app, &st);
    s.subtotal_dialog = Some(st);
    app.set_subtotal_visible(true);
}

/// 分类字段改变:越界忽略;新分类列若原为汇总项则取消勾选(不能既分类又汇总)。
pub fn subtotal_group_selected_changed(ctx: &RibbonCtx, idx: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.subtotal_dialog.as_mut() else { return };
    if idx >= st.col_names.len() {
        return;
    }
    st.group_idx = idx;
    st.agg_checked[idx] = false;
    eprintln!("[ribbon] calc.subtotal group_selected idx={}", idx);
    push_subtotal_dialog(&app, st);
}

/// 汇总方式改变:越界忽略。
pub fn subtotal_func_selected_changed(ctx: &RibbonCtx, idx: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.subtotal_dialog.as_mut() else { return };
    if idx >= SUBTOTAL_FUNCS.len() {
        return;
    }
    st.func_idx = idx;
    eprintln!("[ribbon] calc.subtotal func_selected idx={} ({})", idx, SUBTOTAL_FUNCS[idx].0);
    push_subtotal_dialog(&app, st);
}

/// 切换某列是否为汇总项。点到分类列自己、越界、清除模式下均忽略。
pub fn subtotal_toggle_agg(ctx: &RibbonCtx, idx: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.subtotal_dialog.as_mut() else { return };
    if idx >= st.agg_checked.len() || idx == st.group_idx || st.clear_existing {
        return;
    }
    st.agg_checked[idx] = !st.agg_checked[idx];
    eprintln!("[ribbon] calc.subtotal toggle_agg idx={} -> {}", idx, st.agg_checked[idx]);
    push_subtotal_dialog(&app, st);
}

/// 切换"清除所有分类汇总"。
pub fn subtotal_toggle_clear(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.subtotal_dialog.as_mut() else { return };
    st.clear_existing = !st.clear_existing;
    eprintln!("[ribbon] calc.subtotal toggle_clear -> {}", st.clear_existing);
    push_subtotal_dialog(&app, st);
}

pub fn subtotal_cancel(ctx: &RibbonCtx) {
    ctx.state.borrow_mut().subtotal_dialog = None;
    eprintln!("[ribbon] calc.subtotal 对话框取消");
}

/// 解析 remove_subtotals 的返回:{"removed":N} 或 "err:..."。
/// 外壳同 parse_delete_result(可能包一层 {"ok":..,"return_value"/"value":"<字符串>"})。
fn parse_removed_result(raw: &str) -> Result<u32, String> {
    let mut text = raw.to_string();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if v.get("ok").and_then(|x| x.as_bool()) == Some(false) {
            return Err(format!("脚本执行失败: {}", raw));
        }
        for key in ["return_value", "value"] {
            if let Some(t) = v.get(key).and_then(|x| x.as_str()) {
                text = t.to_string();
                break;
            }
        }
        if let Some(n) = v.get("removed").and_then(|x| x.as_u64()) {
            return Ok(n as u32);
        }
    }
    let t = text.trim();
    let unquoted: String = if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        serde_json::from_str::<String>(t).unwrap_or_else(|_| t.trim_matches('"').to_string())
    } else {
        t.to_string()
    };
    if let Some(m) = unquoted.strip_prefix("err:") {
        return Err(m.to_string());
    }
    match serde_json::from_str::<serde_json::Value>(&unquoted)
        .ok()
        .and_then(|v| v.get("removed").and_then(|x| x.as_u64()))
    {
        Some(n) => Ok(n as u32),
        None => Err(format!("未识别的返回格式: {}", raw)),
    }
}

/// 解析 insert_subtotals_ex 的返回:{"new_end_row":N} 或 "err:..."(外壳同 parse_removed_result)。
fn parse_new_end_result(raw: &str) -> Result<u32, String> {
    let mut text = raw.to_string();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if v.get("ok").and_then(|x| x.as_bool()) == Some(false) {
            return Err(format!("脚本执行失败: {}", raw));
        }
        for key in ["return_value", "value"] {
            if let Some(t) = v.get(key).and_then(|x| x.as_str()) {
                text = t.to_string();
                break;
            }
        }
        if let Some(n) = v.get("new_end_row").and_then(|x| x.as_u64()) {
            return Ok(n as u32);
        }
    }
    let t = text.trim();
    let unquoted: String = if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        serde_json::from_str::<String>(t).unwrap_or_else(|_| t.trim_matches('"').to_string())
    } else {
        t.to_string()
    };
    if let Some(m) = unquoted.strip_prefix("err:") {
        return Err(m.to_string());
    }
    match serde_json::from_str::<serde_json::Value>(&unquoted)
        .ok()
        .and_then(|v| v.get("new_end_row").and_then(|x| x.as_u64()))
    {
        Some(n) => Ok(n as u32),
        None => Err(format!("未识别的返回格式: {}", raw)),
    }
}

/// 确定(两种模式都先 remove_subtotals):
/// - 勾选"清除所有分类汇总" → 只清除。
/// - 否则清除后 insert_subtotals_ex 新建(汇总在上方 + 总计行,单分类字段)。
/// 结尾统一刷新视图。
pub fn subtotal_confirm(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.subtotal_dialog.take() else { return };
    let merges = s.calc_merges.clone();
    let Some(sess) = s.session.as_mut() else { return };
    // r1 = 表头行,数据区 [r1+1, r2],列 [c1, c2],全部 0-based(绝对坐标)
    let script = format!(
        "remove_subtotals({}, {}, {}, {}, {})",
        st.sheet, st.r1, st.c1, st.r2, st.c2
    );
    let raw = sess.execute_script(&script);
    eprintln!(
        "[ribbon] calc.subtotal 清除 ({},{})-({},{}) 返回: {}",
        st.r1, st.c1, st.r2, st.c2, raw
    );
    let removed = match parse_removed_result(&raw) {
        Ok(n) => {
            eprintln!("[ribbon] calc.subtotal 清除汇总行 {} 行", n);
            n
        }
        Err(e) => {
            eprintln!("[ribbon] calc.subtotal 清除失败: {}", e);
            return;
        }
    };
    // 新建模式:先清后建(替换语义,幂等);建失败也继续刷新视图(此时已清除)
    if !st.clear_existing {
        let r2n = st.r2.saturating_sub(removed);
        let group_col = st.c1 + st.group_idx as u32;
        let aggs: Vec<serde_json::Value> = st
            .agg_checked
            .iter()
            .enumerate()
            .filter(|&(i, &b)| b && i != st.group_idx)
            .map(|(i, _)| serde_json::json!([st.c1 + i as u32, SUBTOTAL_FUNCS[st.func_idx].1]))
            .collect();
        if aggs.is_empty() || r2n <= st.r1 {
            eprintln!(
                "[ribbon] calc.subtotal 新建跳过: aggs={} r2n={} r1={}",
                aggs.len(), r2n, st.r1
            );
        } else {
            let spec = serde_json::json!({
                "groups": [{"col": group_col, "aggs": aggs}],
                "summary_below": false,
                "grand_total": true
            });
            let script = format!(
                "insert_subtotals_ex({}, {}, {}, {}, \"{}\")",
                st.sheet, st.r1, st.r1 + 1, r2n,
                spec.to_string().replace('"', "\\\"")
            );
            eprintln!("[ribbon] calc.subtotal 新建 script: {}", script);
            let raw2 = sess.execute_script(&script);
            eprintln!("[ribbon] calc.subtotal 新建返回: {}", raw2);
            match parse_new_end_result(&raw2) {
                Ok(e) => eprintln!("[ribbon] calc.subtotal 新建完成,数据区末行 {}", e),
                Err(e) => eprintln!("[ribbon] calc.subtotal 新建失败: {}", e),
            }
        }
    }
    refresh_view(&app, sess);
    sync_state_inner(&app, sess, st.sheet);
    // 行结构变了:选区落回左上角,并按新行高重算高亮
    crate::calc_selection::select_anchor(&app, sess, &merges, st.sheet, st.r1, st.c1);
    // 行结构已整体重建:清掉该 sheet 的折叠记录,否则 tracker 与新行状态脱节,
    // 重建后首次点击汇总图标会走"展开"分支而无反应
    s.calc_collapse.clear_sheet(st.sheet);
    reflow_selection(&app, &mut s, st.sheet);
}


// ══════════ 分列对话框(text_to_columns_multi) ══════════

/// 分隔符项顺序必须与 TextSplitDialog.slint 的 checked 顺序一致:
/// 逗号、制表符、分号、空格、冒号、其他(自定义,取 custom 文本)。
const TS_FIXED: [&str; 5] = [",", "\t", ";", " ", ":"];
const TS_OTHER_IDX: usize = 5;

/// 分列对话框的瞬时状态(存在 AppState.text_split_dialog)。
pub struct TextSplitDialogState {
    pub sheet: u32,
    /// 处理的列与行区间(闭区间)
    pub col: u32,
    pub row1: u32,
    pub row2: u32,
    pub checked: [bool; 6],
    pub custom: String,
    pub merge: bool,
}

impl TextSplitDialogState {
    /// 当前勾选对应的分隔符列表("其他"仅在勾选且输入非空时加入)。
    fn delims(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for (i, d) in TS_FIXED.iter().enumerate() {
            if self.checked[i] {
                v.push((*d).to_string());
            }
        }
        if self.checked[TS_OTHER_IDX] && !self.custom.is_empty() {
            v.push(self.custom.clone());
        }
        v
    }
}

/// 把字符串包成 Rhai 双引号字面量(转义 \ 与 ")。
fn rhai_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// 解析 preview_text_to_columns 的返回 {"max_cols":N,"overwrites":bool}。
/// 外壳同其它 query_script:{"ok":true,"value":"<字符串化 JSON>"};兼容 return_value、
/// 对象/字符串化 JSON、顶层直接含字段。解析失败返回 None。
fn parse_ttc_preview(raw: &str) -> Option<(u32, bool)> {
    fn pick(v: &serde_json::Value) -> Option<(u32, bool)> {
        let n = v.get("max_cols")?.as_u64()? as u32;
        let o = v.get("overwrites").and_then(|x| x.as_bool()).unwrap_or(false);
        Some((n, o))
    }
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    if let Some(r) = pick(&v) {
        return Some(r);
    }
    for key in ["value", "return_value"] {
        let Some(inner) = v.get(key) else { continue };
        if let Some(r) = pick(inner) {
            return Some(r);
        }
        if let Some(t) = inner.as_str() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(t) {
                if let Some(r) = pick(&parsed) {
                    return Some(r);
                }
            }
        }
    }
    None
}

/// 解析 text_to_columns_multi 的 execute_script 返回里的 max_cols。
/// return_value 是字符串化 JSON {"max_cols":N};兼容对象/顶层。
fn parse_ttc_result(raw: &str) -> Option<u32> {
    parse_ttc_preview(raw).map(|(n, _)| n)
}

/// 把状态灌回 Slint(不含 custom:输入框内容由 Slint 持有,避免打断光标)。
fn push_text_split(app: &crate::AppWindow, st: &TextSplitDialogState, warn: &str, can_confirm: bool) {
    app.set_text_split_checked(slint::ModelRc::new(slint::VecModel::from(st.checked.to_vec())));
    app.set_text_split_merge(st.merge);
    app.set_text_split_warn(warn.into());
    app.set_text_split_can_confirm(can_confirm);
}

/// 重算:组分隔符 → 无分隔符则不可确认;否则调只读预检,决定是否显示覆盖提示。
fn text_split_recompute(app: &crate::AppWindow, s: &mut crate::AppState) {
    let Some(st) = s.text_split_dialog.as_ref() else { return };
    let delims = st.delims();
    let (sheet, col, r1, r2, merge) = (st.sheet, st.col, st.row1, st.row2, st.merge);
    if delims.is_empty() {
        let st = s.text_split_dialog.as_ref().unwrap();
        push_text_split(app, st, "", false);
        return;
    }
    let dj = serde_json::to_string(&delims).unwrap_or_else(|_| "[]".to_string());
    let Some(sess) = s.session.as_mut() else { return };
    let script = format!(
        "preview_text_to_columns({}, {}, {}, {}, {}, {})",
        sheet, r1, r2, col, rhai_quote(&dj), merge
    );
    let raw = sess.query_script(&script);
    let (max_cols, overwrites) = parse_ttc_preview(&raw).unwrap_or((0, false));
    eprintln!(
        "[ribbon] calc.text_to_columns 预检 delims={} merge={} -> max_cols={} overwrites={}",
        dj, merge, max_cols, overwrites
    );
    let warn = if overwrites { "将覆盖右侧已有数据" } else { "" };
    let st = s.text_split_dialog.as_ref().unwrap();
    push_text_split(app, st, warn, true);
}

/// 数据页"文本拆分为列"入口:校验选区后打开对话框,**不直接执行**。
fn text_split_open(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.text_to_columns: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    if c2 > c1 {
        eprintln!("[ribbon] calc.text_to_columns: 多列选区,只处理最左列 {}", crate::col_index_to_letter(c1));
    }
    let col = c1;

    let mut s = ctx.state.borrow_mut();
    let merges = s.calc_merges.clone();
    let Some(sess) = s.session.as_mut() else { return };

    // 行区间:选区多行 → 直接用;只有一行 → 用连续数据区里该列的行段(探测不到就只处理这一行)
    let (row1, row2) = if r1 != r2 {
        (r1, r2)
    } else {
        match expand_to_region(sess, sheet, r1, col, "calc.text_to_columns") {
            Some((er1, ec1, er2, ec2)) if ec1 <= col && col <= ec2 => (er1, er2),
            _ => (r1, r1),
        }
    };

    let hit_merge = merges
        .iter()
        .any(|&(mr1, mc1, mr2, mc2)| mr1 <= row2 && mr2 >= row1 && mc1 <= col && mc2 >= col);
    if hit_merge {
        eprintln!("[ribbon] calc.text_to_columns: 选区与合并单元格相交,已拒绝");
        return;
    }

    let st = TextSplitDialogState {
        sheet,
        col,
        row1,
        row2,
        checked: [false; 6],
        custom: String::new(),
        merge: false,
    };
    eprintln!(
        "[ribbon] calc.text_to_columns 打开对话框 sheet={} col={} rows={}..{}",
        sheet, col, row1, row2
    );
    app.set_text_split_title("分列".into());
    app.set_text_split_range(
        format!("范围:{}:{}", a1(row1, col), a1(row2, col)).into(),
    );
    app.set_text_split_custom("".into());
    push_text_split(&app, &st, "", false);
    s.text_split_dialog = Some(st);
    app.set_text_split_visible(true);
}

pub fn text_split_toggle(ctx: &RibbonCtx, idx: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.text_split_dialog.as_mut() else { return };
    if idx < st.checked.len() {
        st.checked[idx] = !st.checked[idx];
    }
    eprintln!("[ribbon] calc.text_to_columns toggle idx={} checked={:?}", idx, st.checked);
    text_split_recompute(&app, &mut s);
}

pub fn text_split_toggle_merge(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.text_split_dialog.as_mut() else { return };
    st.merge = !st.merge;
    eprintln!("[ribbon] calc.text_to_columns toggle_merge -> {}", st.merge);
    text_split_recompute(&app, &mut s);
}

pub fn text_split_custom_edited(ctx: &RibbonCtx, text: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.text_split_dialog.as_mut() else { return };
    st.custom = text.to_string();
    eprintln!("[ribbon] calc.text_to_columns custom_edited {:?}", st.custom);
    // 注意:不回灌 custom-text,输入框内容由 Slint 持有
    text_split_recompute(&app, &mut s);
}

pub fn text_split_cancel(ctx: &RibbonCtx) {
    ctx.state.borrow_mut().text_split_dialog = None;
    eprintln!("[ribbon] calc.text_to_columns 对话框取消");
}

pub fn text_split_confirm(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    // 先只读校验:没有有效分隔符时保持对话框打开(正常情况下 can-confirm 已灰掉确定,这里兜底)
    let delims = match s.text_split_dialog.as_ref() {
        Some(st) => st.delims(),
        None => return,
    };
    if delims.is_empty() {
        eprintln!("[ribbon] calc.text_to_columns: 无有效分隔符,保持对话框打开");
        app.set_text_split_visible(true);
        return;
    }
    let Some(st) = s.text_split_dialog.take() else { return };
    let dj = serde_json::to_string(&delims).unwrap_or_else(|_| "[]".to_string());
    let merges = s.calc_merges.clone();
    let Some(sess) = s.session.as_mut() else { return };
    let script = format!(
        "text_to_columns_multi({}, {}, {}, {}, {}, {})",
        st.sheet, st.row1, st.row2, st.col, rhai_quote(&dj), st.merge
    );
    let raw = sess.execute_script(&script);
    eprintln!(
        "[ribbon] calc.text_to_columns sheet={} col={} rows={}..{} delims={} merge={} 返回: {}",
        st.sheet, st.col, st.row1, st.row2, dj, st.merge, raw
    );
    match parse_ttc_result(&raw) {
        Some(0) => {
            eprintln!("[ribbon] calc.text_to_columns 内核未执行(max_cols=0)");
            return;
        }
        Some(1) => eprintln!("[ribbon] calc.text_to_columns 所选行均无分隔符命中,无变化"),
        Some(n) => eprintln!("[ribbon] calc.text_to_columns 拆分为最多 {} 列", n),
        None => {
            eprintln!("[ribbon] calc.text_to_columns 返回无法解析");
            return;
        }
    }
    refresh_view(&app, sess);
    sync_state_inner(&app, sess, st.sheet);
    crate::calc_selection::select_anchor(&app, sess, &merges, st.sheet, st.row1, st.col);
    reflow_selection(&app, &mut s, st.sheet);
}

// ══════════ 数据验证对话框(set_data_validation) ══════════

/// kind 下拉 index → 内核 kind 字符串。顺序必须与 DataValidationDialog.slint 头部注释一致。
const DV_KINDS: [&str; 8] = [
    "none", "whole", "decimal", "list", "date", "time", "textLength", "custom",
];
/// operator 下拉 index → 内核 operator 字符串。顺序同上。
const DV_OPERATORS: [&str; 8] = [
    "between", "notBetween", "equal", "notEqual",
    "greaterThan", "lessThan", "greaterThanOrEqual", "lessThanOrEqual",
];

/// 数据验证对话框的瞬时状态(存在 AppState.dv_dialog)。
pub struct DataValidationDialogState {
    pub sheet: u32,
    /// 应用范围(闭区间,行列坐标)
    pub r1: u32,
    pub c1: u32,
    pub r2: u32,
    pub c2: u32,
    pub kind: usize,
    pub operator: usize,
    pub allow_blank: bool,
    pub show_error: bool,
    pub formula1: String,
    pub formula2: String,
    pub error_msg: String,
}

impl DataValidationDialogState {
    /// kind 是否属于"比较类"(需要 operator 与数值)。
    fn is_compare(&self) -> bool {
        matches!(self.kind, 1 | 2 | 4 | 5 | 6)
    }
    /// 是否需要两个值(介于/未介于)。
    fn is_range_op(&self) -> bool {
        self.is_compare() && self.operator <= 1
    }
    /// 该 kind 要求数值输入(整数/小数/文本长度)。日期/时间不强校验格式。
    fn needs_number(&self) -> bool {
        matches!(self.kind, 1 | 2 | 6)
    }

    /// 返回 (能否确认, 提示文案)。空值只灰掉确定不显示红字;格式不对才显示红字。
    fn validate(&self) -> (bool, String) {
        let num_ok = |s: &str| s.trim().parse::<f64>().is_ok();
        match self.kind {
            0 => (true, String::new()),
            3 => (!self.formula1.trim().is_empty(), String::new()),
            7 => (!self.formula1.trim().is_empty(), String::new()),
            _ => {
                let f1 = self.formula1.trim();
                let f2 = self.formula2.trim();
                if f1.is_empty() {
                    return (false, String::new());
                }
                if self.is_range_op() && f2.is_empty() {
                    return (false, String::new());
                }
                if self.needs_number() {
                    if !num_ok(f1) || (self.is_range_op() && !num_ok(f2)) {
                        return (false, "请输入数字".to_string());
                    }
                }
                (true, String::new())
            }
        }
    }
}

/// 把状态灌回 Slint。formula1/2/error_msg 输入框内容由 Slint 持有,这里不回灌。
fn push_dv(app: &crate::AppWindow, st: &DataValidationDialogState) {
    let (ok, warn) = st.validate();
    app.set_dv_kind_index(st.kind as i32);
    app.set_dv_operator_index(st.operator as i32);
    app.set_dv_allow_blank(st.allow_blank);
    app.set_dv_show_error(st.show_error);
    app.set_dv_can_confirm(ok);
    app.set_dv_warn(warn.into());
}

/// 数据页"数据验证"入口:取当前选区,打开对话框(不直接执行)。
fn dv_open(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.data_validation: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let st = DataValidationDialogState {
        sheet,
        r1,
        c1,
        r2,
        c2,
        kind: 1,
        operator: 0,
        allow_blank: true,
        show_error: true,
        formula1: String::new(),
        formula2: String::new(),
        error_msg: String::new(),
    };
    let range_text = if r1 == r2 && c1 == c2 {
        a1(r1, c1)
    } else {
        format!("{}:{}", a1(r1, c1), a1(r2, c2))
    };
    eprintln!(
        "[ribbon] calc.data_validation 打开对话框 sheet={} range={}",
        sheet, range_text
    );
    app.set_dv_title("数据验证".into());
    app.set_dv_range(range_text.into());
    app.set_dv_formula1("".into());
    app.set_dv_formula2("".into());
    app.set_dv_error_msg("".into());
    push_dv(&app, &st);
    ctx.state.borrow_mut().dv_dialog = Some(st);
    app.set_dv_visible(true);
}

pub fn dv_kind_selected(ctx: &RibbonCtx, idx: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.dv_dialog.as_mut() else { return };
    if idx < DV_KINDS.len() {
        st.kind = idx;
    }
    eprintln!("[ribbon] calc.data_validation kind -> {} ({})", st.kind, DV_KINDS[st.kind]);
    push_dv(&app, st);
}

pub fn dv_operator_selected(ctx: &RibbonCtx, idx: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.dv_dialog.as_mut() else { return };
    if idx < DV_OPERATORS.len() {
        st.operator = idx;
    }
    eprintln!("[ribbon] calc.data_validation operator -> {} ({})", st.operator, DV_OPERATORS[st.operator]);
    push_dv(&app, st);
}

pub fn dv_toggle_allow_blank(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.dv_dialog.as_mut() else { return };
    st.allow_blank = !st.allow_blank;
    push_dv(&app, st);
}

pub fn dv_toggle_show_error(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.dv_dialog.as_mut() else { return };
    st.show_error = !st.show_error;
    push_dv(&app, st);
}

pub fn dv_formula1_edited(ctx: &RibbonCtx, text: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.dv_dialog.as_mut() else { return };
    st.formula1 = text.to_string();
    push_dv(&app, st);
}

pub fn dv_formula2_edited(ctx: &RibbonCtx, text: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.dv_dialog.as_mut() else { return };
    st.formula2 = text.to_string();
    push_dv(&app, st);
}

pub fn dv_error_msg_edited(ctx: &RibbonCtx, text: &str) {
    // 错误信息不参与校验,只记录,不需要回灌 UI
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.dv_dialog.as_mut() else { return };
    st.error_msg = text.to_string();
}

pub fn dv_cancel(ctx: &RibbonCtx) {
    ctx.state.borrow_mut().dv_dialog = None;
    eprintln!("[ribbon] calc.data_validation 对话框取消");
}

pub fn dv_confirm(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    // 兜底校验:正常情况下 can-confirm 已灰掉确定
    match s.dv_dialog.as_ref() {
        Some(st) if st.validate().0 => {}
        Some(_) => {
            eprintln!("[ribbon] calc.data_validation: 校验未通过,保持对话框打开");
            app.set_dv_visible(true);
            return;
        }
        None => return,
    }
    let Some(st) = s.dv_dialog.take() else { return };

    let kind = DV_KINDS[st.kind];
    let f1 = st.formula1.trim().to_string();
    let f2 = st.formula2.trim().to_string();
    // 内核 range.sheet 不被读取(渲染与覆盖判断只用坐标),与 xlsx 导入保持一致传空串
    let range = serde_json::json!({
        "sheet": "",
        "start": [st.r1.min(st.r2), st.c1.min(st.c2)],
        "end":   [st.r1.max(st.r2), st.c1.max(st.c2)],
    });
    let operator = if st.is_compare() { DV_OPERATORS[st.operator] } else { "" };
    let rule = serde_json::json!({
        "kind": kind,
        "operator": operator,
        "formula1": f1,
        "formula2": if st.is_range_op() { serde_json::Value::String(f2) } else { serde_json::Value::Null },
        "allow_blank": st.allow_blank,
        "show_error": st.show_error,
        "error_msg": if st.error_msg.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(st.error_msg.clone())
        },
    });
    let script = format!(
        "set_data_validation({}, {}, {})",
        st.sheet,
        rhai_quote(&range.to_string()),
        rhai_quote(&rule.to_string())
    );
    let Some(sess) = s.session.as_mut() else { return };
    let raw = sess.execute_script(&script);
    eprintln!(
        "[ribbon] calc.data_validation sheet={} range=({},{})-({},{}) kind={} 返回: {}",
        st.sheet, st.r1, st.c1, st.r2, st.c2, kind, raw
    );
    let ok = serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| v.get("ok").and_then(|x| x.as_bool()))
        .unwrap_or(false);
    if !ok {
        eprintln!("[ribbon] calc.data_validation 内核返回失败,未写入");
        return;
    }
    refresh_view(&app, sess);
}

/// 数据验证:单元格提交前校验。返回 true=放行,false=拦截(已弹出错误框)。
/// 任何异常路径(非编辑态/选中无效/无会话/解析失败)一律放行,校验是增强功能,不能让用户输不进去。
pub fn dv_validate_input(ctx: &RibbonCtx, text: &str) -> bool {
    let Some(app) = ctx.app_weak.upgrade() else { return true };
    // 仅编辑态校验:非编辑态按回车(text 为空)不应触发 allow_blank=false 的误报
    if !app.get_calc_editing() {
        return true;
    }
    let row = app.get_calc_selected_row();
    let col = app.get_calc_selected_col();
    if row < 0 || col < 0 {
        return true;
    }
    let sheet = ctx.current_sheet_idx.get();
    let verdict = {
        let mut s = ctx.state.borrow_mut();
        let Some(sess) = s.session.as_mut() else { return true };
        let raw = sess.query_script(&format!(
            "validate_cell_input({}, {}, {}, {})",
            sheet, row, col, rhai_quote(text)
        ));
        unwrap_json(&raw)
    };
    let Some(v) = verdict else { return true };
    let valid = v.get("valid").and_then(|x| x.as_bool()).unwrap_or(true);
    let show_error = v.get("show_error").and_then(|x| x.as_bool()).unwrap_or(false);
    if valid || !show_error {
        return true;
    }
    let msg = v.get("error_msg").and_then(|x| x.as_str()).unwrap_or("");
    app.set_dv_err_message(slint::SharedString::from(msg));
    app.set_dv_err_show_cancel(true);
    app.set_dv_err_visible(true);
    false
}

/// 数据验证 list 下拉:按活动格刷新 dv-list-* 三个属性。
/// 选中格变化 / 切 sheet / 新建 sheet 后调用。候选项统一由内核 get_list_candidates 给出
/// (字面量三种格式 + A1 范围引用,含跨 sheet),有候选则显示下拉按钮并填选项;
/// 无 list 规则 / 无候选 / 任何异常都置为不显示。面板一律先关闭。
pub(crate) fn dv_list_refresh(
    app: &crate::AppWindow,
    sess: &mut doc_core::TileCacheRust,
    sheet: u32,
    row: u32,
    col: u32,
) {
    app.set_dv_list_open(false);
    let raw = sess.query_script(&format!("get_list_candidates({}, {}, {})", sheet, row, col));
    let items: Vec<String> = unwrap_json(&raw)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    if items.is_empty() {
        app.set_dv_list_options(slint::ModelRc::new(slint::VecModel::from(
            Vec::<slint::SharedString>::new(),
        )));
        app.set_dv_list_active(false);
        return;
    }
    let model: Vec<slint::SharedString> =
        items.iter().map(|s| slint::SharedString::from(s.as_str())).collect();
    app.set_dv_list_options(slint::ModelRc::new(slint::VecModel::from(model)));
    app.set_dv_list_active(true);
}

/// 解析 detect_header_row 的返回 {"header":true|false}。外壳同其它探测函数:
/// {"ok":true,"value":"<字符串化 JSON>"};兼容 return_value、对象/字符串化 JSON、
/// 顶层直接含 header。解析失败返回 None。
fn parse_header(raw: &str) -> Option<bool> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    if let Some(b) = v.get("header").and_then(|x| x.as_bool()) {
        return Some(b);
    }
    for key in ["value", "return_value"] {
        let Some(inner) = v.get(key) else { continue };
        if let Some(b) = inner.get("header").and_then(|x| x.as_bool()) {
            return Some(b);
        }
        if let Some(t) = inner.as_str() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(t) {
                if let Some(b) = parsed.get("header").and_then(|x| x.as_bool()) {
                    return Some(b);
                }
            }
        }
    }
    None
}

// ══════════ 筛选对话框(get_column_unique_values / apply_auto_filter_rows) ══════════

/// 筛选对话框的瞬时状态(存在 AppState.filter_dialog)。
pub struct FilterDialogState {
    pub sheet: u32,
    pub header_row: u32,
    pub header_col: u32,
    pub end_col: u32,
    pub end_row: u32,
    /// 当前列相对 header_col 的偏移(即 col_filters 的键)
    pub col_off: u32,
    /// 唯一值(展示文本, 个数)
    pub values: Vec<(String, u32)>,
    pub checked: Vec<bool>,
    /// 打开时读到的现有 col_filters(确定时在其上改当前列)
    pub filters: std::collections::HashMap<u32, Vec<String>>,
    /// Some(字段名)=透视表字段筛选(sheet 为透视结果表);None=普通表格筛选
    pub pivot_field: Option<String>,
}

/// 通用外壳解包:{"ok":true,"value":"<字符串化 JSON>"} / return_value / 直接 JSON。
/// get_auto_filter 无筛选时返回 "null",此处得到 Some(Value::Null)。
pub(crate) fn unwrap_json(raw: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    for key in ["value", "return_value"] {
        if let Some(inner) = v.get(key) {
            if let Some(t) = inner.as_str() {
                return serde_json::from_str(t).ok();
            }
            return Some(inner.clone());
        }
    }
    Some(v)
}

fn push_filter(app: &crate::AppWindow, st: &FilterDialogState) {
    let names: Vec<slint::SharedString> =
        st.values.iter().map(|(v, _)| slint::SharedString::from(v.as_str())).collect();
    let counts: Vec<i32> = st.values.iter().map(|(_, n)| *n as i32).collect();
    app.set_filter_items(slint::ModelRc::new(slint::VecModel::from(names)));
    app.set_filter_counts(slint::ModelRc::new(slint::VecModel::from(counts)));
    app.set_filter_checked(slint::ModelRc::new(slint::VecModel::from(st.checked.clone())));
    app.set_filter_all_checked(!st.checked.is_empty() && st.checked.iter().all(|&b| b));
}

/// 数据页"筛选"入口(临时入口:对选中单元格所在列打开对话框)。
pub(crate) fn filter_open(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, _r2, _c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.filter: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let col = c1;
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };

    // 1) 现有 auto_filter(点击列在其范围内才沿用)
    let af = unwrap_json(&sess.query_script(&format!("get_auto_filter({})", sheet)))
        .filter(|v| v.is_object());
    let existing: Option<(u32, u32, u32)> = af.as_ref().and_then(|a| {
        let hr = a.get("header_row")?.as_u64()? as u32;
        let hc = a.get("header_col")?.as_u64()? as u32;
        let ec = a.get("end_col")?.as_u64()? as u32;
        Some((hr, hc, ec))
    }).filter(|&(_, hc, ec)| hc <= col && col <= ec);

    let mut filters: std::collections::HashMap<u32, Vec<String>> = std::collections::HashMap::new();
    let (hr, hc, ec, er);
    if let Some((h_r, h_c, e_c)) = existing {
        if let Some(obj) = af.as_ref().and_then(|a| a.get("col_filters")).and_then(|x| x.as_object()) {
            for (k, v) in obj {
                if let (Ok(off), Some(arr)) = (k.parse::<u32>(), v.as_array()) {
                    filters.insert(off, arr.iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect());
                }
            }
        }
        let Some((_, _, rr2, _)) = expand_to_region(sess, sheet, h_r, col, "calc.filter") else {
            eprintln!("[ribbon] calc.filter: 探测不到数据区,放弃");
            return;
        };
        if rr2 <= h_r {
            eprintln!("[ribbon] calc.filter: 表头下方没有数据行");
            return;
        }
        hr = h_r; hc = h_c; ec = e_c; er = rr2;
    } else {
        let Some((er1, ec1, er2, ec2)) = expand_to_region(sess, sheet, r1, col, "calc.filter") else {
            eprintln!("[ribbon] calc.filter: 探测不到数据区,放弃");
            return;
        };
        if er2 <= er1 {
            eprintln!("[ribbon] calc.filter: 数据区只有一行,无法筛选");
            return;
        }
        hr = er1; hc = ec1; ec = ec2; er = er2;
    }
    let col_off = col - hc;

    // 2) 唯一值
    let raw = sess.query_script(&format!("get_column_unique_values({}, {}, {}, {})", sheet, hr, col, er));
    let Some(uv) = unwrap_json(&raw) else {
        eprintln!("[ribbon] calc.filter: 唯一值返回无法解析: {}", raw);
        return;
    };
    let truncated = uv.get("truncated").and_then(|x| x.as_bool()).unwrap_or(false);
    let values: Vec<(String, u32)> = uv
        .get("values")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .map(|it| {
                    (
                        it.get("v").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        it.get("n").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if values.is_empty() {
        eprintln!("[ribbon] calc.filter: 该列无数据");
        return;
    }
    let checked: Vec<bool> = match filters.get(&col_off) {
        Some(allowed) => values.iter().map(|(v, _)| allowed.contains(v)).collect(),
        None => vec![true; values.len()],
    };

    let st = FilterDialogState {
        sheet, header_row: hr, header_col: hc, end_col: ec, end_row: er,
        col_off, values, checked, filters, pivot_field: None,
    };
    eprintln!(
        "[ribbon] calc.filter 打开 sheet={} 表头行={} 列={} 数据行={}..{} 唯一值={} 已有筛选={} truncated={}",
        sheet, hr, col, hr + 1, er, st.values.len(), existing.is_some(), truncated
    );
    let mut range = format!(
        "列 {}(表头 {}),{} 个不同值",
        crate::col_index_to_letter(col), a1(hr, col), st.values.len()
    );
    if truncated { range.push_str(",仅显示前 5000 项"); }
    app.set_filter_title("筛选".into());
    app.set_filter_range(range.into());
    push_filter(&app, &st);
    s.filter_dialog = Some(st);
    app.set_filter_visible(true);
}

/// 透视表字段筛选入口:sheet=透视结果表索引,source_idx=数据源 sheet 索引
/// (候选值取自源表全部唯一值,不做字段联动),selected 为空=全选。
pub fn filter_open_pivot(ctx: &RibbonCtx, sheet: u32, field: &str, source_idx: u32, selected: &[String]) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    let vals = sess.pivot_unique_values(source_idx, field);
    if vals.is_empty() {
        eprintln!("[ribbon] pivot.filter: 字段 {} 无候选值(源 sheet={})", field, source_idx);
        return;
    }
    let values: Vec<(String, u32)> = vals.into_iter().map(|v| (v, 0u32)).collect();
    let checked: Vec<bool> = if selected.is_empty() {
        vec![true; values.len()]
    } else {
        values.iter().map(|(v, _)| selected.contains(v)).collect()
    };
    let st = FilterDialogState {
        sheet, header_row: 0, header_col: 0, end_col: 0, end_row: 0, col_off: 0,
        values, checked, filters: std::collections::HashMap::new(),
        pivot_field: Some(field.to_string()),
    };
    eprintln!("[ribbon] pivot.filter 打开 sheet={} 字段={} 候选值={} 已选={}", sheet, field, st.values.len(), selected.len());
    app.set_filter_title("筛选".into());
    app.set_filter_range(format!("透视字段 {},{} 个不同值", field, st.values.len()).into());
    push_filter(&app, &st);
    s.filter_dialog = Some(st);
    app.set_filter_visible(true);
}

pub fn filter_toggle(ctx: &RibbonCtx, idx: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.filter_dialog.as_mut() else { return };
    if let Some(b) = st.checked.get_mut(idx) { *b = !*b; }
    push_filter(&app, st);
}

pub fn filter_toggle_all(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.filter_dialog.as_mut() else { return };
    let all = st.checked.iter().all(|&b| b);
    for b in st.checked.iter_mut() { *b = !all; }
    push_filter(&app, st);
}

pub fn filter_cancel(ctx: &RibbonCtx) {
    ctx.state.borrow_mut().filter_dialog = None;
    eprintln!("[ribbon] calc.filter 对话框取消");
}

pub fn filter_confirm(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.filter_dialog.take() else { return };

    // 透视分支:全选=清除筛选(空数组),否则传勾选值;事务包住以便撤销
    if let Some(field) = st.pivot_field.clone() {
        let allowed: Vec<String> = if st.checked.iter().all(|&b| b) {
            Vec::new()
        } else {
            st.values.iter().zip(st.checked.iter())
                .filter(|(_, &c)| c)
                .map(|((v, _), _)| v.clone())
                .collect()
        };
        let Some(sess) = s.session.as_mut() else { return };
        sess.begin_edit();
        let ok = sess.pivot_set_filter(st.sheet, &field, &allowed);
        if ok { sess.commit_edit(); } else { sess.discard_edit(); }
        eprintln!("[ribbon] pivot.filter 确定 sheet={} 字段={} 选中={} ok={}", st.sheet, field, allowed.len(), ok);
        if !ok { return; }
        refresh_view(&app, sess);
        sync_state_inner(&app, sess, st.sheet);
        return;
    }

    let mut filters = st.filters.clone();
    if st.checked.iter().all(|&b| b) {
        filters.remove(&st.col_off);
    } else {
        let allowed: Vec<String> = st
            .values
            .iter()
            .zip(st.checked.iter())
            .filter(|(_, &c)| c)
            .map(|((v, _), _)| v.clone())
            .collect();
        filters.insert(st.col_off, allowed);
    }
    let mut obj = serde_json::Map::new();
    for (k, v) in &filters {
        obj.insert(k.to_string(), serde_json::json!(v));
    }
    let fj = serde_json::json!({ "col_filters": obj }).to_string();

    let merges = s.calc_merges.clone();
    let Some(sess) = s.session.as_mut() else { return };
    let script = format!(
        "apply_auto_filter_rows({}, {}, {}, {}, {}, {})",
        st.sheet, st.header_row, st.header_col, st.end_col, st.end_row, rhai_quote(&fj)
    );
    let raw = sess.execute_script(&script);
    eprintln!(
        "[ribbon] calc.filter 确定 sheet={} 表头行={} 数据行={}..{} col_off={} 返回: {}",
        st.sheet, st.header_row, st.header_row + 1, st.end_row, st.col_off, raw
    );
    let ok = unwrap_json(&raw)
        .and_then(|v| v.get("ok").and_then(|x| x.as_bool()))
        .unwrap_or(true);
    if !ok {
        eprintln!("[ribbon] calc.filter 内核返回失败,未刷新");
        return;
    }
    refresh_view(&app, sess);
    sync_state_inner(&app, sess, st.sheet);
    // 原选中行可能已被隐藏:选区落到该列表头格,并按新行高重算高亮
    crate::calc_selection::select_anchor(&app, sess, &merges, st.sheet, st.header_row, st.header_col + st.col_off);
    reflow_selection(&app, &mut s, st.sheet);
}

/// 筛选下拉面板里的"升序"/"降序"行:对 filter_open 已探测好的数据区
/// (header_row+1..end_row,列=header_col+col_off)按当前列排序。
/// 与 Ribbon 大按钮排序(sort_selection)不同,这里范围已知,不需要重新读选区/
/// 探测数据区/判断表头——filter_open 打开时已经做过一次。
/// 排序含义:对整个数据区排序(含被筛选隐藏的行,隐藏状态不变,即 Excel 的标准行为),
/// 不是"只排可见行"(内核也没有相应能力,做"只排可见行"需要额外查隐藏行状态,
/// 工作量和风险都高得多,不在本次范围)。
/// 排序完成后关闭面板(filter_dialog=None + filter_visible=false),不停留。
pub fn filter_sort(ctx: &RibbonCtx, ascending: bool) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    // 透视分支:必须在持有 st 引用之前拷出 (field, sheet),否则与 s.session 借用冲突
    let pivot = s.filter_dialog.as_ref().and_then(|st| st.pivot_field.clone().map(|f| (f, st.sheet)));
    if let Some((field, sheet)) = pivot {
        s.filter_dialog = None;
        let Some(sess) = s.session.as_mut() else { return };
        sess.begin_edit();
        let ok = sess.pivot_set_sort(sheet, &field, ascending);
        if ok { sess.commit_edit(); } else { sess.discard_edit(); }
        eprintln!("[ribbon] pivot.sort sheet={} 字段={} asc={} ok={}", sheet, field, ascending, ok);
        if ok {
            refresh_view(&app, sess);
            sync_state_inner(&app, sess, sheet);
        }
        drop(s);
        app.set_filter_visible(false);
        return;
    }
    let Some(st) = s.filter_dialog.as_ref() else { return };
    let sheet = st.sheet;
    let r1 = st.header_row + 1;
    let c1 = st.header_col;
    let r2 = st.end_row;
    let c2 = st.end_col;
    let key_col = st.col_off as i64;

    let Some(sess) = s.session.as_mut() else { return };
    let sheet_name = sess.get_sheet_name(sheet);
    let script = format!(
        "sort_range({}, `{{\"sheet\":\"{}\",\"start\":[{},{}],\"end\":[{},{}]}}`, {}, {});",
        sheet, sheet_name, r1, c1, r2, c2, key_col, ascending
    );
    let res = sess.execute_script(&script);
    eprintln!(
        "[ribbon] calc.filter_sort sheet={} range=({},{})-({},{}) key_col={} asc={} 返回: {}",
        sheet, r1, c1, r2, c2, key_col, ascending, res
    );
    if !res.contains("\"ok\":true") {
        eprintln!("[ribbon] calc.filter_sort: 内核返回失败,未刷新");
        return;
    }
    refresh_view(&app, sess);
    sync_state_inner(&app, sess, sheet);
    drop(s);
    ctx.state.borrow_mut().filter_dialog = None;
    app.set_filter_visible(false);
}

// ══════════ 筛选:开/关、清除、重新应用 ══════════

fn filters_to_json(filters: &std::collections::HashMap<u32, Vec<String>>) -> String {
    let mut obj = serde_json::Map::new();
    for (k, v) in filters {
        obj.insert(k.to_string(), serde_json::json!(v));
    }
    serde_json::json!({ "col_filters": obj }).to_string()
}

/// 从表头格向下探测数据区末行;区域只有表头一行或探测失败返回 None。
fn af_end_row(
    sess: &mut doc_core::TileCacheRust,
    sheet: u32,
    hr: u32,
    hc: u32,
    tag: &str,
) -> Option<u32> {
    let (_, _, r2, _) = expand_to_region(sess, sheet, hr, hc, tag)?;
    if r2 > hr { Some(r2) } else { None }
}

/// calc.filter_toggle / calc.filter_clear / calc.filter_reapply
fn filter_cmd(cmd: &str, ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let sheet = ctx.current_sheet_idx.get();
    let sel = read_selection(&app);
    let tag = format!("calc.{}", cmd);
    let mut s = ctx.state.borrow_mut();
    let merges = s.calc_merges.clone();
    let Some(sess) = s.session.as_mut() else { return };

    // 现有 auto_filter:(表头行, 表头列, 末列, col_filters)
    let af = unwrap_json(&sess.query_script(&format!("get_auto_filter({})", sheet)))
        .filter(|v| v.is_object());
    let cur: Option<(u32, u32, u32, std::collections::HashMap<u32, Vec<String>>)> =
        af.as_ref().and_then(|a| {
            let hr = a.get("header_row")?.as_u64()? as u32;
            let hc = a.get("header_col")?.as_u64()? as u32;
            let ec = a.get("end_col")?.as_u64()? as u32;
            let mut m = std::collections::HashMap::new();
            if let Some(obj) = a.get("col_filters").and_then(|x| x.as_object()) {
                for (k, v) in obj {
                    if let (Ok(off), Some(arr)) = (k.parse::<u32>(), v.as_array()) {
                        m.insert(off, arr.iter().filter_map(|x| x.as_str().map(|t| t.to_string())).collect());
                    }
                }
            }
            Some((hr, hc, ec, m))
        });

    let cur_was_some = cur.is_some();
    // (表头行, 表头列, 脚本, 是否改变了行的显示)
    let (hr, hc, script, rows_changed): (u32, u32, String, bool) = match (cmd, cur) {
        ("filter_toggle", Some((hr, hc, _ec, _))) => {
            let er = af_end_row(sess, sheet, hr, hc, &tag).unwrap_or(hr);
            (hr, hc, format!("clear_auto_filter({}, {})", sheet, er), true)
        }
        ("filter_toggle", None) => {
            let Some((r1, c1, _, _)) = sel else {
                eprintln!("[ribbon] {}: 无选中单元格", tag);
                return;
            };
            let Some((er1, ec1, er2, ec2)) = expand_to_region(sess, sheet, r1, c1, &tag) else {
                eprintln!("[ribbon] {}: 探测不到数据区,放弃", tag);
                return;
            };
            if er2 <= er1 {
                eprintln!("[ribbon] {}: 数据区只有一行,无法建立筛选", tag);
                return;
            }
            let fj = filters_to_json(&std::collections::HashMap::new());
            (er1, ec1, format!(
                "apply_auto_filter_rows({}, {}, {}, {}, {}, {})",
                sheet, er1, ec1, ec2, er2, rhai_quote(&fj)
            ), false)
        }
        ("filter_clear", Some((hr, hc, ec, _))) => {
            let Some(er) = af_end_row(sess, sheet, hr, hc, &tag) else {
                eprintln!("[ribbon] {}: 表头下方没有数据行", tag);
                return;
            };
            let fj = filters_to_json(&std::collections::HashMap::new());
            (hr, hc, format!(
                "apply_auto_filter_rows({}, {}, {}, {}, {}, {})",
                sheet, hr, hc, ec, er, rhai_quote(&fj)
            ), true)
        }
        ("filter_reapply", Some((hr, hc, ec, filters))) => {
            let Some(er) = af_end_row(sess, sheet, hr, hc, &tag) else {
                eprintln!("[ribbon] {}: 表头下方没有数据行", tag);
                return;
            };
            let fj = filters_to_json(&filters);
            (hr, hc, format!(
                "apply_auto_filter_rows({}, {}, {}, {}, {}, {})",
                sheet, hr, hc, ec, er, rhai_quote(&fj)
            ), true)
        }
        (_, None) => {
            eprintln!("[ribbon] {}: 当前表没有筛选,忽略", tag);
            return;
        }
        _ => return,
    };

    let raw = sess.execute_script(&script);
    eprintln!("[ribbon] {} 表头=({},{}) 返回: {}", tag, hr, hc, raw);
    let ok = unwrap_json(&raw)
        .and_then(|v| v.get("ok").and_then(|x| x.as_bool()))
        .unwrap_or(true);
    if !ok {
        eprintln!("[ribbon] {} 内核返回失败,未刷新", tag);
        return;
    }
    refresh_view(&app, sess);
    sync_state_inner(&app, sess, sheet);
    // Ribbon 筛选大按钮高亮:filter_toggle 切换开关状态;filter_clear/filter_reapply 之后筛选仍存在,保持高亮
    let now_active = match cmd {
        "filter_toggle" => !cur_was_some,
        "filter_clear" | "filter_reapply" => true,
        _ => app.get_calc_filter_active(),
    };
    app.set_calc_filter_active(now_active);
    if rows_changed {
        // 行的显示变了:选区落到表头格,并按新行高重算高亮
        crate::calc_selection::select_anchor(&app, sess, &merges, sheet, hr, hc);
        reflow_selection(&app, &mut s, sheet);
    }
}
