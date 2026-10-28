//! Calc「开始」页按钮。

use super::RibbonCtx;

pub fn handle(cmd: &str, ctx: &RibbonCtx) {
    // 带参数的命令:"bg_color:#RRGGBB" / "fg_color:#RRGGBB",空参数 = 清除
    if let Some(hex) = cmd.strip_prefix("bg_color:") {
        return set_color(ctx, "bg_color", hex);
    }
    if let Some(hex) = cmd.strip_prefix("fg_color:") {
        return set_color(ctx, "fg_color", hex);
    }
    if let Some(name) = cmd.strip_prefix("font_family:") {
        return set_font_family(ctx, name);
    }
    if let Some(v) = cmd.strip_prefix("font_size:") {
        return set_font_size(ctx, v);
    }
    if let Some(v) = cmd.strip_prefix("num_format:") {
        return set_num_format(ctx, v);
    }
    if let Some(code) = cmd.strip_prefix("num_code:") {
        return apply_num_code(ctx, Some(code));
    }
    if let Some(v) = cmd.strip_prefix("border:") {
        return apply_border(ctx, v);
    }
    if let Some(v) = cmd.strip_prefix("cf:") {
        return apply_cf(ctx, v);
    }
    if let Some(v) = cmd.strip_prefix("style:") {
        return apply_cell_style(ctx, v);
    }
    if let Some(v) = cmd.strip_prefix("tstyle:") {
        return apply_table_style(ctx, v);
    }
    match cmd {
        "bold" => toggle_flag(ctx, "bold"),
        "italic" => toggle_flag(ctx, "italic"),
        "underline" => toggle_flag(ctx, "underline"),
        "wrap" => toggle_flag(ctx, "wrap_text"),
        "decimal_inc" => step_decimals(ctx, 1),
        "decimal_dec" => step_decimals(ctx, -1),
        "font_grow" => step_font_size(ctx, 1),
        "font_shrink" => step_font_size(ctx, -1),
        "align_left" => set_align(ctx, "align_h", "left"),
        "align_center" => set_align(ctx, "align_h", "center"),
        "align_right" => set_align(ctx, "align_h", "right"),
        "align_top" => set_align(ctx, "align_v", "top"),
        "align_middle" => set_align(ctx, "align_v", "middle"),
        "align_bottom" => set_align(ctx, "align_v", "bottom"),
        "merge" => merge_selection(ctx),
        "format_painter" => painter_toggle(ctx),
        "insert_row" => structure_edit(ctx, StructureOp::InsertRow),
        "delete_row" => structure_edit(ctx, StructureOp::DeleteRow),
        "insert_col" => structure_edit(ctx, StructureOp::InsertCol),
        "delete_col" => structure_edit(ctx, StructureOp::DeleteCol),
        _ => eprintln!("[ribbon] calc 未接线的命令: {}", cmd),
    }
}

/// 读取当前选区，归一化为 (r1, c1, r2, c2)（左上到右下）。
/// calc-sel-end-* 为 -1 表示单选，此时终点等于锚点。
pub(super) fn read_selection(app: &super::AppWindow) -> Option<(u32, u32, u32, u32)> {
    let ar = app.get_calc_selected_row();
    let ac = app.get_calc_selected_col();
    if ar < 0 || ac < 0 {
        return None;
    }
    let er = app.get_calc_sel_end_row();
    let ec = app.get_calc_sel_end_col();
    let (er, ec) = if er < 0 || ec < 0 { (ar, ac) } else { (er, ec) };
    Some((
        ar.min(er) as u32,
        ac.min(ec) as u32,
        ar.max(er) as u32,
        ac.max(ec) as u32,
    ))
}

/// 布尔型格式开关:以选区左上角格当前是否已设置该字段决定目标状态
/// (已设置则整区取消,否则整区设置),与 Excel 行为一致。
fn toggle_flag(ctx: &RibbonCtx, field: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.{}: 无选中单元格", field);
        return;
    };
    let sheet = ctx.current_sheet_idx.get();

    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };

    let fmt = sess.calc_get_cell_format(sheet, r1, c1);
    let current = fmt.get(field).and_then(|b| b.as_bool()).unwrap_or(false);
    let target = !current;

    if !sess.calc_apply_format_flag(sheet, r1, c1, r2, c2, field, target) {
        eprintln!("[ribbon] calc.{} 执行失败", field);
        return;
    }
    refresh_view(&app, sess);
    sync_state_inner(&app, sess, sheet);
    // 换行等格式会改变自动行高,按当前选区重算高亮框
    reflow_selection(&app, &mut s, sheet);
}

/// 对齐(单选):锚点格当前值等于目标值则清除(取消),否则整区设为目标值。
fn set_align(ctx: &RibbonCtx, field: &str, value: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.{}: 无选中单元格", field);
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };

    let fmt = sess.calc_get_cell_format(sheet, r1, c1);
    let current = fmt.get(field).and_then(|v| v.as_str()).unwrap_or("");
    let target = if current == value { None } else { Some(value) };

    if !sess.calc_apply_format_str(sheet, r1, c1, r2, c2, field, target) {
        eprintln!("[ribbon] calc.{} 执行失败", field);
        return;
    }
    refresh_view(&app, sess);
    sync_state_at(&app, sess, sheet, r1, c1);
}

/// 颜色(背景/文字):hex 为 "#RRGGBB" 则整区设置,空串则清除该字段。
fn set_color(ctx: &RibbonCtx, field: &str, hex: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.{}: 无选中单元格", field);
        return;
    };
    let value = if hex.is_empty() {
        None
    } else if hex.len() == 7 && hex.starts_with('#') {
        Some(hex)
    } else {
        eprintln!("[ribbon] calc.{} 非法颜色: {}", field, hex);
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    if !sess.calc_apply_format_str(sheet, r1, c1, r2, c2, field, value) {
        eprintln!("[ribbon] calc.{} 执行失败", field);
        return;
    }
    eprintln!("[ribbon] calc.{} ({},{})-({},{}) = {:?}", field, r1, c1, r2, c2, value);
    refresh_view(&app, sess);
}

/// 字体名:非空则整区设置,空串则清除(回落默认字体)。
fn set_font_family(ctx: &RibbonCtx, name: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.font_family: 无选中单元格");
        return;
    };
    let value = if name.is_empty() { None } else { Some(name) };
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    if !sess.calc_apply_format_str(sheet, r1, c1, r2, c2, "font_family", value) {
        eprintln!("[ribbon] calc.font_family 执行失败");
        return;
    }
    eprintln!("[ribbon] calc.font_family ({},{})-({},{}) = {:?}", r1, c1, r2, c2, value);
    sync_state_inner(&app, sess, sheet);
    refresh_view(&app, sess);
    reflow_selection(&app, &mut s, sheet);
}

/// 字号:整区设置。参数为数字文本(下拉预设值)。
fn set_font_size(ctx: &RibbonCtx, text: &str) {
    let Ok(size) = text.trim().parse::<f64>() else {
        eprintln!("[ribbon] calc.font_size 参数非法: {:?}", text);
        return;
    };
    if !(1.0..=409.0).contains(&size) {
        eprintln!("[ribbon] calc.font_size 超出范围: {}", size);
        return;
    }
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.font_size: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    if !sess.calc_apply_format_num(sheet, r1, c1, r2, c2, "font_size", size) {
        eprintln!("[ribbon] calc.font_size 执行失败");
        return;
    }
    eprintln!("[ribbon] calc.font_size ({},{})-({},{}) = {}", r1, c1, r2, c2, size);
    sync_state_inner(&app, sess, sheet);
    refresh_view(&app, sess);
    // 字号变化会改变行高/列宽:重算高亮框
    reflow_selection(&app, &mut s, sheet);
}

/// 合并选区。单格选区无意义,直接忽略。
/// 注意:内核 merge_cells 只登记 merged_cells,不会像 Excel 那样清除被覆盖格的内容。
fn merge_selection(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else { return };
    if r1 == r2 && c1 == c2 {
        eprintln!("[ribbon] calc.merge: 单格选区,忽略");
        return;
    }
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    let script = format!("merge_cells({}, {}, {}, {}, {});", sheet, r1, c1, r2, c2);
    let res = sess.execute_script(&script);
    eprintln!("[ribbon] calc.merge ({},{})-({},{}) 返回: {}", r1, c1, r2, c2, res);
    if !res.contains("\"ok\":true") {
        return;
    }
    refresh_view(&app, sess);
    // 合并后:刷新合并缓存,选区吸附到左上角并整块高亮
    reflow_selection(&app, &mut s, sheet);
}

#[derive(Clone, Copy)]
enum StructureOp {
    InsertRow,
    DeleteRow,
    InsertCol,
    DeleteCol,
}

/// 结构性编辑:以选区左上角为准,每次只插/删 1 行或 1 列。
/// - 插入行/列:在选区首行上方/首列左侧插入。
/// - 删除行/列:只删选区首行/首列(多行/多列删除记入遗留)。
/// 已知遗留:高亮框尺寸不会随行高/列宽变化即时重算;浮动图表、
/// 条件格式、数据验证、公式引用不随结构编辑平移(内核未处理)。
fn structure_edit(ctx: &RibbonCtx, op: StructureOp) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, _, _)) = read_selection(&app) else {
        eprintln!("[ribbon] calc 结构编辑: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };

    let (name, script) = match op {
        StructureOp::InsertRow => ("insert_row", format!("insert_row({}, {});", sheet, r1)),
        StructureOp::DeleteRow => ("delete_row", format!("delete_row({}, {});", sheet, r1)),
        StructureOp::InsertCol => ("insert_col", format!("insert_column({}, {});", sheet, c1)),
        StructureOp::DeleteCol => ("delete_col", format!("delete_column({}, {});", sheet, c1)),
    };
    let res = sess.execute_script(&script);
    eprintln!("[ribbon] calc.{} 选区首格=({},{}) 返回: {}", name, r1, c1, res);
    if !res.contains("\"ok\":true") {
        return;
    }
    refresh_view(&app, sess);
    refresh_formula_bar(&app, sess, sheet, r1, c1);
    sync_state_at(&app, sess, sheet, r1, c1);
    // 行高/列宽/合并区都可能变化,刷新缓存并重算高亮
    reflow_selection(&app, &mut s, sheet);
}

/// 按给定单元格刷新公式栏显示值(公式格显示公式原文,否则显示值)。
/// 与 on_calc_nav / on_calc_cell_click 中的同名逻辑一致。
pub fn refresh_formula_bar(app: &super::AppWindow, sess: &mut doc_core::TileCacheRust, sheet: u32, row: u32, col: u32) {
    let formula = sess.calc_get_cell_formula(sheet, row, col);
    let val = if !formula.is_empty() {
        format!("={}", formula)
    } else {
        sess.calc_get_cell(sheet, row, col)
    };
    app.set_calc_display_value(val.into());
}

/// 按当前选区左上角格的格式,回写 Ribbon 按钮的按下态。
/// 选中格变化、撤销/重做之后都应调用。
pub fn sync_state(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    sync_state_inner(&app, sess, sheet);
}

/// A+/A-:按预设表跳档(与字号下拉一致)。以选区左上角格的字号为基准,
/// 当前值不在表内时,A+ 取第一个更大的档,A- 取第一个更小的档;两端封顶。
fn step_font_size(ctx: &RibbonCtx, dir: i32) {
    const STEPS: [f64; 16] = [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0, 26.0, 28.0, 36.0, 48.0, 72.0];
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, _, _)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.font_step: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let cur = {
        let mut s = ctx.state.borrow_mut();
        let Some(sess) = s.session.as_mut() else { return };
        let fmt = sess.calc_get_cell_format(sheet, r1, c1);
        fmt.get("font_size").and_then(|v| v.as_f64()).unwrap_or(11.0)
    };
    let next = if dir > 0 {
        STEPS.iter().copied().find(|&x| x > cur).unwrap_or(STEPS[STEPS.len() - 1])
    } else {
        STEPS.iter().rev().copied().find(|&x| x < cur).unwrap_or(STEPS[0])
    };
    eprintln!("[ribbon] calc.font_step {} -> {}", cur, next);
    set_font_size(ctx, &format!("{}", next));
}

/// 数字格式预设:(显示名, 格式码)。格式码 None 表示清除字段(常规)。
/// 下拉浮层的显示顺序与此表一致(app.slint 的 num-menu-list 需同步修改)。
const NUM_FORMATS: [(&str, Option<&str>); 7] = [
    ("常规", None),
    ("数字", Some("0.00")),
    ("货币", Some("¥#,##0.00")),
    ("会计专用", Some("#,##0.00;(#,##0.00)")),
    ("百分比", Some("0.00%")),
    ("短日期", Some("yyyy-mm-dd")),
    ("文本", Some("@")),
];

/// 数字格式:参数为预设下标,整区设置;下标 0(常规)清除字段。
fn set_num_format(ctx: &RibbonCtx, idx_text: &str) {
    let Ok(idx) = idx_text.trim().parse::<usize>() else {
        eprintln!("[ribbon] calc.num_format 参数非法: {:?}", idx_text);
        return;
    };
    let Some((_name, code)) = NUM_FORMATS.get(idx).copied() else {
        eprintln!("[ribbon] calc.num_format 下标越界: {}", idx);
        return;
    };
    apply_num_code(ctx, code);
}

/// 按格式码整区写入(None = 清除为常规),随后回显、刷新、重算高亮框。
fn apply_num_code(ctx: &RibbonCtx, code: Option<&str>) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.num_format: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    if !sess.calc_apply_format_str(sheet, r1, c1, r2, c2, "num_format", code) {
        eprintln!("[ribbon] calc.num_format 执行失败");
        return;
    }
    eprintln!("[ribbon] calc.num_format ({},{})-({},{}) = {:?}", r1, c1, r2, c2, code);
    sync_state_inner(&app, sess, sheet);
    refresh_view(&app, sess);
    // 显示串宽度/行高可能变化:重算高亮框
    reflow_selection(&app, &mut s, sheet);
}

/// 边框预设:参数形如 "all:#000000"(模式:笔色)。写入后与其余格式命令同一条回显/重绘链路。
fn apply_border(ctx: &RibbonCtx, arg: &str) {
    let (mode, hex) = arg.split_once(':').unwrap_or((arg, ""));
    if !matches!(mode, "top" | "bottom" | "left" | "right" | "none" | "all" | "outside" | "inside" | "inside_h" | "inside_v") {
        eprintln!("[ribbon] calc.border 未知模式: {}", mode);
        return;
    }
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.border: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    if !sess.calc_apply_borders(sheet, r1, c1, r2, c2, mode, "thin", hex) {
        eprintln!("[ribbon] calc.border 执行失败");
        return;
    }
    eprintln!("[ribbon] calc.border ({},{})-({},{}) mode={} color={}", r1, c1, r2, c2, mode, hex);
    sync_state_inner(&app, sess, sheet);
    refresh_view(&app, sess);
    reflow_selection(&app, &mut s, sheet);
}

// ── 单元格样式。顺序需与 app.slint 的 style-names 等数组一致(两处均由同一脚本生成) ──
/// (名称, 字色, 底色, 加粗, 斜体, 字号(None=默认), 边框列表(模式, 线型, 色), 数字格式码(Some 时只写 num_format))
type StyleDef = (&'static str, Option<&'static str>, Option<&'static str>, bool, bool, Option<f32>, &'static [(&'static str, &'static str, &'static str)], Option<&'static str>);
const CELL_STYLES: [StyleDef; 47] = [
    ("常规", None, None, false, false, None, &[], None),
    ("差", Some("#9C0006"), Some("#FFC7CE"), false, false, None, &[], None),
    ("好", Some("#006100"), Some("#C6EFCE"), false, false, None, &[], None),
    ("适中", Some("#9C5700"), Some("#FFEB9C"), false, false, None, &[], None),
    ("计算", Some("#FA7D00"), Some("#F2F2F2"), true, false, None, &[("all", "thin", "#7F7F7F")], None),
    ("检查单元格", Some("#FFFFFF"), Some("#A5A5A5"), true, false, None, &[("all", "double", "#3F3F3F")], None),
    ("解释性文本", Some("#7F7F7F"), None, false, true, None, &[], None),
    ("警告文本", Some("#FF0000"), None, false, false, None, &[], None),
    ("链接单元格", Some("#FA7D00"), None, false, false, None, &[("bottom", "double", "#FF8001")], None),
    ("输出", Some("#3F3F3F"), Some("#F2F2F2"), true, false, None, &[("all", "thin", "#3F3F3F")], None),
    ("输入", Some("#3F3F76"), Some("#FFCC99"), false, false, None, &[("all", "thin", "#7F7F7F")], None),
    ("注释", None, Some("#FFFFCC"), false, false, None, &[("all", "thin", "#B2B2B2")], None),
    ("标题", Some("#44546A"), None, false, false, Some(18.0), &[], None),
    ("标题 1", Some("#44546A"), None, true, false, Some(15.0), &[("bottom", "thick", "#4472C4")], None),
    ("标题 2", Some("#44546A"), None, true, false, Some(13.0), &[("bottom", "thick", "#8EA9DB")], None),
    ("标题 3", Some("#44546A"), None, true, false, None, &[("bottom", "medium", "#8EA9DB")], None),
    ("标题 4", Some("#44546A"), None, true, false, None, &[], None),
    ("汇总", Some("#000000"), None, true, false, None, &[("top", "thin", "#4472C4"), ("bottom", "double", "#4472C4")], None),
    ("20% - 着色 1", Some("#000000"), Some("#D9E1F2"), false, false, None, &[], None),
    ("20% - 着色 2", Some("#000000"), Some("#FCE4D6"), false, false, None, &[], None),
    ("20% - 着色 3", Some("#000000"), Some("#EDEDED"), false, false, None, &[], None),
    ("20% - 着色 4", Some("#000000"), Some("#FFF2CC"), false, false, None, &[], None),
    ("20% - 着色 5", Some("#000000"), Some("#DDEBF7"), false, false, None, &[], None),
    ("20% - 着色 6", Some("#000000"), Some("#E2EFDA"), false, false, None, &[], None),
    ("40% - 着色 1", Some("#000000"), Some("#B4C6E7"), false, false, None, &[], None),
    ("40% - 着色 2", Some("#000000"), Some("#F8CBAD"), false, false, None, &[], None),
    ("40% - 着色 3", Some("#000000"), Some("#DBDBDB"), false, false, None, &[], None),
    ("40% - 着色 4", Some("#000000"), Some("#FFE699"), false, false, None, &[], None),
    ("40% - 着色 5", Some("#000000"), Some("#BDD7EE"), false, false, None, &[], None),
    ("40% - 着色 6", Some("#000000"), Some("#C6E0B4"), false, false, None, &[], None),
    ("60% - 着色 1", Some("#000000"), Some("#8EA9DB"), false, false, None, &[], None),
    ("60% - 着色 2", Some("#000000"), Some("#F4B084"), false, false, None, &[], None),
    ("60% - 着色 3", Some("#000000"), Some("#C9C9C9"), false, false, None, &[], None),
    ("60% - 着色 4", Some("#000000"), Some("#FFD966"), false, false, None, &[], None),
    ("60% - 着色 5", Some("#000000"), Some("#9BC2E6"), false, false, None, &[], None),
    ("60% - 着色 6", Some("#000000"), Some("#A9D08E"), false, false, None, &[], None),
    ("着色 1", Some("#FFFFFF"), Some("#4472C4"), false, false, None, &[], None),
    ("着色 2", Some("#FFFFFF"), Some("#ED7D31"), false, false, None, &[], None),
    ("着色 3", Some("#FFFFFF"), Some("#A5A5A5"), false, false, None, &[], None),
    ("着色 4", Some("#FFFFFF"), Some("#FFC000"), false, false, None, &[], None),
    ("着色 5", Some("#FFFFFF"), Some("#5B9BD5"), false, false, None, &[], None),
    ("着色 6", Some("#FFFFFF"), Some("#70AD47"), false, false, None, &[], None),
    ("百分比", None, None, false, false, None, &[], Some("0%")),
    ("货币", None, None, false, false, None, &[], Some("¥#,##0.00")),
    ("货币[0]", None, None, false, false, None, &[], Some("¥#,##0")),
    ("千位分隔", None, None, false, false, None, &[], Some("#,##0.00")),
    ("千位分隔[0]", None, None, false, false, None, &[], Some("#,##0")),
];

// ── 格式刷 ──────────────────────────────────────────────
// mode: 0=关 1=单次 2=连续(400ms 内连点两次进入)。按钮再点一次退出。
struct Painter {
    mode: u8,
    fmt: serde_json::Value,
    last_click: Option<std::time::Instant>,
}
thread_local! {
    static PAINTER: std::cell::RefCell<Painter> = std::cell::RefCell::new(Painter {
        mode: 0,
        fmt: serde_json::Value::Null,
        last_click: None,
    });
}

fn painter_toggle(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let now = std::time::Instant::now();
    let (mode, last) = PAINTER.with(|p| {
        let p = p.borrow();
        (p.mode, p.last_click)
    });
    let is_double = last.map_or(false, |t| now.duration_since(t).as_millis() < 400);
    if mode == 1 && is_double {
        PAINTER.with(|p| p.borrow_mut().mode = 2);
        app.set_calc_painter_active(true);
        eprintln!("[ribbon] calc.format_painter 连续模式");
        return;
    }
    if mode != 0 {
        PAINTER.with(|p| {
            let mut p = p.borrow_mut();
            p.mode = 0;
            p.last_click = None;
        });
        app.set_calc_painter_active(false);
        eprintln!("[ribbon] calc.format_painter 关闭");
        return;
    }
    let Some((r1, c1, _, _)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.format_painter: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let fmt = {
        let mut s = ctx.state.borrow_mut();
        match s.session.as_mut() {
            Some(sess) => sess.calc_get_cell_format(sheet, r1, c1),
            None => return,
        }
    };
    eprintln!("[ribbon] calc.format_painter 取格式 ({},{}) {}", r1, c1, fmt);
    PAINTER.with(|p| {
        let mut p = p.borrow_mut();
        p.mode = 1;
        p.fmt = fmt;
        p.last_click = Some(now);
    });
    app.set_calc_painter_active(true);
}

/// Esc 退出格式刷。
pub fn painter_cancel(ctx: &RibbonCtx) {
    PAINTER.with(|p| {
        let mut p = p.borrow_mut();
        p.mode = 0;
        p.last_click = None;
    });
    if let Some(app) = ctx.app_weak.upgrade() {
        app.set_calc_painter_active(false);
    }
    eprintln!("[ribbon] calc.format_painter Esc 退出");
}

/// 网格鼠标抬起时调用(选区此时已确定)。刷状态为关时直接返回。
pub fn painter_on_pointer_up(ctx: &RibbonCtx) {
    let (mode, fmt) = PAINTER.with(|p| {
        let p = p.borrow();
        (p.mode, p.fmt.clone())
    });
    if mode == 0 {
        return;
    }
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else { return };
    let sheet = ctx.current_sheet_idx.get();
    {
        let mut s = ctx.state.borrow_mut();
        let Some(sess) = s.session.as_mut() else { return };
        if sess.calc_paint_format(sheet, r1, c1, r2, c2, &fmt) {
            eprintln!("[ribbon] calc.format_painter 应用到 ({},{})-({},{})", r1, c1, r2, c2);
            sync_state_inner(&app, sess, sheet);
            refresh_view(&app, sess);
            reflow_selection(&app, &mut s, sheet);
        } else {
            eprintln!("[ribbon] calc.format_painter 执行失败");
        }
    }
    if mode == 1 {
        PAINTER.with(|p| {
            let mut p = p.borrow_mut();
            p.mode = 0;
            p.last_click = None;
        });
        app.set_calc_painter_active(false);
    }
}

fn apply_cell_style(ctx: &RibbonCtx, arg: &str) {
    let Some(def) = arg.parse::<usize>().ok().and_then(|i| CELL_STYLES.get(i)) else {
        eprintln!("[ribbon] calc.style 未知参数: {}", arg);
        return;
    };
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, r2, c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.style: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    let ok = match def.7 {
        Some(code) => sess.calc_apply_format_str(sheet, r1, c1, r2, c2, "num_format", Some(code)),
        None => sess.calc_apply_style_fields(sheet, r1, c1, r2, c2, def.1, def.2, def.3, def.4, def.5, def.6),
    };
    if !ok {
        eprintln!("[ribbon] calc.style 执行失败: {}", def.0);
        return;
    }
    eprintln!("[ribbon] calc.style ({},{})-({},{}) {}", r1, c1, r2, c2, def.0);
    sync_state_inner(&app, sess, sheet);
    refresh_view(&app, sess);
    reflow_selection(&app, &mut s, sheet);
}

// ── 套用表格格式。命令 "tstyle:<序号>" / "tstyle:clear" ──
// 序号顺序与 roe excel_table_styles_data 一致: 0~20 Light1~21, 21~48 Medium1~28, 49~59 Dark1~11
// (弹层里的网格也必须按此序排列)。
fn table_style_name_of(arg: &str) -> Option<String> {
    if arg == "clear" {
        return Some(String::new());
    }
    let i: usize = arg.parse().ok()?;
    match i {
        0..=20 => Some(format!("TableStyleLight{}", i + 1)),
        21..=48 => Some(format!("TableStyleMedium{}", i - 20)),
        49..=59 => Some(format!("TableStyleDark{}", i - 48)),
        _ => None,
    }
}

fn apply_table_style(ctx: &RibbonCtx, arg: &str) {
    let Some(style) = table_style_name_of(arg) else {
        eprintln!("[ribbon] calc.tstyle 未知参数: {}", arg);
        return;
    };
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((mut r1, mut c1, mut r2, mut c2)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.tstyle: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    // 单格选区且不在已有表内: 扩成当前数据区(Excel 同款);扩不出就不建 1x1 的表
    if r1 == r2 && c1 == c2 {
        let in_table = sess.calc_get_tables(sheet).iter().any(|t| {
            let Some(r) = t.get("range").and_then(|x| x.as_array()) else { return false };
            let g = |i: usize| r.get(i).and_then(|x| x.as_u64()).unwrap_or(0) as u32;
            g(0) <= r1 && r1 <= g(2) && g(1) <= c1 && c1 <= g(3)
        });
        if !in_table {
            match super::calc_data::expand_to_region(sess, sheet, r1, c1, "tstyle") {
                Some((a, b, c, d)) => { r1 = a; c1 = b; r2 = c; c2 = d; }
                None => {
                    eprintln!("[ribbon] calc.tstyle: 单元格附近无连续数据区,未建表");
                    return;
                }
            }
        }
    }
    match sess.calc_apply_table_style(sheet, r1, c1, r2, c2, &style) {
        Ok(ret) => eprintln!("[ribbon] calc.tstyle ({},{})-({},{}) {} -> {}", r1, c1, r2, c2, style, ret),
        Err(e) => {
            eprintln!("[ribbon] calc.tstyle 失败: {}", e);
            return;
        }
    }
    sync_state_inner(&app, sess, sheet);
    refresh_view(&app, sess);
    reflow_selection(&app, &mut s, sheet);
}

// ── 条件格式(数据条/色阶/清除)。顺序需与 app.slint 的 cf-bar-colors / cf-scales 一致 ──
const CF_BAR_COLORS: [&str; 6] = ["#638EC6", "#63C384", "#FF555A", "#FFB628", "#008AEF", "#D6007B"];
/// (最小色, 中间色(None=2 色阶), 最大色)
const CF_SCALES: [(&str, Option<&str>, &str); 12] = [
    ("#63BE7B", Some("#FFEB84"), "#F8696B"), // 绿-黄-红
    ("#F8696B", Some("#FFEB84"), "#63BE7B"), // 红-黄-绿
    ("#63BE7B", Some("#FCFCFF"), "#F8696B"), // 绿-白-红
    ("#F8696B", Some("#FCFCFF"), "#63BE7B"), // 红-白-绿
    ("#5A8AC6", Some("#FCFCFF"), "#F8696B"), // 蓝-白-红
    ("#F8696B", Some("#FCFCFF"), "#5A8AC6"), // 红-白-蓝
    ("#FCFCFF", None, "#F8696B"),            // 白-红
    ("#F8696B", None, "#FCFCFF"),            // 红-白
    ("#63BE7B", None, "#FCFCFF"),            // 绿-白
    ("#FCFCFF", None, "#63BE7B"),            // 白-绿
    ("#63BE7B", None, "#FFEF9C"),            // 绿-黄
    ("#FFEF9C", None, "#63BE7B"),            // 黄-绿
];

/// "bar:N"(0-5 渐变,6-11 实心)/ "scale:N" → ConditionalFormatKind 的 JSON。
fn cf_kind_json(spec: &str) -> Option<String> {
    let (kind, idx) = spec.split_once(':')?;
    let idx: usize = idx.parse().ok()?;
    match kind {
        "bar" if idx < 12 => Some(format!(
            r##"{{"DataBar":{{"min":{{"kind":"autoMin","val":"","gte":true}},"max":{{"kind":"autoMax","val":"","gte":true}},"bar_color":"{}","border_color":null,"direction":"LeftToRight","bar_only":false,"solid_fill":{},"negative_fill_color":null,"axis_color":null,"axis_position":"automatic"}}}}"##,
            CF_BAR_COLORS[idx % 6],
            idx >= 6
        )),
        "scale" if idx < 12 => {
            let (min_c, mid, max_c) = CF_SCALES[idx];
            let mid_json = match mid {
                Some(m) => format!(r##"["percentile","50","{}"]"##, m),
                None => "null".to_string(),
            };
            Some(format!(
                r##"{{"ColorScale":{{"min_type":"min","min_value":null,"min_color":"{}","mid":{},"max_type":"max","max_value":null,"max_color":"{}"}}}}"##,
                min_c, mid_json, max_c
            ))
        }
        _ => None,
    }
}

/// 条件格式命令:"bar:N" / "scale:N" / "clear:sel" / "clear:sheet"。
fn apply_cf(ctx: &RibbonCtx, arg: &str) {
    // 突出显示规则:打开对话框,确认后由 calc_cf::hl_confirm 写入(走 add,不做可视化互斥)
    if let Some(idx) = arg.strip_prefix("hl:") {
        return super::calc_cf::hl_open(ctx, idx);
    }
    if arg == "icon" {
        return super::calc_cf::icon_open(ctx);
    }
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let sel = read_selection(&app);
    if sel.is_none() && arg != "clear:sheet" {
        eprintln!("[ribbon] calc.cf: 无选中单元格");
        return;
    }
    let (r1, c1, r2, c2) = sel.unwrap_or((0, 0, 0, 0));
    let sheet = ctx.current_sheet_idx.get();
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    let ok = match arg {
        "clear:sel" => sess.calc_clear_cf(sheet, r1, c1, r2, c2),
        "clear:sheet" => sess.calc_clear_cf(sheet, 0, 0, 1_048_575, 16_383),
        _ => {
            let Some(kind) = cf_kind_json(arg) else {
                eprintln!("[ribbon] calc.cf 未知参数: {}", arg);
                return;
            };
            sess.calc_replace_cf_visual(sheet, r1, c1, r2, c2, &kind)
        }
    };
    if !ok {
        eprintln!("[ribbon] calc.cf 执行失败: {}", arg);
        return;
    }
    eprintln!("[ribbon] calc.cf ({},{})-({},{}) {}", r1, c1, r2, c2, arg);
    sync_state_inner(&app, sess, sheet);
    refresh_view(&app, sess);
    reflow_selection(&app, &mut s, sheet);
}

/// 在简单格式码上增减小数位。只处理单段码:前缀/后缀为非数字占位字符
/// (如 ¥、%)时原样保留,中间为 [#,]*0(.0*)? 的数字体。
/// 遇到分号、引号、@、日期字符等复杂码返回 None(调用方忽略)。
fn adjust_decimals(code: &str, dir: i32) -> Option<String> {
    if code.contains(';') || code.contains('"') || code.contains('@') || code.contains('\\') || code.contains('[') {
        return None;
    }
    let chars: Vec<char> = code.chars().collect();
    let is_body = |c: char| matches!(c, '#' | '0' | ',' | '.');
    let start = chars.iter().position(|&c| is_body(c))?;
    let end = chars.iter().rposition(|&c| is_body(c))? + 1;
    let (prefix, body, suffix) = (
        chars[..start].iter().collect::<String>(),
        chars[start..end].iter().collect::<String>(),
        chars[end..].iter().collect::<String>(),
    );
    // 前后缀里不允许出现字母(日期/科学计数等),仅放行符号如 ¥ $ % 空格
    if prefix.chars().chain(suffix.chars()).any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let (int_part, dec_part) = match body.split_once('.') {
        Some((i, d)) => (i.to_string(), d.to_string()),
        None => (body.clone(), String::new()),
    };
    if !int_part.chars().all(|c| matches!(c, '#' | '0' | ',')) || !dec_part.chars().all(|c| c == '0' || c == '#') {
        return None;
    }
    let n = dec_part.chars().count() as i32 + dir;
    let n = n.clamp(0, 10) as usize;
    let new_body = if n == 0 { int_part } else { format!("{}.{}", int_part, "0".repeat(n)) };
    Some(format!("{}{}{}", prefix, new_body, suffix))
}

/// 增加/减少小数位:以选区左上角格的格式码为基准。
/// 常规(无码)按 "0" 起步:增加得 0.0,减少不动。
fn step_decimals(ctx: &RibbonCtx, dir: i32) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((r1, c1, _, _)) = read_selection(&app) else {
        eprintln!("[ribbon] calc.decimals: 无选中单元格");
        return;
    };
    let sheet = ctx.current_sheet_idx.get();
    let cur = {
        let mut s = ctx.state.borrow_mut();
        let Some(sess) = s.session.as_mut() else { return };
        let fmt = sess.calc_get_cell_format(sheet, r1, c1);
        fmt.get("num_format").and_then(|v| v.as_str()).map(String::from)
    };
    let base = match cur.as_deref() {
        None | Some("") | Some("General") => "0".to_string(),
        Some(c) => c.to_string(),
    };
    let Some(next) = adjust_decimals(&base, dir) else {
        eprintln!("[ribbon] calc.decimals: 不支持的格式码 {:?},忽略", cur);
        return;
    };
    if next == base && cur.is_none() {
        return; // 常规下再减少:无变化
    }
    eprintln!("[ribbon] calc.decimals {:?} -> {}", cur, next);
    apply_num_code(ctx, Some(&next));
}

/// 回显数字格式:无字段 → 常规;命中预设 → 预设名;其余 → 自定义。
fn apply_num_format_state(app: &super::AppWindow, code: Option<&str>) {
    let name = match code {
        None | Some("") | Some("General") => "常规",
        Some(c) => NUM_FORMATS
            .iter()
            .find(|(_, k)| *k == Some(c))
            .map(|(n, _)| *n)
            .unwrap_or("自定义"),
    };
    app.set_calc_num_format_text(name.into());
}

/// 回显字体名/字号到功能区触发器。无 font_family 或为 Carlito 时显示默认名
/// (渲染层默认即 Carlito);无字号显示 11。
fn apply_font_state(app: &super::AppWindow, family: Option<String>, size: Option<f64>) {
    let name = match family.as_deref() {
        None | Some("") | Some("Carlito") => "Carlito (默认)".to_string(),
        Some(n) => n.to_string(),
    };
    let size_text = match size {
        Some(v) if v.fract() == 0.0 => format!("{}", v as i64),
        Some(v) => format!("{}", v),
        None => "11".to_string(),
    };
    app.set_calc_font_name(name.into());
    app.set_calc_font_size_text(size_text.into());
}

/// 字体名下拉列表:首次同步时从 roe 取一次(list_available_fonts 只读、与文档无关,
/// 但 query_script 要 Calc 句柄,故挂在 sync_state_* 入口)。已加载(非空)则直接返回;失败则保持空并在下次同步重试。
fn load_font_list(app: &super::AppWindow, sess: &mut doc_core::TileCacheRust) {
    use slint::Model;
    if app.get_font_menu_list().row_count() > 0 {
        return;
    }
    let raw = sess.query_script("list_available_fonts()");
    let Some(names) = super::calc_data::parse_row_texts(&raw) else {
        eprintln!("[ribbon] list_available_fonts 解析失败: {}", raw);
        return;
    };
    if names.is_empty() {
        return;
    }
    let items: Vec<crate::FontItem> = names
        .into_iter()
        .map(|n| crate::FontItem {
            label: if n == "Carlito" { "Carlito (默认)".into() } else { n.clone().into() },
            value: n.into(),
        })
        .collect();
    app.set_font_menu_list(slint::ModelRc::new(slint::VecModel::from(items)));
}

/// 以调用方给定的单元格坐标同步按下态,不经过 slint 属性回读。
/// 导航/点击已经知道新选中格的确切位置,直接传入最可靠。
pub fn sync_state_at(app: &super::AppWindow, sess: &mut doc_core::TileCacheRust, sheet: u32, row: u32, col: u32) {
    load_font_list(app, sess);
    let fmt = sess.calc_get_cell_format(sheet, row, col);
    apply_font_state(app, fmt.get("font_family").and_then(|v| v.as_str()).map(String::from), fmt.get("font_size").and_then(|v| v.as_f64()));
    apply_num_format_state(app, fmt.get("num_format").and_then(|v| v.as_str()));
    let flag = |k: &str| fmt.get(k).and_then(|b| b.as_bool()).unwrap_or(false);
    app.set_calc_bold_active(flag("bold"));
    app.set_calc_italic_active(flag("italic"));
    app.set_calc_underline_active(flag("underline"));
    app.set_calc_wrap_active(flag("wrap_text"));
    let text = |k: &str| fmt.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    app.set_calc_align_h(text("align_h").into());
    app.set_calc_align_v(text("align_v").into());
}

/// 已持有 session 可变借用时使用(避免 RefCell 重复借用)。
pub fn sync_state_inner(app: &super::AppWindow, sess: &mut doc_core::TileCacheRust, sheet: u32) {
    load_font_list(app, sess);
    let Some((r1, c1, _, _)) = read_selection(app) else {
        app.set_calc_bold_active(false);
        app.set_calc_italic_active(false);
        app.set_calc_underline_active(false);
        app.set_calc_wrap_active(false);
        app.set_calc_align_h("".into());
        app.set_calc_align_v("".into());
        apply_font_state(app, None, None);
        apply_num_format_state(app, None);
        return;
    };
    let fmt = sess.calc_get_cell_format(sheet, r1, c1);
    apply_font_state(app, fmt.get("font_family").and_then(|v| v.as_str()).map(String::from), fmt.get("font_size").and_then(|v| v.as_f64()));
    apply_num_format_state(app, fmt.get("num_format").and_then(|v| v.as_str()));
    let flag = |k: &str| fmt.get(k).and_then(|b| b.as_bool()).unwrap_or(false);
    app.set_calc_bold_active(flag("bold"));
    app.set_calc_italic_active(flag("italic"));
    app.set_calc_underline_active(flag("underline"));
    app.set_calc_wrap_active(flag("wrap_text"));
    let text = |k: &str| fmt.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    app.set_calc_align_h(text("align_h").into());
    app.set_calc_align_v(text("align_v").into());
}

/// 结构/格式动作(合并、换行、插入删除行列)之后:
/// 刷新合并区缓存,并按当前选区重算高亮框(行高/列宽/合并区都可能变了)。
/// 调用方已持有 state 的可变借用,故把缓存暂时取出再放回。
pub(super) fn reflow_selection(
    app: &super::AppWindow,
    state: &mut super::AppState,
    sheet: u32,
) {
    let mut merges = std::mem::take(&mut state.calc_merges);
    if let Some(sess) = state.session.as_mut() {
        crate::calc_selection::refresh_merges(&mut merges, sess, sheet);
        crate::calc_selection::reapply(app, sess, &merges, sheet);
    }
    state.calc_merges = merges;
}

/// 重新渲染 Calc 视口。写法与 on_formula_committed 中的重绘一致。
pub fn refresh_view(app: &super::AppWindow, sess: &mut doc_core::TileCacheRust) {
    let scroll_x = app.get_calc_scroll_x();
    let scroll_y = app.get_calc_scroll_y();
    let vw = app.get_calc_view_width();
    let vh = app.get_calc_view_height();
    if vw <= 0 || vh <= 0 {
        return;
    }
    let img = sess.render_sheet_viewport(scroll_x, scroll_y, vw as u32, vh as u32);
    // render_sheet_viewport 返回物理 px（zoom=200，即逻辑 px * 2）
    let (phys_w, phys_h) = doc_core::calc_phys_size(vw as u32, vh as u32);
    if !img.is_empty() && img.len() == (phys_w * phys_h * 4) as usize {
        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&img, phys_w, phys_h);
        app.set_calc_render_scroll_x(scroll_x);
        app.set_calc_render_scroll_y(scroll_y);
        app.set_calc_viewport_image(slint::Image::from_rgba8(buf));
        slint::ComponentHandle::window(app).request_redraw(); // transform 层不会自动重绘
    }
}

/// AiPanel 脚本入口:在 UI 线程执行 Rhai 脚本并全套刷新。
/// 返回 (是否成功, 展示文本)。失败时内核已回滚文档。
pub fn run_ai_script(ctx: &RibbonCtx, script: &str) -> (bool, String) {
    let Some(app) = ctx.app_weak.upgrade() else { return (false, "窗口已关闭".into()) };
    let sheet = ctx.current_sheet_idx.get();
    let (r, c) = read_selection(&app).map(|(r1, c1, _, _)| (r1, c1)).unwrap_or((0, 0));
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return (false, "没有打开的文档".into()) };
    let names_before = sess.get_all_sheet_names();
    let res = sess.execute_script(script);
    eprintln!("[ai] run_script 返回: {}", res);
    if !res.contains("\"ok\":true") {
        return (false, res);
    }
    // AI 脚本可能新增/删除/改名 sheet:同步标签栏;sheet 变多时切到最后一个(新建的)
    let names = sess.get_all_sheet_names();
    let mut sheet = sheet;
    if names != names_before {
        let model: Vec<slint::SharedString> = names.iter().map(|x| x.as_str().into()).collect();
        app.set_calc_sheet_names(slint::ModelRc::new(slint::VecModel::from(model)));
        let new_idx = if names.len() > names_before.len() {
            names.len() - 1
        } else {
            (sheet as usize).min(names.len().saturating_sub(1))
        };
        sheet = new_idx as u32;
        ctx.current_sheet_idx.set(sheet);
        sess.set_current_sheet(new_idx as i32);
        if let Some(name) = names.get(new_idx) {
            app.set_calc_active_sheet(name.as_str().into());
        }
    }
    refresh_view(&app, sess);
    refresh_formula_bar(&app, sess, sheet, r, c);
    sync_state_at(&app, sess, sheet, r, c);
    reflow_selection(&app, &mut s, sheet);
    // 图表缓存/点选框/侧边栏:脚本可能增删改了图表
    crate::refresh_float_objs(&app, &mut s, sheet);
    (true, res)
}
