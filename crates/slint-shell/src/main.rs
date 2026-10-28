slint::include_modules!();

mod calc_selection;
mod calc_outline;
mod geom;
mod shape_catalog;
mod chart_registry;
mod calc_chart;
mod calc_fill;
mod ribbon;
use i_slint_backend_winit::WinitWindowAccessor;

const PT_TO_PX: f32 = 96.0 / 72.0;

/// 将 0-based 列索引转为 Excel 字母列名（0->A, 25->Z, 26->AA, ...）
fn col_index_to_letter(mut idx: u32) -> String {
    let mut result = String::new();
    loop {
        result.insert(0, (b'A' + (idx % 26) as u8) as char);
        if idx < 26 { break; }
        idx = idx / 26 - 1;
    }
    result
}

/// 将字符串数组转为JSON字符串数组字面量（处理引号/反斜杠转义），用于拼接PivotConfig JSON
fn json_string_array(items: &[String]) -> String {
    let escaped: Vec<String> = items.iter().map(|s| {
        let e = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{}\"", e)
    }).collect();
    format!("[{}]", escaped.join(","))
}

/// v227: 公式预览 → Slint 图片。空输入返回 Err("")
fn formula_preview_image(latex: &str, font_pt: f64) -> Result<slint::Image, String> {
    let (rgba, w, h) = doc_core::TileCacheRust::formula_preview_rgba(latex, font_pt, 2.0)?;
    if (w as usize) * (h as usize) * 4 != rgba.len() {
        return Err("预览尺寸不一致".to_string());
    }
    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w, h);
    Ok(slint::Image::from_rgba8(buf))
}

/// 把row/col/value/filter四个字段状态同步到Slint的pivot-*-fields属性（ModelRc）
fn sync_pivot_fields_to_ui(
    app: &AppWindow,
    row_fields: &Rc<RefCell<Vec<String>>>,
    col_fields: &Rc<RefCell<Vec<String>>>,
    value_fields: &Rc<RefCell<Vec<String>>>,
    filter_fields: &Rc<RefCell<Vec<String>>>,
) {
    let to_model = |v: &Vec<String>| -> slint::ModelRc<slint::SharedString> {
        let items: Vec<slint::SharedString> = v.iter().map(|s| s.as_str().into()).collect();
        slint::ModelRc::new(slint::VecModel::from(items))
    };
    app.set_pivot_row_fields(to_model(&row_fields.borrow()));
    app.set_pivot_col_fields(to_model(&col_fields.borrow()));
    app.set_pivot_value_fields(to_model(&value_fields.borrow()));
    app.set_pivot_filter_fields(to_model(&filter_fields.borrow()));

    // 按 pivot_available_fields 顺序，逐个判断该字段是否已存在于任意一个区域，生成等长的勾选状态数组
    let available = app.get_pivot_available_fields();
    let rf = row_fields.borrow();
    let cf = col_fields.borrow();
    let vf = value_fields.borrow();
    let ff = filter_fields.borrow();
    let checked: Vec<bool> = (0..available.row_count())
        .map(|i| {
            let name = available.row_data(i).unwrap_or_default().to_string();
            rf.iter().any(|x| x == &name)
                || cf.iter().any(|x| x == &name)
                || vf.iter().any(|x| x == &name)
                || ff.iter().any(|x| x == &name)
        })
        .collect();
    app.set_pivot_field_checked(slint::ModelRc::new(slint::VecModel::from(checked)));
}
/// 参数提示：反推"光标（恒在末尾）当前处于哪个函数调用的第几个参数"。
/// 仅处理光标在文本末尾的情况（与现有函数名补全触发规则一致，不感知光标
/// 中间插入位置）。返回 (函数名大写, 参数索引0-based)。
///
/// 跳过双引号字符串内的括号/逗号；支持嵌套函数调用（只关心最内层未闭合的
/// 那一层）；若末尾是连续字母（用户还在打函数名），返回 None，让位给函数名
/// 补全弹窗（两者互斥显示）。
fn resolve_arg_hint_target(text: &str) -> Option<(String, usize)> {
    let rest = text.strip_prefix('=')?;

    if rest.chars().last().map(|c| c.is_ascii_alphabetic()).unwrap_or(false) {
        return None;
    }

    let chars: Vec<char> = rest.chars().collect();
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut arg_index: usize = 0;
    let mut open_paren_pos: Option<usize> = None;

    let mut i = chars.len();
    while i > 0 {
        i -= 1;
        let c = chars[i];

        if in_string {
            if c == '"' {
                in_string = false;
            }
            continue;
        }

        match c {
            '"' => { in_string = true; }
            ')' => { depth += 1; }
            '(' => {
                if depth == 0 {
                    open_paren_pos = Some(i);
                    break;
                } else {
                    depth -= 1;
                }
            }
            ',' => {
                if depth == 0 {
                    arg_index += 1;
                }
            }
            _ => {}
        }
    }

    let paren_idx = open_paren_pos?;

    let mut name_start = paren_idx;
    while name_start > 0 {
        let c = chars[name_start - 1];
        if c.is_ascii_alphanumeric() || c == '.' {
            name_start -= 1;
        } else {
            break;
        }
    }

    if name_start == paren_idx {
        return None;
    }

    let func_name: String = chars[name_start..paren_idx].iter().collect();
    if func_name.is_empty() {
        return None;
    }

    Some((func_name.to_uppercase(), arg_index))
}

/// 判断"末尾连续字母前缀"开始的位置，是否处于某个未闭合括号调用内部。
/// 用于阻止函数名补全在已经进入参数列表（左括号内）后继续触发——
/// 例如 "=VLOOKUP(A" 不应该把 "A" 当成新函数名前缀去匹配补全列表。
/// rest 是去掉开头 "=" 的文本；prefix_len 是末尾连续字母的长度。
fn is_inside_unclosed_call(rest: &str, prefix_len: usize) -> bool {
    let chars: Vec<char> = rest.chars().collect();
    let boundary = chars.len().saturating_sub(prefix_len);
    let mut depth: i32 = 0;
    let mut in_string = false;

    for &c in chars.iter().take(boundary) {
        if in_string {
            if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => { in_string = true; }
            '(' => { depth += 1; }
            ')' => { if depth > 0 { depth -= 1; } }
            _ => {}
        }
    }

    depth > 0
}

/// 单个参数的展示信息：标签（如"arg1"）+ 是否可选 + 是否变长（可重复）
struct ArgSpec {
    label: String,
    optional: bool,
    variadic: bool,
}

/// 把 FormulaFuncInfo.args 签名字符串（如
/// "VLOOKUP(arg1: any@scalar, arg2: any@range, arg3: number@scalar, arg4?: logical@scalar)"）
/// 解析成参数列表，不含函数名本身。
fn parse_args_signature(sig: &str) -> Vec<ArgSpec> {
    let inner = match (sig.find('('), sig.rfind(')')) {
        (Some(l), Some(r)) if r > l => &sig[l + 1..r],
        _ => return Vec::new(),
    };
    if inner.trim().is_empty() {
        return Vec::new();
    }

    inner
        .split(',')
        .map(|part| {
            let part = part.trim();
            let name_part = part.split(':').next().unwrap_or(part).trim();
            let variadic = name_part.contains("...");
            let optional = name_part.contains('?');
            let label = name_part.trim_end_matches("...").trim_end_matches('?').trim_end_matches("...").to_string();
            ArgSpec { label, optional, variadic }
        })
        .collect()
}

/// 生成参数提示的三段式展示文本：(前段, 当前高亮段, 后段)，供Slint端用三个
/// Text拼接实现"当前参数高亮，其余普通色"的效果（Slint不支持富文本混排）。
/// 变长参数一旦出现，后续所有溢出的arg_index都钳制高亮到这个变长参数上。
fn render_arg_hint_segments(func_name: &str, args: &[ArgSpec], arg_index: usize) -> (String, String, String) {
    let mut effective_index = arg_index;
    if let Some(last) = args.last() {
        if last.variadic && arg_index >= args.len() {
            effective_index = args.len() - 1;
        }
    }

    let mut before_parts: Vec<String> = Vec::new();
    let mut highlight = String::new();
    let mut after_parts: Vec<String> = Vec::new();

    for (i, a) in args.iter().enumerate() {
        let mut s = a.label.clone();
        if a.variadic { s.push_str("..."); }
        if a.optional { s = format!("[{}]", s); }
        match i.cmp(&effective_index) {
            std::cmp::Ordering::Less => before_parts.push(s),
            std::cmp::Ordering::Equal => highlight = s,
            std::cmp::Ordering::Greater => after_parts.push(s),
        }
    }

    let before = format!(
        "{}({}{}",
        func_name,
        before_parts.join(", "),
        if before_parts.is_empty() { "" } else { ", " }
    );
    let after = format!(
        "{}{})",
        if after_parts.is_empty() { "" } else { ", " },
        after_parts.join(", ")
    );

    (before, highlight, after)
}

use std::cell::RefCell;
use slint::Model;
use std::rc::Rc;
use std::collections::HashMap;

struct AppState {
    session: Option<Box<doc_core::TileCacheRust>>,
    writer_cursor_para: i64,
    writer_cursor_offset: i64,
    // v95新增：光标水平移动"记忆位置"（final文档五.2的ideal_x）。
    // None表示尚未建立/需要重新计算；连续按上下键时保持不变，
    // 任何非上下移动操作（左右移动/点击/输入/删除）都要重新计算并覆盖。
    // 绝不能用roe_cursor_move_line返回的actual_x覆盖这个值（会导致长短行
    // 切换时光标水平位置逐渐跑偏，final文档已明确指出这是常见实现错误）。
    writer_cursor_ideal_x: Option<f32>,
    // v95新增：选区锚点（拖拽/shift+方向键开始的固定端）。
    // 与 writer_cursor_para/offset（表示"焦点"，即选区的活动端）配合表示一个选区；
    // 锚点等于焦点时表示选区已折叠（无选中内容，等同于普通光标）。
    writer_sel_anchor_para: i64,
    writer_sel_anchor_offset: i64,
    // v112新增："编辑会话"模型，替代旧的writer_ime_pending_len相对状态。
    // Some((para, start_offset))表示当前有一轮进行中的IME组合会话，
    // start_offset是这轮会话开始时光标的绝对位置（不随后续输入变化）；
    // None表示当前没有进行中的会话，下次on_writer_ime_composed触发时
    // 需要用当前真实光标位置建立新会话。
    // 与旧方案的本质区别：区间起点是绝对的、固定的，不依赖"数字符数"
    // 这种会因为光标中途移动而脱节的相对状态（ROE_Progress_v112.md
    // 二.2记录的两个严重bug——跨段落串位、offset不同步——根源都是
    // 旧的相对状态设计）。
    writer_ime_session_start: Option<(i64, i64)>,
    // 配合上面的session_start，记录"上一次结算已经写入的文本字节长度"，
    // 用来计算这次要替换的区间终点：end = session_start.1 + prev_len。
    // 只用于计算终点，不是"要删多少"的相对依据——起点始终是绝对的
    // session_start，不会因为这个字段的值而累积误差。
    writer_ime_session_prev_len: u32,
    writer_cell_cursor: Option<(u32, i64, i64)>,
    // v316c：上一次上下移动的结果位置与 ideal_x，用于连续上下键 x 不漂移
    writer_cell_vert: Option<(u32, i64, i64, f32)>,
    writer_cell_sel_anchor: Option<(u32, i64, i64)>, // v318a: 单元格内选区锚点
    writer_cell_sel_focus: Option<(u32, i64, i64)>, // v318c: 选区焦点
    // 拖拽选区（本次新增）：是否正在拖拽中（鼠标左键按下且在writer点击区域内移动）。
    writer_dragging: bool,
    writer_drag_origin: Option<(f32, f32)>, // v232: 按下坐标，用于区分点击与拖选
    // AB测试节流开关：true=只在para/offset变化时才真正调用query_selection_rects，
    // false=每次moved都调用（基线，用于对比）。运行时可切换，见on_writer_cursor_key
    // 里的调试热键处理（如已接线）。默认先关闭节流，作为版本A基线跑起来。
    writer_drag_throttle_enabled: bool,
    // 节流用：记录上一次真正触发query_selection_rects调用时的位置，
    // 用于判断这次moved算出的位置是否"没变化，可跳过"。
    writer_drag_last_query_para: i64,
    writer_drag_last_query_offset: i64,
    // 埋点：本次拖拽（一次按下到松开）内，真正触发query_selection_rects调用的次数。
    // 每次writer_drag_start时清零，用于AB对比统计。
    writer_drag_query_count: u32,
    // v51 E步：每张slide的(y_pt, width_pt, height_pt)，翻页渲染时按impress_current_slide查询
    impress_slides: Vec<(f32, f32, f32)>,
    // ROE_Chart_API_v1（2026-07-07）：本次会话内插入的图表 id -> 最近一次已知的 (x_pt,y_pt,w_pt,h_pt)。
    // hit_test暂不支持浮动对象（P1未实现），前台自行维护这份映射做点击命中判断，
    // 每次 insert_chart / resize_chart / move_chart 成功后同步更新。
    chart_rects: HashMap<u64, (f32, f32, f32, f32)>,
    // 当前选中的图表（0=未选中，与ROE约定的哨兵值一致）
    chart_selected_id: u64,
    // 拖拽临时状态：拖拽开始时的鼠标坐标(逻辑px)和图表rect(pt)，用于计算delta后得到新rect
    chart_drag_start_mouse: (f32, f32),
    chart_drag_start_rect_pt: (f32, f32, f32, f32),
    chart_drag_mode: String,  // "" / "move" / "resize-nw" / "resize-n" / ... 同slint侧命名
    // Calc 浮动对象(图表/统计图,之后图片/图形/文本框)缓存与选中。选中记 (sheet,id)，切 sheet 时清空。
    float_store: calc_chart::ChartStore,
    float_sel: u64,
    float_sel_sheet: u32,
    // Agent模式：会话消息列表（v81 UI骨架 -> 本次假数据/echo闭环接线新增）
    agent_messages: Vec<AgentMessage>,
    // v159样式还原：批注缓存扩展为(author, date, text)三元组，date用于
    // 气泡头部展示（roe内核draw_comment_bubble_placeholder同样在头部展示
    // 作者名+日期），文档打开时从 get_comment_threads_json() 填充一次。
    comment_threads: HashMap<i64, (String, String, String, bool, Vec<(String, String, String)>)>,
    reply_ids: HashMap<i64, Vec<i32>>,
    // Calc 当前 sheet 的合并区缓存 (r1,c1,r2,c2)。click/nav/drag 高频使用,
    // 不在每次事件里查脚本;打开文档/切 sheet/合并/结构编辑/撤销重做后刷新。
    calc_merges: Vec<calc_selection::Merge>,
    // 分类汇总折叠状态(v2方案:精确翻转,不整段无差别覆盖row_hidden)。
    // 会话内内存态,不持久化;文档关闭/重开后清空(继承v1"刷新后折叠状态会丢失"的已知限制)。
    // 详见 calc_outline.rs 与 roe-ext_subtotal_collapse_design_v2.md §2.3。
    calc_collapse: calc_outline::CollapseTracker,
    // 数据页"删除重复值"对话框的瞬时状态(打开时创建,确认/取消时清除)。
    dedup_dialog: Option<ribbon::calc_data::DedupDialogState>,
    subtotal_dialog: Option<ribbon::calc_data::SubtotalDialogState>,
    text_split_dialog: Option<ribbon::calc_data::TextSplitDialogState>,
    dv_dialog: Option<ribbon::calc_data::DataValidationDialogState>,
    cf_hl_dialog: Option<ribbon::calc_cf::CfHlDialogState>,
    spark_dialog: Option<ribbon::calc_sparkline::SparkDialogState>,
    stat_dialog: Option<ribbon::calc_stat::StatDialogState>,
    cf_icon_dialog: Option<ribbon::calc_cf::CfIconDialogState>,
    filter_dialog: Option<ribbon::calc_data::FilterDialogState>,
}

impl AppState {
    fn new() -> Self {
        AppState {
            session: None, writer_cursor_para: 0, writer_cursor_offset: 0,
            writer_cursor_ideal_x: None, writer_sel_anchor_para: 0, writer_sel_anchor_offset: 0,
            writer_ime_session_start: None, writer_ime_session_prev_len: 0, writer_cell_cursor: None, writer_cell_vert: None, writer_cell_sel_anchor: None, writer_cell_sel_focus: None,
            writer_dragging: false, writer_drag_origin: None, writer_drag_throttle_enabled: false,
            writer_drag_last_query_para: -1, writer_drag_last_query_offset: -1,
            writer_drag_query_count: 0,
            impress_slides: Vec::new(),
            chart_rects: HashMap::new(), chart_selected_id: 0,
            chart_drag_start_mouse: (0.0, 0.0), chart_drag_start_rect_pt: (0.0, 0.0, 0.0, 0.0), chart_drag_mode: String::new(),
            float_store: calc_chart::ChartStore::new(), float_sel: 0, float_sel_sheet: 0,
            agent_messages: Vec::new(),
            comment_threads: HashMap::new(),
            reply_ids: HashMap::new(),
            calc_merges: Vec::new(),
            calc_collapse: calc_outline::CollapseTracker::new(),
            dedup_dialog: None,
            subtotal_dialog: None,
            text_split_dialog: None,
            dv_dialog: None,
            cf_hl_dialog: None,
            spark_dialog: None,
            stat_dialog: None,
            cf_icon_dialog: None,
            filter_dialog: None,
        }
    }
}



struct ObjDrag {
    mode: String,
    start: geom::R,      // 数据区 px
    m0: (f32, f32),
    last: geom::R,
    moved: bool,
    id: u64,
    sheet: u32,
    scroll: (f32, f32),  // 拖动开始时的滚动，拖动期间固定使用
}

/// 图表类型字符串 -> 侧边栏下标(与 chart_kind_index_to_str 互逆):
/// 0=Bar 1=HorizontalBar 2=Line 3=Pie 4=Scatter 5=Radar 6=Area 7=Doughnut
fn chart_kind_str_to_index(k: &str) -> i32 {
    match k {
        "Bar" => 0, "HorizontalBar" => 1, "Line" => 2, "Pie" => 3,
        "Scatter" => 4, "Radar" => 5, "Area" => 6, "Doughnut" => 7,
        _ => 2,
    }
}

/// 图表侧边栏由选中状态推导(不单独存"开/关"状态,所以不会跨 sheet/文档传染):
/// 选中普通图 -> 读 roe_get_chart_config 填面板属性并打开;
/// 统计图/无选中 -> 仅关闭 chart 面板(pivot/formula 面板不动)。
/// s.chart_selected_id 作为旧 chart-style-* 回调的镜像,只在这里写。
/// v300: Writer 内容高度变化后刷新画布高度与页数
fn writer_cell_refresh(app: &AppWindow, s: &mut AppState) {
    let Some((cid, cp, coff)) = s.writer_cell_cursor else { return };
    if let Some(sess) = s.session.as_mut() {
        let (cx, cy, _cw, ch) = sess.writer_query_cursor_rect_ex(1, cid, cp as u32, coff as u32);
        app.set_writer_cursor_x(cx as i32);
        app.set_writer_cursor_y(cy as i32);
        app.set_writer_cursor_h(ch as i32);
        app.set_writer_cursor_visible(true);
    }
    // v318d: 选区矩形跟随刷新；选区失效（光标已离开焦点）则清除
    {
        let cur = s.writer_cell_cursor;
        let valid = match (s.writer_cell_sel_anchor, s.writer_cell_sel_focus) {
            (Some(a), Some(f)) if Some(f) == cur && a != f => Some((a, f)),
            _ => None,
        };
        let mut items: Vec<SelectionRect> = Vec::new();
        match valid {
            Some(((acid, ap, ao), (fcid, fp, fo))) => {
                let rj = s.session.as_mut().and_then(|sess| {
                    if acid == fcid { sess.cell_selection_rects(acid, ap as u32, ao as u32, fp as u32, fo as u32) }
                    else { sess.cell_range_rects(acid, fcid) }
                });
                if let Some(json) = rj {
                    if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&json) {
                        for r in arr {
                            let rx = r.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                            let ry = r.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32 + 24.0;
                            let rw = r.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                            let rh = r.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                            items.push(SelectionRect { rx, ry, rw, rh });
                        }
                    }
                }
            }
            None => {
                s.writer_cell_sel_anchor = None;
                s.writer_cell_sel_focus = None;
            }
        }
        app.set_writer_selection_rects(slint::ModelRc::new(slint::VecModel::from(items)));
    }
    let scroll_y = app.get_writer_scroll_y_pt();
    let vw = app.get_writer_view_width() as f32;
    let vh = app.get_writer_view_height() as f32;
    let doc_w = app.get_writer_doc_width_pt();
    if vw > 0.0 && vh > 0.0 {
        let rgba = s.session.as_mut().map(|sess| sess.render_writer_viewport(scroll_y, doc_w, vh)).unwrap_or_default();
        if !rgba.is_empty() {
            let pw = (doc_w * doc_core::doc_bitmap_scale()).round() as u32;
            let ph = (vh * doc_core::doc_bitmap_scale()).round() as u32;
            let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, pw, ph);
            app.set_writer_viewport_image(slint::Image::from_rgba8(buf));
        }
    }
}

fn writer_refresh_canvas(app: &AppWindow, s: &mut AppState) {
    let size = s.session.as_mut().and_then(|sess| sess.get_writer_document_size());
    if let Some((doc_w, canvas_h, page_count)) = size {
        app.set_writer_doc_width_pt(doc_w);
        app.set_writer_canvas_height_pt(canvas_h);
        app.set_page_total(page_count);
    }
}

fn sync_chart_sidebar(app: &AppWindow, s: &mut AppState, sheet: u32) {
    let sel = if s.float_sel != 0
        && s.float_sel_sheet == sheet
        && s.float_store.get(sheet, s.float_sel).is_some()
    { s.float_sel } else { 0 };
    let plain = sel != 0 && s.float_store.is_plain(sheet, sel);
    let sel = if plain { 0 } else { sel };
    let is_stat = sel != 0 && s.float_store.is_stat(sheet, sel);
    // chart_selected_id 供 chart-style-* 回调取 id(统计图后端只认 title/legend/grid)
    s.chart_selected_id = sel;
    app.set_chart_is_stat(is_stat);
    let kind = app.get_sidebar_kind().to_string();
    if sel == 0 {
        if kind == "chart" { app.set_sidebar_kind("none".into()); }
        return;
    }
    if kind != "none" && kind != "chart" { return; }
    let json = match s.session.as_mut() { Some(sess) => sess.get_chart_config(sheet, sel), None => None };
    let Some(json) = json else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else { return };
    let gs = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let gb = |k: &str, d: bool| v.get(k).and_then(|x| x.as_bool()).unwrap_or(d);
    app.set_chart_title_text(gs("title").into());
    app.set_chart_show_legend(gb("show_legend", true));
    app.set_chart_show_grid(gb("show_grid", true));
    if is_stat {
        app.set_sidebar_kind("chart".into());
        return;
    }
    app.set_chart_kind_index(chart_kind_str_to_index(&gs("kind")));
    app.set_chart_x_axis_label(gs("x_axis_label").into());
    app.set_chart_y_axis_label(gs("y_axis_label").into());
    let hex = gs("border_color");
    let h = hex.trim().trim_start_matches('#');
    let brush = if h.len() == 6 && h.is_ascii() {
        let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0xcc);
        slint::Color::from_rgb_u8(p(0), p(2), p(4))
    } else {
        slint::Color::from_rgb_u8(0xcc, 0xcc, 0xcc)
    };
    app.set_chart_border_color(hex.clone().into());
    app.set_chart_border_color_brush(brush);
    app.set_chart_border_width(v.get("border_width").and_then(|x| x.as_f64()).unwrap_or(1.0) as f32);
    app.set_sidebar_kind("chart".into());
}

fn select_float_obj(app: &AppWindow, s: &mut AppState, sheet: u32, id: u64) {
    if let Some(r) = s.float_store.get(sheet, id) {
        s.float_sel = id;
        s.float_sel_sheet = sheet;
        app.set_calc_obj_x_pt(r.x);
        app.set_calc_obj_y_pt(r.y);
        app.set_calc_obj_w_pt(r.w);
        app.set_calc_obj_h_pt(r.h);
        app.set_calc_obj_rotatable(false);
        app.set_calc_obj_shape(s.float_store.is_shape(sheet, id));
        app.set_calc_obj_id(id as i32);
        sync_chart_sidebar(app, s, sheet);
    }
}

/// 从 roe 重新装载指定 sheet 的浮动对象缓存，并把选中对象的 pt 矩形同步到界面。
/// 选中对象已不存在（被删/被撤销）或 sheet 已切换时清空选中。
/// 按当前 float_store + 位图渲染时的 scroll 推送图表覆盖层(几何见 calc_chart::float_pic_clips)。
fn push_float_pics(app: &AppWindow, s: &AppState, sheet: u32) {
    let rects = crate::calc_chart::ordered_rects(&s.float_store, sheet);
    let vw = app.get_calc_view_width() as f32;
    let vh = app.get_calc_view_height() as f32;
    let clips = crate::calc_chart::float_pic_clips(
        &rects,
        (app.get_calc_render_scroll_x(), app.get_calc_render_scroll_y()),
        (app.get_calc_scroll_x(), app.get_calc_scroll_y()),
        vw, vh, doc_core::calc_bitmap_scale());
    let pics: Vec<FloatPic> = clips.into_iter().map(|c| FloatPic {
        x: c.x, y: c.y, w: c.w, h: c.h, cx: c.cx, cy: c.cy, cw: c.cw, ch: c.ch,
    }).collect();
    app.set_calc_float_pics(slint::ModelRc::new(slint::VecModel::from(pics)));
}

fn refresh_float_objs(app: &AppWindow, s: &mut AppState, sheet: u32) {
    let json = match s.session.as_mut() { Some(sess) => sess.list_objects(sheet).or_else(|| sess.list_charts(sheet)), None => None };
    let Some(json) = json else { return };
    if s.float_store.load_sheet_json(sheet, &json).is_none() { return; }
    if s.float_sel != 0 {
        let found = if s.float_sel_sheet == sheet { s.float_store.get(sheet, s.float_sel) } else { None };
        match found {
            Some(r) => {
                app.set_calc_obj_x_pt(r.x);
                app.set_calc_obj_y_pt(r.y);
                app.set_calc_obj_w_pt(r.w);
                app.set_calc_obj_h_pt(r.h);
            }
            None => {
                s.float_sel = 0;
                app.set_calc_obj_id(-1);
                sync_chart_sidebar(app, s, sheet);
            }
        }
    }
    push_float_pics(app, &*s, sheet);
}

mod ai_config;

/// 设置页语言下标 -> 翻译语言。0=简体中文(原文)；1=English；其余语言暂无译文，退回中文。
fn apply_ui_language(idx: i32) {
    let code = match idx { 1 => "en", 2 => "ja", 3 => "ko", 4 => "fr", 5 => "de", 6 => "es", 7 => "zh_TW", _ => "" };
    let _ = slint::select_bundled_translation(code);
}

fn main() -> Result<(), slint::PlatformError> {
    // ===== v67新增：window_attributes_hook 探针，先只打印日志验证hook本身能正常工作，不改任何属性 =====
    {
        let mut backend = i_slint_backend_winit::Backend::new().unwrap();
        backend.window_attributes_hook = Some(Box::new(|attributes| {
            eprintln!("[WIN-ATTR-HOOK] hook被调用了，当前resizable={:?}", attributes.resizable);
            attributes
        }));
        slint::platform::set_platform(Box::new(backend))
            .expect("set_platform失败，可能已经被别处初始化过");
    }

    let app = AppWindow::new()?;
    if let Ok(l) = std::env::var("ROE_LANG") { let _ = slint::select_bundled_translation(&l); }
    // 套用表格格式弹层的缩略图取色(60 项,下标与 tstyle:<idx> 一致)
    {
        let pv = doc_core::table_preview::table_style_previews();
        let col = |c: Option<[u8; 3]>| match c {
            Some(v) => slint::Color::from_argb_u8(255, v[0], v[1], v[2]),
            None => slint::Color::from_argb_u8(0, 0, 0, 0),
        };
        app.set_tstyle_hdr(slint::ModelRc::new(slint::VecModel::from(pv.iter().map(|p| col(p.header_bg)).collect::<Vec<_>>())));
        app.set_tstyle_a(slint::ModelRc::new(slint::VecModel::from(pv.iter().map(|p| col(p.stripe_a)).collect::<Vec<_>>())));
        app.set_tstyle_b(slint::ModelRc::new(slint::VecModel::from(pv.iter().map(|p| col(p.stripe_b)).collect::<Vec<_>>())));
        app.set_tstyle_line(slint::ModelRc::new(slint::VecModel::from(pv.iter().map(|p| col(p.line)).collect::<Vec<_>>())));
    }
    let state = Rc::new(RefCell::new(AppState::new()));
    let app_weak = app.as_weak();
    let current_path: Rc<RefCell<Option<std::path::PathBuf>>> = Rc::new(RefCell::new(None));

    // Agent 接线（v1，正式版）：AgentContextProvider 独立于 AppState 持有，
    // 不放进 Rc<RefCell<AppState>>。理由：AppState 是纯 UI 线程本地状态
    // （RefCell 天然不 Send），而 AgentContextProvider 内部是
    // Arc<RwLock<...>>，专门设计给 agent-bridge 后台 tokio task 跨线程
    // 读取，两者语义不同，不混在一起。详见
    // agent接线设计方案_v1_讨论收敛稿.md 五节。
    let agent_context = agent_bridge::AgentContextProvider::default();

    // Agent 接线（v1，正式版）：on_event 闭包不能捕获
    // Rc<RefCell<AppState>>（RefCell 天生不 Send，闭包整体要满足
    // Send + Sync 才能被 agent-bridge 后台 task 跨线程调用，即便闭包
    // 实际执行永远发生在 UI 线程也不行——这是 Rust 类型系统的硬约束，
    // 不是可以绕过的实现细节）。
    //
    // 架构决策（与用户 + Gemini 讨论一致）：不引入 Arc<Mutex<...>> 包一层
    // state 的折中方案，而是彻底放弃 state.agent_messages 这份 Rust 侧
    // 镜像，统一以 Slint 侧的 agent_messages Model 作为唯一事实来源。
    // 理由：Slint Model 机制本身就是为“UI 状态单一来源”设计的；这里
    // 加一层 Rust 侧同步镜像纯粹是冗余，且在事件驱动场景下容易产生两份
    // 状态不一致的风险，不新增同步锁反而是更干净的收敛，不是权宜之计。
    //
    // 因此：闭包只捕获 app_weak，全部读写走
    // app.get_agent_messages()/set_agent_messages()。
    let app_weak_for_runtime = app_weak.clone();
    let agent_runtime = agent_bridge::AgentRuntimeHandle::spawn_headless(
        move |event| {
            let app_weak = app_weak_for_runtime.clone();
            let Some(app) = app_weak.upgrade() else { return };
            let existing = app.get_agent_messages();
            let mut msgs: Vec<AgentMessage> =
                (0..existing.row_count()).filter_map(|i| existing.row_data(i)).collect();

            match event {
                agent_bridge::AgentUiEvent::TextDelta(text) => {
                    // 流式增量：如果最后一条已经是 assistant 消息，追加
                    // 到它上面（同一轮回复的连续文字增量），否则新开
                    //一条 assistant 消息。
                    if let Some(last) = msgs.last_mut() {
                        if last.kind == "assistant" {
                            let base = if last.raw.is_empty() { last.text.to_string() } else { last.raw.to_string() };
                            let raw = format!("{}{}", base, text);
                            last.text = agent_bridge::md_plain::strip_markdown(&raw).into();
                            last.raw = raw.into();
                            app.set_agent_messages(std::rc::Rc::new(slint::VecModel::from(msgs)).into());
                            return;
                        }
                    }
                    msgs.push(AgentMessage {
                        kind: "assistant".into(),
                        text: agent_bridge::md_plain::strip_markdown(&text).into(),
                        raw: text.into(),
                        script_code: "".into(),
                        script_status: "".into(),
                        thinking_seconds: 0.0,
                        expanded: false,
                        shadow_handle: "".into(),
                    });
                    app.set_agent_messages(std::rc::Rc::new(slint::VecModel::from(msgs)).into());
                }
                agent_bridge::AgentUiEvent::ThinkingStatus(is_thinking) => {
                    app.set_agent_sending(is_thinking);
                    // 思考态开始时插入一条 thinking 卡片；结束时不需要
                    // 移除它——后续的 assistant/script 消息会自然接在
                    // 后面，thinking 卡片本身作为历史记录保留（与初始
                    // 假数据块此前呈现的四种气泡语义一致）。
                    if is_thinking {
                        msgs.push(AgentMessage {
                            raw: "".into(),
                            kind: "thinking".into(),
                            text: "".into(),
                            script_code: "".into(),
                            script_status: "".into(),
                            thinking_seconds: 0.0,
                            expanded: false,
                            shadow_handle: "".into(),
                        });
                        app.set_agent_messages(std::rc::Rc::new(slint::VecModel::from(msgs)).into());
                    }
                }
                agent_bridge::AgentUiEvent::ToolExecutionPending { shadow_handle, script_preview } => {
                    msgs.push(AgentMessage {
                        raw: "".into(),
                        kind: "script".into(),
                        text: "AI 生成了一段修改脚本，请确认是否生效".into(),
                        script_code: script_preview.into(),
                        script_status: "pending".into(),
                        thinking_seconds: 0.0,
                        expanded: false,
                        shadow_handle: shadow_handle.into(),
                    });
                    app.set_agent_messages(std::rc::Rc::new(slint::VecModel::from(msgs)).into());
                }
                agent_bridge::AgentUiEvent::ExecutionSuccess(text) => {
                    // 把最近一条处于 confirming/discarding 中间态的
                    // script 消息，翻正为最终的 confirmed/discarded；
                    // 找不到匹配项（比如这是无关的系统提示，如
                    // "已打开文档" 之外的其它成功提示）时，退化为普通
                    // assistant 消息追加显示，不丢弃这条反馈。
                    if let Some(msg) = msgs.iter_mut().rev().find(|m| {
                        m.kind == "script" && (m.script_status == "confirming" || m.script_status == "discarding")
                    }) {
                        msg.script_status = if msg.script_status == "confirming" {
                            "confirmed".into()
                        } else {
                            "discarded".into()
                        };
                    } else {
                        msgs.push(AgentMessage {
                            raw: "".into(),
                            kind: "assistant".into(),
                            text: text.into(),
                            script_code: "".into(),
                            script_status: "".into(),
                            thinking_seconds: 0.0,
                            expanded: false,
                            shadow_handle: "".into(),
                        });
                    }
                    app.set_agent_messages(std::rc::Rc::new(slint::VecModel::from(msgs)).into());
                }
                agent_bridge::AgentUiEvent::FileSaved(path) => {
                    // 程序在保存真正成功时发的提示行(纯界面反馈,不进后台历史)
                    msgs.push(AgentMessage {
                        raw: "".into(),
                        kind: "notice".into(),
                        text: format!("已保存 {path}").into(),
                        script_code: "".into(),
                        script_status: "".into(),
                        thinking_seconds: 0.0,
                        expanded: false,
                        shadow_handle: "".into(),
                    });
                    app.set_agent_messages(std::rc::Rc::new(slint::VecModel::from(msgs)).into());
                }
                agent_bridge::AgentUiEvent::ConfigError(text) => {
                    ai_push(&app, 2, format!("⚠ {text}"));
                    app.set_ai_save_status(text.clone().into());
                }
                agent_bridge::AgentUiEvent::ExecutionError(text) => {
                    // 错误场景同样尝试翻正中间态卡片（这次翻回 pending，
                    // 允许用户重试），找不到则作为普通 assistant 消息
                    // 显示错误文案。
                    if let Some(msg) = msgs.iter_mut().rev().find(|m| {
                        m.kind == "script" && (m.script_status == "confirming" || m.script_status == "discarding")
                    }) {
                        msg.script_status = "pending".into();
                    }
                    msgs.push(AgentMessage {
                        raw: "".into(),
                        kind: "assistant".into(),
                        text: format!("⚠ {text}").into(),
                        script_code: "".into(),
                        script_status: "".into(),
                        thinking_seconds: 0.0,
                        expanded: false,
                        shadow_handle: "".into(),
                    });
                    app.set_agent_messages(std::rc::Rc::new(slint::VecModel::from(msgs)).into());
                }
            }
        },
        ai_config::AiConfig::load().to_agent_config(),
    );

    // ── 侧边栏 AI 直连:第二个 runtime(独立历史),事件写 ai_log ──
    // kind: 4=用户消息 5=AI 回复(流式追加);ConfigError 由 Agent 页 runtime 处理,此处忽略。
    let app_weak_for_panel = app_weak.clone();
    let panel_runtime = agent_bridge::AgentRuntimeHandle::spawn_with(
        move |event| {
            let Some(app) = app_weak_for_panel.upgrade() else { return };
            match event {
                agent_bridge::AgentUiEvent::TextDelta(t) => ai_append_reply(&app, &t),
                agent_bridge::AgentUiEvent::ThinkingStatus(b) => app.set_ai_sending(b),
                agent_bridge::AgentUiEvent::ExecutionSuccess(t) => ai_push(&app, 1, t),
                agent_bridge::AgentUiEvent::ExecutionError(t) => {
                    app.set_ai_sending(false);
                    ai_push(&app, 2, t);
                }
                agent_bridge::AgentUiEvent::ToolExecutionPending { .. }
                | agent_bridge::AgentUiEvent::ConfigError(_)
                | agent_bridge::AgentUiEvent::FileSaved(_) => {}
            }
        },
        agent_context.clone(),
        ai_config::AiConfig::load().to_agent_config(),
        std::sync::Arc::new(agent_bridge::UiExecutor::new()),
    );

    // ── 打开/新建文档后的公共收尾：安装 session、同步 Agent 上下文、初始化界面 ──
    // 打开文件与三个“新建”入口共用，避免 200 行界面初始化复制多份。
    let finish_open: Rc<dyn Fn(Box<doc_core::TileCacheRust>, u64)> = {
        let state_fo = state.clone();
        let app_weak_fo = app_weak.clone();
        let agent_context_fo = agent_context.clone();
        let current_path_fo = current_path.clone();
        let panel_runtime_fo = panel_runtime.clone();
        Rc::new(move |mut session: Box<doc_core::TileCacheRust>, handle: u64| {
            *current_path_fo.borrow_mut() = None;
            { let ext = match session.get_doc_type().to_string().as_str() { "calc" => "xlsx", "impress" => "pptx", _ => "docx" };
              if let Some(a) = app_weak_fo.upgrade() { a.set_doc_file_name(format!("未命名.{}", ext).into()); } }
            let state = state_fo.clone();
            let app_weak = app_weak_fo.clone();
            let agent_context = agent_context_fo.clone();
            let panel_runtime = panel_runtime_fo.clone();
            let doc_type = session.get_doc_type();
            let page_count = session.get_page_count().max(1);
            eprintln!("[open] doc_type={} page_count={}", doc_type, page_count);
            {
                let mut s = state.borrow_mut();
                s.session = Some(session);
                // 换文档:旧文档的折叠记录(key 为 (sheet,row))会串到新文档同位置,整体重置
                s.calc_collapse = calc_outline::CollapseTracker::new();
            }
            agent_context.set_active_doc(Some(agent_bridge::ActiveDocumentSnapshot {
                raw_handle: handle,
                doc_id: String::new(),
            }));
            if let Some(app) = app_weak.upgrade() {
                app.set_doc_type(doc_type.clone().into());
                // 打开文档时复位缩放,不沿用上一个文档的状态
                if &*doc_type == "calc" {
                    doc_core::set_calc_scale(1.0);
                    app.set_calc_scale(doc_core::calc_scale());
                }
                if &*doc_type == "impress" {
                    app.set_impress_user_zoomed(false);
                    app.set_impress_pan_x(0.0);
                    app.set_impress_pan_y(0.0);
                }
                app.set_sidebar_kind("none".into());
                app.set_pivot_result_sheet(-1);
                { let mut st = state.borrow_mut(); st.float_store.clear_all(); st.float_sel = 0; st.chart_selected_id = 0; }
                app.set_calc_obj_id(-1);
                app.set_show_start_page(false);
                app.set_status_message("".into());
                app.set_ai_script_result("".into());
                app.set_ai_script_ok(true);
                app.set_ai_panel_visible(false);
                app.set_ai_log(slint::ModelRc::new(slint::VecModel::<AiMsg>::default()));
                app.set_ai_sending(false);
                panel_runtime.send_action(agent_bridge::AgentAction::ResetHistory);
                app.set_page_total(page_count);
                app.set_page_current(1);

                if doc_type == "writer" {
                    let mut s = state.borrow_mut();
                    // v117新增：打开Writer文档时，重置光标/选区/IME会话状态到文档开头，
                    // 不再继承上一个文档残留的para/offset（此前bug：state是跨文档复用的
                    // 同一个struct，只有session字段被替换，光标相关字段从未被重置，导致
                    // 切换文档后光标位置/选区错误地"继承"自上一个文档）。同时主动将光标
                    // 定位到para=0 offset=0并显示，不再需要用户先点击一次才能看到光标。
                    s.writer_cursor_para = 0;
                    s.writer_cursor_offset = 0;
                    s.writer_cursor_ideal_x = None;
                    s.writer_sel_anchor_para = 0;
                    s.writer_sel_anchor_offset = 0;
                    s.writer_ime_session_start = None;
                    s.writer_ime_session_prev_len = 0;
                    if let Some(sess) = s.session.as_mut() {
                        // v51更新：改用canvas_height（含留白+页间距+所有页面），替代旧的单页height手工补偿
                        if let Some((doc_w, canvas_h, page_count)) = sess.get_writer_document_size() {
                            app.set_writer_doc_width_pt(doc_w);
                            app.set_writer_canvas_height_pt(canvas_h);
                            eprintln!("[WRITER-OPEN] doc_w={}pt canvas_height={}pt page_count={}", doc_w, canvas_h, page_count);
                        }
                        // v117新增：查询para=0 offset=0的屏幕坐标，让光标一打开就可见，
                        // 不用等待用户点击。模式抄自on_writer_drag_start（1470行附近）
                        // 已验证过的"查坐标+设置光标可见"逻辑，保持风格一致。
                        let (cx, cy, _cw, ch) = sess.writer_query_cursor_rect(0, 0);
                        app.set_writer_cursor_x(cx as i32);
                        app.set_writer_cursor_y(cy as i32);
                        app.set_writer_cursor_h(ch as i32);
                        app.set_writer_cursor_visible(true);
                        eprintln!("[WRITER-OPEN] 初始光标定位到 para=0 offset=0 -> x={:.1} y={:.1} h={:.1}", cx, cy, ch);
                    }
                    app.set_writer_selection_rects(slint::ModelRc::new(slint::VecModel::from(Vec::<SelectionRect>::new())));
                    app.set_writer_needs_render(true);
                    // v303: 换文档时清掉上一个文档遗留的对象选中框 / 画图模式 / 形状菜单
                    app.set_writer_obj_sel(-1);
                    app.set_writer_draw_kind("".into());
                    app.set_writer_shape_menu_open(false);
                    // v117新增：打开文档后主动让writer-ime-input获得焦点，与点击后的
                    // 行为保持一致（TouchArea.clicked里调用的是同一个方法），确保
                    // 打开文档后无需点击即可直接输入。
                    app.invoke_writer_ime_focus();
                    // v158第三节起点：文档打开后拉取一次全文档批注文本，缓存到
                    // state.comment_threads，供viewport渲染后拼装气泡时查author/text用。
                    // 只在文档打开时调用一次（v158 3.1第1点：这份数据不需要每次滚动都重新拿，
                    // 只在文档打开/批注变化时更新；本次没有编辑批注的接口，所以实际只有
                    // "文档打开后"这一个触发时机）。
                    // 修复记录：此处不能再新开 state.borrow_mut()——外层的 `s` 从上面
                    // `let mut s = state.borrow_mut();`（doc_type=="writer"分支开头）
                    // 起就一直持有借用，直到这个if块结束才释放，这里嵌套再borrow同一个
                    // RefCell会直接panic("RefCell already borrowed")。复用外层已有的 s。
                    if let Some(sess) = s.session.as_mut() {
                        let threads_json = sess.get_comment_threads_json();
                        let mut map = HashMap::new();
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&threads_json) {
                            if let Some(arr) = v.get("threads").and_then(|t| t.as_array()) {
                                for thread in arr {
                                    if let Some(root) = thread.get("root") {
                                        let id = root.get("id").and_then(|x| x.as_i64()).unwrap_or(-1);
                                        let author = root.get("author").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        let date = root.get("date").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        let text = root.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        let done = root.get("done").and_then(|x| x.as_bool()).unwrap_or(false);
                                        let mut replies_vec: Vec<(String, String, String)> = Vec::new();
                                        if let Some(replies_arr) = thread.get("replies").and_then(|r| r.as_array()) {
                                            for reply in replies_arr {
                                                let r_author = reply.get("author").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                                let r_date = reply.get("date").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                                let r_text = reply.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                                replies_vec.push((r_author, r_date, r_text));
                                            }
                                        }
                                        if id >= 0 {
                                            map.insert(id, (author, date, text, done, replies_vec));
                                        }
                                    }
                                }
                            }
                        }
                        eprintln!("[COMMENT-OPEN] 缓存批注讨论串 count={}", map.len());
                        s.comment_threads = map;
                    }
                } else if doc_type == "impress" {
                    // v51 E步：改为整页翻页模式（硬切，无动画，参照OnlyOffice非编辑态浏览体感）。
                    // 不再使用旧的render_page（整页静态渲染）和Flickable连续滚动；
                    // 改为渲染impress_current_slide对应的单张slide，通过viewport接口按y偏移定位。
                    {
                        let mut s = state.borrow_mut();
                        if let Some(sess) = s.session.as_mut() {
                            if let Some((canvas_w, canvas_h, slide_count, slides)) = sess.get_impress_document_size() {
                                app.set_impress_canvas_width_pt(canvas_w);
                                app.set_impress_canvas_height_pt(canvas_h);
                                app.set_impress_slide_count(slide_count.max(1));
                                eprintln!("[IMPRESS-OPEN] canvas_w={}pt canvas_height={}pt slide_count={} slides={:?}", canvas_w, canvas_h, slide_count, slides);
                                s.impress_slides = slides;
                            } else {
                                eprintln!("[IMPRESS-OPEN] get_impress_document_size failed");
                                s.impress_slides.clear();
                            }
                        }
                        // s 在此花括号结束时释放borrow，避免下面refresh_all_impress_thumbnails内部
                        // 再次borrow_mut(state)时发生"RefCell already borrowed"panic。
                    }
                    app.set_impress_sel_id(-1);
                    app.set_impress_current_slide(1);
                    // 首张slide渲染由 changed impress-current-slide / changed width / height 触发（见app.slint）
                    app.set_impress_needs_render(true);
                    // 缩略图：文档打开时全量渲染一遍所有slide（见 refresh_all_impress_thumbnails 定义处的说明）
                    refresh_all_impress_thumbnails(&app, &state);
                } else if doc_type == "calc" {
                    // total_w/h 用于滚动条比例，初始渲染由 changed width/height 触发
                    {
                        let mut s = state.borrow_mut();
                        if let Some(sess) = s.session.as_mut() {
                            let total_w = sess.get_sheet_total_width(0);
                            let total_h = sess.get_sheet_total_height(0);
                            app.set_calc_sheet_total_width((total_w) * (96.0 / 72.0));
                            app.set_calc_sheet_total_height((total_h) * (96.0 / 72.0));
                            eprintln!("[CALC-INIT] total_w={:?} total_h={:?}", total_w, total_h);

                            // 初始化SheetTabBar：拉取所有sheet名称，默认激活第一个
                            let sheet_names = sess.get_all_sheet_names();
                            eprintln!("[CALC-INIT] sheet_names={:?}", sheet_names);
                            let names_model: Vec<slint::SharedString> = sheet_names.iter().map(|s| s.as_str().into()).collect();
                            app.set_calc_sheet_names(slint::ModelRc::new(slint::VecModel::from(names_model)));
                            if let Some(first) = sheet_names.first() {
                                app.set_calc_active_sheet(first.as_str().into());
                            }
                            // 合并区缓存:打开文档时读取第 0 张 sheet
                            let mut merges = Vec::new();
                            calc_selection::refresh_merges(&mut merges, sess, 0);
                            s.calc_merges = merges;
                            // 默认选中 A1(与手动点击同一链路),并让 cell-edit 取焦点
                            let mg = s.calc_merges.clone();
                            if let Some(sess) = s.session.as_mut() {
                                calc_selection::select_anchor(&app, sess, &mg, 0, 0, 0);
                            }
                            app.set_calc_focus_tick(app.get_calc_focus_tick() + 1);
                        }
                    }
                    // 局部补丁（遗留问题二）：calc-canvas 的首屏渲染完全依赖
                    // `changed width/height`（见 app.slint calc-canvas 那段），
                    // 但"切换到另一个xlsx文档"这个场景下 calc-canvas 的实际像素
                    // 尺寸大概率和切换前一样（同一窗口、同一侧栏状态），changed
                    // 不会fire，viewport就停留在旧文档画面，需手动滚动才刷新。
                    // writer/impress都有独立的 needs-render 标志位做双保险，
                    // calc目前没有——这是架构层面的缺口，先不动架构，这里直接
                    // 主动渲染一次兜底（若 calc_view_width/height 还是0，说明
                    // 是首次打开、changed事件本来就会补上，这里空转一次无害）。
                    {
                        let scale = app.window().scale_factor();
                        let vw = app.get_calc_view_width() as u32;
                        let vh = app.get_calc_view_height() as u32;
                        if vw > 0 && vh > 0 {
                            let mut s2 = state.borrow_mut();
                            if let Some(sess) = s2.session.as_mut() {
                                let buf = sess.render_sheet_viewport(0.0, 0.0, vw, vh);
                                eprintln!("[CALC-OPEN-RENDER] 主动刷新viewport vw={} vh={} buf.len={}", vw, vh, buf.len());
                                if !buf.is_empty() {
                                    let (phys_w, phys_h) = doc_core::calc_phys_size(vw, vh);
                                    let pixel_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                                        &buf, phys_w, phys_h,
                                    );
                                    app.set_calc_render_scroll_x(0.0);
                                    app.set_calc_render_scroll_y(0.0);
                                    app.set_calc_viewport_image(slint::Image::from_rgba8(pixel_buf));
                                    app.window().request_redraw(); // transform 层不会自动重绘
                                }
                            }
                        }
                    }
                }
            }
        })
    };

    // ── 文件打开 ──────────────────────────────────────────────────────────────
    let app_weak_open = app_weak.clone();
    // 页面图片缓存，key=0-based页码

    let finish_open_file = finish_open.clone();
    let path_open = current_path.clone();
    app.on_open_file_clicked(move || {
        let app_weak = app_weak_open.clone();
        let file = rfd::FileDialog::new()
            .add_filter("文档", &["docx", "xlsx", "pptx", "odt", "ods", "odp", "wai", "sai", "pai"])
            .add_filter("所有文件", &["*"])
            .pick_file();
        let Some(path) = file else { return; };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                if let Some(app) = app_weak.upgrade() {
                    app.set_status_message(format!("读取文件失败: {}", e).into());
                }
                return;
            }
        };
        let mut session = doc_core::new_tile_cache();
        let handle = session.open_document_from_bytes(&bytes, 99);
        if handle == 0 {
            if let Some(app) = app_weak.upgrade() {
                app.set_status_message(format!("打开文档失败: {:?}", path).into());
            }
            return;
        }
        finish_open_file(session, handle);
        *path_open.borrow_mut() = Some(path.clone());
        if let (Some(a), Some(n)) = (app_weak.upgrade(), path.file_name()) { a.set_doc_file_name(n.to_string_lossy().to_string().into()); }
    });

    // ── 保存(OOXML,统一另存为,不覆盖源文件) ──────────────────────────────
    let app_weak_save = app_weak.clone();
    let state_save = state.clone();
    let path_save = current_path.clone();
    app.on_save_clicked(move || {
        let Some(app) = app_weak_save.upgrade() else { return };
        let (hint, ext) = {
            let st = state_save.borrow();
            let Some(sess) = st.session.as_ref() else {
                app.set_status_message("没有打开的文档".into());
                return;
            };
            let t = sess.get_doc_type().to_string();
            match t.as_str() {
                "writer" => (4, "docx"),
                "calc" => (5, "xlsx"),
                "impress" => (6, "pptx"),
                _ => {
                    app.set_status_message("当前文档类型不支持保存".into());
                    return;
                }
            }
        };
        let stem = path_save
            .borrow()
            .as_ref()
            .and_then(|p| p.file_stem().map(|x| x.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "未命名".to_string());
        let default_name = format!("{}_roe.{}", stem, ext);
        let Some(mut out_path) = rfd::FileDialog::new()
            .set_file_name(default_name)
            .add_filter("文档", &[ext])
            .save_file() else { return };
        if out_path.extension().is_none() {
            out_path.set_extension(ext);
        }
        let bytes = {
            let mut st = state_save.borrow_mut();
            match st.session.as_mut() {
                Some(sess) => sess.save_document_to_ooxml_bytes(hint),
                None => Vec::new(),
            }
        };
        if bytes.is_empty() {
            eprintln!("[save] 序列化失败 hint={}", hint);
            app.set_status_message("保存失败:序列化出错".into());
            return;
        }
        let tmp = out_path.with_extension(format!("{}.tmp", ext));
        let res = std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &out_path));
        match res {
            Ok(_) => {
                eprintln!("[save] 已保存 {:?} ({} bytes)", out_path, bytes.len());
                app.set_status_message(format!("已保存: {}", out_path.display()).into());
                if let Some(n) = out_path.file_name() { app.set_doc_file_name(n.to_string_lossy().to_string().into()); }
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                eprintln!("[save] 写文件失败: {}", e);
                app.set_status_message(format!("保存失败: {}", e).into());
            }
        }
    });

    // ── 新建空白文档（Writer / Calc / Impress）──
    let finish_open_new_writer = finish_open.clone();
    let app_weak_new_writer = app_weak.clone();
    app.on_new_writer_clicked(move || {
        let mut session = doc_core::new_tile_cache();
        let handle = session.new_document(1);
        if handle == 0 {
            if let Some(app) = app_weak_new_writer.upgrade() {
                app.set_status_message("新建文档失败".into());
            }
            return;
        }
        finish_open_new_writer(session, handle);
    });

    let finish_open_new_calc = finish_open.clone();
    let app_weak_new_calc = app_weak.clone();
    app.on_new_calc_clicked(move || {
        let mut session = doc_core::new_tile_cache();
        let handle = session.new_document(2);
        if handle == 0 {
            if let Some(app) = app_weak_new_calc.upgrade() {
                app.set_status_message("新建文档失败".into());
            }
            return;
        }
        finish_open_new_calc(session, handle);
    });

    let finish_open_new_impress = finish_open.clone();
    let app_weak_new_impress = app_weak.clone();
    app.on_new_impress_clicked(move || {
        let mut session = doc_core::new_tile_cache();
        let handle = session.new_document(3);
        if handle == 0 {
            if let Some(app) = app_weak_new_impress.upgrade() {
                app.set_status_message("新建文档失败".into());
            }
            return;
        }
        finish_open_new_impress(session, handle);
    });


    // ── Calc 当前Sheet索引（多sheet共享）────────────────────────────────────────
    let current_sheet_idx = Rc::new(std::cell::Cell::new(0u32));
    // Ribbon 按钮统一入口（见 src/ribbon/mod.rs）
    ribbon::install(&app, ribbon::RibbonCtx {
        state: state.clone(),
        app_weak: app_weak.clone(),
        current_sheet_idx: current_sheet_idx.clone(),
    });
    // ── Calc 编辑节流Timer（80ms，避免每次击键都重渲染）─────────────────────────
    let _calc_edit_timer = Rc::new(slint::Timer::default());
    // ── Calc 透视表字段状态（两步弹窗共享：row/col/value/filter字段名列表）──────
    let pivot_row_fields = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let pivot_col_fields = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let pivot_value_fields = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let pivot_filter_fields = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    // ── 透视表第一步弹窗（range/new_sheet/location）暂存，供第二步确认时使用 ────
    let pivot_pending_source = Rc::new(std::cell::RefCell::new((String::new(), true, String::new())));
    // ── 透视表数据源sheet索引（弹窗打开时锁定，不随current_sheet_idx变化，避免生成后串源）──
    let pivot_source_sheet_idx = Rc::new(std::cell::Cell::new(0u32));
    // ── 上一次生成的透视表sheet索引，用于下次生成前先删除旧的，避免累加新sheet ──
    let pivot_last_result_sheet_idx: Rc<std::cell::Cell<Option<u32>>> = Rc::new(std::cell::Cell::new(None));

    // ── Calc sheet切换回调 ────────────────────────────────────────────────────
    let sheet_idx_switched = current_sheet_idx.clone();
    let app_weak_sheet = app_weak.clone();
    let state_sheet_switched = state.clone();
    app.on_calc_sheet_switched(move |idx| {
        let app = match app_weak_sheet.upgrade() { Some(a) => a, None => return };

        // 切sheet前若有未提交的编辑，先用旧sheet坐标提交，避免编辑内容漂移到新sheet
        // （新问题#1，见 roe-ext_progress_v217.md 遗留问题一）。
        // 注：真正的主修复在 app.slint 的 SheetTabBar.sheet-switched 回调里——
        // 那里在调用 calc-sheet-switched(idx) 之前，已经照抄"点击单元格外部"
        // 的逻辑做了 formula-committed + 清空 cell-edit.text，所以正常路径下
        // 走到这里时 calc_editing 应该已经是 false。这段留作防御性兜底（万一
        // 将来又新增一条"离开编辑态"的入口忘了在Slint端同步处理），理论上不会
        // 被触发，但触发了也不会出错（用的是切换前的坐标，逻辑是自洽的）。
        if app.get_calc_editing() {
            let old_sheet = sheet_idx_switched.get();
            let col = app.get_calc_selected_col();
            let row = app.get_calc_selected_row();
            let text = app.get_calc_display_value();
            if col >= 0 && row >= 0 {
                let ok = {
                    let mut s = state_sheet_switched.borrow_mut();
                    match s.session.as_mut() {
                        Some(sess) => sess.calc_set_cell(old_sheet, row as u32, col as u32, text.as_str()),
                        None => false,
                    }
                };
                eprintln!("[CALC-SHEET] flush pending edit before switch: sheet={} ({},{})={:?} ok={}", old_sheet, row, col, text.as_str(), ok);
            }
            app.set_calc_editing(false);
        }

        sheet_idx_switched.set(idx as u32);
        app.set_sidebar_kind("none".into());
        eprintln!("[CALC-SHEET] switched to sheet_idx={}", idx);
        let scale = app.window().scale_factor();
        let vw = (app.get_calc_view_width() as f32) as u32;
        let vh = (app.get_calc_view_height() as f32) as u32;
        if vw == 0 || vh == 0 { return; }
        let mut s = state_sheet_switched.borrow_mut();
        if let Some(sess) = s.session.as_mut() {
            sess.set_current_sheet(idx);
            // 滚动总宽高随 sheet 刷新(此前只在打开文件时按 sheet 0 设置一次)
            app.set_calc_sheet_total_width((sess.get_sheet_total_width(idx as u32)) * (96.0 / 72.0));
            app.set_calc_sheet_total_height((sess.get_sheet_total_height(idx as u32)) * (96.0 / 72.0));
            {
                let (r, c) = (app.get_calc_selected_row(), app.get_calc_selected_col());
                if r >= 0 && c >= 0 {
                    ribbon::calc_data::dv_list_refresh(&app, sess, idx as u32, r as u32, c as u32);
                } else {
                    app.set_dv_list_active(false);
                    app.set_dv_list_open(false);
                }
            }
            let buf = sess.render_sheet_viewport(0.0, 0.0, vw, vh);
            if !buf.is_empty() {
                let (phys_w, phys_h) = doc_core::calc_phys_size(vw, vh);
                let pixel_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &buf, phys_w, phys_h,
                );
                app.set_calc_render_scroll_x(0.0);
                app.set_calc_render_scroll_y(0.0);
                app.set_calc_viewport_image(slint::Image::from_rgba8(pixel_buf));
                app.window().request_redraw(); // transform 层不会自动重绘
            }
        }
        refresh_float_objs(&app, &mut s, idx as u32);
        // 切sheet后地址栏/选区/高亮框全部重置，避免残留上一个sheet的状态
        app.set_calc_address("A1".into());
        // 此前遗漏了 selected_row/col 的重置：地址栏文本回到"A1"了，但真正
        // 驱动选中格的这两个属性仍停留在旧sheet的坐标，导致后续在新sheet输入时
        // calc_display_value 用旧坐标刷新／编辑框残留旧文本，表现为切sheet后
        // 打字内容跟上次编辑内容拼接（如"123"变"123123"）。见遗留问题一追加排查。
        app.set_calc_selected_row(0);
        app.set_calc_selected_col(0);
        app.set_calc_range_x(0.0);
        app.set_calc_range_y(0.0);
        app.set_calc_range_w(0.0);
        app.set_calc_range_h(0.0);
        app.set_calc_sel_end_row(-1);
        app.set_calc_sel_end_col(-1);
        app.set_calc_sel_x_pt(0.0);
        app.set_calc_sel_y_pt(0.0);
        app.set_calc_sel_w_pt(0.0);
        app.set_calc_sel_h_pt(0.0);
        // 同步刷新公式栏显示值为新sheet A1格的真实内容（公式优先，否则取值），
        // 避免残留上一个sheet编辑到一半的文本（模式抄 on_calc_paste 的刷新逻辑）。
        // 复用上面已经borrow的 s（RefCell同一时刻只能有一个mut borrow，
        // 不能像最初patch那样另开一次borrow_mut，否则运行时panic）。
        if let Some(sess) = s.session.as_mut() {
            let new_sheet = idx as u32;
            let a1_formula = sess.calc_get_cell_formula(new_sheet, 0, 0);
            let a1_val = if !a1_formula.is_empty() {
                format!("={}", a1_formula)
            } else {
                sess.calc_get_cell(new_sheet, 0, 0)
            };
            app.set_calc_display_value(a1_val.into());
        }
        // 新 sheet 的合并区缓存
        let mut merges = std::mem::take(&mut s.calc_merges);
        if let Some(sess) = s.session.as_mut() {
            calc_selection::refresh_merges(&mut merges, sess, idx as u32);
        }
        s.calc_merges = merges;
    });

    // ── Calc 图表覆盖层刷新(位图更新后由 slint changed 触发;借用失败则跳过,下次位图更新会再算)
    {
        let state_fp = state.clone();
        let app_weak_fp = app_weak.clone();
        app.on_calc_float_pics_refresh(move || {
            let Some(app) = app_weak_fp.upgrade() else { return };
            let Ok(mut s) = state_fp.try_borrow_mut() else { return };
            let Some(sheet) = s.session.as_mut().map(|x| x.get_current_sheet().max(0) as u32) else { return };
            push_float_pics(&app, &s, sheet);
        });
    }

    // ── Calc 滚动重渲染（节流：只有位置变化才重渲染）────────────────────────────
    let state_calc = state.clone();
    let _app_weak_calc = app_weak.clone();
    let app_weak_calc = app_weak.clone();
    app.on_calc_render_viewport(move |scroll_x_pt, scroll_y_pt| {
        let app = match app_weak_calc.upgrade() { Some(a) => a, None => return };
        let scale = app.window().scale_factor();
        let vw = (app.get_calc_view_width() as f32) as u32;
        let vh = (app.get_calc_view_height() as f32) as u32;
        eprintln!("[CALC-VIEWPORT] called scroll=({:.1},{:.1}) vw={} vh={} scale={}", scroll_x_pt, scroll_y_pt, vw, vh, scale);
        if vw == 0 || vh == 0 { eprintln!("[CALC-VIEWPORT] skip: vw/vh=0"); return; }
        let mut s = state_calc.borrow_mut();
        if s.session.is_none() { eprintln!("[CALC-VIEWPORT] skip: no session"); return; }
        if let Some(sess) = s.session.as_mut() {
            let __t0 = std::time::Instant::now();
            let buf = sess.render_sheet_viewport(scroll_x_pt, scroll_y_pt, vw, vh);
            eprintln!("[CALC-VIEWPORT] render_sheet_viewport took {:?}", __t0.elapsed());
            eprintln!("[CALC-VIEWPORT] buf.len={}", buf.len());
            if !buf.is_empty() {
                let (phys_w, phys_h) = doc_core::calc_phys_size(vw, vh);
                eprintln!("[CALC-VIEWPORT] creating image {}x{}", phys_w, phys_h);
                let __t1 = std::time::Instant::now();
                let pixel_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &buf, phys_w, phys_h,
                );
                app.set_calc_render_scroll_x(scroll_x_pt);
                app.set_calc_render_scroll_y(scroll_y_pt);
                app.set_calc_viewport_image(slint::Image::from_rgba8(pixel_buf));
                app.window().request_redraw(); // transform 层不会自动重绘
                eprintln!("[CALC-VIEWPORT] pixel_buf+set_image took {:?}", __t1.elapsed());
            }
        }
    });


    // ── Calc 键盘导航 ────────────────────────────────────────────────────────
    let state_nav = state.clone();
    let app_weak_nav = app_weak.clone();
    let sheet_idx_nav = current_sheet_idx.clone();
    // ── Calc 公式栏实时编辑（仅维护编辑态：自动补全，不写数据模型）──────────────
    // 参照LOK(ScInputHandler::KeyInput vs EnterHandler)和OnlyOffice(CellEditor
    // open/close(saveValue))的标准设计：按键输入只维护编辑框自身文本/建议列表，
    // 不触碰单元格数据模型；只有明确的"提交"动作（回车on_formula_committed、
    // 点击候选词on_suggestion_picked）才会写入并触发重算。此前版本每字符都调
    // calc_set_cell，导致输入中的半成品公式（如"=vlookup("）被当成合法公式
    // 反复求值，是"公式栏值异常/#NAME?污染单元格"问题的根因，已改为不再实时写入。
    let state_edited = state.clone();
    let app_weak_edited = app_weak.clone();
    app.on_formula_edited(move |text| {
        eprintln!("[AUTOCOMPLETE-DEBUG] on_formula_edited fired, text={:?}", text.as_str());
        let app = match app_weak_edited.upgrade() { Some(a) => a, None => return };
        let col = app.get_calc_selected_col();
        let row = app.get_calc_selected_row();
        if col < 0 || row < 0 {
            eprintln!("[AUTOCOMPLETE-DEBUG] early return: col={} row={}", col, row);
            return;
        }

        // 公式函数自动补全：仅在"="开头且末尾是连续字母时触发（不感知光标
        // 中间插入位置）。数据源roe_get_formula_function_list已从34个硬编码
        // 常用函数升级为formualizer-eval全量376个内置函数（2026-07-10之后
        // ROE交付），前缀匹配对完整清单同样成立。
        {
            let suggestions: Vec<slint::SharedString> = if let Some(rest) = text.strip_prefix('=') {
                let prefix: String = rest.chars().rev()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .collect::<Vec<_>>().into_iter().rev().collect();
                if prefix.is_empty() || is_inside_unclosed_call(rest, prefix.len()) {
                    Vec::new()
                } else {
                    let prefix_upper = prefix.to_uppercase();
                    let mut s = state_edited.borrow_mut();
                    let result = if let Some(sess) = s.session.as_mut() {
                        match sess.get_formula_function_list() {
                            Some(json_str) => {
                                eprintln!("[AUTOCOMPLETE-DEBUG] got function list, len={}", json_str.len());
                                match serde_json::from_str::<serde_json::Value>(&json_str) {
                                    Ok(serde_json::Value::Array(arr)) => {
                                        arr.iter()
                                            .filter_map(|v| v.get("name").and_then(|n| n.as_str()))
                                            .filter(|name| name.starts_with(&prefix_upper))
                                            .map(|name| slint::SharedString::from(name))
                                            .collect()
                                    }
                                    _ => { eprintln!("[AUTOCOMPLETE-DEBUG] json parse failed or not array"); Vec::new() },
                                }
                            }
                            None => { eprintln!("[AUTOCOMPLETE-DEBUG] get_formula_function_list returned None"); Vec::new() },
                        }
                    } else {
                        Vec::new()
                    };
                    eprintln!("[AUTOCOMPLETE-DEBUG] prefix={:?} matched {} suggestions", prefix_upper, result.len());
                    result
                }
            } else {
                Vec::new()
            };
            app.set_suggestion_list(std::rc::Rc::new(slint::VecModel::from(suggestions)).into());

            // 参数提示：与函数名补全互斥（只有当本次未命中函数名补全时才计算，
            // 否则优先显示补全弹窗）。
            if app.get_suggestion_list().row_count() == 0 {
                if let Some((func_name, arg_index)) = resolve_arg_hint_target(text.as_str()) {
                    let mut s = state_edited.borrow_mut();
                    let sig = if let Some(sess) = s.session.as_mut() {
                        sess.get_formula_function_list()
                            .and_then(|json_str| serde_json::from_str::<serde_json::Value>(&json_str).ok())
                            .and_then(|v| v.as_array().cloned())
                            .and_then(|arr| {
                                arr.iter().find_map(|item| {
                                    let name = item.get("name")?.as_str()?;
                                    if name == func_name {
                                        item.get("args")?.as_str().map(|s| s.to_string())
                                    } else {
                                        None
                                    }
                                })
                            })
                    } else {
                        None
                    };
                    drop(s);
                    if let Some(sig) = sig {
                        let args = parse_args_signature(&sig);
                        let (before, highlight, after) = render_arg_hint_segments(&func_name, &args, arg_index);
                        app.set_arg_hint_before(before.into());
                        app.set_arg_hint_highlight(highlight.into());
                        app.set_arg_hint_after(after.into());
                    } else {
                        app.set_arg_hint_before("".into());
                        app.set_arg_hint_highlight("".into());
                        app.set_arg_hint_after("".into());
                    }
                } else {
                    app.set_arg_hint_before("".into());
                    app.set_arg_hint_highlight("".into());
                    app.set_arg_hint_after("".into());
                }
            } else {
                app.set_arg_hint_before("".into());
                app.set_arg_hint_highlight("".into());
                app.set_arg_hint_after("".into());
            }
        }
    });

    // ── Calc 公式栏取消（Esc恢复原值）────────────────────────────────────────
    let app_weak_cancelled = app_weak.clone();
    let state_cancelled = state.clone();
    let sheet_idx_cancelled = current_sheet_idx.clone();
    // ── Calc 函数库下拉:点分类按钮时按 category 过滤函数清单,灌入弹层 ──
    let state_fnlib = state.clone();
    let app_weak_fnlib = app_weak.clone();
    app.on_fn_menu_requested(move |cat| {
        let app = match app_weak_fnlib.upgrade() { Some(a) => a, None => return };
        let (title, cats): (&str, Vec<&str>) = match cat.as_str() {
            "financial" => ("财务", vec!["financial"]),
            "logical" => ("逻辑", vec!["logical", "logical_ext"]),
            "text" => ("文本", vec!["text"]),
            "datetime" => ("日期和时间", vec!["datetime"]),
            "lookup" => ("查找与引用", vec!["lookup", "reference_fns"]),
            "math" => ("数学和三角", vec!["math", "random"]),
            "other" => ("其他", vec!["stats", "engineering", "info", "database", "lambda"]),
            _ => return,
        };
        let json = {
            let mut s = state_fnlib.borrow_mut();
            let r = s.session.as_mut().and_then(|sess| sess.get_formula_function_list());
            r
        };
        let mut items: Vec<(String, String)> = Vec::new();
        if let Some(Ok(serde_json::Value::Array(arr))) =
            json.map(|j| serde_json::from_str::<serde_json::Value>(&j))
        {
            for v in arr {
                let c = v.get("category").and_then(|x| x.as_str()).unwrap_or("");
                if !cats.iter().any(|k| *k == c) { continue; }
                let n = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
                let d = v.get("desc").and_then(|x| x.as_str()).unwrap_or("");
                if n.is_empty() { continue; }
                items.push((n.to_string(), d.to_string()));
            }
        }
        items.sort();
        eprintln!("[FN-MENU] cat={} items={}", cat.as_str(), items.len());
        let names: Vec<slint::SharedString> = items.iter().map(|(n, _)| n.as_str().into()).collect();
        let descs: Vec<slint::SharedString> = items.iter().map(|(_, d)| d.as_str().into()).collect();
        app.set_fn_menu_title(format!("{}函数 ({})", title, items.len()).into());
        app.set_fn_names(slint::ModelRc::new(slint::VecModel::from(names)));
        app.set_fn_descs(slint::ModelRc::new(slint::VecModel::from(descs)));
    });

    app.on_formula_cancelled(move || {
        let app = match app_weak_cancelled.upgrade() { Some(a) => a, None => return };
        let col = app.get_calc_selected_col();
        let row = app.get_calc_selected_row();
        if col < 0 || row < 0 { return; }
        // 用 calc-edit-original 恢复单元格原值并重渲染
        let original = app.get_calc_edit_original();
        eprintln!("[FORMULA-CANCEL] restore to {:?}", original.as_str());
        let mut s = state_cancelled.borrow_mut();
        if let Some(sess) = s.session.as_mut() {
            sess.calc_set_cell(sheet_idx_cancelled.get(), row as u32, col as u32, original.as_str());
            let scroll_x = app.get_calc_scroll_x();
            let scroll_y = app.get_calc_scroll_y();
            let vw = app.get_calc_view_width();
            let vh = app.get_calc_view_height();
            let img = sess.render_sheet_viewport(scroll_x, scroll_y, vw as u32, vh as u32);
            if !img.is_empty() {
                let (phys_w, phys_h) = doc_core::calc_phys_size(vw as u32, vh as u32);
                if phys_w > 0 && phys_h > 0 && img.len() == (phys_w * phys_h * 4) as usize {
                    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&img, phys_w, phys_h);
                    app.set_calc_render_scroll_x(scroll_x);
                    app.set_calc_render_scroll_y(scroll_y);
                    app.set_calc_viewport_image(slint::Image::from_rgba8(buf));
                    app.window().request_redraw(); // transform 层不会自动重绘
                }
            }
        }
    });

    // ── Calc 公式函数自动补全：点击候选词替换输入框末尾前缀
    let app_weak_suggestion = app_weak.clone();
    app.on_suggestion_picked(move |name| {
        let app = match app_weak_suggestion.upgrade() { Some(a) => a, None => return };
        let current = app.get_calc_display_value();
        // 复用与自动补全触发逻辑一致的规则：仅替换"="之后末尾这段连续字母，
        // 前面的内容（比如已经打完的部分公式）原样保留。
        if let Some(rest) = current.strip_prefix('=') {
            let prefix_len = rest.chars().rev()
                .take_while(|c| c.is_ascii_alphabetic())
                .count();
            let keep = &rest[..rest.len() - prefix_len];
            let new_text = format!("={}{}", keep, name);
            app.set_calc_display_value(new_text.clone().into());
            app.invoke_formula_edited(new_text.into());
        }
        app.set_suggestion_list(std::rc::Rc::new(slint::VecModel::from(Vec::<slint::SharedString>::new())).into());
    });

    // ── Calc 数据验证 list 下拉:点选候选项写回当前格(不下移、不重选) ──
    let state_dvpick = state.clone();
    let app_weak_dvpick = app_weak.clone();
    let sheet_idx_dvpick = current_sheet_idx.clone();
    app.on_dv_list_picked(move |text| {
        let app = match app_weak_dvpick.upgrade() { Some(a) => a, None => return };
        let col = app.get_calc_selected_col();
        let row = app.get_calc_selected_row();
        if col < 0 || row < 0 { return; }
        let mut s = state_dvpick.borrow_mut();
        if let Some(sess) = s.session.as_mut() {
            let ok = sess.calc_set_cell(sheet_idx_dvpick.get(), row as u32, col as u32, text.as_str());
            eprintln!("[CALC-DVLIST] pick set_cell({},{},{:?}) ok={}", row, col, text.as_str(), ok);
            if ok {
                let scroll_x = app.get_calc_scroll_x();
                let scroll_y = app.get_calc_scroll_y();
                let vw = app.get_calc_view_width();
                let vh = app.get_calc_view_height();
                let img = sess.render_sheet_viewport(scroll_x, scroll_y, vw as u32, vh as u32);
                if !img.is_empty() {
                    let (phys_w, phys_h) = doc_core::calc_phys_size(vw as u32, vh as u32);
                    if phys_w > 0 && phys_h > 0 && img.len() == (phys_w * phys_h * 4) as usize {
                        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&img, phys_w, phys_h);
                        app.set_calc_render_scroll_x(scroll_x);
                        app.set_calc_render_scroll_y(scroll_y);
                        app.set_calc_viewport_image(slint::Image::from_rgba8(buf));
                        app.window().request_redraw(); // transform 层不会自动重绘
                    }
                }
                // 公式栏显示同步为刚写入的值
                app.set_calc_display_value(text.clone());
            }
        }
    });

    // ── Calc 公式栏提交（Enter确认写入单元格）─────────────────────────────────
    let state_formula = state.clone();
    let app_weak_formula = app_weak.clone();
    let sheet_idx_formula = current_sheet_idx.clone();
    app.on_formula_committed(move |text| {
        let app = match app_weak_formula.upgrade() { Some(a) => a, None => return };
        let col = app.get_calc_selected_col();
        let row = app.get_calc_selected_row();
        if col < 0 || row < 0 { return; }
        let mut s = state_formula.borrow_mut();
        if let Some(sess) = s.session.as_mut() {
            let ok = sess.calc_set_cell(sheet_idx_formula.get(), row as u32, col as u32, text.as_str());
            eprintln!("[CALC-FORMULA] set_cell({},{},{:?}) ok={}", row, col, text.as_str(), ok);
            if ok {
                // 重渲染viewport
                let scroll_x = app.get_calc_scroll_x();
                let scroll_y = app.get_calc_scroll_y();
                let vw = app.get_calc_view_width();
                let vh = app.get_calc_view_height();
                let img = sess.render_sheet_viewport(scroll_x, scroll_y, vw as u32, vh as u32);
                if !img.is_empty() {
                    // render_sheet_viewport返回物理px（zoom=200，即逻辑px*2）
                    let (phys_w, phys_h) = doc_core::calc_phys_size(vw as u32, vh as u32);
                    if phys_w > 0 && phys_h > 0 && img.len() == (phys_w * phys_h * 4) as usize {
                        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&img, phys_w, phys_h);
                        app.set_calc_render_scroll_x(scroll_x);
                        app.set_calc_render_scroll_y(scroll_y);
                        app.set_calc_viewport_image(slint::Image::from_rgba8(buf));
                        app.window().request_redraw(); // transform 层不会自动重绘
                    }
                }
                // Enter后下移一行，提前读取新单元格值更新公式栏
                let next_row = row + 1;
                let next_formula = sess.calc_get_cell_formula(sheet_idx_formula.get(), next_row as u32, col as u32);
                let next_val = if !next_formula.is_empty() {
                    format!("={}", next_formula)
                } else {
                    sess.calc_get_cell(sheet_idx_formula.get(), next_row as u32, col as u32)
                };
                let next_col_letter = col_to_letter(col as u32);
                let next_address = format!("{}{}", next_col_letter, next_row + 1);
                app.set_calc_display_value(next_val.into());
                app.set_calc_address(next_address.into());
            }
        }
    });

    app.on_calc_nav(move |dir| {
        let app = match app_weak_nav.upgrade() { Some(a) => a, None => return };
        let col = app.get_calc_selected_col();
        let row = app.get_calc_selected_row();
        if col < 0 || row < 0 { return; }
        let d = match dir.as_str() {
            "up" => calc_selection::Dir::Up,
            "down" => calc_selection::Dir::Down,
            "left" | "shift-tab" => calc_selection::Dir::Left,
            "right" | "tab" => calc_selection::Dir::Right,
            _ => return,
        };
        let mut s = state_nav.borrow_mut();
        // 合并区缓存很小,克隆一份避免与 session 的可变借用冲突
        let merges = s.calc_merges.clone();
        if let Some(sess) = s.session.as_mut() {
            let sheet_idx = sheet_idx_nav.get();
            let (nr, nc) = calc_selection::step(&merges, row as u32, col as u32, d);
            // 新格子出视口时自动滚到可见，必要时加长总长
            let (r1, c1, r2, c2) = calc_selection::cell_rect(&merges, nr, nc);
            let x0 = sess.get_col_offset_pt(sheet_idx, c1) * PT_TO_PX;
            let x1 = (sess.get_col_offset_pt(sheet_idx, c2) + sess.get_column_width_pt(sheet_idx, c2)) * PT_TO_PX;
            let y0 = sess.get_row_offset_pt(sheet_idx, r1) * PT_TO_PX;
            let y1 = (sess.get_row_offset_pt(sheet_idx, r2) + sess.get_row_height_pt(sheet_idx, r2)) * PT_TO_PX;
            let cw = (app.get_calc_view_width() as f32 - 38.0).max(50.0);
            let ch = (app.get_calc_view_height() as f32 - 26.0).max(50.0);
            let (old_sx, old_sy) = (app.get_calc_scroll_x(), app.get_calc_scroll_y());
            let (mut sx, mut sy) = (old_sx, old_sy);
            if x0 < sx { sx = x0; } else if x1 > sx + cw { sx = x1 - cw; }
            if y0 < sy { sy = y0; } else if y1 > sy + ch { sy = y1 - ch; }
            sx = sx.max(0.0);
            sy = sy.max(0.0);
            if sy + ch + 300.0 >= app.get_calc_sheet_total_height() {
                sess.extend_sheet_total(sheet_idx, true);
            }
            if sx + cw + 300.0 >= app.get_calc_sheet_total_width() {
                sess.extend_sheet_total(sheet_idx, false);
            }
            app.set_calc_sheet_total_width(sess.get_sheet_total_width(sheet_idx) * (96.0 / 72.0));
            app.set_calc_sheet_total_height(sess.get_sheet_total_height(sheet_idx) * (96.0 / 72.0));
            let scrolled = sx != old_sx || sy != old_sy;
            if scrolled {
                app.set_calc_scroll_x(sx);
                app.set_calc_scroll_y(sy);
            }
            calc_selection::select_anchor(&app, sess, &merges, sheet_idx, nr, nc);
            if scrolled {
                let vw = app.get_calc_view_width();
                let vh = app.get_calc_view_height();
                let img = sess.render_sheet_viewport(sx, sy, vw as u32, vh as u32);
                if !img.is_empty() {
                    let (phys_w, phys_h) = doc_core::calc_phys_size(vw as u32, vh as u32);
                    if phys_w > 0 && phys_h > 0 && img.len() == (phys_w * phys_h * 4) as usize {
                        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&img, phys_w, phys_h);
                        app.set_calc_render_scroll_x(sx);
                        app.set_calc_render_scroll_y(sy);
                        app.set_calc_viewport_image(slint::Image::from_rgba8(buf));
                        app.window().request_redraw();
                    }
                }
            }
        }
    });
    // ── Calc 单元格点击选中 ──────────────────────────────────────────────────
    let pv_row_click = pivot_row_fields.clone();
    let pv_col_click = pivot_col_fields.clone();
    let pv_val_click = pivot_value_fields.clone();
    let pv_flt_click = pivot_filter_fields.clone();
    let state_click = state.clone();
    let app_weak_click = app_weak.clone();
    let sheet_idx_click = current_sheet_idx.clone();
    app.on_calc_cell_click(move |mouse_x, mouse_y| {
        let app = match app_weak_click.upgrade() { Some(a) => a, None => return };
        let zoom_factor = 2.0_f32;
        let header_col_w = 60.0_f32 / zoom_factor;  // 逻辑px
        let header_row_h = 36.0_f32 / zoom_factor;  // 逻辑px
        let scroll_x = app.get_calc_scroll_x();
        let scroll_y = app.get_calc_scroll_y();
        // ── 标题区分流:左上角忽略;列标题选整列;行标题选整行 ──
        if mouse_x < header_col_w || mouse_y < header_row_h {
            let mut s = state_click.borrow_mut();
            let merges = s.calc_merges.clone();
            if let Some(sess) = s.session.as_mut() {
                let sheet_idx = sheet_idx_click.get();
                if mouse_x < header_col_w && mouse_y < header_row_h {
                    // 左上角交叉格:全选
                    calc_selection::select_all(&app, sess, &merges, sheet_idx);
                    eprintln!("[CALC-HEADER] select_all addr={}", app.get_calc_address());
                    return;
                }
                let (fill_rows, fill_cols) = calc_selection::fill_extent(&app, sess, sheet_idx);
                if mouse_y < header_row_h {
                    // 列标题:只用 x 换算列号
                    let x_pt = ((mouse_x - header_col_w) + scroll_x) / PT_TO_PX;
                    let col = sess.get_col_at_offset(sheet_idx, x_pt);
                    if col >= 0 {
                        let last_row = fill_rows;
                        calc_selection::select_column(&app, sess, &merges, sheet_idx, col as u32, last_row);
                        eprintln!("[CALC-HEADER] col={} last_row={} addr={}", col, last_row, app.get_calc_address());
                    }
                } else {
                    // 行标题:只用 y 换算行号
                    let y_pt = ((mouse_y - header_row_h) + scroll_y) / PT_TO_PX;
                    let row = sess.get_row_at_offset(sheet_idx, y_pt);
                    if row >= 0 {
                        let last_col = fill_cols;
                        calc_selection::select_row(&app, sess, &merges, sheet_idx, row as u32, last_col);
                        eprintln!("[CALC-HEADER] row={} last_col={} addr={}", row, last_col, app.get_calc_address());
                    }
                }
            }
            return;
        }
        let x_pt = ((mouse_x - header_col_w) + scroll_x) / PT_TO_PX;
        let y_pt = ((mouse_y - header_row_h) + scroll_y) / PT_TO_PX;

        // v226: 在透视结果 sheet 上点击表格 → 展开透视面板（已有别的面板时不抢）
        let is_pivot_click = sheet_idx_click.get() as i32 == app.get_pivot_result_sheet()
            || state_click.borrow().session.as_ref()
                .map_or(false, |sess| sess.is_pivot_sheet(sheet_idx_click.get() as u32));
        if is_pivot_click && app.get_sidebar_kind().as_str() == "none" {
            // 从 PivotMeta 还原侧边栏的可用字段与四个区域(外部 xlsx 打开的透视表也适用)
            let cfg_json = {
                let sb = state_click.borrow();
                sb.session.as_ref().and_then(|sess| sess.pivot_config_json(sheet_idx_click.get() as u32))
            };
            if let Some(v) = cfg_json.and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok()) {
                let arr = |k: &str| -> Vec<String> {
                    v.get(k).and_then(|x| x.as_array())
                        .map(|a| a.iter().filter_map(|t| t.as_str().map(|t| t.to_string())).collect())
                        .unwrap_or_default()
                };
                let fields: Vec<slint::SharedString> = arr("fields").iter().map(|f| f.as_str().into()).collect();
                app.set_pivot_available_fields(slint::ModelRc::new(slint::VecModel::from(fields)));
                *pv_row_click.borrow_mut() = arr("rows");
                *pv_col_click.borrow_mut() = arr("cols");
                *pv_val_click.borrow_mut() = arr("values");
                *pv_flt_click.borrow_mut() = arr("filters");
                sync_pivot_fields_to_ui(&app, &pv_row_click, &pv_col_click, &pv_val_click, &pv_flt_click);
            }
            app.set_sidebar_kind("pivot".into());
        }

        // ── 图表命中优先判断（v73新增）：覆盖整个calc-canvas的大TouchArea会无条件
        // 抢焦点+触发单元格选中，导致图表在选中态下也无法用鼠标再次选中它自己
        // （点击会穿透成"选中背后的单元格"）。这里先检查点击坐标是否落在任意已知
        // 图表矩形内，命中则只更新图表选中态并直接return，不再执行下面的单元格选中逻辑；
        // 未命中任何图表时才继续原逻辑，并顺带清空chart_selected_id（点击空白区域=取消图表选中）。
        {
            let mut s_chart = state_click.borrow_mut();
            if s_chart.chart_selected_id != 0 {
                s_chart.chart_selected_id = 0;
                drop(s_chart);
                if app.get_sidebar_kind().as_str() == "chart" { app.set_sidebar_kind("none".into()); }
            }
        }

        let mut s = state_click.borrow_mut();
        let merges = s.calc_merges.clone();
        if let Some(sess) = s.session.as_mut() {
            let sheet_idx = sheet_idx_click.get();
            let col = sess.get_col_at_offset(sheet_idx, x_pt);
            let row = sess.get_row_at_offset(sheet_idx, y_pt);
            if col >= 0 && row >= 0 {
                // ── 透视表字段按钮命中(页/行/列字段),先于普通表头筛选 ──────────
                if sess.is_pivot_sheet(sheet_idx) {
                    let pv_hit: Option<(f32, f32, String, u32, Vec<String>)> = sess
                        .pivot_buttons_json(sheet_idx)
                        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
                        .and_then(|v| {
                            for b in v.as_array()? {
                                let br = b.get("row")?.as_i64()? as i32;
                                let bc = b.get("col")?.as_i64()? as i32;
                                if br != row || bc != col { continue; }
                                let src = b.get("source_idx")?.as_i64()?;
                                if src < 0 { continue; }
                                let ch_px = sess.get_row_height_pt(sheet_idx, row as u32) * PT_TO_PX;
                                let cw_px = sess.get_column_width_pt(sheet_idx, col as u32) * PT_TO_PX;
                                let btn_margin = 1.0_f32;
                                let btn_size = (ch_px - 2.0).min(cw_px - 2.0).clamp(4.0, 16.0);
                                let cx_px = sess.get_col_offset_pt(sheet_idx, col as u32) * PT_TO_PX;
                                let cy_px = sess.get_row_offset_pt(sheet_idx, row as u32) * PT_TO_PX;
                                let bx = cx_px + (cw_px - btn_size - btn_margin) - scroll_x + header_col_w;
                                let by = cy_px + ((ch_px - btn_size) / 2.0) - scroll_y + header_row_h;
                                if mouse_x >= bx && mouse_x <= bx + btn_size
                                    && mouse_y >= by && mouse_y <= by + btn_size {
                                    let field = b.get("field_name")?.as_str()?.to_string();
                                    let selected: Vec<String> = b.get("selected")
                                        .and_then(|x| x.as_array())
                                        .map(|a| a.iter().filter_map(|t| t.as_str().map(|t| t.to_string())).collect())
                                        .unwrap_or_default();
                                    return Some((bx, by + btn_size, field, src as u32, selected));
                                }
                            }
                            None
                        });
                    if let Some((anchor_x, anchor_y, field, src, selected)) = pv_hit {
                        drop(s);
                        let ctx = crate::ribbon::RibbonCtx {
                            state: state_click.clone(),
                            app_weak: app_weak_click.clone(),
                            current_sheet_idx: sheet_idx_click.clone(),
                        };
                        app.set_filter_anchor_x(anchor_x);
                        app.set_filter_anchor_y(anchor_y);
                        crate::ribbon::calc_data::filter_open_pivot(&ctx, sheet_idx as u32, &field, src, &selected);
                        return;
                    }
                }

                // ── 表头筛选按钮命中检测(v214公式,正式接线) ──────────────────
                let af = crate::ribbon::calc_data::unwrap_json(
                    &sess.query_script(&format!("get_auto_filter({})", sheet_idx))
                ).filter(|v| v.is_object());
                let btn_hit = af.as_ref().and_then(|a| {
                    let header_row = a.get("header_row")?.as_u64()? as i32;
                    let header_col = a.get("header_col")?.as_u64()? as i32;
                    let end_col = a.get("end_col")?.as_u64()? as i32;
                    if row != header_row || col < header_col || col > end_col {
                        return None;
                    }
                    let ch_pt = sess.get_row_height_pt(sheet_idx, row as u32);
                    let cw_pt = sess.get_column_width_pt(sheet_idx, col as u32);
                    let ch_px = ch_pt * PT_TO_PX;
                    let cw_px = cw_pt * PT_TO_PX;
                    let btn_margin = 1.0_f32;
                    let btn_size = (ch_px - 2.0).min(cw_px - 2.0).clamp(4.0, 16.0);
                    let cell_x0_pt = sess.get_col_offset_pt(sheet_idx, col as u32);
                    let cell_y0_pt = sess.get_row_offset_pt(sheet_idx, row as u32);
                    let cx_px = cell_x0_pt * PT_TO_PX;
                    let cy_px = cell_y0_pt * PT_TO_PX;
                    let bx_local = cw_px - btn_size - btn_margin;
                    let byy_local = (ch_px - btn_size) / 2.0;
                    let btn_screen_x = cx_px + bx_local - scroll_x + header_col_w;
                    let btn_screen_y = cy_px + byy_local - scroll_y + header_row_h;
                    let hit = mouse_x >= btn_screen_x && mouse_x <= btn_screen_x + btn_size
                        && mouse_y >= btn_screen_y && mouse_y <= btn_screen_y + btn_size;
                    if hit {
                        Some((btn_screen_x, btn_screen_y + btn_size))
                    } else {
                        None
                    }
                });

                // ── 折叠图标命中检测(第一步:仅检测,不触发实际折叠/展开) ──────────────
                // 图标固定画在当前视口最左可见列(col_start)，贴该列右边界，与内核
                // draw_collapse_btn(roe c14da82，贴右边界)保持坐标公式一致。
                let x0_pt = scroll_x / PT_TO_PX;
                let col_start = sess.get_col_at_offset(sheet_idx, x0_pt).max(0) as u32;
                let collapse_hit = if col as u32 == col_start {
                    let cur_level_raw = sess.query_script(&format!("get_row_outline_level({}, {})", sheet_idx, row));
                    let next_level_raw = sess.query_script(&format!("get_row_outline_level({}, {})", sheet_idx, row + 1));
                    // 注意(教训同v229§1.1): unwrap_json语义是"value字段本身又是一段
                    // JSON字符串,需要二次解析",这里value直接是数字,不能走它,
                    // 要直接对整个返回值做一次serde_json::from_str。
                    let cur_level = serde_json::from_str::<serde_json::Value>(&cur_level_raw)
                        .ok()
                        .and_then(|v| v.get("value").and_then(|x| x.as_i64()));
                    let next_level = serde_json::from_str::<serde_json::Value>(&next_level_raw)
                        .ok()
                        .and_then(|v| v.get("value").and_then(|x| x.as_i64()));
                    match (cur_level, next_level) {
                        (Some(cur), Some(next)) if cur < next => {
                            let ch_pt = sess.get_row_height_pt(sheet_idx, row as u32);
                            let cw_pt = sess.get_column_width_pt(sheet_idx, col_start);
                            let ch_px = ch_pt * PT_TO_PX;
                            let cw_px = cw_pt * PT_TO_PX;
                            let btn_margin = 1.0_f32;
                            let btn_size = (ch_px - 2.0).min(cw_px - 2.0).clamp(4.0, 16.0);
                            let cell_x0_pt = sess.get_col_offset_pt(sheet_idx, col_start);
                            let cell_y0_pt = sess.get_row_offset_pt(sheet_idx, row as u32);
                            let cx_px = cell_x0_pt * PT_TO_PX;
                            let cy_px = cell_y0_pt * PT_TO_PX;
                            let bx_local = cw_px - btn_size - btn_margin;
                            let byy_local = (ch_px - btn_size) / 2.0;
                            let btn_screen_x = cx_px + bx_local - scroll_x + header_col_w;
                            let btn_screen_y = cy_px + byy_local - scroll_y + header_row_h;
                            mouse_x >= btn_screen_x && mouse_x <= btn_screen_x + btn_size
                                && mouse_y >= btn_screen_y && mouse_y <= btn_screen_y + btn_size
                        }
                        _ => false,
                    }
                } else {
                    false
                };
                eprintln!("[COLLAPSE-TRACE] 进入判断分支 col={} col_start={} collapse_hit={}", col, col_start, collapse_hit);
                if collapse_hit {
                    eprintln!("[COLLAPSE-TRACE] 确认命中,开始处理 row={}", row);
                    let sum_row = row as u32;
                    // 第一阶段：只用 sess，查完 scan_group_rows 需要的 level 数据
                    // 和折叠分支需要的当前隐藏状态，不碰 s.calc_collapse。
                    // 扫描上界:used range 行数-1(get_used_range 返回行数,同 fill_extent 用法)。
                    // 必须在 get_level 闭包借用 sess 之前取。
                    let (used_rows, _) = sess.get_used_range(sheet_idx);
                    let max_row = used_rows.saturating_sub(1);
                    let get_level = |r: u32| -> u8 {
                        let raw = sess.query_script(&format!("get_row_outline_level({}, {})", sheet_idx, r));
                        let lvl = serde_json::from_str::<serde_json::Value>(&raw)
                            .ok()
                            .and_then(|v| v.get("value").and_then(|x| x.as_i64()))
                            .unwrap_or(0) as u8;
                        eprintln!("[COLLAPSE-TRACE-SCAN] get_level({})={}", r, lvl);
                        lvl
                    };
                    eprintln!("[COLLAPSE-TRACE] 即将调用scan_group_rows, sum_row={}", sum_row);
                    // TODO: summary_below 暂写死 false(汇总在明细上方,与样本 subtotal-above.xlsx 的
                    // outlinePr summaryBelow="0" 一致)。内核目前未解析该属性(roe 仓库 grep 零命中),
                    // 待 roe 暴露 get_summary_below 后改为动态取值。
                    let group_range = calc_outline::scan_group_rows(get_level, sum_row, false, max_row);
                    eprintln!("[COLLAPSE-TRACE] scan_group_rows返回 group_range={:?}", group_range);
                    let hidden_now: std::collections::HashMap<u32, bool> = match group_range {
                        Some((start_row, end_row)) if end_row >= start_row => {
                            (start_row..=end_row).map(|r| {
                                let raw = sess.query_script(&format!("get_row_hidden({}, {})", sheet_idx, r));
                                let hidden = serde_json::from_str::<serde_json::Value>(&raw)
                                    .ok()
                                    .and_then(|v| v.get("value").and_then(|x| x.as_bool()))
                                    .unwrap_or(false);
                                (r, hidden)
                            }).collect()
                        }
                        _ => std::collections::HashMap::new(),
                    };

                    if let Some((start_row, end_row)) = group_range {
                        // 第二阶段：sess 这次借用到这里事实上已不再需要，但因为
                        // sess 是 if let 绑定、生命周期覆盖整个外层块，编译器
                        // 仍视为存活。用一对花括号包住"只碰s.calc_collapse"的
                        // 这部分，让它成为独立语句，不与sess同时出现在同一表达式里
                        // (借用检查器按NLL分析实际使用点，这里的关键是rows_to_change
                        // 计算过程中不再出现sess，之前报错正是因为闭包捕获了sess)。
                        let is_collapsed = s.calc_collapse.group_is_collapsed(sheet_idx, sum_row);
                        let rows_to_change: Vec<u32> = if is_collapsed {
                            // 当前折叠 → 展开：只翻转 tracker 记录过的行(精确翻转，
                            // 不误伤筛选隐藏的行；v229§四清单沿用v2设计文档§2.3)。
                            s.calc_collapse.expand_group(sheet_idx, sum_row, start_row, end_row)
                        } else {
                            // 当前展开 → 折叠：只隐藏"目前可见"的行(纯数据闭包,不捕获sess)。
                            let currently_hidden_fn = |r: u32| -> bool {
                                hidden_now.get(&r).copied().unwrap_or(false)
                            };
                            s.calc_collapse.collapse_group(sheet_idx, sum_row, start_row, end_row, currently_hidden_fn)
                        };

                        // 第三阶段：重新从 s.session 取 sess，做写操作+重绘。
                        if !rows_to_change.is_empty() {
                            if let Some(sess2) = s.session.as_mut() {
                                let hidden_val = if is_collapsed { "false" } else { "true" };
                                let stmts: String = rows_to_change.iter()
                                    .map(|r| format!("set_row_hidden({}, {}, {});", sheet_idx, r, hidden_val))
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                let result = sess2.execute_script(&stmts);
                                eprintln!("[COLLAPSE-TOGGLE] sum_row={} start_row={} end_row={} is_collapsed(before)={} rows_changed={:?} result={}",
                                    sum_row, start_row, end_row, is_collapsed, rows_to_change, result);

                                // execute_script不会自动触发viewport重绘，手动补一次(沿用insert_chart处的既有模式)。
                                // render_sheet_viewport 返回物理 px 缓冲(逻辑×2),建图必须用物理尺寸,
                                // 并校验缓冲长度(同"取消编辑"处的既有写法)。scroll 现取,不复用闭包外旧值。
                                let cur_sx = app.get_calc_scroll_x();
                                let cur_sy = app.get_calc_scroll_y();
                                let vw = app.get_calc_view_width() as u32;
                                let vh = app.get_calc_view_height() as u32;
                                if vw > 0 && vh > 0 {
                                    let buf = sess2.render_sheet_viewport(cur_sx, cur_sy, vw, vh);
                                    let (phys_w, phys_h) = doc_core::calc_phys_size(vw, vh);
                                    if !buf.is_empty() && buf.len() == (phys_w * phys_h * 4) as usize {
                                        let pixel_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&buf, phys_w, phys_h);
                                        app.set_calc_render_scroll_x(cur_sx);
                                        app.set_calc_render_scroll_y(cur_sy);
                                        app.set_calc_viewport_image(slint::Image::from_rgba8(pixel_buf));
                                        app.window().request_redraw(); // transform 层不会自动重绘
                                    }
                                }
                            }
                        }
                    } else {
                        eprintln!("[COLLAPSE-TOGGLE] sum_row={} scan_group_rows返回None(空分组或方向上无可折叠行,合法情况,跳过)", sum_row);
                    }
                    return;
                }

                if let Some((anchor_x, anchor_y)) = btn_hit {
                    drop(s);
                    let ctx = crate::ribbon::RibbonCtx {
                        state: state_click.clone(),
                        app_weak: app_weak_click.clone(),
                        current_sheet_idx: sheet_idx_click.clone(),
                    };
                    app.set_filter_anchor_x(anchor_x);
                    app.set_filter_anchor_y(anchor_y);
                    crate::ribbon::calc_data::filter_open(&ctx);
                    return;
                }

                calc_selection::select_anchor(&app, sess, &merges, sheet_idx, row as u32, col as u32);
                eprintln!(
                    "[CALC-CLICK] hit=({},{}) anchor=({},{}) addr={} highlight=({:.1},{:.1},{:.1},{:.1})",
                    row, col,
                    app.get_calc_selected_row(), app.get_calc_selected_col(),
                    app.get_calc_address(),
                    app.get_calc_highlight_x(), app.get_calc_highlight_y(),
                    app.get_calc_highlight_w(), app.get_calc_highlight_h(),
                );
            }
        }
    });

    // ── Calc 格式刷:鼠标抬起时执行 ──
    {
        let ctx = crate::ribbon::RibbonCtx {
            state: state.clone(),
            app_weak: app_weak.clone(),
            current_sheet_idx: current_sheet_idx.clone(),
        };
        let ctx_cancel = ctx.clone();
        app.on_calc_pointer_up(move || {
            crate::ribbon::calc_home::painter_on_pointer_up(&ctx);
        });
        app.on_calc_painter_cancel(move || {
            crate::ribbon::calc_home::painter_cancel(&ctx_cancel);
        });
    }

    // ── Calc 浮动对象：选中 / 拖动 / 缩放（通用 SelectionBox）──
    {
        let k = calc_chart::PT_TO_PX;

        let st = state.clone(); let aw = app_weak.clone(); let si = current_sheet_idx.clone();
        app.on_calc_obj_try_select(move |vx, vy| {
            let Some(app) = aw.upgrade() else { return false };
            let sheet = si.get();
            let cm = calc_chart::CoordMap::new(app.get_calc_scroll_x(), app.get_calc_scroll_y());
            let mut s = st.borrow_mut();
            refresh_float_objs(&app, &mut s, sheet);
            match s.float_store.hit_view(sheet, &cm, vx, vy) {
                Some(id) => { select_float_obj(&app, &mut s, sheet, id); true }
                None => false,
            }
        });

        let st = state.clone(); let aw = app_weak.clone(); let si = current_sheet_idx.clone();
        app.on_calc_obj_miss_clicked(move |x, y| {
            let Some(app) = aw.upgrade() else { return };
            let sheet = si.get();
            let (vx, vy) = (x + calc_chart::HEADER_W, y + calc_chart::HEADER_H);
            let cm = calc_chart::CoordMap::new(app.get_calc_scroll_x(), app.get_calc_scroll_y());
            {
                let mut s = st.borrow_mut();
                refresh_float_objs(&app, &mut s, sheet);
                if let Some(id) = s.float_store.hit_view(sheet, &cm, vx, vy) {
                    select_float_obj(&app, &mut s, sheet, id);
                    return;
                }
                s.float_sel = 0;
                sync_chart_sidebar(&app, &mut s, sheet);
            }
            app.set_calc_obj_id(-1);
            app.set_calc_sel_end_row(-1);
            app.set_calc_sel_end_col(-1);
            app.invoke_calc_cell_click(vx, vy);
        });

        let drag: Rc<RefCell<Option<ObjDrag>>> = Rc::new(RefCell::new(None));

        let d1 = drag.clone(); let aw = app_weak.clone(); let si = current_sheet_idx.clone();
        app.on_calc_obj_drag_started(move |mode, mx, my| {
            let Some(app) = aw.upgrade() else { return };
            let id = app.get_calc_obj_id();
            if id < 0 { return; }
            let (sx, sy) = (app.get_calc_scroll_x(), app.get_calc_scroll_y());
            {
                let rp = (app.get_calc_render_scroll_x(), app.get_calc_render_scroll_y());
                let (vw, vh) = (app.get_calc_view_width() as f32, app.get_calc_view_height() as f32);
                let (gx, gy) = (app.get_calc_obj_x_pt(), app.get_calc_obj_y_pt());
                let (gw, gh) = (app.get_calc_obj_w_pt(), app.get_calc_obj_h_pt());
                let gr = crate::calc_chart::PtRect { x: gx, y: gy, w: gw, h: gh };
                let clips = crate::calc_chart::float_pic_clips(&[gr], rp, rp, vw, vh, doc_core::calc_bitmap_scale());
                let full = mode.as_str() != "rotate" && clips.len() == 1
                    && (clips[0].w - gw * k).abs() < 1.0 && (clips[0].h - gh * k).abs() < 1.0;
                if full {
                    app.set_calc_ghost_cx(clips[0].cx);
                    app.set_calc_ghost_cy(clips[0].cy);
                    app.set_calc_ghost_cw(clips[0].cw);
                    app.set_calc_ghost_ch(clips[0].ch);
                }
                app.set_calc_ghost_on(full);
            }
            let start = geom::R {
                x: app.get_calc_obj_x_pt() * k - sx, y: app.get_calc_obj_y_pt() * k - sy,
                w: app.get_calc_obj_w_pt() * k, h: app.get_calc_obj_h_pt() * k, rot: 0.0,
            };
            *d1.borrow_mut() = Some(ObjDrag {
                mode: mode.to_string(), start, m0: (mx, my), last: start, moved: false,
                id: id as u64, sheet: si.get(), scroll: (sx, sy),
            });
        });

        let d2 = drag.clone(); let aw = app_weak.clone();
        app.on_calc_obj_drag_moved(move |mx, my, shift, ctrl| {
            let Some(app) = aw.upgrade() else { return };
            let mut g = d2.borrow_mut();
            let Some(d) = g.as_mut() else { return };
            let new = match d.mode.as_str() {
                "move" => Some(geom::compute_move(d.start, mx - d.m0.0, my - d.m0.1)),
                "rotate" => None,
                h => geom::compute_resize(d.start, h, d.m0, (mx, my), shift, ctrl, 8.0),
            };
            let Some(r) = new else { return };
            d.last = r;
            d.moved = true;
            app.set_calc_obj_x_pt(((r.x + d.scroll.0) / k).max(0.0));
            app.set_calc_obj_y_pt(((r.y + d.scroll.1) / k).max(0.0));
            app.set_calc_obj_w_pt(r.w / k);
            app.set_calc_obj_h_pt(r.h / k);
        });

        let d3 = drag.clone(); let st = state.clone(); let aw = app_weak.clone();
        app.on_calc_obj_drag_ended(move || {
            let Some(app) = aw.upgrade() else { return };
            app.set_calc_ghost_on(false);
            let Some(d) = d3.borrow_mut().take() else { return };
            if !d.moved { return; }
            let x_pt = ((d.last.x + d.scroll.0) / k).max(0.0);
            let y_pt = ((d.last.y + d.scroll.1) / k).max(0.0);
            let (w_pt, h_pt) = (d.last.w / k, d.last.h / k);
            {
                let mut s = st.borrow_mut();
                if let Some(sess) = s.session.as_mut() {
                    sess.begin_edit();
                    let done = sess.set_chart_rect(d.sheet, d.id, x_pt, y_pt, w_pt, h_pt) || sess.set_object_rect(d.sheet, d.id, x_pt, y_pt, w_pt, h_pt);
                    if done { sess.commit_edit(); } else { sess.discard_edit(); }
                    eprintln!("[CALC-OBJ] drag end id={} mode={} rect_pt=({:.1},{:.1},{:.1},{:.1}) ok={}", d.id, d.mode, x_pt, y_pt, w_pt, h_pt, done);
                }
                refresh_float_objs(&app, &mut s, d.sheet);
                if let Some(sess) = s.session.as_mut() {
                    app.set_calc_sheet_total_width((sess.get_sheet_total_width(d.sheet)) * (96.0 / 72.0));
                    app.set_calc_sheet_total_height((sess.get_sheet_total_height(d.sheet)) * (96.0 / 72.0));
                }
            }
            // state 借用已释放，渲染回调内部会再借用
            app.invoke_calc_render_viewport(d.scroll.0, d.scroll.1);
        });
    }

    // ── Calc 拖拽范围选择 ──────────────────────────────────────────────────────
    let state_drag = state.clone();
    let app_weak_drag = app_weak.clone();
    let sheet_idx_drag = current_sheet_idx.clone();
    app.on_calc_range_drag(move |mouse_x, mouse_y| {
        let app = match app_weak_drag.upgrade() { Some(a) => a, None => return };
        let zoom_factor = 2.0_f32;
        let header_col_w = 60.0_f32 / zoom_factor;
        let header_row_h = 36.0_f32 / zoom_factor;
        let scroll_x = app.get_calc_scroll_x();
        let scroll_y = app.get_calc_scroll_y();
        let start_col = app.get_calc_selected_col();
        let start_row = app.get_calc_selected_row();
        if start_col < 0 || start_row < 0 { return; }
        if mouse_x < header_col_w || mouse_y < header_row_h { return; }
        let x_pt = ((mouse_x - header_col_w) + scroll_x) / PT_TO_PX;
        let y_pt = ((mouse_y - header_row_h) + scroll_y) / PT_TO_PX;
        let mut s = state_drag.borrow_mut();
        let merges = s.calc_merges.clone();
        if let Some(sess) = s.session.as_mut() {
            let sheet_idx = sheet_idx_drag.get();
            let end_col = sess.get_col_at_offset(sheet_idx, x_pt);
            let end_row = sess.get_row_at_offset(sheet_idx, y_pt);
            if end_col < 0 || end_row < 0 { return; }
            calc_selection::drag_to(
                &app, sess, &merges, sheet_idx,
                (start_row as u32, start_col as u32),
                end_row as u32, end_col as u32,
            );
        }
    });

    // ── Calc 复制/粘贴（Ctrl+C/V）───────────────────────────────────────────
    let state_copy = state.clone();
    let app_weak_copy = app_weak.clone();
    let sheet_idx_copy = current_sheet_idx.clone();
    app.on_calc_copy(move || {
        let app = match app_weak_copy.upgrade() { Some(a) => a, None => return };
        let start_col = app.get_calc_selected_col();
        let start_row = app.get_calc_selected_row();
        if start_col < 0 || start_row < 0 { return; }
        let end_col = app.get_calc_sel_end_col();
        let end_row = app.get_calc_sel_end_row();
        let (end_col, end_row) = if end_col < 0 || end_row < 0 { (start_col, start_row) } else { (end_col, end_row) };
        let min_col = start_col.min(end_col) as u32;
        let max_col = start_col.max(end_col) as u32;
        let min_row = start_row.min(end_row) as u32;
        let max_row = start_row.max(end_row) as u32;
        let mut s = state_copy.borrow_mut();
        if let Some(sess) = s.session.as_mut() {
            let sheet_idx = sheet_idx_copy.get();
            let ok = sess.calc_copy_range(sheet_idx as u32, min_row, min_col, max_row, max_col);
            eprintln!("[CALC-COPY] range=({},{})-({},{}) ok={}", min_row, min_col, max_row, max_col, ok);
        }
    });

    // ── Calc 删除（Delete/Backspace）：清空选中格/选中区域内容 ─────────────────
    let state_delete = state.clone();
    let app_weak_delete = app_weak.clone();
    let sheet_idx_delete = current_sheet_idx.clone();
    app.on_calc_delete_cell(move || {
        let app = match app_weak_delete.upgrade() { Some(a) => a, None => return };
        let start_col = app.get_calc_selected_col();
        let start_row = app.get_calc_selected_row();
        if start_col < 0 || start_row < 0 { return; }
        let end_col = app.get_calc_sel_end_col();
        let end_row = app.get_calc_sel_end_row();
        let (end_col, end_row) = if end_col < 0 || end_row < 0 { (start_col, start_row) } else { (end_col, end_row) };
        let min_col = start_col.min(end_col) as u32;
        let max_col = start_col.max(end_col) as u32;
        let min_row = start_row.min(end_row) as u32;
        let max_row = start_row.max(end_row) as u32;
        let mut s = state_delete.borrow_mut();
        if let Some(sess) = s.session.as_mut() {
            let sheet_idx = sheet_idx_delete.get();
            let mut all_ok = true;
            for row in min_row..=max_row {
                for col in min_col..=max_col {
                    let ok = sess.calc_clear_cell(sheet_idx as u32, row, col);
                    all_ok = all_ok && ok;
                }
            }
            eprintln!("[CALC-DELETE] range=({},{})-({},{}) ok={}", min_row, min_col, max_row, max_col, all_ok);
            if all_ok {
                // 重渲染viewport
                let scroll_x = app.get_calc_scroll_x();
                let scroll_y = app.get_calc_scroll_y();
                let vw = app.get_calc_view_width();
                let vh = app.get_calc_view_height();
                let img = sess.render_sheet_viewport(scroll_x, scroll_y, vw as u32, vh as u32);
                if !img.is_empty() {
                    let (phys_w, phys_h) = doc_core::calc_phys_size(vw as u32, vh as u32);
                    if phys_w > 0 && phys_h > 0 && img.len() == (phys_w * phys_h * 4) as usize {
                        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&img, phys_w, phys_h);
                        app.set_calc_render_scroll_x(scroll_x);
                        app.set_calc_render_scroll_y(scroll_y);
                        app.set_calc_viewport_image(slint::Image::from_rgba8(buf));
                        app.window().request_redraw(); // transform 层不会自动重绘
                    }
                }
                // 清空后公式栏也应同步清空（若清空的是当前选中格）
                app.set_calc_display_value("".into());
            }
        }
    });

    // Calc 撤销/重做:cell-edit 拦截 Ctrl+Z 后转发到这里(避免落入 Slint TextInput
    // 内置 undo 栈导致 split_at panic)。roe_undo 对 Calc 文档返回什么尚未验证,
    // 这里只看 Some/None,并打印返回内容供下一步确认。
    let state_cundo = state.clone();
    let app_weak_cundo = app_weak.clone();
    let sheet_idx_cundo = current_sheet_idx.clone();
    app.on_calc_undo_redo(move |is_redo| {
        let app = match app_weak_cundo.upgrade() { Some(a) => a, None => return };
        let mut s = state_cundo.borrow_mut();
        let Some(sess) = s.session.as_mut() else { return };
        if sess.get_doc_type() != "calc" { return; }
        let res = if is_redo { sess.redo() } else { sess.undo() };
        eprintln!("[CALC-UNDO-REDO] is_redo={} result={:?}", is_redo, res);
        if res.is_none() { return; }
        // 撤销后:退出编辑态、清空 cell-edit 残留文本,避免带旧内容
        app.set_calc_editing(false);
        app.invoke_calc_clear_edit_buffer();

        // fix: 撤销/重做可能改变 sheet 数量(如撤销"新增Sheet")，此前这里完全没有
        // 同步 SheetTabBar 名称列表和 current_sheet_idx/session.current_sheet，
        // 导致：1) 标签栏残留已撤销掉的sheet名；2) 若当前索引正好指向被撤销掉的
        // sheet，后续 render_sheet_viewport 传入越界 index 报 code=-4。
        // bug记录见 roe-ext_progress_v211起。做法参照 on_calc_add_sheet_requested
        // 里"新建sheet后同步两套索引+标签栏"的既有模式。
        {
            let updated_names = sess.get_all_sheet_names();
            let names_model: Vec<slint::SharedString> = updated_names.iter().map(|s| s.as_str().into()).collect();
            app.set_calc_sheet_names(slint::ModelRc::new(slint::VecModel::from(names_model.clone())));
            if !updated_names.is_empty() {
                let old_idx = sheet_idx_cundo.get() as usize;
                let clamped_idx = old_idx.min(updated_names.len() - 1);
                sheet_idx_cundo.set(clamped_idx as u32);
                sess.set_current_sheet(clamped_idx as i32);
                if let Some(name) = updated_names.get(clamped_idx) {
                    app.set_calc_active_sheet(name.as_str().into());
                }
                if old_idx != clamped_idx {
                    eprintln!("[CALC-UNDO-REDO] current_sheet_idx越界({} -> {})，已clamp", old_idx, clamped_idx);
                }
            }
        }

        // 重渲染 viewport
        let scroll_x = app.get_calc_scroll_x();
        let scroll_y = app.get_calc_scroll_y();
        let vw = app.get_calc_view_width();
        let vh = app.get_calc_view_height();
        if vw > 0 && vh > 0 {
            let img = sess.render_sheet_viewport(scroll_x, scroll_y, vw as u32, vh as u32);
            let (phys_w, phys_h) = doc_core::calc_phys_size(vw as u32, vh as u32);
            if !img.is_empty() && img.len() == (phys_w * phys_h * 4) as usize {
                let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&img, phys_w, phys_h);
                app.set_calc_render_scroll_x(scroll_x);
                app.set_calc_render_scroll_y(scroll_y);
                app.set_calc_viewport_image(slint::Image::from_rgba8(buf));
                app.window().request_redraw(); // transform 层不会自动重绘
            }
        }
        // 撤销/重做后:合并区/行高/列宽都可能变化,先刷新合并缓存,再按当前选区重算高亮
        let sheet = sheet_idx_cundo.get();
        let mut merges = std::mem::take(&mut s.calc_merges);
        if let Some(sess) = s.session.as_mut() {
            calc_selection::refresh_merges(&mut merges, sess, sheet);
            calc_selection::reapply(&app, sess, &merges, sheet);
            // Ribbon 按钮按下态按当前选中格重新同步
            ribbon::calc_home::sync_state_inner(&app, sess, sheet);
            // fix: 撤销/重做后公式栏未同步刷新（bug记录见roe-ext_progress_v211起）。
            // reapply() 内部已把 snap 后的锚点写回 calc-selected-row/col，这里直接读回来用。
            let ar = app.get_calc_selected_row();
            let ac = app.get_calc_selected_col();
            if ar >= 0 && ac >= 0 {
                ribbon::calc_home::refresh_formula_bar(&app, sess, sheet, ar as u32, ac as u32);
            }
        }
        s.calc_merges = merges;
        refresh_float_objs(&app, &mut s, sheet);
    });

    let state_paste = state.clone();
    let app_weak_paste = app_weak.clone();
    let sheet_idx_paste = current_sheet_idx.clone();
    app.on_calc_paste(move || {
        let app = match app_weak_paste.upgrade() { Some(a) => a, None => return };
        let dest_col = app.get_calc_selected_col();
        let dest_row = app.get_calc_selected_row();
        if dest_col < 0 || dest_row < 0 { return; }
        let mut s = state_paste.borrow_mut();
        if let Some(sess) = s.session.as_mut() {
            let sheet_idx = sheet_idx_paste.get();
            let ok = sess.calc_paste_range(sheet_idx as u32, dest_row as u32, dest_col as u32);
            eprintln!("[CALC-PASTE] dest=({},{}) ok={}", dest_row, dest_col, ok);
            if ok {
                // 粘贴后刷新公式栏当前格显示值（粘贴目标左上角格可能已变化，
                // 公式单元格显示公式原文，非公式显示值）
                let dest_formula = sess.calc_get_cell_formula(sheet_idx, dest_row as u32, dest_col as u32);
                let cell_val = if !dest_formula.is_empty() {
                    format!("={}", dest_formula)
                } else {
                    sess.calc_get_cell(sheet_idx, dest_row as u32, dest_col as u32)
                };
                app.set_calc_display_value(cell_val.into());

                // 粘贴改变了数据，需立即触发一次viewport重绘（不像编辑那样走80ms防抖
                // Timer，粘贴是单次离散操作，没有连续高频触发的顾虑，同步渲染即可）
                let scroll_x = app.get_calc_scroll_x();
                let scroll_y = app.get_calc_scroll_y();
                let vw = app.get_calc_view_width();
                let vh = app.get_calc_view_height();
                let img = sess.render_sheet_viewport(scroll_x, scroll_y, vw as u32, vh as u32);
                if !img.is_empty() {
                    let (phys_w, phys_h) = doc_core::calc_phys_size(vw as u32, vh as u32);
                    if phys_w > 0 && phys_h > 0 && img.len() == (phys_w * phys_h * 4) as usize {
                        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&img, phys_w, phys_h);
                        app.set_calc_render_scroll_x(scroll_x);
                        app.set_calc_render_scroll_y(scroll_y);
                        app.set_calc_viewport_image(slint::Image::from_rgba8(buf));
                        app.window().request_redraw(); // transform 层不会自动重绘
                    }
                }
            }
        }
    });

    // ── Writer翻页（防抖+页面缓存） ───────────────────────────────────────────
    let _writer_target_page = Rc::new(RefCell::new(1i32));
    let _writer_last_page = Rc::new(RefCell::new(0i32));
    let _writer_timer = Rc::new(slint::Timer::default());



    // ── v264 转到页 ──
    {
        let st = state.clone();
        let aw = app_weak.clone();
        app.on_goto_page(move |p| {
            let Some(app) = aw.upgrade() else { return };
            let Ok(n) = p.trim().parse::<i32>() else { app.set_status_message("请输入页码".into()); return; };
            let mut s = st.borrow_mut();
            let Some(sess) = s.session.as_mut() else { return };
            if sess.get_doc_type() != "writer" { return; }
            let tops = sess.get_writer_page_tops();
            drop(s);
            if tops.is_empty() { return; }
            let total = tops.len() as i32;
            let n = n.clamp(1, total);
            let vh = app.get_writer_view_height() as f32;
            let canvas = app.get_writer_canvas_height_pt();
            let max_scroll = (canvas - vh).max(0.0);
            let target = (tops[(n - 1) as usize] - 8.0).max(0.0).min(max_scroll);
            app.invoke_scroll_writer_to(target);
            app.set_page_current(n);
            app.set_status_message(format!("已转到第 {} 页，共 {} 页", n, total).into());
        });
    }

    // ── Writer滚动节流Timer ──────────────────────────────────────────────────
    let writer_scroll_timer = Rc::new(slint::Timer::default());
    {
        let timer = writer_scroll_timer.clone();
        let app_weak_t = app_weak.clone();
        let state_t = state.clone();
        app.on_writer_scroll_tick(move || {
            let app_weak_inner = app_weak_t.clone();
            let state_inner = state_t.clone();
            timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(80), move || {
                if let Some(app) = app_weak_inner.upgrade() {
                    let scroll_y = app.get_writer_scroll_y_pt();
                    let vw = app.get_writer_view_width() as f32;
                    let vh = app.get_writer_view_height() as f32;
                    let doc_w = app.get_writer_doc_width_pt();
                    if vw <= 0.0 || vh <= 0.0 { return; }
                    let mut s = state_inner.borrow_mut();
                    if let Some(sess) = s.session.as_mut() {
                        if sess.get_doc_type() != "writer" { return; }
                        let rgba = sess.render_writer_viewport(scroll_y, doc_w, vh);
                        if rgba.is_empty() { return; }
                        let pw = (doc_w * doc_core::doc_bitmap_scale()).round() as u32;
                        let ph = (vh * doc_core::doc_bitmap_scale()).round() as u32;
                        if (pw * ph * 4) as usize != rgba.len() { return; }
                        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, pw, ph);
                        app.set_writer_viewport_image(slint::Image::from_rgba8(buf));
                        eprintln!("[WRITER-VP] OK scroll_y={:.1} {}x{}px", scroll_y, pw, ph);

                        {
                            let tops = sess.get_writer_page_tops();
                            let probe = scroll_y + vh / 3.0;
                            let cur = tops.iter().rposition(|&t| t <= probe).map(|i| i as i32 + 1).unwrap_or(1);
                            if app.get_page_current() != cur { app.set_page_current(cur); }
                        }
                    }
                }
            });
        });
    }

    // ── Writer视口渲染 ───────────────────────────────────────────────────────
    let state_wr = state.clone();
    let app_weak_wr = app_weak.clone();
    app.on_writer_render_viewport(move |scroll_y_pt| {
        let app = match app_weak_wr.upgrade() { Some(a) => a, None => return };
        let vw = app.get_writer_view_width() as f32;
        let vh = app.get_writer_view_height() as f32;
        let doc_w = app.get_writer_doc_width_pt();
        if vw <= 0.0 || vh <= 0.0 { return; }
        let mut s = state_wr.borrow_mut();
        if let Some(sess) = s.session.as_mut() {
            if sess.get_doc_type() != "writer" { return; }
            let __t0 = std::time::Instant::now();
            let rgba = sess.render_writer_viewport(scroll_y_pt, doc_w, vh);
            eprintln!("[WRITER-VP] render_writer_viewport took {:?}", __t0.elapsed());
            if rgba.is_empty() {
                eprintln!("[WRITER-VP] render failed scroll_y={}", scroll_y_pt);
                return;
            }
            let pw = (doc_w * doc_core::doc_bitmap_scale()).round() as u32;
            let ph = (vh * doc_core::doc_bitmap_scale()).round() as u32;
            if (pw * ph * 4) as usize != rgba.len() {
                eprintln!("[WRITER-VP] buf size mismatch {}x{} buf={}", pw, ph, rgba.len());
                return;
            }
            let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, pw, ph);
            app.set_writer_viewport_image(slint::Image::from_rgba8(buf));
            eprintln!("[WRITER-VP] OK scroll_y={:.1} {}x{}px", scroll_y_pt, pw, ph);

                        {
                            let tops = sess.get_writer_page_tops();
                            let probe = scroll_y_pt + vh / 3.0;
                            let cur = tops.iter().rposition(|&t| t <= probe).map(|i| i as i32 + 1).unwrap_or(1);
                            if app.get_page_current() != cur { app.set_page_current(cur); }
                        }

            // v158第三节起点：每次viewport渲染完成后，取本次可见的批注命中坐标，
            // 结合state.comment_threads缓存的author/text拼出CommentBubble数组。
            // 坐标换算：hit的line_top_pt是文档坐标系累计pt，Slint端会再减去
            // writer-scroll-y-pt换算成屏幕坐标（见app.slint的for循环）；气泡贴在
            // 文档右侧外部（doc_w+8pt处），固定宽度180pt/高度60pt做第一版最小验证，
            // 不做自适应换行高度计算（后续如果文字被截断，需要按text长度估算高度）。
            // v285: 每次渲染前重新拉取批注文本，保证插入/撤销/重做后气泡内容同步
            let fresh_threads: Option<HashMap<i64, (String, String, String, bool, Vec<(String, String, String)>)>> = {
                let threads_json = sess.get_comment_threads_json();
                let mut map = HashMap::new();
                let mut ok = false;
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&threads_json) {
                    if let Some(arr) = v.get("threads").and_then(|x| x.as_array()) {
                        ok = true;
                        for thread in arr {
                            if let Some(root) = thread.get("root") {
                                let id = root.get("id").and_then(|x| x.as_i64()).unwrap_or(-1);
                                let author = root.get("author").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                let date = root.get("date").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                let text = root.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                let done = root.get("done").and_then(|x| x.as_bool()).unwrap_or(false);
                                let mut replies_vec: Vec<(String, String, String)> = Vec::new();
                                if let Some(ra) = thread.get("replies").and_then(|r| r.as_array()) {
                                    for reply in ra {
                                        replies_vec.push((
                                            reply.get("author").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                                            reply.get("date").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                                            reply.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                                        ));
                                    }
                                }
                                if id >= 0 { map.insert(id, (author, date, text, done, replies_vec)); }
                            }
                        }
                    }
                }
                if ok { Some(map) } else { None }
            };
            let fresh_reply_ids: HashMap<i64, Vec<i32>> = {
                let mut m = HashMap::new();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&sess.get_comment_threads_json()) {
                    if let Some(arr) = v.get("threads").and_then(|x| x.as_array()) {
                        for t in arr {
                            let rid = t.get("root").and_then(|r| r.get("id")).and_then(|x| x.as_i64()).unwrap_or(-1);
                            let ids: Vec<i32> = t.get("replies").and_then(|r| r.as_array())
                                .map(|ra| ra.iter().map(|x| x.get("id").and_then(|y| y.as_i64()).unwrap_or(-1) as i32).collect())
                                .unwrap_or_default();
                            if rid >= 0 { m.insert(rid, ids); }
                        }
                    }
                }
                m
            };
            let hits_json = sess.take_comment_hits_json();
            if let Some(m) = fresh_threads { s.comment_threads = m; }
            s.reply_ids = fresh_reply_ids;
            let mut bubbles: Vec<CommentBubble> = Vec::new();
            // v293: 回复与父批注同锚点，命中里会出现回复 id；映射回根 id 并按根去重
            let mut reply_to_root: HashMap<i64, i64> = HashMap::new();
            for (root_id, rids) in s.reply_ids.iter() {
                for r in rids { if *r >= 0 { reply_to_root.insert(*r as i64, *root_id); } }
            }
            let mut seen_bubble_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
            // 与bubbles等长同下标对应的估算高度数组，供拼装完成后的避让算法使用
            // （v161新增，见ROE_Progress_v161批注气泡重叠修复）。
            let mut bubble_estimated_heights: Vec<f32> = Vec::new();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&hits_json) {
                if let Some(arr) = v.get("hits").and_then(|h| h.as_array()) {
                    for hit in arr {
                        let raw_comment_id = hit.get("comment_id").and_then(|x| x.as_i64()).unwrap_or(-1);
                        let comment_id = reply_to_root.get(&raw_comment_id).copied().unwrap_or(raw_comment_id);
                        if comment_id >= 0 && !seen_bubble_ids.insert(comment_id) { continue; }
                        let line_top = hit.get("line_top_pt").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                        // v162新增：连接线起点应对齐高亮行底部而非顶部，内核本身已在
                        // roe-api/src/comments.rs输出line_bottom_pt字段，此前main.rs未读取，
                        // 是v161连接线纵向错位的根因之一（另一因是Path fit:contain已在v162修复）。
                        let line_bottom = hit.get("line_bottom_pt").and_then(|x| x.as_f64()).unwrap_or(line_top as f64) as f32;
                        // color_idx来自roe内核（与WriterTileRenderer::pending_comment_hits同源），
                        // 直接复用，不在Slint端重新计算hash，保证与roe侧高亮色一一对应。
                        let color_idx = hit.get("color_idx").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
                        if comment_id < 0 { continue; }
                        let (author, date, text, done, replies_raw) = s.comment_threads.get(&comment_id)
                            .cloned()
                            .unwrap_or_else(|| ("".to_string(), "".to_string(), "".to_string(), false, Vec::new()));
                        let replies_raw_for_estimate = replies_raw.clone();
                        let rids: Vec<i32> = s.reply_ids.get(&comment_id).cloned().unwrap_or_default();
                        let replies: Vec<CommentReply> = replies_raw.into_iter().enumerate()
                            .map(|(ri, (r_author, r_date, r_text))| CommentReply {
                                id: rids.get(ri).copied().unwrap_or(-1),
                                author: r_author.into(),
                                date: r_date.into(),
                                text: r_text.into(),
                            })
                            .collect();
                        // 头像缩写：按空格分词取每个词首字母，最多2个，全大写，
                        // 逻辑抄自roe-writer/src/tile.rs draw_comment_bubble_placeholder
                        // 里reply_avatar_label的计算方式（ROE_Progress_v158/v159参照）。
                        // 非ASCII首字符（如中文姓名）直接取该字符本身，不裁剪。
                        let initials: String = {
                            let first_char = author.chars().next();
                            match first_char {
                                Some(c) if c.is_ascii_alphabetic() => {
                                    author.split_whitespace()
                                        .filter_map(|w| w.chars().next())
                                        .map(|c| c.to_ascii_uppercase())
                                        .take(2)
                                        .collect::<String>()
                                }
                                Some(c) => c.to_string(),
                                None => String::new(),
                            }
                        };
                        // ry暂时等于line_top（原始锚点位置），下方避让算法会在bubbles
                        // 全部拼装完成后统一调整ry；anchor_ry固定保存原始值不再变动，供
                        // Slint端画连接线用（v161新增，修复重叠避让后锚点-气泡错位问题）。
                        bubbles.push(CommentBubble {
                            rx: (doc_w + 8.0) as f32,
                            ry: line_top,
                            anchor_ry: line_bottom,
                            // v159样式还原：加高加宽以容纳头像+作者名+日期+按钮这一整行头部
                            // （原90pt只留了正文空间），仍是临时固定值，不是最终自适应方案。
                            rw: 260.0,
                            rh: 110.0,
                            comment_id: comment_id as i32,
                            color_idx: color_idx,
                            author: author.clone().into(),
                            date: date.into(),
                            text: text.clone().into(),
                            avatar_initials: initials.into(),
                            done: done,
                            replies: slint::ModelRc::new(slint::VecModel::from(replies.clone())),
                        });
                        // 估算气泡高度（供下方避让算法用，不追求与Slint端content-layout.
                        // preferred-height像素级一致，只要求"足够大不小于实际"，宁可算多
                        // 留白也不能算少导致仍然重叠）。构成：上下padding共16px + 头部行
                        // 约30px + 正文按字符数估算换行行数*行高 + 每条reply(分隔线+作者行+
                        // 正文换行)*行高 + "添加回复"链接一行14px + VerticalLayout spacing
                        // (6px)*子项数量。中文字符按2倍宽度估算（比英文更容易换行）。
                        fn estimate_text_lines(s: &str, chars_per_line: usize) -> usize {
                            let weighted_len: usize = s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
                            ((weighted_len.max(1) + chars_per_line - 1) / chars_per_line).max(1)
                        }
                        let body_lines = estimate_text_lines(&text, 40);
                        let mut estimated_h: f32 = 16.0 + 30.0 + (body_lines as f32) * 14.0 + 14.0 + 6.0 * 2.0;
                        for (_, _, r_text) in replies_raw_for_estimate.iter() {
                            let reply_lines = estimate_text_lines(r_text, 36);
                            estimated_h += 1.0 + 14.0 + (reply_lines as f32) * 13.0 + 6.0 + 6.0;
                        }
                        bubble_estimated_heights.push(estimated_h);
                    }
                }
            }
            // 纵向堆叠避让算法（v161新增，解决ROE_Progress_v160记录的批注气泡重叠问题）。
            // 思路：按anchor_ry（原始锚点位置）排序，贪心地把后一个气泡推到"上一个气泡结束
            // 位置+间距"之后（如果它的期望位置比这更靠上）。anchor_ry本身不变，只调整ry
            // （气泡实际显示y），Slint端会用两者的差值画连接线标出锚点与气泡的错位。
            // 参考roe内核render_with_comments.rs里的贪心避让思路（只借鉴算法，不调用其实现，
            // 遵循v160 1.6节架构原则）。BUBBLE_GAP_PT是气泡之间的最小垂直间距。
            const BUBBLE_GAP_PT: f32 = 8.0;
            let mut order: Vec<usize> = (0..bubbles.len()).collect();
            order.sort_by(|&a, &b| bubbles[a].anchor_ry.partial_cmp(&bubbles[b].anchor_ry).unwrap());
            let mut cursor_y: f32 = f32::NEG_INFINITY;
            for &idx in order.iter() {
                let desired_y = bubbles[idx].anchor_ry;
                let actual_y = if cursor_y.is_finite() { cursor_y.max(desired_y) } else { desired_y };
                bubbles[idx].ry = actual_y;
                cursor_y = actual_y + bubble_estimated_heights[idx] + BUBBLE_GAP_PT;
            }
            eprintln!("[WRITER-VP] 批注气泡 count={}", bubbles.len());
            for b in bubbles.iter() {
                eprintln!("[WRITER-VP-DEBUG] id={} rx={:.1} ry={:.1} anchor_ry={:.1} rw={:.1}",
                    b.comment_id, b.rx, b.ry, b.anchor_ry, b.rw);
            }
            app.set_writer_comment_bubbles(slint::ModelRc::new(slint::VecModel::from(bubbles)));
        }
    });

    // v51 E步：翻页模式不再需要滚动节流Timer（原impress-scroll-tick已废弃，见app.slint改造）。
    // 保留空回调注册，避免旧UI残留调用点时编译期/运行期报错；app.slint已改为不再触发它。
    app.on_impress_scroll_tick(move || {});

    // ============ Impress 缩略图渲染 ============
    // 依赖 doc_core::TileCacheRust::render_page(page_idx, width_px, height_px) -> Vec<u8>(RGBA8)，
    // 底层调用 roe_render_page（roe-api/src/tile.rs），按 page_idx 独立渲染任意一页，
    // 不依赖"当前活动slide"状态，不影响主viewport的编辑/显示状态，可随时调用。
    //
    // 缩略图输出固定小尺寸，宽度按面板卡片宽度（132px逻辑px，见app.slint缩略图Rectangle width）
    // 乘以2倍做HiDPI适配；render_page内部会用roe_get_render_page_size按此宽度反推等比例高度，
    // 不需要在这里手工按slide宽高比计算height。
    const IMPRESS_THUMB_WIDTH_PX: i32 = 264; // 132px逻辑 * 2 (HiDPI scale=2)

    // 单张缩略图刷新：渲染指定page_idx，替换impress-thumbnail-images数组里对应下标的一项。
    //
    // TODO(ROE编辑接口接入后): 当前Impress编辑功能(M1/M5：文本框输入/形状增删/图片插入等)尚未
    // 通过roe-script接入，内容目前只会因为"切页"发生变化（见on_impress_render_viewport里的
    // 调用点），所以现在只在切页时调用本函数即可覆盖所有会导致内容变化的场景。
    // 将来M5编辑功能接入后，只需要在对应的编辑完成回调（比如未来的
    // on_impress_text_edited / on_impress_shape_changed 之类）末尾同样调用一次
    // refresh_impress_thumbnail(&app, &state, idx)，不需要改动本函数或缩略图渲染管线本身。
    fn refresh_impress_thumbnail(app: &AppWindow, state: &Rc<RefCell<AppState>>, page_idx: usize) {
        let rgba = {
            let mut s = state.borrow_mut();
            let sess = match s.session.as_mut() { Some(s) => s, None => return };
            // 注意：session.render_page内部虽然height_px参数不参与实际渲染计算（ROE按
            // width反推等比例高度），但函数入口处仍校验 height_px > 0，传0会被静默拒绝直接返回
            // 空Vec（不会打印任何错误日志，之前排查耗时的原因）。这里传与宽度相同的占位正数即可。
            sess.render_page(page_idx as i32, IMPRESS_THUMB_WIDTH_PX, IMPRESS_THUMB_WIDTH_PX)
        };
        if rgba.is_empty() {
            eprintln!("[IMPRESS-THUMB] render_page failed page_idx={}", page_idx);
            return;
        }
        let pixel_count = rgba.len() / 4;
        // render_page内部按宽度反推高度，这里从buf长度和已知宽度反推实际高度用于构造SharedPixelBuffer
        let w = IMPRESS_THUMB_WIDTH_PX as u32;
        if w == 0 || pixel_count as u32 % w != 0 {
            eprintln!("[IMPRESS-THUMB] buf size {} not divisible by width {}", rgba.len(), w);
            return;
        }
        let h = pixel_count as u32 / w;
        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w, h);
        let new_img = slint::Image::from_rgba8(buf);

        // 读出现有数组，替换对应下标，整体set回去（与项目里其余ModelRc更新方式保持一致，见文件内其它set_xxx(VecModel::from(...))调用点）
        let model = app.get_impress_thumbnail_images();
        let mut items: Vec<slint::Image> = (0..model.row_count()).map(|i| model.row_data(i).unwrap()).collect();
        if page_idx >= items.len() {
            items.resize(page_idx + 1, slint::Image::default());
        }
        items[page_idx] = new_img;
        app.set_impress_thumbnail_images(slint::ModelRc::new(slint::VecModel::from(items)));
    }

    // 全量刷新：文档打开时，依次渲染所有slide的缩略图。
    fn refresh_all_impress_thumbnails(app: &AppWindow, state: &Rc<RefCell<AppState>>) {
        let slide_count = app.get_impress_slide_count().max(0) as usize;
        // 先分配好长度为slide_count的占位数组，避免渲染过程中UI侧因下标越界报错
        app.set_impress_thumbnail_images(slint::ModelRc::new(slint::VecModel::from(vec![slint::Image::default(); slide_count])));
        for idx in 0..slide_count {
            refresh_impress_thumbnail(app, state, idx);
        }
    }

    // v320h4: 母版视图缩略图（母版与版式平铺列表）
    fn refresh_master_thumbs(app: &AppWindow, state: &Rc<RefCell<AppState>>) {
        let items = {
            let s = state.borrow();
            s.session.as_ref().map(|x| x.impress_master_items()).unwrap_or_default()
        };
        let mut imgs: Vec<slint::Image> = Vec::new();
        let mut mis: Vec<i32> = Vec::new();
        let mut lis: Vec<i32> = Vec::new();
        for (mi, li, _name) in items.iter() {
            let r = {
                let mut s = state.borrow_mut();
                s.session.as_mut().and_then(|x| x.impress_master_render(*mi, *li, IMPRESS_THUMB_WIDTH_PX))
            };
            let img = match r {
                Some((rgba, w, h)) => {
                    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w, h);
                    slint::Image::from_rgba8(buf)
                }
                None => {
                    eprintln!("[MASTER-THUMB] render failed master={} layout={}", mi, li);
                    slint::Image::default()
                }
            };
            imgs.push(img);
            mis.push(*mi as i32);
            lis.push(*li);
        }
        app.set_impress_master_thumbs(slint::ModelRc::new(slint::VecModel::from(imgs)));
        app.set_impress_master_mi(slint::ModelRc::new(slint::VecModel::from(mis)));
        app.set_impress_master_li(slint::ModelRc::new(slint::VecModel::from(lis)));
        app.set_impress_master_sel(0);
        // v320n: hover 提示文本
        let tips: Vec<slint::SharedString> = {
            let s = state.borrow();
            s.session.as_ref().map(|x| x.impress_master_tips()).unwrap_or_default().into_iter().map(slint::SharedString::from).collect()
        };
        app.set_impress_master_tips(slint::ModelRc::new(slint::VecModel::from(tips)));
    }

    // v320k: 按当前选中行刷新"可删除版式"状态（母版行与被使用的版式不可删）
    fn refresh_master_can_delete(app: &AppWindow, state: &Rc<RefCell<AppState>>) {
        let row = app.get_impress_master_sel();
        let mi = app.get_impress_master_mi().row_data(row as usize).unwrap_or(0) as u32;
        let li = app.get_impress_master_li().row_data(row as usize).unwrap_or(-1);
        let can = if li < 0 {
            false
        } else {
            let mut s = state.borrow_mut();
            match s.session.as_mut() {
                Some(sess) => {
                    let u = sess.impress_master_layout_usage(mi);
                    u.get(li as usize).map(|n| *n == 0).unwrap_or(false) && u.len() > 1
                }
                None => false,
            }
        };
        app.set_impress_master_can_delete(can);
    }

    // v320k: 重建列表后选中指定版式所在的行
    fn select_master_layout(app: &AppWindow, state: &Rc<RefCell<AppState>>, mi: i32, li: i32) {
        refresh_master_thumbs(app, state);
        let lis = app.get_impress_master_li();
        let mis = app.get_impress_master_mi();
        let mut row = 0i32;
        for r in 0..lis.row_count() {
            if lis.row_data(r) == Some(li) && mis.row_data(r) == Some(mi) { row = r as i32; break; }
        }
        app.set_impress_master_sel(row);
        show_master_canvas(app, state, row);
        app.set_impress_sel_id(-1);
        refresh_master_can_delete(app, state);
    }

    {
        let app_weak_dup = app_weak.clone();
        let state_dup = state.clone();
        app.on_impress_master_duplicate_layout(move || {
            let app = match app_weak_dup.upgrade() { Some(a) => a, None => return };
            let row = app.get_impress_master_sel();
            let mi = app.get_impress_master_mi().row_data(row as usize).unwrap_or(0);
            let li = app.get_impress_master_li().row_data(row as usize).unwrap_or(-1);
            if li < 0 { return; }
            let new_li = {
                let mut s = state_dup.borrow_mut();
                match s.session.as_mut() {
                    Some(sess) => {
                        sess.begin_edit();
                        let n = sess.impress_master_duplicate_layout(mi as u32, li as u32);
                        if n >= 0 { sess.commit_edit(); } else { sess.discard_edit(); }
                        n
                    }
                    None => -1,
                }
            };
            if new_li >= 0 { select_master_layout(&app, &state_dup, mi, new_li); }
        });
        // v320n: 版式重命名（打开弹窗 + 应用）
        let app_weak_rn = app_weak.clone();
        let state_rn = state.clone();
        app.on_impress_master_rename_open(move || {
            let app = match app_weak_rn.upgrade() { Some(a) => a, None => return };
            let row = app.get_impress_master_sel();
            let li = app.get_impress_master_li().row_data(row as usize).unwrap_or(-1);
            if li < 0 { return; }
            let name: String = {
                let s = state_rn.borrow();
                match s.session.as_ref() {
                    Some(sess) => sess.impress_master_items().get(row as usize).map(|t| t.2.to_string()).unwrap_or_default(),
                    None => String::new(),
                }
            };
            app.set_impress_rename_text(name.into());
            app.set_impress_rename_visible(true);
        });
        let app_weak_ra = app_weak.clone();
        let state_ra = state.clone();
        app.on_impress_master_rename_apply(move |new_name| {
            let app = match app_weak_ra.upgrade() { Some(a) => a, None => return };
            let row = app.get_impress_master_sel();
            let mi = app.get_impress_master_mi().row_data(row as usize).unwrap_or(0);
            let li = app.get_impress_master_li().row_data(row as usize).unwrap_or(-1);
            if li < 0 { return; }
            let nm = new_name.trim().to_string();
            if nm.is_empty() { return; }
            let ok = {
                let mut s = state_ra.borrow_mut();
                match s.session.as_mut() {
                    Some(sess) => {
                        sess.begin_edit();
                        let r = sess.impress_master_rename_layout(mi as u32, li as u32, &nm);
                        if r { sess.commit_edit(); } else { sess.discard_edit(); }
                        r
                    }
                    None => false,
                }
            };
            if ok { select_master_layout(&app, &state_ra, mi, li); }
        });
        // v320n: 插入占位符（打开弹窗 + 应用）
        let app_weak_po = app_weak.clone();
        let state_po = state.clone();
        app.on_impress_master_ph_open(move || {
            let app = match app_weak_po.upgrade() { Some(a) => a, None => return };
            let row = app.get_impress_master_sel();
            let mi = app.get_impress_master_mi().row_data(row as usize).unwrap_or(0);
            let li = app.get_impress_master_li().row_data(row as usize).unwrap_or(-1);
            if li < 0 { return; }
            let have: Vec<String> = {
                let mut s = state_po.borrow_mut();
                match s.session.as_mut() {
                    Some(sess) => sess.impress_master_layout_placeholders(mi as u32, li as u32),
                    None => Vec::new(),
                }
            };
            let has = |k: &str| have.iter().any(|x| x == k);
            app.set_impress_ph_has_title(has("title"));
            app.set_impress_ph_has_body(has("body"));
            app.set_impress_ph_has_date(has("dt"));
            app.set_impress_ph_has_footer(has("ftr"));
            app.set_impress_ph_has_num(has("sldNum"));
            app.set_impress_ph_visible(true);
        });
        let app_weak_pa = app_weak.clone();
        let state_pa = state.clone();
        app.on_impress_master_ph_apply(move |kind| {
            let app = match app_weak_pa.upgrade() { Some(a) => a, None => return };
            let row = app.get_impress_master_sel();
            let mi = app.get_impress_master_mi().row_data(row as usize).unwrap_or(0);
            let li = app.get_impress_master_li().row_data(row as usize).unwrap_or(-1);
            if li < 0 { return; }
            let k = kind.to_string();
            let ok = {
                let mut s = state_pa.borrow_mut();
                match s.session.as_mut() {
                    Some(sess) => {
                        sess.begin_edit();
                        let r = sess.impress_master_insert_placeholder(mi as u32, li as u32, &k);
                        if r >= 0 { sess.commit_edit(); } else { sess.discard_edit(); }
                        r >= 0
                    }
                    None => false,
                }
            };
            if ok { select_master_layout(&app, &state_pa, mi, li); }
        });
        let app_weak_del = app_weak.clone();
        let state_del = state.clone();
        app.on_impress_master_delete_layout(move || {
            let app = match app_weak_del.upgrade() { Some(a) => a, None => return };
            if !app.get_impress_master_can_delete() { return; }
            let row = app.get_impress_master_sel();
            let mi = app.get_impress_master_mi().row_data(row as usize).unwrap_or(0);
            let li = app.get_impress_master_li().row_data(row as usize).unwrap_or(-1);
            if li < 0 { return; }
            let ok = {
                let mut s = state_del.borrow_mut();
                match s.session.as_mut() {
                    Some(sess) => {
                        sess.begin_edit();
                        let d = sess.impress_master_delete_layout(mi as u32, li as u32);
                        if d { sess.commit_edit(); } else { sess.discard_edit(); }
                        d
                    }
                    None => false,
                }
            };
            if ok {
                let target = if li > 0 { li - 1 } else { 0 };
                select_master_layout(&app, &state_del, mi, target);
            }
        });
    }

    {
        let app_weak_me = app_weak.clone();
        let state_me = state.clone();
        app.on_impress_master_enter(move || {
            let app = match app_weak_me.upgrade() { Some(a) => a, None => return };
            refresh_master_thumbs(&app, &state_me);
            let cur = (app.get_impress_current_slide() - 1).max(0) as u32;
            let cur_li = {
                let s = state_me.borrow();
                s.session.as_ref().map(|x| x.impress_slide_layout(cur)).unwrap_or(-1)
            };
            let mut row = 0i32;
            if cur_li >= 0 {
                let lis = app.get_impress_master_li();
                for r in 0..lis.row_count() {
                    if lis.row_data(r) == Some(cur_li) { row = r as i32; break; }
                }
            }
            app.set_impress_master_sel(row);
            show_master_canvas(&app, &state_me, row);
            app.set_impress_sel_id(-1);
            refresh_master_can_delete(&app, &state_me);
        });
    }

    // v320i: 只刷新母版列表里某一行的缩略图（不重置选中）
    fn refresh_master_thumb_one(app: &AppWindow, state: &Rc<RefCell<AppState>>, row: i32) {
        if row < 0 { return; }
        let mi = match app.get_impress_master_mi().row_data(row as usize) { Some(v) => v, None => return };
        let li = match app.get_impress_master_li().row_data(row as usize) { Some(v) => v, None => return };
        let r = {
            let mut s = state.borrow_mut();
            s.session.as_mut().and_then(|x| x.impress_master_render(mi as u32, li, IMPRESS_THUMB_WIDTH_PX))
        };
        if let Some((rgba, w, h)) = r {
            let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w, h);
            app.get_impress_master_thumbs().set_row_data(row as usize, slint::Image::from_rgba8(buf));
        }
    }

    // v320h5: 母版视图画布：渲染选中的母版或版式到中间大图
    fn show_master_canvas(app: &AppWindow, state: &Rc<RefCell<AppState>>, row: i32) {
        if row < 0 { return; }
        let mi = match app.get_impress_master_mi().row_data(row as usize) { Some(v) => v, None => return };
        let li = match app.get_impress_master_li().row_data(row as usize) { Some(v) => v, None => return };
        let w_px = (app.get_impress_canvas_width_pt() * doc_core::impress_bitmap_scale()).round() as i32;
        if w_px <= 0 { return; }
        let r = {
            let mut s = state.borrow_mut();
            s.session.as_mut().and_then(|x| x.impress_master_render(mi as u32, li, w_px))
        };
        match r {
            Some((rgba, w, h)) => {
                let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w, h);
                app.set_impress_viewport_image(slint::Image::from_rgba8(buf));
            }
            None => eprintln!("[MASTER-CANVAS] render failed master={} layout={}", mi, li),
        }
    }

    {
        let app_weak_mp = app_weak.clone();
        let state_mp = state.clone();
        app.on_impress_master_pick(move |row| {
            let app = match app_weak_mp.upgrade() { Some(a) => a, None => return };
            show_master_canvas(&app, &state_mp, row);
            app.set_impress_sel_id(-1);
            refresh_master_can_delete(&app, &state_mp);
        });
        let app_weak_mc = app_weak.clone();
        let state_mc = state.clone();
        app.on_impress_master_close(move || {
            if let Some(a) = app_weak_mc.upgrade() { a.set_impress_sel_id(-1); }
            let app = match app_weak_mc.upgrade() { Some(a) => a, None => return };
            refresh_all_impress_thumbnails(&app, &state_mc);
            app.invoke_impress_render_viewport(0.0);
        });
    }

    // -- Impress翻页渲染（v51 E步改造：按impress_current_slide查slide的y偏移+固定高度渲染，硬切换无动画）--
    let state_ir = state.clone();
    let app_weak_ir = app_weak.clone();
    app.on_impress_render_viewport(move |_unused_scroll_y_pt| {
        let app = match app_weak_ir.upgrade() { Some(a) => a, None => return };
        if app.get_impress_master_mode() { return; }
        let vw = app.get_impress_view_width() as f32;
        if vw <= 0.0 { return; }
        let mut s = state_ir.borrow_mut();
        let idx = (app.get_impress_current_slide() - 1).max(0) as usize;
        // 用当前slide自身的width/height渲染（而非整体canvas宽度），避免contain模式下双重留白导致灰边
        let (slide_y, slide_w, slide_h) = s.impress_slides.get(idx)
            .map(|(y, w, h)| (*y, *w, *h))
            .unwrap_or((0.0, app.get_impress_canvas_width_pt(), app.get_impress_canvas_height_pt()));
        // 批3:用户没手动缩放过时,按窗口自适应(contain,四周留 24px)
        if !app.get_impress_user_zoomed() && slide_w > 0.0 && slide_h > 0.0 {
            let vh = app.get_impress_view_height() as f32;
            if vh > 0.0 {
                let fit = ((((vw - 48.0) / slide_w).min((vh - 48.0) / slide_h)) * 100.0).floor() / 100.0;
                let fit = fit.clamp(0.25, 3.0);
                if (fit - app.get_impress_scale()).abs() > 1e-4 {
                    doc_core::set_impress_scale(fit);
                    app.set_impress_scale(fit);
                    app.set_zoom_level(fit);
                    app.set_impress_pan_x(0.0);
                    app.set_impress_pan_y(0.0);
                }
            }
        }
        if let Some(sess) = s.session.as_mut() {
            if sess.get_doc_type() != "impress" { return; }
            let __t0 = std::time::Instant::now();
            let rgba = sess.render_impress_viewport(0.0, slide_y, slide_w, slide_h);
            eprintln!("[IMPRESS-VP] render_impress_viewport took {:?}", __t0.elapsed());
            if rgba.is_empty() {
                eprintln!("[IMPRESS-VP] render failed slide_idx={} slide_y={}", idx, slide_y);
                return;
            }
            let pw = (slide_w * doc_core::impress_bitmap_scale()).round() as u32;
            let ph = (slide_h * doc_core::impress_bitmap_scale()).round() as u32;
            if (pw * ph * 4) as usize != rgba.len() {
                eprintln!("[IMPRESS-VP] buf size mismatch {}x{} buf={}", pw, ph, rgba.len());
                return;
            }
            let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, pw, ph);
            app.set_impress_viewport_image(slint::Image::from_rgba8(buf));
            app.set_impress_canvas_width_pt(slide_w);
            app.set_impress_canvas_height_pt(slide_h);
            eprintln!("[IMPRESS-VP] OK slide_idx={} slide_y={:.1} {}x{}px", idx, slide_y, pw, ph);
            eprintln!("[IMPRESS-DEBUG] view_width={} view_height={} slide_w={} slide_h={} scale={} buf_pw={} buf_ph={}", app.get_impress_view_width(), app.get_impress_view_height(), slide_w, slide_h, app.window().scale_factor(), pw, ph);
        }
        // 释放对state_ir的借用（s/sess）后，再刷新当前页缩略图，避免RefCell重复借用panic
        // （refresh_impress_thumbnail内部会自己再borrow_mut(state_ir)）。
        drop(s);
        // 缩略图：切页进入这一页时顺带刷新它自己的缩略图。
        // TODO(ROE编辑接口接入后): 目前Impress编辑功能(M1/M5)尚未打通，内容只会因"切页"变化，
        // 所以现在只在切页时刷新当前页缩略图即可覆盖所有会变化的场景。
        // 将来ROE的编辑类C-ABI接口（插入文字/形状/图片等）接入后，只需在"编辑动作完成"的回调
        // 末尾同样调用一次 refresh_impress_thumbnail(&app, &state_ir, idx) 即可保持缩略图实时同步，
        // 不需要改动本函数或缩略图渲染逻辑本身。
        refresh_impress_thumbnail(&app, &state_ir, idx);
    });

    // -- Impress 点击选中形状：坐标为幻灯片内 pt，hit_test 命中后查矩形，写回选中框属性 --
    let state_ic = state.clone();
    let app_weak_ic = app_weak.clone();
    app.on_impress_canvas_clicked(move |x, y| {
        let app = match app_weak_ic.upgrade() { Some(a) => a, None => return };
        app.invoke_impress_text_exit();
        let idx = (app.get_impress_current_slide() - 1).max(0) as u32;
        if app.get_impress_master_mode() {
            let row = app.get_impress_master_sel();
            let mi = app.get_impress_master_mi().row_data(row as usize).unwrap_or(0) as u32;
            let li = app.get_impress_master_li().row_data(row as usize).unwrap_or(-1);
            let mres: Option<(u32, (f32, f32, f32, f32, f32))> = {
                let mut s = state_ic.borrow_mut();
                match s.session.as_mut() {
                    Some(sess) => match sess.impress_master_hit_test(mi, li, x, y) {
                        Some(id) => sess.impress_master_query_shape_rect(mi, li, id).map(|r| (id, r)),
                        None => None,
                    },
                    None => None,
                }
            };
            eprintln!("[MASTER-SEL] click=({:.1},{:.1}) mi={} li={} result={:?}", x, y, mi, li, mres);
            match mres {
                Some((id, (rx, ry, rw, rh, rot))) => {
                    app.set_impress_sel_x(rx);
                    app.set_impress_sel_y(ry);
                    app.set_impress_sel_w(rw);
                    app.set_impress_sel_h(rh);
                    app.set_impress_sel_rot(rot);
                    app.set_impress_sel_line(false);
                    app.set_impress_sel_is_chart(false);
                    app.set_impress_sel_id(id as i32);
                }
                None => app.set_impress_sel_id(-1),
            }
            return;
        }
        let result: Option<(u32, (f32, f32, f32, f32, f32))> = {
            let mut s = state_ic.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            if sess.get_doc_type() != "impress" { return; }
            match sess.impress_hit_test(idx, x, y) {
                Some(id) => sess.impress_query_shape_rect(idx, id).map(|r| (id, r)),
                None => None,
            }
        };
        eprintln!("[IMPRESS-SEL] click=({:.1},{:.1}) slide={} result={:?}", x, y, idx, result);
        match result {
            Some((id, (rx, ry, rw, rh, rot))) => {
                app.set_impress_sel_x(rx);
                app.set_impress_sel_y(ry);
                app.set_impress_sel_w(rw);
                app.set_impress_sel_h(rh);
                app.set_impress_sel_rot(rot);
                app.set_impress_sel_id(id as i32);
                {
                    let mut s = state_ic.borrow_mut();
                    let is_chart = s.session.as_mut()
                        .and_then(|sess| sess.impress_get_chart_json(idx, id)).is_some();
                    app.set_impress_sel_is_chart(is_chart);
                }
                let li = {
                    let mut s = state_ic.borrow_mut();
                    s.session.as_mut().and_then(|sess| sess.impress_query_line_info(idx, id))
                };
                match li {
                    Some((true, fh, fv)) => {
                        let (x1, y1, x2, y2) = geom::line_endpoints(rx, ry, rw, rh, rot, fh, fv);
                        app.set_impress_line_x1(x1);
                        app.set_impress_line_y1(y1);
                        app.set_impress_line_x2(x2);
                        app.set_impress_line_y2(y2);
                        app.set_impress_sel_line(true);
                    }
                    _ => app.set_impress_sel_line(false),
                }
            }
            None => app.set_impress_sel_id(-1),
        }
    });

    // -- Impress 拖动句柄：拖动中只改预览框 + 替身，松手才向后端提交一次，再重渲染 --
    struct ImpressDrag {
        mode: String,
        start: geom::R,
        m0: (f32, f32),
        last: geom::R,
        moved: bool,
        clip: (f32, f32, f32, f32), // 起始外接区域（pt，已钳到幻灯片内）
        ghost_ok: bool,
        slide_idx: u32,
        shape_id: u32,
        click_edit: Option<(f32, f32)>, // v318t: 已选中文字形状上按下，无移动松手则进入编辑
    }
    let impress_drag: Rc<RefCell<Option<ImpressDrag>>> = Rc::new(RefCell::new(None));

    let state_ds2 = state.clone();
    let drag_s = impress_drag.clone();
    let app_weak_ds = app_weak.clone();
    app.on_impress_drag_started(move |mode, mx, my| {
        let app = match app_weak_ds.upgrade() { Some(a) => a, None => return };
        let id = app.get_impress_sel_id();
        if id < 0 { return; }
        // 2c：已选中的文字形状，点内部（离边缘8pt以外）= 进入编辑/定位光标，不拖动
        if mode == "move" && !app.get_impress_sel_line() && app.get_impress_text_edit_id() >= 0 {
            let (sx, sy, sw_, sh_) = (app.get_impress_sel_x(), app.get_impress_sel_y(), app.get_impress_sel_w(), app.get_impress_sel_h());
            if app.get_impress_sel_rot().abs() < 0.5 {
                let edge = 8.0f32.min(sw_ / 4.0).min(sh_ / 4.0);
                let inner = mx > sx + edge && mx < sx + sw_ - edge && my > sy + edge && my < sy + sh_ - edge;
                if inner {
                    let slide = if app.get_impress_text_edit_id() >= 0 { app.get_impress_text_edit_slide() as u32 } else { (app.get_impress_current_slide() - 1).max(0) as u32 };
                    let is_text = {
                        let s = state_ds2.borrow();
                        s.session.as_ref().map(|sess| sess.impress_text_is_empty(slide, id as u32).is_some() || sess.impress_is_table(slide, id as u32).unwrap_or(false)).unwrap_or(false)
                    };
                    if is_text {
                        app.invoke_impress_text_click(mx, my);
                        app.set_impress_text_selecting(true);
                        return;
                    }
                }
            }
        }
        // 编辑中点了边缘/其它：先退出编辑再走原拖动逻辑
        let was_editing = app.get_impress_text_edit_id() >= 0;
        if was_editing { app.invoke_impress_text_exit(); }
        let id = app.get_impress_sel_id();
        if id < 0 { return; }
        let start = geom::R {
            x: app.get_impress_sel_x(), y: app.get_impress_sel_y(),
            w: app.get_impress_sel_w(), h: app.get_impress_sel_h(),
            rot: app.get_impress_sel_rot(),
        };
        let (cx, cy) = geom::center(start);
        let (sn, cs) = start.rot.to_radians().sin_cos();
        let bw = start.w * cs.abs() + start.h * sn.abs();
        let bh = start.w * sn.abs() + start.h * cs.abs();
        let sw = app.get_impress_canvas_width_pt();
        let sh = app.get_impress_canvas_height_pt();
        let x0 = (cx - bw / 2.0).max(0.0);
        let y0 = (cy - bh / 2.0).max(0.0);
        let x1 = (cx + bw / 2.0).min(sw);
        let y1 = (cy + bh / 2.0).min(sh);
        let ghost_ok = x1 > x0 && y1 > y0 && sw > 0.0;
        let img_w = app.get_impress_viewport_image().size().width as f32;
        let px_scale = if sw > 0.0 && img_w > 0.0 { img_w / sw } else { 2.0 };
        app.set_impress_ghost_cx((x0 * px_scale).round() as i32);
        app.set_impress_ghost_cy((y0 * px_scale).round() as i32);
        app.set_impress_ghost_cw(((x1 - x0) * px_scale).round() as i32);
        app.set_impress_ghost_ch(((y1 - y0) * px_scale).round() as i32);
        eprintln!("[IMPRESS-DRAG] DOWN mode={} id={} rect=({:.1},{:.1},{:.1},{:.1}) rot={:.1} clip=({:.1},{:.1},{:.1},{:.1}) px_scale={:.2}",
            mode, id, start.x, start.y, start.w, start.h, start.rot, x0, y0, x1 - x0, y1 - y0, px_scale);
        let click_edit: Option<(f32, f32)> = if mode == "move" && !was_editing && !app.get_impress_master_mode() && !app.get_impress_sel_line() {
            let slide_t = (app.get_impress_current_slide() - 1).max(0) as u32;
            let is_text = {
                let s = state_ds2.borrow();
                s.session.as_ref().map(|sess| sess.impress_text_is_empty(slide_t, id as u32).is_some() || sess.impress_is_table(slide_t, id as u32).unwrap_or(false)).unwrap_or(false)
            };
            if is_text { Some((mx, my)) } else { None }
        } else { None };
        *drag_s.borrow_mut() = Some(ImpressDrag {
            click_edit,
            mode: mode.to_string(), start, m0: (mx, my), last: start, moved: false,
            clip: (x0, y0, x1 - x0, y1 - y0), ghost_ok,
            slide_idx: (app.get_impress_current_slide() - 1).max(0) as u32,
            shape_id: id as u32,
        });
    });

    let drag_m = impress_drag.clone();
    let app_weak_dm = app_weak.clone();
    app.on_impress_drag_moved(move |mx, my, shift, ctrl| {
        let app = match app_weak_dm.upgrade() { Some(a) => a, None => return };
        let mut g = drag_m.borrow_mut();
        let d = match g.as_mut() { Some(d) => d, None => return };
        let new = match d.mode.as_str() {
            "move" => Some(geom::compute_move(d.start, mx - d.m0.0, my - d.m0.1)),
            "rotate" => Some(geom::compute_rotate(d.start, d.m0, (mx, my), shift)),
            h => geom::compute_resize(d.start, h, d.m0, (mx, my), shift, ctrl, 8.0),
        };
        let r = match new { Some(r) => r, None => return };
        d.last = r;
        d.moved = true;
        app.set_impress_sel_x(r.x);
        app.set_impress_sel_y(r.y);
        app.set_impress_sel_w(r.w);
        app.set_impress_sel_h(r.h);
        app.set_impress_sel_rot(r.rot);
        // 替身：移动=快照平移；旋转=快照绕中心转；缩放=快照拉伸到新矩形（起始形状已旋转时不画替身）
        let (gx, gy, gw, gh, grot, show) = match d.mode.as_str() {
            "move" => (d.clip.0 + (r.x - d.start.x), d.clip.1 + (r.y - d.start.y), d.clip.2, d.clip.3, 0.0, d.ghost_ok),
            "rotate" => (d.clip.0, d.clip.1, d.clip.2, d.clip.3, r.rot - d.start.rot, d.ghost_ok),
            _ => if d.start.rot.abs() < 0.01 { (r.x, r.y, r.w, r.h, 0.0, d.ghost_ok) } else { (0.0, 0.0, 0.0, 0.0, 0.0, false) },
        };
        app.set_impress_ghost_x(gx);
        app.set_impress_ghost_y(gy);
        app.set_impress_ghost_w(gw);
        app.set_impress_ghost_h(gh);
        app.set_impress_ghost_rot(grot);
        app.set_impress_ghost_visible(show);
    });

    let drag_e = impress_drag.clone();
    let state_de = state.clone();
    let app_weak_de = app_weak.clone();
    app.on_impress_drag_ended(move || {
        let app = match app_weak_de.upgrade() { Some(a) => a, None => return };
        let d = match drag_e.borrow_mut().take() { Some(d) => d, None => return };
        app.set_impress_ghost_visible(false);
        if !d.moved {
            eprintln!("[IMPRESS-DRAG] UP 无移动，不提交");
            if let Some((cx, cy)) = d.click_edit {
                app.invoke_impress_text_click(cx, cy);
            }
            return;
        }
        if app.get_impress_master_mode() {
            let sel_row = app.get_impress_master_sel();
            let mi = app.get_impress_master_mi().row_data(sel_row as usize).unwrap_or(0) as u32;
            let li = app.get_impress_master_li().row_data(sel_row as usize).unwrap_or(-1);
            let lr = d.last;
            let rect = {
                let mut s = state_de.borrow_mut();
                match s.session.as_mut() {
                    Some(sess) => {
                        if d.mode != "rotate" {
                            sess.begin_edit();
                            let okr = sess.impress_master_set_shape_rect(mi, li, d.shape_id, lr.x, lr.y, lr.w, lr.h);
                            if okr { sess.commit_edit(); } else { sess.discard_edit(); }
                        } else {
                            // v320o: 母版里旋转手柄提交（只改这一份，不传播）
                            sess.begin_edit();
                            let okr = sess.impress_master_set_shape_rotation(mi, li, d.shape_id, lr.rot);
                            if okr { sess.commit_edit(); } else { sess.discard_edit(); }
                        }
                        sess.impress_master_query_shape_rect(mi, li, d.shape_id)
                    }
                    None => None,
                }
            };
            if let Some((qx, qy, qw, qh, qr)) = rect {
                app.set_impress_sel_x(qx);
                app.set_impress_sel_y(qy);
                app.set_impress_sel_w(qw);
                app.set_impress_sel_h(qh);
                app.set_impress_sel_rot(qr);
            }
            show_master_canvas(&app, &state_de, sel_row);
            refresh_master_thumb_one(&app, &state_de, sel_row);
            return;
        }
        let r = d.last;
        let ok = {
            let mut s = state_de.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            sess.begin_edit();
            let done = if d.mode == "rotate" {
                sess.impress_set_shape_rotation(d.slide_idx, d.shape_id, r.rot)
            } else {
                { let okr = sess.impress_set_shape_rect(d.slide_idx, d.shape_id, r.x, r.y, r.w, r.h); if okr { let _ = sess.impress_table_settle(d.slide_idx, d.shape_id); } okr }
            };
            if done { sess.commit_edit(); }
            done
        };
        eprintln!("[IMPRESS-DRAG] UP 提交 mode={} ok={} final=({:.1},{:.1},{:.1},{:.1}) rot={:.1}",
            d.mode, ok, r.x, r.y, r.w, r.h, r.rot);
        if ok {
            app.invoke_impress_render_viewport(0.0);
        }
        // 用后端实际值回写选中框（失败时恢复到起始矩形）
        let q = {
            let mut s = state_de.borrow_mut();
            s.session.as_mut().and_then(|sess| sess.impress_query_shape_rect(d.slide_idx, d.shape_id))
        };
        let (qx, qy, qw, qh, qr) = q.unwrap_or((d.start.x, d.start.y, d.start.w, d.start.h, d.start.rot));
        app.set_impress_sel_x(qx);
        app.set_impress_sel_y(qy);
        app.set_impress_sel_w(qw);
        app.set_impress_sel_h(qh);
        app.set_impress_sel_rot(qr);
    });

    // -- Impress 直线端点拖动：拖端点/拖线身，松手提交一次 --
    struct LineDrag {
        mode: String,
        p1: (f32, f32),
        p2: (f32, f32),
        m0: (f32, f32),
        moved: bool,
        slide_idx: u32,
        shape_id: u32,
    }
    let line_drag: Rc<RefCell<Option<LineDrag>>> = Rc::new(RefCell::new(None));

    let ld_s = line_drag.clone();
    let app_weak_lds = app_weak.clone();
    app.on_impress_line_drag_started(move |mode, mx, my| {
        let app = match app_weak_lds.upgrade() { Some(a) => a, None => return };
        let id = app.get_impress_sel_id();
        if id < 0 { return; }
        eprintln!("[IMPRESS-LINE] DOWN mode={} id={}", mode, id);
        *ld_s.borrow_mut() = Some(LineDrag {
            mode: mode.to_string(),
            p1: (app.get_impress_line_x1(), app.get_impress_line_y1()),
            p2: (app.get_impress_line_x2(), app.get_impress_line_y2()),
            m0: (mx, my),
            moved: false,
            slide_idx: (app.get_impress_current_slide() - 1).max(0) as u32,
            shape_id: id as u32,
        });
    });

    let ld_m = line_drag.clone();
    let app_weak_ldm = app_weak.clone();
    app.on_impress_line_drag_moved(move |mx, my| {
        let app = match app_weak_ldm.upgrade() { Some(a) => a, None => return };
        let mut g = ld_m.borrow_mut();
        let d = match g.as_mut() { Some(d) => d, None => return };
        let (dx, dy) = (mx - d.m0.0, my - d.m0.1);
        d.moved = true;
        if d.mode != "p2" {
            app.set_impress_line_x1(d.p1.0 + dx);
            app.set_impress_line_y1(d.p1.1 + dy);
        }
        if d.mode != "p1" {
            app.set_impress_line_x2(d.p2.0 + dx);
            app.set_impress_line_y2(d.p2.1 + dy);
        }
    });

    let ld_e = line_drag.clone();
    let state_lde = state.clone();
    let app_weak_lde = app_weak.clone();
    app.on_impress_line_drag_ended(move || {
        let app = match app_weak_lde.upgrade() { Some(a) => a, None => return };
        let d = match ld_e.borrow_mut().take() { Some(d) => d, None => return };
        let (x1, y1, x2, y2) = (
            app.get_impress_line_x1(), app.get_impress_line_y1(),
            app.get_impress_line_x2(), app.get_impress_line_y2(),
        );
        let len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
        let mut committed = false;
        if d.moved && len >= 2.0 {
            let ok = {
                let mut s = state_lde.borrow_mut();
                let sess = match s.session.as_mut() { Some(v) => v, None => return };
                sess.begin_edit();
                let done = sess.impress_set_line_endpoints(d.slide_idx, d.shape_id, x1, y1, x2, y2);
                if done { sess.commit_edit(); }
                done

            };
            eprintln!("[IMPRESS-LINE] UP 提交 mode={} ok={} ({:.1},{:.1})->({:.1},{:.1})", d.mode, ok, x1, y1, x2, y2);
            committed = ok;
        } else {
            eprintln!("[IMPRESS-LINE] UP 不提交 moved={} len={:.1}", d.moved, len);
        }
        if committed {
            app.invoke_impress_render_viewport(0.0);
        }
        // 用后端实际值回写端点与选中框（未提交时恢复原状）
        let q = {
            let mut s = state_lde.borrow_mut();
            s.session.as_mut().and_then(|sess| {
                let r = sess.impress_query_shape_rect(d.slide_idx, d.shape_id)?;
                let li = sess.impress_query_line_info(d.slide_idx, d.shape_id)?;
                Some((r, li))
            })
        };
        if let Some(((qx, qy, qw, qh, qr), (_, fh, fv))) = q {
            let (a, b, c, e) = geom::line_endpoints(qx, qy, qw, qh, qr, fh, fv);
            app.set_impress_line_x1(a);
            app.set_impress_line_y1(b);
            app.set_impress_line_x2(c);
            app.set_impress_line_y2(e);
            app.set_impress_sel_x(qx);
            app.set_impress_sel_y(qy);
            app.set_impress_sel_w(qw);
            app.set_impress_sel_h(qh);
            app.set_impress_sel_rot(qr);
        }
    });

    // ---- v220：幻灯片新建 / 复制 / 删除 ----
    // 重读页数与各页位置，设置当前页，重绘主视图与全部缩略图。
    fn impress_reload_slides_v220(app: &AppWindow, state: &Rc<RefCell<AppState>>, new_current: i32) {
        let size = {
            let mut s = state.borrow_mut();
            s.session.as_mut().and_then(|sess| sess.get_impress_document_size())
        };
        if let Some((cw, ch, cnt, slides)) = size {
            app.set_impress_canvas_width_pt(cw);
            app.set_impress_canvas_height_pt(ch);
            app.set_impress_slide_count(cnt.max(1));
            state.borrow_mut().impress_slides = slides;
        }
        let cnt = app.get_impress_slide_count().max(1);
        app.set_impress_sel_id(-1);
        app.set_impress_current_slide(new_current.max(1).min(cnt));
        app.set_impress_needs_render(true);
        refresh_all_impress_thumbnails(app, state);
    }

    // 撤销/重做之后：页数与界面不一致时同步。
    fn impress_sync_slide_count_v220(app: &AppWindow, state: &Rc<RefCell<AppState>>) {
        let cnt_now = {
            let mut s = state.borrow_mut();
            s.session.as_mut().and_then(|sess| sess.get_impress_document_size()).map(|v| v.2)
        };
        if let Some(c) = cnt_now {
            if c.max(1) != app.get_impress_slide_count() {
                impress_reload_slides_v220(app, state, app.get_impress_current_slide());
            }
        }
    }

    {
        let app_weak = app_weak.clone();
        let state = state.clone();
        app.on_impress_slide_new(move || {
            let app = match app_weak.upgrade() { Some(a) => a, None => return };
            if app.get_impress_text_edit_id() >= 0 { app.invoke_impress_text_exit(); }
            let cur = (app.get_impress_current_slide() - 1).max(0) as u32;
            let new_idx: i32 = {
                let mut s = state.borrow_mut();
                let sess = match s.session.as_mut() { Some(v) => v, None => return };
                if sess.get_doc_type() != "impress" { return; }
                sess.begin_edit();
                let r = sess.impress_insert_slide(cur);
                if r < 0 { sess.discard_edit(); } else { sess.commit_edit(); }
                r
            };
            eprintln!("[IMPRESS-SLIDE] new after={} -> {}", cur, new_idx);
            if new_idx < 0 { app.set_status_message("新建幻灯片失败".into()); return; }
            impress_reload_slides_v220(&app, &state, new_idx + 1);
        });
    }
    {
        fn parse_layout_names_v320(js: &str) -> Vec<String> {
            let key = "\"name\":\"";
            let mut out = Vec::new();
            let mut rest = js;
            while let Some(p) = rest.find(key) {
                rest = &rest[p + key.len()..];
                let mut name = String::new();
                let mut chars = rest.char_indices();
                let mut end = rest.len();
                while let Some((i, c)) = chars.next() {
                    if c == '\\' {
                        if let Some((_, n)) = chars.next() { name.push(n); }
                        continue;
                    }
                    if c == '"' { end = i + 1; break; }
                    name.push(c);
                }
                out.push(name);
                rest = &rest[end..];
            }
            out
        }
        let app_weak = app_weak.clone();
        let state = state.clone();
        app.on_impress_layout_menu_prepare(move || {
            let app = match app_weak.upgrade() { Some(a) => a, None => return };
            let cur = (app.get_impress_current_slide() - 1).max(0) as u32;
            let (names, li) = {
                let mut s = state.borrow_mut();
                let sess = match s.session.as_mut() { Some(v) => v, None => return };
                if sess.get_doc_type() != "impress" { return; }
                let js = sess.impress_list_layouts(cur).unwrap_or_default();
                (parse_layout_names_v320(&js), sess.impress_slide_layout(cur))
            };
            eprintln!("[IMPRESS-SLIDE] layout menu slide={} cur={} count={}", cur, li, names.len());
            let model: Vec<slint::SharedString> = names.into_iter().map(|n| n.into()).collect();
            app.set_impress_layout_names(slint::ModelRc::new(slint::VecModel::from(model)));
            app.set_impress_layout_cur(li);
        });
    }
    {
        let app_weak = app_weak.clone();
        let state = state.clone();
        app.on_impress_layout_pick(move |idx| {
            let app = match app_weak.upgrade() { Some(a) => a, None => return };
            if app.get_impress_text_edit_id() >= 0 { app.invoke_impress_text_exit(); }
            let cur = (app.get_impress_current_slide() - 1).max(0) as u32;
            let ok: bool = {
                let mut s = state.borrow_mut();
                let sess = match s.session.as_mut() { Some(v) => v, None => return };
                if sess.get_doc_type() != "impress" { return; }
                sess.begin_edit();
                let r = sess.impress_set_slide_layout(cur, idx.max(0) as u32);
                if r { sess.commit_edit(); } else { sess.discard_edit(); }
                r
            };
            eprintln!("[IMPRESS-SLIDE] pick layout slide={} layout={} ok={}", cur, idx, ok);
            if !ok { app.set_status_message("set layout failed".into()); return; }
            impress_reload_slides_v220(&app, &state, cur as i32 + 1);
        });
    }
    {
        let app_weak = app_weak.clone();
        let state = state.clone();
        app.on_impress_slide_reset_layout(move || {
            let app = match app_weak.upgrade() { Some(a) => a, None => return };
            if app.get_impress_text_edit_id() >= 0 { app.invoke_impress_text_exit(); }
            let cur = (app.get_impress_current_slide() - 1).max(0) as u32;
            let ok: bool = {
                let mut s = state.borrow_mut();
                let sess = match s.session.as_mut() { Some(v) => v, None => return };
                if sess.get_doc_type() != "impress" { return; }
                let li = sess.impress_slide_layout(cur);
                if li < 0 { false } else {
                    sess.begin_edit();
                    let r = sess.impress_set_slide_layout(cur, li as u32);
                    if r { sess.commit_edit(); } else { sess.discard_edit(); }
                    r
                }
            };
            eprintln!("[IMPRESS-SLIDE] reset layout slide={} ok={}", cur, ok);
            if !ok { app.set_status_message("重置版式失败".into()); return; }
            impress_reload_slides_v220(&app, &state, cur as i32 + 1);
        });
    }
    {
        let app_weak = app_weak.clone();
        let state = state.clone();
        app.on_impress_slide_duplicate(move |idx| {
            let app = match app_weak.upgrade() { Some(a) => a, None => return };
            if app.get_impress_text_edit_id() >= 0 { app.invoke_impress_text_exit(); }
            let new_idx: i32 = {
                let mut s = state.borrow_mut();
                let sess = match s.session.as_mut() { Some(v) => v, None => return };
                if sess.get_doc_type() != "impress" { return; }
                sess.begin_edit();
                let r = sess.impress_duplicate_slide(idx.max(0) as u32);
                if r < 0 { sess.discard_edit(); } else { sess.commit_edit(); }
                r
            };
            eprintln!("[IMPRESS-SLIDE] duplicate {} -> {}", idx, new_idx);
            if new_idx < 0 { app.set_status_message("复制幻灯片失败".into()); return; }
            impress_reload_slides_v220(&app, &state, new_idx + 1);
        });
    }
    {
        let app_weak = app_weak.clone();
        let state = state.clone();
        app.on_impress_slide_delete(move |idx| {
            let app = match app_weak.upgrade() { Some(a) => a, None => return };
            if app.get_impress_slide_count() <= 1 {
                app.set_status_message("至少保留一张幻灯片".into());
                return;
            }
            if app.get_impress_text_edit_id() >= 0 { app.invoke_impress_text_exit(); }
            let ok: bool = {
                let mut s = state.borrow_mut();
                let sess = match s.session.as_mut() { Some(v) => v, None => return };
                if sess.get_doc_type() != "impress" { return; }
                sess.begin_edit();
                let r = sess.impress_delete_slide(idx.max(0) as u32);
                if r { sess.commit_edit(); } else { sess.discard_edit(); }
                r
            };
            eprintln!("[IMPRESS-SLIDE] delete {} -> ok={}", idx, ok);
            if !ok { app.set_status_message("删除幻灯片失败".into()); return; }
            impress_reload_slides_v220(&app, &state, idx + 1);
        });
    }

    // ---- v220d：键盘 Delete 删除选中形状 ----
    {
        let app_weak = app_weak.clone();
        let state = state.clone();
        app.on_impress_shape_delete(move || {
            let app = match app_weak.upgrade() { Some(a) => a, None => return };
            let id = app.get_impress_sel_id();
            if id < 0 { return; }
            if app.get_impress_master_mode() {
                let row = app.get_impress_master_sel();
                let mi = app.get_impress_master_mi().row_data(row as usize).unwrap_or(0) as u32;
                let li = app.get_impress_master_li().row_data(row as usize).unwrap_or(-1);
                let okm = {
                    let mut s = state.borrow_mut();
                    match s.session.as_mut() {
                        Some(sess) => {
                            sess.begin_edit();
                            let r = sess.impress_master_delete_shape(mi, li, id as u32);
                            if r { sess.commit_edit(); } else { sess.discard_edit(); }
                            r
                        }
                        None => false,
                    }
                };
                eprintln!("[MASTER-SHAPE] delete id={} -> ok={}", id, okm);
                if okm {
                    app.set_impress_sel_id(-1);
                    show_master_canvas(&app, &state, row);
                    refresh_master_thumb_one(&app, &state, row);
                }
                return;
            }
            let idx = (app.get_impress_current_slide() - 1).max(0) as u32;
            let ok = {
                let mut s = state.borrow_mut();
                let sess = match s.session.as_mut() { Some(v) => v, None => return };
                if sess.get_doc_type() != "impress" { return; }
                sess.begin_edit();
                let r = sess.impress_delete_shape(idx, id as u32);
                if r { sess.commit_edit(); } else { sess.discard_edit(); }
                r
            };
            eprintln!("[IMPRESS-SHAPE] delete id={} -> ok={}", id, ok);
            if !ok { return; }
            app.set_impress_sel_id(-1);
            app.invoke_impress_render_viewport(0.0);
            refresh_impress_thumbnail(&app, &state, idx as usize);
        });
    }

    // -- Impress 撤销/重做：整文档快照换回，之后重查选中形状并重渲染 --
    let state_iu = state.clone();
    let app_weak_iu = app_weak.clone();
    app.on_impress_undo_redo(move |is_redo| {
        if let Some(app_m) = app_weak_iu.upgrade() {
            if app_m.get_impress_master_mode() {
                let ok = {
                    let mut s = state_iu.borrow_mut();
                    match s.session.as_mut() {
                        Some(sess) => (if is_redo { sess.redo() } else { sess.undo() }).is_some(),
                        None => false,
                    }
                };
                if ok {
                    let row = app_m.get_impress_master_sel();
                    refresh_master_thumbs(&app_m, &state_iu);
                    // v320o: 撤销后行数可能变少，选中行夹到有效范围
                    let cnt = app_m.get_impress_master_li().row_count() as i32;
                    let row = if cnt <= 0 { 0 } else { row.min(cnt - 1).max(0) };
                    app_m.set_impress_master_sel(row);
                    show_master_canvas(&app_m, &state_iu, row);
                    app_m.set_impress_sel_id(-1);
                    refresh_master_can_delete(&app_m, &state_iu);
                }
                return;
            }
        }

        let app = match app_weak_iu.upgrade() { Some(a) => a, None => return };
        let idx = (app.get_impress_current_slide() - 1).max(0) as u32;
        let sel_id = app.get_impress_sel_id();
        let info = {
            let mut s = state_iu.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            if sess.get_doc_type() != "impress" { return; }
            let res = if is_redo { sess.redo() } else { sess.undo() };
            eprintln!("[IMPRESS-UNDO] is_redo={} ok={}", is_redo, res.is_some());
            if res.is_none() { return; }
            if sel_id >= 0 {
                Some((
                    sess.impress_query_shape_rect(idx, sel_id as u32),
                    sess.impress_query_line_info(idx, sel_id as u32),
                ))
            } else { None }
        };
        if let Some((rect, li)) = info {
            match rect {
                Some((qx, qy, qw, qh, qr)) => {
                    app.set_impress_sel_x(qx);
                    app.set_impress_sel_y(qy);
                    app.set_impress_sel_w(qw);
                    app.set_impress_sel_h(qh);
                    app.set_impress_sel_rot(qr);
                    match li {
                        Some((true, fh, fv)) => {
                            let (a, b, c, e) = geom::line_endpoints(qx, qy, qw, qh, qr, fh, fv);
                            app.set_impress_line_x1(a);
                            app.set_impress_line_y1(b);
                            app.set_impress_line_x2(c);
                            app.set_impress_line_y2(e);
                            app.set_impress_sel_line(true);
                        }
                        _ => app.set_impress_sel_line(false),
                    }
                }
                None => app.set_impress_sel_id(-1),
            }
        }
        app.invoke_impress_render_viewport(0.0);
        // v220: 撤销/重做可能改变页数，同步页数与缩略图
        impress_sync_slide_count_v220(&app, &state_iu);
        // v320o: 只刷新当前页和前后各一页的缩略图
        {
            let cnt = app.get_impress_slide_count().max(0) as usize;
            let c = idx as usize;
            for i in c.saturating_sub(1)..=(c + 1) {
                if i < cnt { refresh_impress_thumbnail(&app, &state_iu, i); }
            }
        }
    });

    // -- Impress 叠放次序（右键菜单）：包一次编辑事务，可撤销 --
    let state_zo = state.clone();
    let app_weak_zo = app_weak.clone();
    app.on_impress_zorder(move |op| {
        let app = match app_weak_zo.upgrade() { Some(a) => a, None => return };
        let sel = app.get_impress_sel_id();
        if sel < 0 { return; }
        let idx = (app.get_impress_current_slide() - 1).max(0) as u32;
        let ok = {
            let mut s = state_zo.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            if sess.get_doc_type() != "impress" { return; }
            sess.begin_edit();
            let done = sess.impress_shape_order(idx, sel as u32, op.as_str());
            if done { sess.commit_edit(); }
            done
        };
        eprintln!("[IMPRESS-ZORDER] op={} id={} ok={}", op, sel, ok);
        if ok { app.invoke_impress_render_viewport(0.0); }
    });

    // -- Impress 形状菜单：第一次打开时生成缩略图模型（之后复用）--
    let state_sm = state.clone();
    let app_weak_sm = app_weak.clone();
    app.on_impress_shape_menu_prepare(move || {
        use slint::Model;
        let app = match app_weak_sm.upgrade() { Some(a) => a, None => return };
        if app.get_impress_shape_groups().row_count() > 0 { return; }
        let mut groups: Vec<ShapeGroup> = Vec::new();
        {
            let mut s = state_sm.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            for (title, items) in shape_catalog::GROUPS.iter() {
                let mut v: Vec<ShapeEntry> = Vec::new();
                for (prst, name) in items.iter() {
                    let path = sess.impress_shape_preview_path(prst, 24.0, 24.0);
                    let is_line = prst.starts_with("line") || prst.contains("Connector");
                    v.push(ShapeEntry {
                        prst: (*prst).into(),
                        name: (*name).into(),
                        path: path.into(),
                        is_line,
                    });
                }
                groups.push(ShapeGroup {
                    title: (*title).into(),
                    items: slint::ModelRc::new(slint::VecModel::from(v)),
                });
            }
        }
        eprintln!("[IMPRESS-SHAPEMENU] 生成 {} 组缩略图", groups.len());
        app.set_impress_shape_groups(slint::ModelRc::new(slint::VecModel::from(groups)));
    });

    // -- Impress 插入形状：绘制覆盖层松手后提交（一次绘制一条撤销记录）--
    let state_dc = state.clone();
    let app_weak_dc = app_weak.clone();
    // -- Impress 插入图片：选文件 → 等比缩放(不超过幻灯片60%)居中 → 事务插入 → 选中 --
    let app_weak_img = app_weak.clone();
    let state_img = state.clone();
    app.on_impress_insert_image_clicked(move || {
        let app = match app_weak_img.upgrade() { Some(a) => a, None => return };
        let file = rfd::FileDialog::new()
            .add_filter("图片", &["png", "jpg", "jpeg"])
            .pick_file();
        let Some(path) = file else { return; };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => { app.set_status_message(format!("读取图片失败: {}", e).into()); return; }
        };
        let (iw, ih) = match doc_core::TileCacheRust::impress_image_dimensions(&bytes) {
            Some(v) if v.0 > 0 && v.1 > 0 => v,
            _ => { app.set_status_message("无法识别图片（仅支持 PNG/JPEG）".into()); return; }
        };
        let idx = (app.get_impress_current_slide() - 1).max(0) as u32;
        if app.get_impress_master_mode() {
            let m_row = app.get_impress_master_sel();
            let m_mi = app.get_impress_master_mi().row_data(m_row as usize).unwrap_or(0) as u32;
            let m_li = app.get_impress_master_li().row_data(m_row as usize).unwrap_or(-1);
            let (sw, sh) = (app.get_impress_canvas_width_pt(), app.get_impress_canvas_height_pt());
            let (sw, sh) = if sw > 0.0 && sh > 0.0 { (sw, sh) } else { (960.0f32, 540.0f32) };
            let scale = ((sw * 0.6) / iw as f32).min((sh * 0.6) / ih as f32);
            let (w, h) = (iw as f32 * scale, ih as f32 * scale);
            let (x, y) = ((sw - w) / 2.0, (sh - h) / 2.0);
            let new_id: Option<u32> = {
                let mut s = state_img.borrow_mut();
                let sess = match s.session.as_mut() { Some(v) => v, None => return };
                sess.begin_edit();
                let id = sess.impress_master_insert_image(m_mi, m_li, &bytes, x, y, w, h);
                if id < 0 { sess.discard_edit(); None } else { sess.commit_edit(); Some(id as u32) }
            };
            match new_id {
                None => app.set_status_message("插入图片失败".into()),
                Some(id) => {
                    show_master_canvas(&app, &state_img, m_row);
                    refresh_master_thumb_one(&app, &state_img, m_row);
                    let q = {
                        let mut s = state_img.borrow_mut();
                        s.session.as_mut().and_then(|sess| sess.impress_master_query_shape_rect(m_mi, m_li, id))
                    };
                    if let Some((qx, qy, qw, qh, qr)) = q {
                        app.set_impress_sel_x(qx);
                        app.set_impress_sel_y(qy);
                        app.set_impress_sel_w(qw);
                        app.set_impress_sel_h(qh);
                        app.set_impress_sel_rot(qr);
                        app.set_impress_sel_line(false);
                        app.set_impress_sel_id(id as i32);
                    }
                }
            }
            return;
        }
        let (sw, sh) = {
            let st = state_img.borrow();
            st.impress_slides.get(idx as usize).map(|v| (v.1, v.2)).unwrap_or((960.0f32, 540.0f32))
        };
        let scale = ((sw * 0.6) / iw as f32).min((sh * 0.6) / ih as f32);
        let (w, h) = (iw as f32 * scale, ih as f32 * scale);
        let (x, y) = ((sw - w) / 2.0, (sh - h) / 2.0);
        let new_id: Option<u32> = {
            let mut s = state_img.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            if sess.get_doc_type() != "impress" { return; }
            sess.begin_edit();
            let id = sess.impress_insert_image(idx, &bytes, x, y, w, h);
            if id < 0 { sess.discard_edit(); None } else { sess.commit_edit(); Some(id as u32) }
        };
        eprintln!("[IMPRESS-IMAGE] {}x{} -> rect=({:.1},{:.1},{:.1},{:.1}) id={:?}", iw, ih, x, y, w, h, new_id);
        match new_id {
            None => app.set_status_message("插入图片失败".into()),
            Some(id) => {
                app.invoke_impress_render_viewport(0.0);
                let q = {
                    let mut s = state_img.borrow_mut();
                    s.session.as_mut().and_then(|sess| sess.impress_query_shape_rect(idx, id))
                };
                if let Some((qx, qy, qw, qh, qr)) = q {
                    app.set_impress_sel_x(qx);
                    app.set_impress_sel_y(qy);
                    app.set_impress_sel_w(qw);
                    app.set_impress_sel_h(qh);
                    app.set_impress_sel_rot(qr);
                    app.set_impress_sel_line(false);
                    app.set_impress_sel_id(id as i32);
                }
            }
        }
    });

    // -- Impress 插入图表对话框 V2（v221）：缩略图为矢量线稿(零渲染)，大预览延后渲染并缓存 --
    let c2_prev_cache: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<(usize, usize), slint::Image>>> = Default::default();
    let c2_timer = std::rc::Rc::new(slint::Timer::default());
    let app_weak_c2s = app_weak.clone();
    let state_c2s = state.clone();
    app.on_chart2_select(move |cat, ty| {
        let app = match app_weak_c2s.upgrade() { Some(a) => a, None => return };
        let cat = (cat.max(0) as usize).min(crate::chart_registry::CATS.len() - 1);
        let defs = crate::chart_registry::types_in_cat(cat);
        if defs.is_empty() { return; }
        let ty = (ty.max(0) as usize).min(defs.len() - 1);
        let cats: Vec<slint::SharedString> = crate::chart_registry::CATS.iter().map(|s| (*s).into()).collect();
        app.set_chart2_cats(slint::ModelRc::new(slint::VecModel::from(cats)));
        let names: Vec<slint::SharedString> = defs.iter().map(|d| d.name.into()).collect();
        app.set_chart2_type_names(slint::ModelRc::new(slint::VecModel::from(names)));
        let ids: Vec<slint::SharedString> = defs.iter().map(|d| d.id.into()).collect();
        app.set_chart2_type_ids(slint::ModelRc::new(slint::VecModel::from(ids)));
        app.set_chart2_type_title(defs[ty].name.into());
        let pkey = (cat, ty);
        if let Some(im) = c2_prev_cache.borrow().get(&pkey).cloned() {
            app.set_chart2_preview(im);
            return;
        }
        app.set_chart2_preview(slint::Image::default());
        let def = defs[ty];
        let app_weak2 = app_weak_c2s.clone();
        let state2 = state_c2s.clone();
        let cache2 = c2_prev_cache.clone();
        c2_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(30), move || {
            let app = match app_weak2.upgrade() { Some(a) => a, None => return };
            if app.get_chart2_cat() as usize != cat || app.get_chart2_type() as usize != ty { return; }
            let t0 = std::time::Instant::now();
            let is_stat = crate::chart_registry::is_stat_def(def);
            let jp = if is_stat { crate::chart_registry::stat_json(def) } else { crate::chart_registry::chart_json(def, 640, 400, false) };
            let im = {
                let mut s = state2.borrow_mut();
                match s.session.as_mut().and_then(|sess| if is_stat { sess.render_stat_chart_preview(&jp, 640, 400) } else { sess.render_chart_preview(&jp) }) {
                    Some((rgba, w, h)) => slint::Image::from_rgba8(
                        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w, h)),
                    None => slint::Image::default(),
                }
            };
            cache2.borrow_mut().insert((cat, ty), im.clone());
            app.set_chart2_preview(im);
            eprintln!("[CHART2] 预览渲染 cat={} type={} 用时{:?}", cat, ty, t0.elapsed());
        });
    });

    // -- Impress 插入图表（v221 阶段1）：假数据 → 居中(60%) → 事务插入 → 选中 --
    let app_weak_cins = app_weak.clone();
    let state_cins = state.clone();
    let sheet_idx_c2 = current_sheet_idx.clone();
    app.on_chart2_confirm(move |c2_cat, c2_ty| {
        let app = match app_weak_cins.upgrade() { Some(a) => a, None => return };
        // ── Calc 分支：选区真实数据 → 普通图(Rhai insert_chart)/统计图(C-ABI,pt 落点) ──
        {
            let is_writer = state_cins.borrow().session.as_ref().map(|x| x.get_doc_type() == "writer").unwrap_or(false);
            if is_writer {
                let (p, o) = { let s = state_cins.borrow(); (s.writer_cursor_para, s.writer_cursor_offset) };
                if p < 0 || o < 0 { app.set_status_message("请先在正文中点击要插入图表的位置".into()); return; }
                let page_w: f32 = app.get_writer_doc_width_pt() as f32;
                let w = (page_w - 144.0).clamp(200.0, 432.0);
                let h = w * 0.5625;
                let is_stat = crate::chart_registry::is_stat(c2_cat, c2_ty);
                let json = match crate::chart_registry::insert_json(c2_cat, c2_ty) { Some(j) => j, None => return };
                let res = {
                    let s = state_cins.borrow();
                    s.session.as_ref().map(|x| x.writer_insert_chart(p as u32, o as u32, &json, w, h, is_stat))
                };
                match res {
                    Some(Ok(())) => {
                        {
                            let mut s = state_cins.borrow_mut();
                            if o == 0 { s.writer_cursor_para += 1; }
                            writer_refresh_canvas(&app, &mut s);
                        }
                        app.set_writer_needs_render(true);
                        app.set_status_message("已插入图表".into());
                    }
                    Some(Err(e)) => app.set_status_message(format!("插入图表失败: {}", e).into()),
                    None => {}
                }
                return;
            }
        }
        let is_calc = state_cins.borrow().session.as_ref().map(|x| x.get_doc_type() == "calc").unwrap_or(false);
        if is_calc {
            let cat_i = (c2_cat.max(0) as usize).min(crate::chart_registry::CATS.len() - 1);
            let defs = crate::chart_registry::types_in_cat(cat_i);
            if defs.is_empty() { return; }
            let def = defs[(c2_ty.max(0) as usize).min(defs.len() - 1)];
            let sheet_idx = sheet_idx_c2.get();
            let mut s = state_cins.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };

            // 选区：多格取矩形；单格(临时)从该格向右下取最多 501 行 x 13 列
            let sr = app.get_calc_selected_row().max(0) as u32;
            let sc = app.get_calc_selected_col().max(0) as u32;
            let (er, ec) = (app.get_calc_sel_end_row(), app.get_calc_sel_end_col());
            let multi = er >= 0 && ec >= 0 && (er as u32 != sr || ec as u32 != sc);
            let (r1, c1, mut r2, mut c2) = if multi {
                (sr.min(er as u32), sc.min(ec as u32), sr.max(er as u32), sc.max(ec as u32))
            } else {
                // 单格：取当前数据区(Excel CurrentRegion)；探测不到则回退为向右下读固定范围
                match crate::ribbon::calc_data::expand_to_region(sess, sheet_idx, sr, sc, "chart-insert") {
                    Some((a, b, c, d)) => (a, b, c, d),
                    None => (sr, sc, sr + crate::chart_registry::MAX_POINTS as u32, sc + crate::chart_registry::MAX_SERIES as u32),
                }
            };
            r2 = r2.min(r1 + crate::chart_registry::MAX_POINTS as u32);
            c2 = c2.min(c1 + crate::chart_registry::MAX_SERIES as u32);
            let data = sess.get_range_json(sheet_idx, r1, c1, r2, c2)
                .and_then(|j| crate::chart_registry::range_json_to_matrix(&j))
                .and_then(|m| crate::chart_registry::extract_chart_data(&m));
            let data = match data {
                Some(d) => d,
                None => { app.set_status_message("插入图表失败：所选区域没有可用于绘图的数据".into()); return; }
            };

            // 落点：视口中心(逻辑 px) -> pt。px = pt*96/72 - scroll + 表头(30,18)
            let (cw, ch) = (420.0f32, 260.0f32);
            let k = 96.0f32 / 72.0;
            let scroll_x = app.get_calc_scroll_x();
            let scroll_y = app.get_calc_scroll_y();
            let vw_px = app.get_calc_view_width() as f32;
            let vh_px = app.get_calc_view_height() as f32;
            let x_pt = (((vw_px / 2.0 - 30.0 + scroll_x) / k) - cw / 2.0).max(0.0);
            let y_pt = (((vh_px / 2.0 - 18.0 + scroll_y) / k) - ch / 2.0).max(0.0);

            let chart_id: u64 = if crate::chart_registry::is_stat_def(def) {
                match crate::chart_registry::stat_json_real(def, &data, None) {
                    Some(j) => sess.insert_calc_stat_chart(sheet_idx, &j, x_pt, y_pt, cw, ch).unwrap_or(0),
                    None => 0,
                }
            } else {
                match crate::chart_registry::chart_json_real(def, &data, cw, ch, None, false) {
                    Some(j) => {
                        let ar = sess.get_row_at_offset(sheet_idx, y_pt).max(0) as u32;
                        let ac = sess.get_col_at_offset(sheet_idx, x_pt).max(0) as u32;
                        let esc = j.replace('\\', "\\\\").replace('"', "\\\"");
                        let script = format!("insert_chart({}, {}, {}, \"{}\")", sheet_idx, ar, ac, esc);
                        let res = sess.execute_script(&script);
                        eprintln!("[CHART2-CALC] insert_chart result={}", res);
                        serde_json::from_str::<serde_json::Value>(&res).ok()
                            .filter(|v| v.get("ok").and_then(|o| o.as_bool()) == Some(true))
                            .and_then(|v| v.get("return_value").and_then(|rv| rv.as_i64()))
                            .filter(|id| *id > 0).map(|id| id as u64).unwrap_or(0)
                    }
                    None => 0,
                }
            };
            if chart_id == 0 {
                app.set_status_message("插入图表失败".into());
                return;
            }

            // 滚动范围 + 重绘(按当前滚动位置)
            app.set_calc_sheet_total_width((sess.get_sheet_total_width(sheet_idx)) * (96.0 / 72.0));
            app.set_calc_sheet_total_height((sess.get_sheet_total_height(sheet_idx)) * (96.0 / 72.0));
            let scale = app.window().scale_factor();
            let (vw, vh) = (vw_px as u32, vh_px as u32);
            if vw > 0 && vh > 0 {
                let buf = sess.render_sheet_viewport(scroll_x, scroll_y, vw, vh);
                if !buf.is_empty() {
                    let (pw, ph) = doc_core::calc_phys_size(vw, vh);
                    let pb = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&buf, pw, ph);
                    app.set_calc_render_scroll_x(scroll_x);
                    app.set_calc_render_scroll_y(scroll_y);
                    app.set_calc_viewport_image(slint::Image::from_rgba8(pb));
                    app.window().request_redraw(); // transform 层不会自动重绘
                }
            }

            refresh_float_objs(&app, &mut s, sheet_idx);
            for w in &data.warnings { eprintln!("[CHART2-CALC] {}", w); }
            if let Some(w) = data.warnings.first() { app.set_status_message(w.as_str().into()); }
            eprintln!("[CHART2-CALC] 插入 id={} sel=({},{})-({},{})", chart_id, r1, c1, r2, c2);
            return;
        }
        let idx = (app.get_impress_current_slide() - 1).max(0) as u32;
        let (sw, sh) = {
            let st = state_cins.borrow();
            st.impress_slides.get(idx as usize).map(|v| (v.1, v.2)).unwrap_or((960.0f32, 540.0f32))
        };
        let (w, h) = (sw * 0.6, sh * 0.6);
        let (x, y) = ((sw - w) / 2.0, (sh - h) / 2.0);
        let is_stat = crate::chart_registry::is_stat(c2_cat, c2_ty);
        let chart_json_owned = match crate::chart_registry::insert_json(c2_cat, c2_ty) { Some(s) => s, None => return };
        let chart_json = chart_json_owned.as_str();
        let new_id: Option<u32> = {
            let mut s = state_cins.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            if sess.get_doc_type() != "impress" { return; }
            sess.begin_edit();
            let id = if is_stat { sess.impress_insert_stat_chart(idx, chart_json, x, y, w, h) } else { sess.impress_insert_chart(idx, chart_json, x, y, w, h) };
            if id < 0 { sess.discard_edit(); None } else { sess.commit_edit(); Some(id as u32) }
        };
        eprintln!("[IMPRESS-CHART] rect=({:.1},{:.1},{:.1},{:.1}) id={:?}", x, y, w, h, new_id);
        match new_id {
            None => app.set_status_message("插入图表失败".into()),
            Some(id) => {
                app.invoke_impress_render_viewport(0.0);
                let q = {
                    let mut s = state_cins.borrow_mut();
                    s.session.as_mut().and_then(|sess| sess.impress_query_shape_rect(idx, id))
                };
                if let Some((qx, qy, qw, qh, qr)) = q {
                    app.set_impress_sel_x(qx);
                    app.set_impress_sel_y(qy);
                    app.set_impress_sel_w(qw);
                    app.set_impress_sel_h(qh);
                    app.set_impress_sel_rot(qr);
                    app.set_impress_sel_line(false);
                    app.set_impress_sel_id(id as i32);
                }
            }
        }
    });

    // -- Impress 文本框编辑状态（2b）--
    #[derive(Default)]
    struct ImpressTextEd {
        cell: Option<(u32, u32)>, // v319u: Some = editing a table cell
        is_new: bool, // 本次编辑是否由新建文本框而来
        anchor: Option<(u32, u32)>, // v318s: 选区锚点（与光标不同即有选区）
        para: u32,
        offset: u32,
        start: Option<(u32, u32)>, // 输入法会话起点 (para, offset)
        prev: String,              // 会话内已写入文档的文字
    }
    fn impress_parse_pos(j: &str) -> Option<(u32, u32)> {
        let v: serde_json::Value = serde_json::from_str(j).ok()?;
        Some((v["para"].as_u64()? as u32, v["offset"].as_u64()? as u32))
    }
    macro_rules! impress_caret_update {
        ($app:expr, $sess:expr, $slide:expr, $shape:expr, $para:expr, $off:expr) => {{
            match $sess.impress_text_caret($slide, $shape, $para, $off)
                .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            {
                Some(v) => {
                    let g = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
                    let (cx1, cx2, cy1, cy2) = (g("x1"), g("x2"), g("y1"), g("y2"));
                    let (cdx, cdy) = ((cx2 - cx1).abs(), (cy2 - cy1).abs());
                    let (cw, ch) = if cdx > cdy { (cdx.max(4.0), 0.0f32) } else { (0.0f32, cdy.max(4.0)) };
                    $app.set_impress_text_caret_x(cx1.min(cx2));
                    $app.set_impress_text_caret_w(cw);
                    $app.set_impress_text_caret_y(cy1.min(cy2));
                    $app.set_impress_text_caret_h(ch);
                    $app.set_impress_text_caret_ok(true);
                }
                None => $app.set_impress_text_caret_ok(false),
            }
        }};
    }
    fn impress_parse_cell(j: &str) -> Option<(u32, u32)> {
        let v: serde_json::Value = serde_json::from_str(j).ok()?;
        Some((v["row"].as_u64()? as u32, v["col"].as_u64()? as u32))
    }
    macro_rules! impress_cell_caret_update {
        ($app:expr, $sess:expr, $slide:expr, $shape:expr, $r:expr, $c:expr, $off:expr) => {{
            match $sess.impress_cell_caret($slide, $shape, $r, $c, $off)
                .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            {
                Some(v) => {
                    let g = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
                    let (cx1, cx2, cy1, cy2) = (g("x1"), g("x2"), g("y1"), g("y2"));
                    let (cdx, cdy) = ((cx2 - cx1).abs(), (cy2 - cy1).abs());
                    let (cw, ch) = if cdx > cdy { (cdx.max(4.0), 0.0f32) } else { (0.0f32, cdy.max(4.0)) };
                    $app.set_impress_text_caret_x(cx1.min(cx2));
                    $app.set_impress_text_caret_w(cw);
                    $app.set_impress_text_caret_y(cy1.min(cy2));
                    $app.set_impress_text_caret_h(ch);
                    $app.set_impress_text_caret_ok(true);
                }
                None => $app.set_impress_text_caret_ok(false),
            }
        }};
    }
    let ite = std::rc::Rc::new(std::cell::RefCell::new(ImpressTextEd::default()));

    let ite_dc = ite.clone();
    app.on_impress_draw_committed(move |kind, x1, y1, x2, y2| {
        let app = match app_weak_dc.upgrade() { Some(a) => a, None => return };
        app.invoke_impress_text_exit();
        let idx = (app.get_impress_current_slide() - 1).max(0) as u32;
        let kind = kind.to_string();
        let is_straight = kind == "line" || kind == "lineWithArrow" || kind == "lineWithTwoArrows";
        let dist = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
        let (mut ax1, mut ay1, mut ax2, mut ay2) = (x1, y1, x2, y2);
        if dist < 4.0 {
            // 单击：直线给 120pt 横线，其余以点击点为中心给 120x80
            if is_straight { ax2 = ax1 + 120.0; ay2 = ay1; }
            else if kind == "textbox" { ax1 = x1 - 100.0; ay1 = y1 - 20.0; ax2 = x1 + 100.0; ay2 = y1 + 20.0; }
            else if kind == "textboxV" { ax1 = x1 - 20.0; ay1 = y1 - 100.0; ax2 = x1 + 20.0; ay2 = y1 + 100.0; }
            else { ax1 = x1 - 60.0; ay1 = y1 - 40.0; ax2 = x1 + 60.0; ay2 = y1 + 40.0; }
        }
        let rx = ax1.min(ax2);
        let ry = ay1.min(ay2);
        let rw = (ax2 - ax1).abs();
        let rh = (ay2 - ay1).abs();
        let (rw, rh) = if is_straight { (rw.max(1.0), rh.max(1.0)) } else { (rw.max(8.0), rh.max(8.0)) };
        let new_id: Option<u32> = {
            let mut s = state_dc.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            if sess.get_doc_type() != "impress" { return; }
            sess.begin_edit();
            let id = if kind == "textbox" || kind == "textboxV" {
                sess.impress_insert_textbox(idx, rx, ry, rw, rh, kind == "textboxV")
            } else {
                sess.impress_insert_shape(idx, &kind, rx, ry, rw, rh)
            };
            if id < 0 {
                sess.discard_edit();
                None
            } else {
                if is_straight {
                    sess.impress_set_line_endpoints(idx, id as u32, ax1, ay1, ax2, ay2);
                }
                // 文本框：保持 begin_edit 打开，退出编辑时才 commit/discard
                if !(kind == "textbox" || kind == "textboxV") { sess.commit_edit(); }
                Some(id as u32)
            }
        };
        eprintln!("[IMPRESS-INSERT] kind={} rect=({:.1},{:.1},{:.1},{:.1}) id={:?}", kind, rx, ry, rw, rh, new_id);
        if let Some(id) = new_id {
            app.invoke_impress_render_viewport(0.0);
            let q = {
                let mut s = state_dc.borrow_mut();
                s.session.as_mut().and_then(|sess| {
                    let r = sess.impress_query_shape_rect(idx, id)?;
                    let li = sess.impress_query_line_info(idx, id);
                    Some((r, li))
                })
            };
            if let Some(((qx, qy, qw, qh, qr), li)) = q {
                app.set_impress_sel_x(qx);
                app.set_impress_sel_y(qy);
                app.set_impress_sel_w(qw);
                app.set_impress_sel_h(qh);
                app.set_impress_sel_rot(qr);
                match li {
                    Some((true, fh, fv)) => {
                        let (a, b, c, e) = geom::line_endpoints(qx, qy, qw, qh, qr, fh, fv);
                        app.set_impress_line_x1(a);
                        app.set_impress_line_y1(b);
                        app.set_impress_line_x2(c);
                        app.set_impress_line_y2(e);
                        app.set_impress_sel_line(true);
                    }
                    _ => app.set_impress_sel_line(false),
                }
                app.set_impress_sel_id(id as i32);
                if kind == "textbox" || kind == "textboxV" {
                    app.set_impress_text_edit_dirty(false);
                    app.set_impress_text_edit_slide(idx as i32);
                    app.set_impress_text_edit_id(id as i32);
                    ite_dc.borrow_mut().is_new = true;
                    app.set_impress_text_clear_seq(app.get_impress_text_clear_seq() + 1);
                    {
                        let mut s = state_dc.borrow_mut();
                        if let Some(sess) = s.session.as_mut() {
                            impress_caret_update!(app, sess, idx, id, 0u32, 0u32);
                        }
                    }
                }
            }
        }
    });

    // -- Impress 文本框：退出编辑。无文字则 discard（整个创建回滚），有文字则 commit --
    let app_weak_te = app_weak.clone();
    let state_te = state.clone();
    let ite_te = ite.clone();
    app.on_impress_text_exit(move || {
        let app = match app_weak_te.upgrade() { Some(a) => a, None => return };
        let id = app.get_impress_text_edit_id();
        if id < 0 { return; }
        let dirty = app.get_impress_text_edit_dirty();
        app.set_impress_text_edit_id(-1);
        app.set_impress_text_edit_dirty(false);
        app.set_impress_text_caret_ok(false);
        app.set_impress_text_sel_rects(slint::ModelRc::new(slint::VecModel::from(Vec::<SelectionRect>::new())));
        app.set_impress_text_clear_seq(app.get_impress_text_clear_seq() + 1);
        let was_new = ite_te.borrow().is_new;
        *ite_te.borrow_mut() = ImpressTextEd::default();
        let slide = app.get_impress_text_edit_slide() as u32;
        let mut deselect = false;
        {
            let mut s = state_te.borrow_mut();
            if let Some(sess) = s.session.as_mut() {
                let empty = dirty && sess.impress_text_is_empty(slide, id as u32).unwrap_or(false);
                if !dirty {
                    sess.discard_edit();
                    deselect = was_new;
                } else if empty {
                    if was_new {
                        sess.discard_edit();
                    } else {
                        sess.impress_delete_shape(slide, id as u32);
                        sess.commit_edit();
                    }
                    deselect = true;
                } else {
                    sess.commit_edit();
                }
            }
        }
        if deselect {
            app.set_impress_sel_id(-1);
            app.set_impress_sel_line(false);
        }
        app.invoke_impress_render_viewport(0.0);
        eprintln!("[IMPRESS-TEXT] exit id={} dirty={} -> {}", id, dirty, if dirty { "commit" } else { "discard" });
    });

    // -- Impress 文本框（2c）：点击进入编辑 / 编辑中点击定位光标 --
    let app_weak_tk2 = app_weak.clone();
    let state_tk2 = state.clone();
    // v318s: 选区高亮刷新 / 选区删除
    macro_rules! impress_sel_update {
        ($app:expr, $sess:expr, $slide:expr, $shape:expr, $st:expr) => {{
            let mut rects: Vec<SelectionRect> = Vec::new();
            if let Some(a) = $st.anchor {
                let c = ($st.para, $st.offset);
                if a != c {
                    let (s0, e0) = if a <= c { (a, c) } else { (c, a) };
                    if let Some(j) = $sess.impress_text_sel_rects($slide, $shape, s0.0, s0.1, e0.0, e0.1) {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&j) {
                            if let Some(arr) = v.as_array() {
                                for it in arr {
                                    let g = |k: &str| it[k].as_f64().unwrap_or(0.0) as f32;
                                    rects.push(SelectionRect { rx: g("x"), ry: g("y"), rw: g("w"), rh: g("h") });
                                }
                            }
                        }
                    }
                }
            }
            $app.set_impress_text_sel_rects(slint::ModelRc::new(slint::VecModel::from(rects)));
        }};
    }
    macro_rules! impress_delete_sel {
        ($sess:expr, $slide:expr, $shape:expr, $st:expr) => {{
            let mut done = false;
            if let Some(a) = $st.anchor {
                let c = ($st.para, $st.offset);
                if a != c {
                    let (s0, e0) = if a <= c { (a, c) } else { (c, a) };
                    let mut cur = e0;
                    let mut guard = 0u32;
                    while cur > s0 && guard < 20000 {
                        guard += 1;
                        match $sess.impress_text_delete($slide, $shape, cur.0, cur.1, true).and_then(|j| impress_parse_pos(&j)) {
                            Some(p) => { if p == cur { break; } cur = p; }
                            None => break,
                        }
                    }
                    $st.para = cur.0;
                    $st.offset = cur.1;
                    $st.anchor = None;
                    done = true;
                }
            }
            done
        }};
    }
    let ite_tk2 = ite.clone();
    app.on_impress_text_click(move |x, y| {
        let app = match app_weak_tk2.upgrade() { Some(a) => a, None => return };
        let editing = app.get_impress_text_edit_id() >= 0;
        let (slide, id) = if editing {
            (app.get_impress_text_edit_slide() as u32, app.get_impress_text_edit_id())
        } else {
            ((app.get_impress_current_slide() - 1).max(0) as u32, app.get_impress_sel_id())
        };
        if id < 0 { return; }
        let shape = id as u32;
        let mut st = ite_tk2.borrow_mut();
        let mut s = state_tk2.borrow_mut();
        let sess = match s.session.as_mut() { Some(v) => v, None => return };
        if sess.get_doc_type() != "impress" { return; }
        if sess.impress_is_table(slide, shape).unwrap_or(false) {
            let hit_cell = sess.impress_cell_at(slide, shape, x, y).and_then(|j| impress_parse_cell(&j))
                .or(if editing { st.cell } else { None });
            let hit = hit_cell.and_then(|(r, c)| {
                sess.impress_cell_hit(slide, shape, r, c, x, y)
                    .and_then(|j| impress_parse_pos(&j))
                    .map(|p| (r, c, p.1))
            });
            eprintln!("[IMPRESS-CELL] click ({:.1},{:.1}) editing={} -> {:?}", x, y, editing, hit);
            if let Some((r, c, o)) = hit {
                if !editing {
                    sess.begin_edit();
                    *st = ImpressTextEd::default();
                    app.set_impress_text_edit_dirty(false);
                    app.set_impress_text_edit_slide(slide as i32);
                    app.set_impress_text_edit_id(id);
                }
                st.cell = Some((r, c));
                st.para = 0;
                st.offset = o;
                st.start = None;
                st.prev.clear();
                st.anchor = None;
                app.set_impress_text_clear_seq(app.get_impress_text_clear_seq() + 1);
                impress_cell_caret_update!(app, sess, slide, shape, r, c, o);
                app.set_impress_text_sel_rects(slint::ModelRc::new(slint::VecModel::from(Vec::<SelectionRect>::new())));
            }
            return;
        }
        if !editing { sess.begin_edit(); }
        let pos = sess.impress_text_hit(slide, shape, x, y).and_then(|j| impress_parse_pos(&j));
        eprintln!("[IMPRESS-TEXT] click ({:.1},{:.1}) editing={} -> {:?}", x, y, editing, pos);
        match pos {
            None => { if !editing { sess.discard_edit(); } }
            Some((p, o)) => {
                if !editing {
                    *st = ImpressTextEd::default();
                    app.set_impress_text_edit_dirty(false);
                    app.set_impress_text_edit_slide(slide as i32);
                    app.set_impress_text_edit_id(id);
                }
                st.para = p;
                st.offset = o;
                st.start = None;
                st.prev.clear();
                st.anchor = Some((p, o));
                app.set_impress_text_clear_seq(app.get_impress_text_clear_seq() + 1);
                impress_caret_update!(app, sess, slide, shape, p, o);
                impress_sel_update!(app, sess, slide, shape, st);
            }
        }
    });

    // -- Impress 文本框：输入法组字 / 回车 / 退格Delete（2b）--
    let app_weak_tc = app_weak.clone();
    let state_tc = state.clone();
    let ite_tc = ite.clone();
    app.on_impress_text_composed(move |text| {
        let app = match app_weak_tc.upgrade() { Some(a) => a, None => return };
        let id = app.get_impress_text_edit_id();
        if id < 0 { return; }
        if text.chars().any(|c| c.is_control()) { return; }
        let slide = app.get_impress_text_edit_slide() as u32;
        let shape = id as u32;
        let text = text.to_string();
        let mut edited = false;
        {
            let mut st = ite_tc.borrow_mut();
            let mut s = state_tc.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            if sess.get_doc_type() != "impress" { return; }
            if st.start.is_none() && app.get_impress_text_edit_dirty() { sess.commit_edit(); sess.begin_edit(); app.set_impress_text_edit_dirty(false); st.is_new = false; }
if let Some((cr, cc)) = st.cell {
                let (sp, so) = match st.start {
                    Some(v) => v,
                    None => { let v = (st.para, st.offset); st.start = Some(v); st.prev.clear(); v }
                };
                let _ = sp;
                let mut off = so + st.prev.len() as u32;
                let prev_chars: Vec<char> = st.prev.chars().collect();
                for ch in prev_chars.iter().rev() {
                    if sess.impress_cell_delete(slide, shape, cr, cc, off, true).is_none() { break; }
                    off -= ch.len_utf8() as u32;
                    edited = true;
                }
                let mut pos = so;
                st.prev.clear();
                if !text.is_empty() {
                    if let Some(p) = sess.impress_cell_insert(slide, shape, cr, cc, so, &text).and_then(|j| impress_parse_pos(&j)) {
                        pos = p.1;
                        st.prev = text.clone();
                        edited = true;
                    }
                }
                st.para = 0;
                st.offset = pos;
                eprintln!("[IMPRESS-CELL] composed {:?} -> cell=({},{}) offset={}", text, cr, cc, pos);
                if let Some(nh) = sess.impress_table_fit(slide, shape) { app.set_impress_sel_h(nh); }
                                impress_cell_caret_update!(app, sess, slide, shape, cr, cc, pos);
                drop(st);
                drop(s);
                if edited { app.set_impress_text_edit_dirty(true); }
                app.invoke_impress_render_viewport(0.0);
                return;
            }
if st.start.is_none() {
                if impress_delete_sel!(sess, slide, shape, st) { edited = true; }
                st.anchor = None;
            }
            let (sp, so) = match st.start {
                Some(v) => v,
                None => { let v = (st.para, st.offset); st.start = Some(v); st.prev.clear(); v }
            };
            let mut off = so + st.prev.len() as u32;
            let prev_chars: Vec<char> = st.prev.chars().collect();
            for ch in prev_chars.iter().rev() {
                if sess.impress_text_delete(slide, shape, sp, off, true).is_none() { break; }
                off -= ch.len_utf8() as u32;
                edited = true;
            }
            let mut pos = (sp, so);
            st.prev.clear();
            if !text.is_empty() {
                if let Some(p) = sess.impress_text_insert(slide, shape, sp, so, &text).and_then(|j| impress_parse_pos(&j)) {
                    pos = p;
                    st.prev = text.clone();
                    edited = true;
                }
            }
            st.para = pos.0;
            st.offset = pos.1;
            eprintln!("[IMPRESS-TEXT] composed {:?} -> para={} offset={}", text, pos.0, pos.1);
            impress_caret_update!(app, sess, slide, shape, pos.0, pos.1);
            impress_sel_update!(app, sess, slide, shape, st);
        }
        if edited { app.set_impress_text_edit_dirty(true); }
        app.invoke_impress_render_viewport(0.0);
    });

    let app_weak_tn = app_weak.clone();
    let state_tn = state.clone();
    let ite_tn = ite.clone();
    app.on_impress_text_enter(move || {
        let app = match app_weak_tn.upgrade() { Some(a) => a, None => return };
        let id = app.get_impress_text_edit_id();
        if id < 0 { return; }
        let slide = app.get_impress_text_edit_slide() as u32;
        let shape = id as u32;
        let mut edited = false;
        {
            let mut st = ite_tn.borrow_mut();
            let mut s = state_tn.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            if sess.get_doc_type() != "impress" { return; }
if app.get_impress_text_edit_dirty() { sess.commit_edit(); sess.begin_edit(); app.set_impress_text_edit_dirty(false); st.is_new = false; }
            st.start = None;
            st.prev.clear();
            if impress_delete_sel!(sess, slide, shape, st) { edited = true; }
            let (p0, o0) = (st.para, st.offset);
            if st.cell.is_some() { return; }
            if let Some(p) = sess.impress_text_split(slide, shape, p0, o0).and_then(|j| impress_parse_pos(&j)) {
                st.para = p.0;
                st.offset = p.1;
                edited = true;
            }
            eprintln!("[IMPRESS-TEXT] enter -> para={} offset={}", st.para, st.offset);
            st.anchor = None;
            impress_caret_update!(app, sess, slide, shape, st.para, st.offset);
            impress_sel_update!(app, sess, slide, shape, st);
        }
        if edited { app.set_impress_text_edit_dirty(true); }
        app.invoke_impress_render_viewport(0.0);
    });

    let app_weak_tk = app_weak.clone();
    let state_tk = state.clone();
    let ite_tk = ite.clone();
    app.on_impress_text_key(move |key, shift, _ctrl| {
        let app = match app_weak_tk.upgrade() { Some(a) => a, None => return };
        let id = app.get_impress_text_edit_id();
        if id < 0 { return; }
        let is_bs = key == slint::SharedString::from(slint::platform::Key::Backspace);
        let is_del = key == slint::SharedString::from(slint::platform::Key::Delete);
        let is_l = key == slint::SharedString::from(slint::platform::Key::LeftArrow);
        let is_r = key == slint::SharedString::from(slint::platform::Key::RightArrow);
        let is_u = key == slint::SharedString::from(slint::platform::Key::UpArrow);
        let is_d = key == slint::SharedString::from(slint::platform::Key::DownArrow);
        if is_l || is_r || is_u || is_d {
            app.set_impress_text_clear_seq(app.get_impress_text_clear_seq() + 1);
            let slide = app.get_impress_text_edit_slide() as u32;
            let shape = id as u32;
            let mut st = ite_tk.borrow_mut();
            let mut s = state_tk.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            if sess.get_doc_type() != "impress" { return; }
            st.start = None;
            st.prev.clear();
            let (p0, o0) = (st.para, st.offset);
            if shift { if st.anchor.is_none() { st.anchor = Some((p0, o0)); } } else { st.anchor = None; }
            if let Some((cr, cc)) = st.cell {
                st.anchor = None;
                let newo: Option<u32> = if is_l || is_r {
                    sess.impress_cell_move(slide, shape, cr, cc, o0, if is_l { 0 } else { 1 })
                } else {
                    let cj = sess.impress_cell_caret(slide, shape, cr, cc, o0)
                        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok());
                    match cj {
                        Some(v) => {
                            let g = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
                            let (cx, cy1, cy2) = (g("x1"), g("y1"), g("y2"));
                            let h = (cy2 - cy1).abs().max(4.0);
                            let ty = if is_u { cy1.min(cy2) - h * 0.5 } else { cy1.max(cy2) + h * 0.5 };
                            sess.impress_cell_hit(slide, shape, cr, cc, cx, ty)
                                .and_then(|j| impress_parse_pos(&j))
                                .map(|p| p.1)
                        }
                        None => None,
                    }
                };
                if let Some(o) = newo { st.offset = o; }
                st.para = 0;
                eprintln!("[IMPRESS-CELL] arrow from {} -> {:?}", o0, newo);
                impress_cell_caret_update!(app, sess, slide, shape, cr, cc, st.offset);
                return;
            }
            let kv_geo: Option<(f32, f32, f32, f32)> = sess.impress_text_caret(slide, shape, p0, o0)
                .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
                .map(|v| { let g = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32; (g("x1"), g("x2"), g("y1"), g("y2")) });
            let (kv_vert, kv_cw) = match kv_geo {
                Some((a, b, c, d)) => ((a - b).abs() > (c - d).abs(), a > b),
                None => (false, true),
            };
            let kv_lr = is_l || is_r;
            let newpos: Option<(u32, u32)> = if kv_vert && kv_lr {
                match kv_geo {
                    Some((a, b, _c, d)) => {
                        let kh = (a - b).abs().max(4.0);
                        let tx = if is_l { a.min(b) - kh * 0.5 } else { a.max(b) + kh * 0.5 };
                        sess.impress_text_hit(slide, shape, tx, d).and_then(|j| impress_parse_pos(&j))
                    }
                    None => None,
                }
            } else if kv_vert || kv_lr {
                let kdir = if kv_vert { if (kv_cw && !is_u) || (!kv_cw && is_u) { 1 } else { 0 } } else if is_l { 0 } else { 1 };
                sess.impress_text_move(slide, shape, p0, o0, kdir)
                    .and_then(|j| impress_parse_pos(&j))
            } else {
                let cj = sess.impress_text_caret(slide, shape, p0, o0)
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok());
                match cj {
                    Some(v) => {
                        let g = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
                        let (cx, cy1, cy2) = (g("x1"), g("y1"), g("y2"));
                        let h = (cy2 - cy1).abs().max(4.0);
                        let ty = if is_u { cy1.min(cy2) - h * 0.5 } else { cy1.max(cy2) + h * 0.5 };
                        sess.impress_text_hit(slide, shape, cx, ty).and_then(|j| impress_parse_pos(&j))
                    }
                    None => None,
                }
            };
            eprintln!("[IMPRESS-TEXT] arrow from ({},{}) -> {:?}", p0, o0, newpos);
            if let Some((p, o)) = newpos { st.para = p; st.offset = o; }
            impress_caret_update!(app, sess, slide, shape, st.para, st.offset);
            impress_sel_update!(app, sess, slide, shape, st);
            return;
        }
        if !is_bs && !is_del { return; } // 方向键留给 2c
        app.set_impress_text_clear_seq(app.get_impress_text_clear_seq() + 1);
        let slide = app.get_impress_text_edit_slide() as u32;
        let shape = id as u32;
        let mut edited = false;
        {
            let mut st = ite_tk.borrow_mut();
            let mut s = state_tk.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            if sess.get_doc_type() != "impress" { return; }
            st.start = None;
            st.prev.clear();
            if app.get_impress_text_edit_dirty() { sess.commit_edit(); sess.begin_edit(); app.set_impress_text_edit_dirty(false); st.is_new = false; }
if let Some((cr, cc)) = st.cell {
                let co = st.offset;
                if let Some(j) = sess.impress_cell_delete(slide, shape, cr, cc, co, is_bs) {
                    if let Some(p) = impress_parse_pos(&j) { st.offset = p.1; }
                    edited = true;
                }
                st.para = 0;
                st.anchor = None;
                eprintln!("[IMPRESS-CELL] delete(bs={}) at {} -> {}", is_bs, co, st.offset);
                if let Some(nh) = sess.impress_table_fit(slide, shape) { app.set_impress_sel_h(nh); }
                                impress_cell_caret_update!(app, sess, slide, shape, cr, cc, st.offset);
                drop(st);
                drop(s);
                if edited { app.set_impress_text_edit_dirty(true); }
                app.invoke_impress_render_viewport(0.0);
                return;
            }
            let sel_deleted = impress_delete_sel!(sess, slide, shape, st);
            if sel_deleted { edited = true; }
            let (p0, o0) = (st.para, st.offset);
            if let Some(j) = if sel_deleted { None } else { sess.impress_text_delete(slide, shape, p0, o0, is_bs) } {
                eprintln!("[IMPRESS-TEXT] delete(bs={}) at ({},{}) -> {}", is_bs, p0, o0, j);
                if let Some(p) = impress_parse_pos(&j) {
                    st.para = p.0;
                    st.offset = p.1;
                }
                edited = true;
            }
            st.anchor = None;
            impress_caret_update!(app, sess, slide, shape, st.para, st.offset);
            impress_sel_update!(app, sess, slide, shape, st);
        }
        if edited { app.set_impress_text_edit_dirty(true); }
        app.invoke_impress_render_viewport(0.0);
    });

    // v319o: Impress in-app clipboard and select-all
    {
        let app_weak_cl = app_weak.clone();
        let state_cl = state.clone();
        let ite_cl = ite.clone();
        app.on_impress_text_clip(move |op| {
            let app = match app_weak_cl.upgrade() { Some(a) => a, None => return };
            let id = app.get_impress_text_edit_id();
            if id < 0 { return; }
            let slide = app.get_impress_text_edit_slide() as u32;
            let shape = id as u32;
            let mut edited = false;
            {
                let mut st = ite_cl.borrow_mut();
                let mut s = state_cl.borrow_mut();
                let sess = match s.session.as_mut() { Some(v) => v, None => return };
                if sess.get_doc_type() != "impress" { return; }
                st.start = None;
                st.prev.clear();
                app.set_impress_text_clear_seq(app.get_impress_text_clear_seq() + 1);
                let op = op.to_string();
                let range = st.anchor.and_then(|a| {
                    let c = (st.para, st.offset);
                    if a == c { None } else if a <= c { Some((a, c)) } else { Some((c, a)) }
                });
                match op.as_str() {
                    "all" => {
                        if let Some(p) = sess.impress_text_extent(slide, shape).and_then(|j| impress_parse_pos(&j)) {
                            st.anchor = Some((0, 0));
                            st.para = p.0;
                            st.offset = p.1;
                        }
                    }
                    "copy" | "cut" => {
if op == "cut" && app.get_impress_text_edit_dirty() { sess.commit_edit(); sess.begin_edit(); app.set_impress_text_edit_dirty(false); st.is_new = false; }
                        if let Some((s0, e0)) = range {
                            let ok = sess.impress_text_copy(slide, shape, s0.0, s0.1, e0.0, e0.1);
                            if ok && op == "cut" && impress_delete_sel!(sess, slide, shape, st) { edited = true; }
                        }
                    }
                    "paste" => {
if app.get_impress_text_edit_dirty() { sess.commit_edit(); sess.begin_edit(); app.set_impress_text_edit_dirty(false); st.is_new = false; }
                        if impress_delete_sel!(sess, slide, shape, st) { edited = true; }
                        let (p0, o0) = (st.para, st.offset);
                        if let Some(p) = sess.impress_text_paste(slide, shape, p0, o0).and_then(|j| impress_parse_pos(&j)) {
                            st.para = p.0;
                            st.offset = p.1;
                            edited = true;
                        }
                        st.anchor = None;
                    }
                    _ => {}
                }
                impress_caret_update!(app, sess, slide, shape, st.para, st.offset);
                impress_sel_update!(app, sess, slide, shape, st);
            }
            if edited { app.set_impress_text_edit_dirty(true); }
            app.invoke_impress_render_viewport(0.0);
        });
    }

    // v318s: Impress 文字拖选
    {
        let app_weak_td = app_weak.clone();
        let state_td = state.clone();
        let ite_td = ite.clone();
        app.on_impress_text_drag(move |x, y| {
            let Some(app) = app_weak_td.upgrade() else { return };
            let id = app.get_impress_text_edit_id();
            if id < 0 { return; }
            let slide = app.get_impress_text_edit_slide() as u32;
            let shape = id as u32;
            let mut st = ite_td.borrow_mut();
            let mut s = state_td.borrow_mut();
            let sess = match s.session.as_mut() { Some(v) => v, None => return };
            if sess.get_doc_type() != "impress" { return; }
            if let Some((p, o)) = sess.impress_text_hit(slide, shape, x, y).and_then(|j| impress_parse_pos(&j)) {
                if (p, o) != (st.para, st.offset) {
                    st.para = p;
                    st.offset = o;
                    st.start = None;
                    st.prev.clear();
                    impress_caret_update!(app, sess, slide, shape, p, o);
                    impress_sel_update!(app, sess, slide, shape, st);
                }
            }
        });
    }
    // -- Impress翻页：滚轮/触控板累积delta，超过阈值切下一张/上一张（硬切，无动画）--
    let impress_wheel_accum = Rc::new(std::cell::Cell::new(0.0f32));
    let app_weak_iw = app_weak.clone();
    let state_iw = state.clone();
    // v318x: Impress 字体名/字号回显（300ms 轮询，只在值变化时写属性）
    {
        let app_weak_fe = app_weak.clone();
        let state_fe = state.clone();
        let ite_fe = ite.clone();
        let echo_timer = slint::Timer::default();
        echo_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(300), move || {
            let Some(app) = app_weak_fe.upgrade() else { return };
            let sel = app.get_impress_sel_id();
            let mut name = String::new();
            let mut size = String::new();
            if sel >= 0 {
                let editing = app.get_impress_text_edit_id() == sel;
                let slide = if editing { app.get_impress_text_edit_slide() as u32 } else { (app.get_impress_current_slide() - 1).max(0) as u32 };
                let (p1, o1, p2, o2) = if editing {
                    match ite_fe.try_borrow() {
                        Ok(st) => { let c = (st.para, st.offset); let a = st.anchor.unwrap_or(c); (a.0, a.1, c.0, c.1) }
                        Err(_) => return,
                    }
                } else { (u32::MAX, 0, 0, 0) };
                let j = match state_fe.try_borrow() {
                    Ok(s) => s.session.as_ref().and_then(|sess| sess.impress_text_style_query(slide, sel as u32, p1, o1, p2, o2)),
                    Err(_) => return,
                };
                if let Some(v) = j.and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok()) {
                    if !v["font_mixed"].as_bool().unwrap_or(false) {
                        name = match v["font"].as_str() {
                            Some("Carlito") => "Carlito (默认)".to_string(),
                            Some(n) => n.to_string(),
                            None => "默认".to_string(),
                        };
                    }
                    if !v["size_mixed"].as_bool().unwrap_or(false) {
                        if let Some(sz) = v["size"].as_f64() {
                            size = if sz.fract() == 0.0 { format!("{}", sz as i64) } else { format!("{:.1}", sz) };
                        }
                    }
                }
            }
            if app.get_impress_font_name().as_str() != name { app.set_impress_font_name(name.into()); }
            if app.get_impress_font_size_text().as_str() != size { app.set_impress_font_size_text(size.into()); }
        });
        std::mem::forget(echo_timer);
    }
    // v318y: Impress 段落样式（对齐）。编辑中按选区/光标所在段，否则整个形状
    {
        let app_weak_pa = app_weak.clone();
        let state_pa = state.clone();
        let ite_pa = ite.clone();
        app.on_impress_para_style(move |cmd| {
            let Some(app) = app_weak_pa.upgrade() else { return };
            let cmd = cmd.to_string();
            let sel = app.get_impress_sel_id();
            if sel < 0 {
                app.set_status_message("请先选中文本框".into());
                return;
            }
            let editing = app.get_impress_text_edit_id() == sel;
            let slide = if editing { app.get_impress_text_edit_slide() as u32 } else { (app.get_impress_current_slide() - 1).max(0) as u32 };
            let (p1, p2) = if editing {
                let st = ite_pa.borrow();
                let a = st.anchor.map(|a| a.0).unwrap_or(st.para);
                (a.min(st.para), a.max(st.para))
            } else { (u32::MAX, 0) };
            let ok = {
                let mut s = state_pa.borrow_mut();
                let sess = match s.session.as_mut() { Some(x) => x, None => return };
                if sess.get_doc_type() != "impress" { return; }
                if !editing { sess.begin_edit(); }
                let done = sess.impress_set_para_style(slide, sel as u32, p1, p2, &cmd);
                if !editing {
                    if done { sess.commit_edit(); } else { sess.discard_edit(); }
                } else if done {
                    app.set_impress_text_edit_dirty(true);
                    let st = ite_pa.borrow();
                    impress_caret_update!(app, sess, slide, sel as u32, st.para, st.offset);
                    impress_sel_update!(app, sess, slide, sel as u32, st);
                }
                done
            };
            if ok { app.invoke_impress_render_viewport(0.0); }
            else { app.set_status_message("设置段落样式失败".into()); }
        });
    }
    app.on_impress_wheel_scrolled(move |delta_y| {
        let app = match app_weak_iw.upgrade() { Some(a) => a, None => return };
        const THRESHOLD: f32 = 60.0;
        let acc = impress_wheel_accum.get() + delta_y;
        if acc.abs() < THRESHOLD {
            impress_wheel_accum.set(acc);
            return;
        }
        impress_wheel_accum.set(0.0);
        let s = state_iw.borrow();
        let slide_count = s.impress_slides.len().max(1) as i32;
        drop(s);
        let cur = app.get_impress_current_slide();
        let next = if acc < 0.0 { cur + 1 } else { cur - 1 };
        let clamped = next.clamp(1, slide_count);
        if clamped != cur {
            app.set_impress_current_slide(clamped);
        }
    });

    // ── Writer键盘输入 ────────────────────────────────────────────────────────
    let state_writer_key = state.clone();
    let app_weak_wk = app_weak.clone();
    app.on_writer_key_input(move |text| {
        // v100 fix：Shift 等修饰键在 Slint 侧会连带触发一次带控制字符
        // （如 U+0010 DLE）的文本事件，过滤掉，避免把控制字符当普通
        // 字符插入进文档（表现为渲染出一个 tofu 方块）。
        // v105 fix：\n/\r（回车）本身也是控制字符，会被上面这条过滤器
        // 误伤，导致回车事件从未能传到下面的处理分支（见
        // ROE_Progress_v104.md「三、3.2 根因·第一层」）。这里精确放行
        // \n/\r，其余控制字符（如DLE）继续按原逻辑过滤。
        if text.chars().any(|c| c.is_control() && c != '\n' && c != '\r') {
            return;
        }
        let app = match app_weak_wk.upgrade() { Some(a) => a, None => return };
        let mut s = state_writer_key.borrow_mut();
        // 先读光标位置，再借用session（避免借用冲突）
        let para = s.writer_cursor_para;
        let offset = s.writer_cursor_offset;
        if text == "\n" || text == "\r" {
            if let Some((cid, cp, coff)) = s.writer_cell_cursor {
                let res = s.session.as_mut().and_then(|sess| sess.cell_split(cid, cp as u32, coff as u32));
                eprintln!("[V316-CELL-ENTER] cid={} para={} off={} -> {:?}", cid, cp, coff, res);
                if let Some((np, no)) = res {
                    s.writer_cell_cursor = Some((cid, np, no));
                    s.writer_ime_session_start = None;
                    s.writer_ime_session_prev_len = 0;
                }
                writer_cell_refresh(&app, &mut s);
                return;
            }
        }
        eprintln!("[V115-DIAG-KEYINPUT] 打字时读到 s.writer_cursor_para={} s.writer_cursor_offset={} text={:?}", para, offset, text);
        let sess = match s.session.as_mut() { Some(s) => s, None => return };
        if sess.get_doc_type() != "writer" { return; }
        // v105：回车换行改为调用 roe_split_paragraph（v104文档「三」
        // 排查清楚的正确语义：在光标处把段落切成两半，不再是v100遗留
        // 的、无条件在文档末尾追加空段落的占位实现 append_paragraph）。
        // 普通字符插入已切换为 roe_insert_char（v100 新增，snapshot/undo
        // 事务、UTF-8 字符边界校验均在 roe-api 内部完成，不再由前端手写
        // 字符串切片，也不再需要经过 get_paragraph/set_paragraph 整段
        // 读写这条 script 路径）。
        if text == "\n" || text == "\r" {
            // v251: 空列表项再按回车 -> 取消列表（不拆段）
            let empty_list_item = sess.get_paragraph_info(para as u32)
                .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
                .map(|v| v["para_len"].as_i64() == Some(0) && v["is_list"].as_bool() == Some(true))
                .unwrap_or(false);
            if empty_list_item {
                match sess.writer_set_list(para as u32, 0, 0) {
                    Ok(()) => eprintln!("[WRITER-KEY] 空列表项回车 -> 取消列表 para={}", para),
                    Err(e) => eprintln!("[WRITER-KEY] 取消列表失败: {}", e),
                }
                if let Some(sess) = s.session.as_mut() {
                    let (cx, cy, _cw, ch_h) = sess.writer_query_cursor_rect(para as u64, 0);
                    s.writer_cursor_ideal_x = Some(cx);
                    app.set_writer_cursor_x(cx as i32);
                    app.set_writer_cursor_y(cy as i32);
                    app.set_writer_cursor_h(ch_h as i32);
                    app.set_writer_cursor_visible(true);
                }
                // v251b: 取消列表后立即重绘（return 会跳过下方统一重绘）
                s.writer_sel_anchor_para = para;
                s.writer_sel_anchor_offset = 0;
                s.writer_cursor_offset = 0;
                {
                    let scroll_y = app.get_writer_scroll_y_pt();
                    let vw = app.get_writer_view_width() as f32;
                    let vh = app.get_writer_view_height() as f32;
                    let doc_w = app.get_writer_doc_width_pt();
                    if vw > 0.0 && vh > 0.0 {
                        let rgba = s.session.as_mut().map(|sess| sess.render_writer_viewport(scroll_y, doc_w, vh)).unwrap_or_default();
                        if !rgba.is_empty() {
                            let pw = (doc_w * doc_core::doc_bitmap_scale()).round() as u32;
                            let ph = (vh * doc_core::doc_bitmap_scale()).round() as u32;
                            let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, pw, ph);
                            app.set_writer_viewport_image(slint::Image::from_rgba8(buf));
                            eprintln!("[WRITER-KEY] rerender OK (取消列表) {}x{}px", pw, ph);
                        }
                    }
                }
                return;
            }
            match sess.split_paragraph(para as u32, offset as u32) {
                Some(new_loc_json) => {
                    eprintln!("[WRITER-KEY] split_paragraph text={:?} -> {}", text, new_loc_json);
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&new_loc_json) {
                        let new_para = v["local_para_index"].as_i64();
                        let new_offset = v["char_offset"].as_i64();
                        if let (Some(new_para), Some(new_offset)) = (new_para, new_offset) {
                            s.writer_cursor_para = new_para;
                            s.writer_cursor_offset = new_offset;
                            // 与 insert_char 分支保持一致：分段后重新查询
                            // 光标屏幕坐标并更新显示、同步选区锚点（折叠）。
                            s.writer_sel_anchor_para = new_para;
                            s.writer_sel_anchor_offset = new_offset;
                            if let Some(sess) = s.session.as_mut() {
                                let (cx, cy, _cw, ch_h) = sess.writer_query_cursor_rect(new_para as u64, new_offset as u64);
                                s.writer_cursor_ideal_x = Some(cx);
                                app.set_writer_cursor_x(cx as i32);
                                app.set_writer_cursor_y(cy as i32);
                                app.set_writer_cursor_h(ch_h as i32);
                                app.set_writer_cursor_visible(true);
                            }
                        }
                    }
                }
                None => {
                    eprintln!("[WRITER-KEY] split_paragraph FAILED text={:?} para={} offset={}", text, para, offset);
                }
            }
        } else {
            // roe_insert_char 按 Unicode 码点插入，text 理论上是单个字符
            // （Slint 文本输入事件逐字符触发），但为稳妥起见仍按 chars()
            // 取第一个字符处理，多字符输入（比如未来支持粘贴）不在本次
            // 范围内。
            let Some(ch) = text.chars().next() else { return; };
            match sess.insert_char(para as u32, offset as u32, ch as u32) {
                Some(new_loc_json) => {
                    eprintln!("[WRITER-KEY] insert_char text={:?} -> {}", text, new_loc_json);
                    // 从返回的 JSON 中取新的 char_offset 更新光标位置。
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&new_loc_json) {
                        if let Some(new_offset) = v["char_offset"].as_i64() {
                            s.writer_cursor_offset = new_offset;
                            // v100 fix：插入后重新查询光标屏幕坐标并更新显示，
                            // 对齐 on_writer_cursor_key（方向键路径）的既有模式
                            // （query_cursor_rect -> set_writer_cursor_x/y/h）。
                            // 此前遗漏这一步，导致插入后光标视觉上不跟随移动
                            // （文档内容和内部offset其实是对的，只是没重画光标）。
                            s.writer_sel_anchor_para = para;
                            s.writer_sel_anchor_offset = new_offset;
                            if let Some(sess) = s.session.as_mut() {
                                let (cx, cy, _cw, ch_h) = sess.writer_query_cursor_rect(para as u64, new_offset as u64);
                                s.writer_cursor_ideal_x = Some(cx);
                                app.set_writer_cursor_x(cx as i32);
                                app.set_writer_cursor_y(cy as i32);
                                app.set_writer_cursor_h(ch_h as i32);
                                app.set_writer_cursor_visible(true);
                            }
                        }
                    }
                }
                None => {
                    eprintln!("[WRITER-KEY] insert_char FAILED text={:?} para={} offset={}", text, para, offset);
                }
            }
        }
        // 重新渲染视口
        let scroll_y = app.get_writer_scroll_y_pt();
        let vw = app.get_writer_view_width() as f32;
        let vh = app.get_writer_view_height() as f32;
        let doc_w = app.get_writer_doc_width_pt();
        if vw > 0.0 && vh > 0.0 {
            let rgba = s.session.as_mut().map(|sess| sess.render_writer_viewport(scroll_y, doc_w, vh)).unwrap_or_default();
            if !rgba.is_empty() {
                let pw = (doc_w * doc_core::doc_bitmap_scale()).round() as u32;
                let ph = (vh * doc_core::doc_bitmap_scale()).round() as u32;
                let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, pw, ph); if true {
                    app.set_writer_viewport_image(slint::Image::from_rgba8(buf));
                    eprintln!("[WRITER-KEY] rerender OK {}x{}px", pw, ph);
                }
            } else {
                eprintln!("[WRITER-KEY] rerender FAILED");
            }
        }
    });

    // v254: Tab / Shift+Tab 在列表项里调整缩进级别
    let state_wtab = state.clone();
    let app_weak_wtab = app_weak.clone();
    app.on_writer_tab(move |shift| {
        let app = match app_weak_wtab.upgrade() { Some(a) => a, None => return false };
        {
            let mut s = state_wtab.borrow_mut();
            if let Some((cid, _cp, _co)) = s.writer_cell_cursor {
                let res = s.session.as_mut().and_then(|sess| sess.cell_tab(cid, shift));
                eprintln!("[V316-CELL-TAB] cid={} shift={} -> {:?}", cid, shift, res);
                if let Some((ncid, np, no)) = res {
                    s.writer_cell_cursor = Some((ncid, np, no));
                    s.writer_ime_session_start = None;
                    s.writer_ime_session_prev_len = 0;
                }
                app.invoke_writer_ime_clear_buffer();
                writer_cell_refresh(&app, &mut s);
                return true;
            }
        }
        let handled = {
            let s = state_wtab.borrow();
            let para = s.writer_cursor_para;
            let offset = s.writer_cursor_offset;
            match s.session.as_ref() {
                Some(sess) if sess.get_doc_type() == "writer" && para >= 0 => {
                    let is_list = sess.get_paragraph_info(para as u32)
                        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
                        .map(|v| v["is_list"].as_bool() == Some(true))
                        .unwrap_or(false);
                    is_list && (shift || offset == 0)
                }
                _ => false,
            }
        };
        if handled {
            let cmd = if shift { "writer.indent_dec" } else { "writer.indent_inc" };
            app.invoke_ribbon_action(cmd.into());
        }
        handled
    });

    // ── Writer点击（记录光标位置，暂时只打log）────────────────────────────────
    let state_wclick = state.clone();
    let app_weak_wclick = app_weak.clone();
    app.on_writer_click(move |x, y| {
        eprintln!("[WRITER-CLICK] x={:.1} y={:.1}", x, y);
        // v232: 位置离按下点超过3个单位 = 拖选结束，保留拖选得到的选区，不按"点击"折叠
        {
            let mut st = state_wclick.borrow_mut();
            if let Some((ox, oy)) = st.writer_drag_origin.take() {
                if (x - ox).abs() > 3.0 || (y - oy).abs() > 3.0 {
                    st.writer_dragging = false;
                    return;
                }
            }
        }
        // v117清理：writer_dragging 此前从未在拖拽结束时重置为false（v116发现的
        // 遗留技术债）。实机日志验证过这个字段没有引起实际bug（因为每次拖拽都是
        // 全新的drag-start重新置true，不存在"上次没释放污染这次"的情况），但仍
        // 顺手在这里补上正确的收尾语义：点击（松手）即视为拖拽结束。
        state_wclick.borrow_mut().writer_dragging = false;
        let app = match app_weak_wclick.upgrade() { Some(a) => a, None => return };
        let _scale = app.window().scale_factor();
        // v95修复：此前这里写的是"ROE坐标系=逻辑px，直接传"，是错误注释——
        // roe内部只认pt，x/y是Slint传入的逻辑px鼠标坐标，必须除以PT_TO_PX转成pt。
        // 这个bug此前一直未暴露，是因为方向键从未接线、点击定位效果没人仔细核对过。
        // v95再修正：Writer视口的Image用的是「pt数值直接当px用」（1:1，不乘PT_TO_PX），
        // 与calc那边不同——calc才是真正需要PT_TO_PX换算的地方。之前误套用了calc的换算，
        // 导致光标位置随字符偏移量累积性跑偏。这里直接1:1使用，不做换算。
        let x_pt = x;
        // v97修复：v95的注释和符号都错了。roe-api/src/tile.rs::roe_render_writer_viewport
        // 明确定义 doc_vp_y = vp_y - canvas_offset（画布坐标转文档坐标是减法：
        // "vp_y=0 对应画布顶部，vp_y=canvas_offset 对应第一页顶部"）。
        // 之前这里误写成加法，导致每次点击传给 roe_hit_test 的y比真实文档坐标
        // 大了 2×CANVAS_OFFSET_PT 量级的等效偏移（点击后又在query_cursor_rect里
        // 正确地+24转回画布坐标显示，两次符号错误的加法和一次本该的减法叠加，
        // 表现为点击命中位置系统性地偏向下方几行）。
        // 与 session.rs::writer_query_cursor_rect 的 +CANVAS_OFFSET_PT 不是同一个
        // 方向的操作：那边是"文档坐标->画布坐标"（加法，本来就没错，不用改），
        // 这里是"画布坐标->文档坐标"（减法）。
        use doc_core::CANVAS_OFFSET_PT;
        let y_pt = y - CANVAS_OFFSET_PT;
        let mut s = state_wclick.borrow_mut();
        // v300: 点中内联图片 → 显示选中框，不移动文字光标
        {
            let hit = s.session.as_mut().and_then(|sess| sess.writer_hit_object(x_pt, y_pt));
            if let Some((bi, ox, oy, ow, oh)) = hit {
                app.set_writer_obj_sel(bi as i32);
                app.set_writer_obj_x(ox);
                app.set_writer_obj_y(oy);
                app.set_writer_obj_w(ow);
                app.set_writer_obj_h(oh);
                app.set_writer_cursor_visible(false);
                return;
            }
            if app.get_writer_obj_sel() >= 0 { app.set_writer_obj_sel(-1); }
        }

        // v227: 公式命中 → 展开公式面板；点到别处则收起
        {
            let hit_box = s.session.as_mut().and_then(|sess| sess.formula_hit_box(x_pt as f64, y_pt as f64));
            let hit = hit_box.map(|b| (b.0, b.1));
            match hit_box {
                Some(b) => {
                    app.set_formula_box_x(b.2 as f32);
                    app.set_formula_box_y(b.3 as f32);
                    app.set_formula_box_w(b.4 as f32);
                    app.set_formula_box_h(b.5 as f32);
                    app.set_formula_box_visible(true);
                }
                None => { app.set_formula_box_visible(false); }
            }
            match hit {
                Some((p, r)) => {
                    let latex = s.session.as_mut().and_then(|sess| sess.formula_get(p, r)).unwrap_or_default();
                    eprintln!("[FORMULA-HIT] para={} run={} latex={:?}", p, r, latex);
                    app.set_formula_para(p as i32);
                    app.set_formula_run(r as i32);
                    app.set_formula_latex(latex.clone().into());
                    match formula_preview_image(&latex, 18.0) {
                        Ok(img) => { app.set_formula_preview_image(img); app.set_formula_error("".into()); }
                        Err(e) => { app.set_formula_preview_image(slint::Image::default()); app.set_formula_error(e.into()); }
                    }
                    app.set_sidebar_kind("formula".into());
                }
                None => {
                    if app.get_sidebar_kind().as_str() == "formula" {
                        app.set_sidebar_kind("none".into());
                    }
                }
            }
        }

        // v112：方向键是"编辑会话"的结束时机点之一（ROE_Progress_v112.md
        // 三.4）。清空Slint侧IME输入框内容（走clear-buffer()防重入保护，
        // 不直接对self.text赋值，避免二.3描述的递归edited问题）+ 重置
        // Rust侧会话状态，确保下次IME组合从当前光标位置重新开始一轮新会话，
        // 不会延续到已经不再相邻的旧起点。
        app.invoke_writer_ime_clear_buffer();
        s.writer_ime_session_start = None;
        s.writer_ime_session_prev_len = 0;
        if let Some(sess) = s.session.as_mut() {
            let (ct_ex, cid_ex, para_ex, off_ex) = sess.writer_hit_test_ex(x_pt, y_pt);
            if ct_ex == 1 {
                let (cx, cy, _cw, ch) = sess.writer_query_cursor_rect_ex(1, cid_ex, para_ex as u32, off_ex as u32);
                eprintln!("[V314-CELL] cid={} para={} off={} cursor x={:.1} y={:.1} h={:.1}", cid_ex, para_ex, off_ex, cx, cy, ch);
                app.set_writer_cursor_x(cx as i32);
                app.set_writer_cursor_y(cy as i32);
                app.set_writer_cursor_h(ch as i32);
                app.set_writer_cursor_visible(true);
                return;
            }
            let (para, offset) = sess.writer_hit_test(x_pt, y_pt);
            eprintln!("[WRITER-HIT] para={} offset={}", para, offset);
            if para >= 0 {
                eprintln!("[V96-DEBUG] handle={} 即将查询 para={} offset={}", sess.debug_handle(), para, offset);
                let (cx, cy, cw, ch) = sess.writer_query_cursor_rect(para as u64, offset as u64);
                eprintln!("[WRITER-CURSOR] x={:.1} y={:.1} w={:.1} h={:.1}", cx, cy, cw, ch);
                // v96调试：同一个offset立刻再query一次，看结果是否稳定
                let (cx2, cy2, cw2, ch2) = sess.writer_query_cursor_rect(para as u64, offset as u64);
                eprintln!("[V96-DEBUG] 复查同一offset: x={:.1} y={:.1} w={:.1} h={:.1} (与首次{}一致)", cx2, cy2, cw2, ch2, if (cx-cx2).abs() < 0.01 { "" } else { "不" });
                // 保存光标位置供键盘输入使用（sess借用在此之后不再使用）
                s.writer_cursor_para = para;
                s.writer_cursor_offset = offset;
                eprintln!("[V115-DIAG-CLICK] 写入后立刻读回: s.writer_cursor_para={} s.writer_cursor_offset={} (期望等于 para={} offset={})", s.writer_cursor_para, s.writer_cursor_offset, para, offset);
                // 点击是"非上下移动"操作，按final文档五.2规则，需要重新计算并覆盖ideal_x
                s.writer_cursor_ideal_x = Some(cx);
                // 点击会折叠选区：锚点=焦点
                s.writer_sel_anchor_para = para;
                s.writer_sel_anchor_offset = offset;
                // v95再修正：Writer视口坐标系是「pt数值直接当px用」，不乘PT_TO_PX
                // （writer-doc-width-pt这个property本身就是这么用的，见app.slint 1125/1127行）。
                app.set_writer_cursor_x(cx as i32);
                app.set_writer_cursor_y(cy as i32);
                app.set_writer_cursor_h(ch as i32);
                app.set_writer_cursor_visible(true);
            }
        }
    });

    // v300: Writer 图片缩放提交
    {
        let app_weak_or = app_weak.clone();
        let state_or = state.clone();
        app.on_writer_obj_resize(move |w, h| {
            let Some(app) = app_weak_or.upgrade() else { return };
            let idx = app.get_writer_obj_sel();
            if idx < 0 { return; }
            let res = {
                let s = state_or.borrow();
                s.session.as_ref().map(|x| x.writer_resize_object(idx as u32, w, h))
            };
            match res {
                Some(Ok(())) => {
                    {
                        let mut s = state_or.borrow_mut();
                        writer_refresh_canvas(&app, &mut s);
                    }
                    app.set_writer_needs_render(true);
                }
                Some(Err(e)) => app.set_status_message(format!("调整图片失败: {}", e).into()),
                None => {}
            }
        });
    }
    // v318n: 图表「编辑数据」小窗口（Writer）
    {
        const CD_STRIDE: usize = 13;
        const CD_MAXR: usize = 51;
        const CD_MAXC: usize = 12;

        // 打开：取 JSON → 填表格
        {
            let app_weak_cd = app_weak.clone();
            let state_cd = state.clone();
            app.on_chart_edit_data(move || {
                let Some(app) = app_weak_cd.upgrade() else { return };
                let is_imp_doc = {
                    let mut s = state_cd.borrow_mut();
                    s.session.as_mut().map(|x| x.get_doc_type() == "impress").unwrap_or(false)
                };
                let imp = is_imp_doc && app.get_impress_sel_id() >= 0 && app.get_impress_sel_is_chart();
                app.set_chart_dlg_impress(imp);
                let idx = app.get_writer_obj_sel();
                if idx < 0 && !imp { return; }
                let json = {
                    let mut s = state_cd.borrow_mut();
                    if imp {
                        let sl = (app.get_impress_current_slide() - 1).max(0) as u32;
                        let sid = app.get_impress_sel_id() as u32;
                        s.session.as_mut().and_then(|x| x.impress_get_chart_json(sl, sid))
                    } else {
                        s.session.as_mut().and_then(|x| x.writer_get_chart_json(idx as u32))
                    }
                };
                let Some(json) = json else {
                    app.set_status_message("该图表暂不支持编辑数据".into());
                    return;
                };
                let v: serde_json::Value = match serde_json::from_str(&json) {
                    Ok(v) => v,
                    Err(e) => {
                        app.set_status_message(format!("图表数据解析失败: {}", e).into());
                        return;
                    }
                };
                let labels: Vec<String> = v["x_labels"].as_array()
                    .map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect())
                    .unwrap_or_default();
                let series: Vec<serde_json::Value> = v["series"].as_array().cloned().unwrap_or_default();
                let kind = v["kind"].as_str().unwrap_or("").to_string();
                let single = kind == "Pie" || kind == "Doughnut";
                let maxdata = series.iter()
                    .map(|s| s["data"].as_array().map(|d| d.len()).unwrap_or(0))
                    .max().unwrap_or(0);
                let rows = labels.len().max(maxdata).clamp(1, CD_MAXR - 1);
                let cols = if single { 1 } else { series.len().clamp(1, CD_MAXC) };
                let mut cells = vec![slint::SharedString::default(); CD_STRIDE * CD_MAXR];
                for c in 0..cols {
                    if let Some(sr) = series.get(c) {
                        cells[c + 1] = sr["name"].as_str().unwrap_or("").into();
                        if let Some(d) = sr["data"].as_array() {
                            for (r, n) in d.iter().enumerate().take(rows) {
                                if let Some(f) = n.as_f64() {
                                    cells[(r + 1) * CD_STRIDE + c + 1] = format!("{}", f).into();
                                }
                            }
                        }
                    }
                }
                for r in 0..rows {
                    if let Some(l) = labels.get(r) {
                        cells[(r + 1) * CD_STRIDE] = l.as_str().into();
                    }
                }
                app.set_chart_dlg_rows(rows as i32);
                app.set_chart_dlg_cols(cols as i32);
                app.set_chart_dlg_single(single);
                app.set_chart_dlg_cells(slint::ModelRc::new(slint::VecModel::from(cells)));
                app.set_chart_dlg_gen(app.get_chart_dlg_gen() + 1);
                app.set_chart_dlg_visible(true);
            });
        }

        // 加行/加列/删行/删列
        {
            let app_weak_cr = app_weak.clone();
            app.on_chart_dlg_resize(move |dr, dc| {
                let Some(app) = app_weak_cr.upgrade() else { return };
                let rows = (app.get_chart_dlg_rows() + dr).clamp(1, (CD_MAXR - 1) as i32);
                let mut cols = (app.get_chart_dlg_cols() + dc).clamp(1, CD_MAXC as i32);
                if app.get_chart_dlg_single() { cols = 1; }
                let cells = app.get_chart_dlg_cells();
                for r in 0..CD_MAXR {
                    for c in 0..CD_STRIDE {
                        if r > rows as usize || c > cols as usize {
                            slint::Model::set_row_data(&cells, r * CD_STRIDE + c, slint::SharedString::default());
                        }
                    }
                }
                app.set_chart_dlg_rows(rows);
                app.set_chart_dlg_cols(cols);
                app.set_chart_dlg_gen(app.get_chart_dlg_gen() + 1);
            });
        }

        // 行列互换
        {
            let app_weak_ct = app_weak.clone();
            app.on_chart_dlg_transpose(move || {
                let Some(app) = app_weak_ct.upgrade() else { return };
                let rows = app.get_chart_dlg_rows() as usize;
                let cols = app.get_chart_dlg_cols() as usize;
                if cols > CD_MAXR - 1 || rows > CD_MAXC {
                    app.set_status_message("行列互换后超出 50 行 × 12 列上限".into());
                    return;
                }
                let cells = app.get_chart_dlg_cells();
                let mut out = vec![slint::SharedString::default(); CD_STRIDE * CD_MAXR];
                for r in 0..=rows {
                    for c in 0..=cols {
                        if let Some(t) = slint::Model::row_data(&cells, r * CD_STRIDE + c) {
                            out[c * CD_STRIDE + r] = t;
                        }
                    }
                }
                app.set_chart_dlg_cells(slint::ModelRc::new(slint::VecModel::from(out)));
                app.set_chart_dlg_rows(cols as i32);
                app.set_chart_dlg_cols(rows as i32);
                app.set_chart_dlg_gen(app.get_chart_dlg_gen() + 1);
            });
        }

        // 确定：表格 → JSON → 写回 → 刷新
        {
            let app_weak_cc = app_weak.clone();
            let state_cc = state.clone();
            app.on_chart_dlg_confirm(move || {
                let Some(app) = app_weak_cc.upgrade() else { return };
                let imp = app.get_chart_dlg_impress();
                let idx = app.get_writer_obj_sel();
                if idx < 0 && !imp {
                    app.set_chart_dlg_visible(false);
                    return;
                }
                let json = {
                    let mut s = state_cc.borrow_mut();
                    if imp {
                        let sl = (app.get_impress_current_slide() - 1).max(0) as u32;
                        let sid = app.get_impress_sel_id() as u32;
                        s.session.as_mut().and_then(|x| x.impress_get_chart_json(sl, sid))
                    } else {
                        s.session.as_mut().and_then(|x| x.writer_get_chart_json(idx as u32))
                    }
                };
                let Some(json) = json else {
                    app.set_status_message("读取图表失败，未写回".into());
                    return;
                };
                let mut v: serde_json::Value = match serde_json::from_str(&json) {
                    Ok(v) => v,
                    Err(e) => {
                        app.set_status_message(format!("图表数据解析失败: {}", e).into());
                        return;
                    }
                };
                let old: Vec<serde_json::Value> = v["series"].as_array().cloned().unwrap_or_default();
                let rows = app.get_chart_dlg_rows().max(1) as usize;
                let cols = app.get_chart_dlg_cols().max(1) as usize;
                let cells = app.get_chart_dlg_cells();
                let get = |r: usize, c: usize| -> String {
                    slint::Model::row_data(&cells, r * CD_STRIDE + c)
                        .map(|s| s.to_string())
                        .unwrap_or_default()
                };
                let labels: Vec<serde_json::Value> = (0..rows)
                    .map(|r| serde_json::Value::String(get(r + 1, 0)))
                    .collect();
                let mut new_series: Vec<serde_json::Value> = Vec::new();
                for c in 0..cols {
                    let mut item = match old.get(c) {
                        Some(o) => o.clone(),
                        None => {
                            let mut o = old.last().cloned()
                                .unwrap_or_else(|| serde_json::json!({"name": "", "data": []}));
                            o["color"] = serde_json::Value::Null;
                            o["trendline"] = serde_json::Value::Bool(false);
                            o
                        }
                    };
                    let mut name = get(0, c + 1).trim().to_string();
                    if name.is_empty() { name = format!("系列{}", c + 1); }
                    let data: Vec<serde_json::Value> = (0..rows).map(|r| {
                        let f = get(r + 1, c + 1).trim().parse::<f64>().unwrap_or(0.0);
                        serde_json::json!(if f.is_finite() { f } else { 0.0 })
                    }).collect();
                    item["name"] = serde_json::Value::String(name);
                    item["data"] = serde_json::Value::Array(data);
                    new_series.push(item);
                }
                v["x_labels"] = serde_json::Value::Array(labels);
                v["series"] = serde_json::Value::Array(new_series);
                let out = match serde_json::to_string(&v) {
                    Ok(s) => s,
                    Err(e) => {
                        app.set_status_message(format!("序列化失败: {}", e).into());
                        return;
                    }
                };
                if imp {
                    let sl = (app.get_impress_current_slide() - 1).max(0) as u32;
                    let sid = app.get_impress_sel_id() as u32;
                    let ok = {
                        let mut s = state_cc.borrow_mut();
                        match s.session.as_mut() {
                            Some(x) => {
                                let done = x.impress_set_chart_json(sl, sid, &out);
                                if done { x.commit_edit(); }
                                done
                            }
                            None => false,
                        }
                    };
                    if ok {
                        app.set_chart_dlg_visible(false);
                        app.invoke_impress_render_viewport(0.0);
                    } else {
                        app.set_status_message("写回图表数据失败".into());
                    }
                    return;
                }
                let res = {
                    let mut s = state_cc.borrow_mut();
                    s.session.as_mut().map(|x| x.writer_set_chart_json(idx as u32, &out))
                };
                match res {
                    Some(Ok(_)) => {
                        app.set_chart_dlg_visible(false);
                        {
                            let mut s = state_cc.borrow_mut();
                            writer_refresh_canvas(&app, &mut s);
                        }
                        app.set_writer_needs_render(true);
                    }
                    Some(Err(e)) => app.set_status_message(format!("写回图表数据失败: {}", e).into()),
                    None => {}
                }
            });
        }
    }
    // v318n9: Impress 选中 id 变化时探测是否普通图表
    {
        let app_weak_pc = app_weak.clone();
        let state_pc = state.clone();
        app.on_impress_probe_chart(move || {
            let Some(app) = app_weak_pc.upgrade() else { return };
            let id = app.get_impress_sel_id();
            if id < 0 { app.set_impress_sel_is_chart(false); return; }
            let sl = (app.get_impress_current_slide() - 1).max(0) as u32;
            let is_chart = match state_pc.try_borrow_mut() {
                Ok(mut s) => s.session.as_mut()
                    .and_then(|x| x.impress_get_chart_json(sl, id as u32)).is_some(),
                Err(_) => false,
            };
            app.set_impress_sel_is_chart(is_chart);
        });
    }
    // v309: Writer 插入表格
    {
        let app_weak_it = app_weak.clone();
        let state_it = state.clone();
        app.on_writer_insert_table(move |rows, cols| {
            let Some(app) = app_weak_it.upgrade() else { return };
            let after = {
                let s = state_it.borrow();
                s.writer_cursor_para.max(0) as u32
            };
            let res = {
                let s = state_it.borrow();
                s.session.as_ref().map(|x| x.writer_insert_table(after, rows as u32, cols as u32))
            };
            match res {
                Some(Ok(_)) => {
                    {
                        let mut s = state_it.borrow_mut();
                        writer_refresh_canvas(&app, &mut s);
                    }
                    app.set_writer_needs_render(true);
                }
                Some(Err(e)) => app.set_status_message(format!("插入表格失败: {}", e).into()),
                None => {}
            }
        });
    }
    // v308: Writer 对象旋转提交
    {
        let app_weak_ro = app_weak.clone();
        let state_ro = state.clone();
        app.on_writer_obj_rotate(move |deg| {
            let Some(app) = app_weak_ro.upgrade() else { return };
            let idx = app.get_writer_obj_sel();
            if idx < 0 { return; }
            let res = {
                let s = state_ro.borrow();
                s.session.as_ref().map(|x| x.writer_set_object_rotation(idx as u32, deg))
            };
            match res {
                Some(Ok(())) => {
                    {
                        let mut s = state_ro.borrow_mut();
                        writer_refresh_canvas(&app, &mut s);
                    }
                    app.set_writer_needs_render(true);
                }
                Some(Err(e)) => app.set_status_message(format!("旋转失败: {}", e).into()),
                None => {}
            }
        });
    }
    // v305: Writer 选中对象变化 → 决定是否显示「形状格式」tab
    {
        let app_weak_sc = app_weak.clone();
        let state_sc = state.clone();
        app.on_writer_obj_sel_changed(move |idx| {
            let Some(app) = app_weak_sc.upgrade() else { return };
            let kind = if idx < 0 { 0 } else {
                state_sc.try_borrow().ok()
                    .and_then(|s| s.session.as_ref().map(|x| x.writer_object_kind(idx as u32)))
                    .unwrap_or(0)
            };
            let is_shape = kind == 2;
            app.set_writer_obj_is_shape(is_shape);
            app.set_writer_obj_is_chart(kind == 3);
            let rot = if idx < 0 { 0.0 } else {
                state_sc.try_borrow().ok()
                    .and_then(|s| s.session.as_ref().map(|x| x.writer_object_rotation(idx as u32)))
                    .unwrap_or(0.0)
            };
            app.set_writer_obj_rot(rot);
            if is_shape {
                app.set_active_ribbon_tab("shapefmt".into());
            } else if app.get_active_ribbon_tab().as_str() == "shapefmt" {
                app.set_active_ribbon_tab("home".into());
            }
        });
    }
    // v305: Writer 形状样式（填充/边框颜色/边框粗细）
    {
        let app_weak_ss = app_weak.clone();
        let state_ss = state.clone();
        app.on_writer_shape_style(move |cmd| {
            let Some(app) = app_weak_ss.upgrade() else { return };
            let idx = app.get_writer_obj_sel();
            if idx < 0 { return; }
            let cmd = cmd.to_string();
            let (k, v) = match cmd.split_once(':') { Some(p) => p, None => return };
            let (fill, stroke, w) = match k {
                "fill" => (v, "", -1.0f32),
                "line" => ("", v, -1.0f32),
                "width" => ("", "", v.parse::<f32>().unwrap_or(-1.0)),
                _ => return,
            };
            let res = {
                let s = state_ss.borrow();
                s.session.as_ref().map(|x| x.writer_set_shape_style(idx as u32, fill, stroke, w))
            };
            match res {
                Some(Ok(())) => {
                    {
                        let mut s = state_ss.borrow_mut();
                        writer_refresh_canvas(&app, &mut s);
                    }
                    app.set_writer_needs_render(true);
                }
                Some(Err(e)) => app.set_status_message(format!("设置形状样式失败: {}", e).into()),
                None => {}
            }
        });
    }
    // v318o: Impress 图片背景
    {
        let app_weak_bi = app_weak.clone();
        let state_bi = state.clone();
        app.on_impress_bg_image(move || {
            let Some(app) = app_weak_bi.upgrade() else { return };
            let file = rfd::FileDialog::new().add_filter("图片", &["png", "jpg", "jpeg"]).pick_file();
            let Some(path) = file else { return };
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => { app.set_status_message(format!("读取图片失败: {}", e).into()); return; }
            };
            let is_png = bytes.starts_with(&[0x89, b'P', b'N', b'G']);
            let mime = if is_png { "image/png" } else { "image/jpeg" };
            if app.get_impress_master_mode() {
            let m_row = app.get_impress_master_sel();
            let m_mi = app.get_impress_master_mi().row_data(m_row as usize).unwrap_or(0) as u32;
            let m_li = app.get_impress_master_li().row_data(m_row as usize).unwrap_or(-1);
                let ok = {
                    let mut s = state_bi.borrow_mut();
                    let sess = match s.session.as_mut() { Some(x) => x, None => return };
                    sess.begin_edit();
                    let done = sess.impress_master_set_background_image(m_mi, m_li, &bytes, mime);
                    if done { sess.commit_edit(); } else { sess.discard_edit(); }
                    done
                };
                if ok {
                    show_master_canvas(&app, &state_bi, m_row);
                    refresh_master_thumb_one(&app, &state_bi, m_row);
                } else { app.set_status_message("设置图片背景失败".into()); }
                return;
            }
            let slide = (app.get_impress_current_slide() - 1).max(0) as u32;
            let ok = {
                let mut s = state_bi.borrow_mut();
                let sess = match s.session.as_mut() { Some(x) => x, None => return };
                if sess.get_doc_type() != "impress" { return; }
                sess.begin_edit();
                let done = sess.impress_set_slide_background_image(slide, &bytes, mime);
                if done { sess.commit_edit(); } else { sess.discard_edit(); }
                done
            };
            if ok { app.invoke_impress_render_viewport(0.0); }
            else { app.set_status_message("设置图片背景失败".into()); }
        });
    }
    // v318o: Impress 形状样式 / 幻灯片背景（fill:#rrggbb fill:none line:#rrggbb line:none width:n bg:#rrggbb bg:none）
    {
        let app_weak_is = app_weak.clone();
        let state_is = state.clone();
        app.on_impress_style(move |cmd| {
            let Some(app) = app_weak_is.upgrade() else { return };
            let cmd = cmd.to_string();
            let (k, v) = match cmd.split_once(':') { Some(p) => p, None => return };
            if v.starts_with("grad:") && k != "bg" {
                app.set_status_message("Impress 暂不支持渐变填充".into());
                return;
            }
            if app.get_impress_master_mode() {
            let m_row = app.get_impress_master_sel();
            let m_mi = app.get_impress_master_mi().row_data(m_row as usize).unwrap_or(0) as u32;
            let m_li = app.get_impress_master_li().row_data(m_row as usize).unwrap_or(-1);
                if k != "bg" {
                    app.set_status_message("母版视图暂不支持形状填充和线条样式".into());
                    return;
                }
                if v.starts_with("grad:") {
                    app.set_status_message("母版背景暂不支持渐变".into());
                    return;
                }
                let ok = {
                    let mut s = state_is.borrow_mut();
                    let sess = match s.session.as_mut() { Some(x) => x, None => return };
                    sess.begin_edit();
                    let done = sess.impress_master_set_background(m_mi, m_li, v);
                    if done { sess.commit_edit(); } else { sess.discard_edit(); }
                    done
                };
                if ok {
                    show_master_canvas(&app, &state_is, m_row);
                    refresh_master_thumb_one(&app, &state_is, m_row);
                } else { app.set_status_message("设置背景失败".into()); }
                return;
            }
            let slide = (app.get_impress_current_slide() - 1).max(0) as u32;
            let sel = app.get_impress_sel_id();
            let ok = {
                let mut s = state_is.borrow_mut();
                let sess = match s.session.as_mut() { Some(x) => x, None => return };
                if sess.get_doc_type() != "impress" { return; }
                if k != "bg" && sel < 0 { return; }
                sess.begin_edit();
                let done = match k {
                    "fill" => sess.impress_set_shape_style(slide, sel as u32, v, "", -1.0),
                    "line" => sess.impress_set_shape_style(slide, sel as u32, "", v, -1.0),
                    "width" => sess.impress_set_shape_style(slide, sel as u32, "", "", v.parse::<f32>().unwrap_or(-1.0)),
                    "bg" => sess.impress_set_slide_background(slide, v),
                    _ => false,
                };
                if done { sess.commit_edit(); } else { sess.discard_edit(); }
                done
            };
            if ok {
                app.invoke_impress_render_viewport(0.0);
            } else {
                app.set_status_message("设置样式失败".into());
            }
        });
    }
    // v318r: Impress 插入表格（居中，宽为页宽 80%，行高 28pt）
    {
        let app_weak_it2 = app_weak.clone();
        let state_it2 = state.clone();
        app.on_impress_insert_table(move |rows, cols| {
            let Some(app) = app_weak_it2.upgrade() else { return };
            let slide = (app.get_impress_current_slide() - 1).max(0) as u32;
            let mut cw = app.get_impress_canvas_width_pt() as f32;
            let mut ch = app.get_impress_canvas_height_pt() as f32;
            if cw <= 0.0 { cw = 720.0; }
            if ch <= 0.0 { ch = 405.0; }
            let w = cw * 0.8;
            let h = (rows as f32 * 28.0).min(ch * 0.8);
            let x = (cw - w) / 2.0;
            let y = ((ch - h) / 2.0).max(0.0);
            let id = {
                let mut s = state_it2.borrow_mut();
                let sess = match s.session.as_mut() { Some(v) => v, None => return };
                if sess.get_doc_type() != "impress" { return; }
                sess.begin_edit();
                let id = sess.impress_insert_table(slide, rows as u32, cols as u32, x, y, w, h);
                if id >= 0 { sess.commit_edit(); } else { sess.discard_edit(); }
                id
            };
            if id >= 0 {
                app.set_impress_sel_id(id);
                app.invoke_impress_render_viewport(0.0);
            } else {
                app.set_status_message("插入表格失败".into());
            }
        });
    }
    // v318q: Impress 文字样式（整个形状）
    {
        let app_weak_ts = app_weak.clone();
        let state_ts = state.clone();
        let ite_ts = ite.clone();
        app.on_impress_text_style(move |cmd| {
            let Some(app) = app_weak_ts.upgrade() else { return };
            let cmd = cmd.to_string();
            let slide = (app.get_impress_current_slide() - 1).max(0) as u32;
            let sel = app.get_impress_sel_id();
            if sel < 0 {
                app.set_status_message("请先选中文本框".into());
                return;
            }
            let ok = {
                let mut s = state_ts.borrow_mut();
                let sess = match s.session.as_mut() { Some(x) => x, None => return };
                if sess.get_doc_type() != "impress" { return; }
                let editing = app.get_impress_text_edit_id() == sel;
                let range = if editing {
                    let st = ite_ts.borrow();
                    st.anchor.and_then(|a| {
                        let c = (st.para, st.offset);
                        if a == c { None } else if a <= c { Some((a, c)) } else { Some((c, a)) }
                    })
                } else { None };
                if !editing { sess.begin_edit(); }
                if editing { if app.get_impress_text_edit_dirty() { sess.commit_edit(); sess.begin_edit(); app.set_impress_text_edit_dirty(false); ite_ts.borrow_mut().is_new = false; } let mut stx = ite_ts.borrow_mut(); stx.start = None; stx.prev.clear(); app.set_impress_text_clear_seq(app.get_impress_text_clear_seq() + 1); }
let done = match range {
                    Some((a, c)) => sess.impress_set_text_style_range(slide, sel as u32, a.0, a.1, c.0, c.1, &cmd),
                    None => sess.impress_set_text_style(slide, sel as u32, &cmd),
                };
                if !editing {
                    if done { sess.commit_edit(); } else { sess.discard_edit(); }
                } else if done {
                    app.set_impress_text_edit_dirty(true);
                    let st = ite_ts.borrow();
                    impress_caret_update!(app, sess, slide, sel as u32, st.para, st.offset);
                    impress_sel_update!(app, sess, slide, sel as u32, st);
                }
                done
            };
            if ok { app.invoke_impress_render_viewport(0.0); }
            else { app.set_status_message("设置文字样式失败".into()); }
        });
    }
    // v302: Writer 画图提交 → 插入形状并选中
    {
        let app_weak_ds = app_weak.clone();
        let state_ds = state.clone();
        app.on_writer_draw_committed(move |kind, x1, y1, x2, y2| {
            let Some(app) = app_weak_ds.upgrade() else { return };
            let kind = kind.to_string();
            let is_straight = kind == "line" || kind == "lineWithArrow" || kind == "lineWithTwoArrows";
            let dist = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
            let (mut ax1, mut ay1, mut ax2, mut ay2) = (x1, y1, x2, y2);
            if dist < 4.0 {
                if is_straight { ax2 = ax1 + 120.0; ay2 = ay1; }
                else { ax1 = x1 - 60.0; ay1 = y1 - 40.0; ax2 = x1 + 60.0; ay2 = y1 + 40.0; }
            }
            let rx = ax1.min(ax2).max(0.0);
            let ry = (ay1.min(ay2) - 24.0).max(0.0);
            let rw = (ax2 - ax1).abs().max(8.0);
            let rh = (ay2 - ay1).abs().max(if is_straight { 1.0 } else { 8.0 });
            let res = {
                let s = state_ds.borrow();
                s.session.as_ref().map(|x| x.writer_insert_shape(&kind, rx, ry, rw, rh))
            };
            match res {
                Some(Ok(idx)) => {
                    let hit = {
                        let mut s = state_ds.borrow_mut();
                        s.session.as_mut().and_then(|x| x.writer_hit_object(rx + rw / 2.0, ry + rh / 2.0))
                    };
                    {
                        let mut s = state_ds.borrow_mut();
                        if s.writer_cursor_para >= idx as i64 { s.writer_cursor_para += 1; }
                        if s.writer_sel_anchor_para >= idx as i64 { s.writer_sel_anchor_para += 1; }
                        writer_refresh_canvas(&app, &mut s);
                    }
                    if let Some((bi, ox, oy, ow, oh)) = hit {
                        if bi == idx {
                            app.set_writer_obj_sel(bi as i32);
                            app.set_writer_obj_x(ox);
                            app.set_writer_obj_y(oy);
                            app.set_writer_obj_w(ow);
                            app.set_writer_obj_h(oh);
                            app.set_writer_cursor_visible(false);
                        }
                    }
                    app.set_writer_needs_render(true);
                    app.set_status_message("已插入形状".into());
                }
                Some(Err(e)) => app.set_status_message(format!("插入形状失败: {}", e).into()),
                None => {}
            }
        });
    }
    // v228: Ribbon「公式」→ 在光标处插入占位公式并展开面板
    {
        let app_weak_fi = app_weak.clone();
        let state_fi = state.clone();
        app.on_writer_insert_formula(move || {
            let Some(app) = app_weak_fi.upgrade() else { return };
            let (p, o) = {
                let s = state_fi.borrow();
                (s.writer_cursor_para, s.writer_cursor_offset)
            };
            if p < 0 || o < 0 {
                app.set_status_message("插入公式失败: 没有可插入的光标位置".into());
                return;
            }
            eprintln!("[FORMULA-INSERT] 待插入占位 para={} offset={}", p, o);
            app.set_formula_box_visible(false);
            app.set_formula_para(p as i32);
            app.set_formula_run(-1);
            app.set_formula_offset(o as i32);
            app.set_formula_latex("".into());
            app.set_formula_preview_image(slint::Image::default());
            app.set_formula_error("".into());
            app.set_sidebar_kind("formula".into());
        });
    }
    // v227: 公式面板回调 —— 输入时刷新预览；应用时写回文档
    {
        let app_weak_fe = app_weak.clone();
        app.on_formula_latex_edited(move |text| {
            let Some(app) = app_weak_fe.upgrade() else { return };
            match formula_preview_image(text.as_str(), 18.0) {
                Ok(img) => { app.set_formula_preview_image(img); app.set_formula_error("".into()); }
                Err(e) => {
                    if e.is_empty() { app.set_formula_preview_image(slint::Image::default()); }
                    app.set_formula_error(e.into());
                }
            }
        });
    }
    {
        let app_weak_fa = app_weak.clone();
        let state_fa = state.clone();
        app.on_formula_apply(move |text| {
            let Some(app) = app_weak_fa.upgrade() else { return };
            let p = app.get_formula_para();
            let r = app.get_formula_run();
            let off = app.get_formula_offset();
            if p < 0 { return; }
            let res: Result<(u32, u32), String> = {
                let mut s = state_fa.borrow_mut();
                match s.session.as_mut() {
                    Some(sess) => {
                        if r < 0 {
                            if off < 0 {
                                Err("没有可插入的位置".to_string())
                            } else {
                                sess.formula_insert(p as u32, off as u32, text.as_str())
                            }
                        } else {
                            sess.formula_set(p as u32, r as u32, text.as_str()).map(|_| (p as u32, r as u32))
                        }
                    }
                    None => Err("没有打开的文档".to_string()),
                }
            };
            match res {
                Ok((np, nr)) => {
                    app.set_formula_para(np as i32);
                    app.set_formula_run(nr as i32);
                    eprintln!("[FORMULA-APPLY] OK para={} run={}", np, nr);
                    app.set_formula_error("".into());
                    app.set_writer_needs_render(true);
                }
                Err(e) => {
                    eprintln!("[FORMULA-APPLY] 失败: {}", e);
                    app.set_formula_error(e.into());
                }
            }
        });
    }

    // ── Writer 拖拽选区（本次新增）AB测试基线版 ────────────────────────────────
    // on_writer_drag_start：鼠标左键按下，记录锚点=焦点（折叠选区起点）。
    // on_writer_drag_move：拖拽中持续触发，更新焦点、查询选区矩形、渲染高亮。
    // 节流开关 writer_drag_throttle_enabled 默认为false（版本A基线：每次moved都查）。
    // 埋点：每次真正触发query_selection_rects调用打印耗时，供AB对比。
    // ── v245 查找替换 ──
    macro_rules! find_select {
        ($app:expr, $st:expr, $p:expr, $a:expr, $b:expr) => {{
            let mut s = $st.borrow_mut();
            let (cx, cy, ch, rects_json) = {
                let Some(sess) = s.session.as_mut() else { return };
                let (cx, cy, _cw, ch) = sess.writer_query_cursor_rect($p as u64, $b as u64);
                let rj = sess.query_selection_rects($p as u32, $a as u32, $p as u32, $b as u32);
                (cx, cy, ch, rj)
            };
            s.writer_sel_anchor_para = $p as _;
            s.writer_sel_anchor_offset = $a as _;
            s.writer_cursor_para = $p as _;
            s.writer_cursor_offset = $b as _;
            s.writer_cursor_ideal_x = Some(cx);
            drop(s);
            $app.set_writer_cursor_x(cx as i32);
            $app.set_writer_cursor_y(cy as i32);
            $app.set_writer_cursor_h(ch as i32);
            $app.set_writer_cursor_visible(true);
            let mut items: Vec<SelectionRect> = Vec::new();
            {
                let sc: f32 = $app.get_writer_scroll_y_pt();
                let vh: f32 = $app.get_writer_view_height() as f32;
                let canvas: f32 = $app.get_writer_canvas_height_pt();
                let cyf = cy as f32;
                let chf = ch as f32;
                if vh > 0.0 && (cyf < sc + 20.0 || cyf + chf > sc + vh - 20.0) {
                    let max_scroll = (canvas - vh).max(0.0);
                    let target = (cyf - vh * 0.35).max(0.0).min(max_scroll);
                    $app.invoke_scroll_writer_to(target);
                }
            }
            if let Some(json) = rects_json {
                if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&json) {
                    for r in arr {
                        let rx = r.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let ry = r.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32 + 24.0;
                        let rw = r.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let rh = r.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        items.push(SelectionRect { rx, ry, rw, rh });
                    }
                }
            }
            $app.set_writer_selection_rects(slint::ModelRc::new(slint::VecModel::from(items)));
        }};
    }
    macro_rules! find_run {
        ($app:expr, $st:expr, $q:expr, $forward:expr, $from:expr) => {{
            let q: String = $q;
            let (cs, ww, rx) = ($app.get_find_match_case(), $app.get_find_whole_word(), $app.get_find_regex());
            let res = {
                let mut s = $st.borrow_mut();
                let from: (i64, i64) = match $from {
                    Some(f) => f,
                    None => {
                        let a = (s.writer_sel_anchor_para as i64, s.writer_sel_anchor_offset as i64);
                        let c = (s.writer_cursor_para as i64, s.writer_cursor_offset as i64);
                        let (lo, hi) = if a <= c { (a, c) } else { (c, a) };
                        let f = if $forward { hi } else { lo };
                        if f.0 < 0 { (0, 0) } else { f }
                    }
                };
                let Some(sess) = s.session.as_mut() else { return };
                let r = sess.writer_find(&q, cs, ww, rx, from.0 as u32, from.1 as u32, $forward);
                r
            };
            match res {
                Ok(Some((p, a, b, idx, total))) => {
                    find_select!($app, $st, p, a, b);
                    $app.set_status_message(format!("第 {} 处，共 {} 处", idx, total).into());
                }
                Ok(None) => { $app.set_status_message("未找到".into()); }
                Err(e) => { $app.set_status_message(format!("查找失败: {}", e).into()); }
            }
        }};
    }
    {
        let st = state.clone();
        let aw = app_weak.clone();
        app.on_find_next(move |q| {
            let Some(app) = aw.upgrade() else { return };
            if q.is_empty() { app.set_status_message("请输入要查找的内容".into()); return; }
            find_run!(app, st, q.to_string(), true, None);
        });
    }
    {
        let st = state.clone();
        let aw = app_weak.clone();
        app.on_find_prev(move |q| {
            let Some(app) = aw.upgrade() else { return };
            if q.is_empty() { app.set_status_message("请输入要查找的内容".into()); return; }
            find_run!(app, st, q.to_string(), false, None);
        });
    }
    {
        let st = state.clone();
        let aw = app_weak.clone();
        app.on_replace_one(move |f, r| {
            let Some(app) = aw.upgrade() else { return };
            let q = f.to_string();
            let rep = r.to_string();
            if q.is_empty() { app.set_status_message("请输入要查找的内容".into()); return; }
            let (cs, ww, rx) = (app.get_find_match_case(), app.get_find_whole_word(), app.get_find_regex());
            let sel = {
                let s = st.borrow();
                let a = (s.writer_sel_anchor_para as i64, s.writer_sel_anchor_offset as i64);
                let c = (s.writer_cursor_para as i64, s.writer_cursor_offset as i64);
                if a <= c { (a, c) } else { (c, a) }
            };
            let mut next_from: Option<(i64, i64)> = None;
            if sel.0.0 >= 0 && sel.0.0 == sel.1.0 && sel.0 != sel.1 {
                let hit = {
                    let s = st.borrow();
                    match s.session.as_ref() {
                        Some(sess) => sess.writer_find(&q, cs, ww, rx, sel.0.0 as u32, sel.0.1 as u32, true),
                        None => return,
                    }
                };
                if let Ok(Some((p, a, b, _, _))) = hit {
                    if p as i64 == sel.0.0 && a as i64 == sel.0.1 && b as i64 == sel.1.1 {
                        let ok = {
                            let s = st.borrow();
                            s.session.as_ref().and_then(|x| x.replace_range(p, a, b, &rep)).is_some()
                        };
                        if ok {
                            app.set_writer_needs_render(true);
                            next_from = Some((p as i64, a as i64 + rep.len() as i64));
                        }
                    }
                }
            }
            find_run!(app, st, q, true, next_from);
        });
    }
    {
        let st = state.clone();
        let aw = app_weak.clone();
        app.on_replace_all(move |f, r| {
            let Some(app) = aw.upgrade() else { return };
            let q = f.to_string();
            let rep = r.to_string();
            if q.is_empty() { app.set_status_message("请输入要查找的内容".into()); return; }
            let (cs, ww, rx) = (app.get_find_match_case(), app.get_find_whole_word(), app.get_find_regex());
            let res = {
                let s = st.borrow();
                match s.session.as_ref() {
                    Some(sess) => sess.writer_replace_all(&q, &rep, cs, ww, rx),
                    None => return,
                }
            };
            match res {
                Ok(n) if n > 0 => {
                    app.set_writer_needs_render(true);
                    app.set_status_message(format!("已替换 {} 处", n).into());
                }
                Ok(_) => { app.set_status_message("未找到".into()); }
                Err(e) => { app.set_status_message(format!("替换失败: {}", e).into()); }
            }
        });
    }

    let state_wdragstart = state.clone();
    let app_weak_wdragstart = app_weak.clone();
    app.on_writer_drag_start(move |x, y| {
        let app = match app_weak_wdragstart.upgrade() { Some(a) => a, None => return };
        let x_pt = x;
        use doc_core::CANVAS_OFFSET_PT;
        let y_pt = y - CANVAS_OFFSET_PT;
        let mut s = state_wdragstart.borrow_mut();
        s.writer_drag_origin = Some((x, y));
        s.writer_cell_cursor = None;
        s.writer_cell_sel_anchor = None;
        s.writer_cell_sel_focus = None;

        // v112：方向键是"编辑会话"的结束时机点之一（ROE_Progress_v112.md
        // 三.4）。清空Slint侧IME输入框内容（走clear-buffer()防重入保护，
        // 不直接对self.text赋值，避免二.3描述的递归edited问题）+ 重置
        // Rust侧会话状态，确保下次IME组合从当前光标位置重新开始一轮新会话，
        // 不会延续到已经不再相邻的旧起点。
        app.invoke_writer_ime_clear_buffer();
        s.writer_ime_session_start = None;
        s.writer_ime_session_prev_len = 0;
        if let Some(sess) = s.session.as_mut() {
            let (ct_ex, cid_ex, para_ex, off_ex) = sess.writer_hit_test_ex(x_pt, y_pt);
            if ct_ex == 1 {
                let (cx, cy, _cw, ch) = sess.writer_query_cursor_rect_ex(1, cid_ex, para_ex as u32, off_ex as u32);
                eprintln!("[V314-CELL-DOWN] cid={} para={} off={} x={:.1} y={:.1} h={:.1}", cid_ex, para_ex, off_ex, cx, cy, ch);
                s.writer_cell_cursor = Some((cid_ex, para_ex, off_ex));
                s.writer_cell_sel_anchor = Some((cid_ex, para_ex, off_ex));
                s.writer_dragging = true;
                app.set_writer_cursor_x(cx as i32);
                app.set_writer_cursor_y(cy as i32);
                app.set_writer_cursor_h(ch as i32);
                app.set_writer_cursor_visible(true);
                app.set_writer_selection_rects(slint::ModelRc::new(slint::VecModel::from(Vec::<SelectionRect>::new())));
                return;
            }
            let (para, offset) = sess.writer_hit_test(x_pt, y_pt);
            eprintln!("[WRITER-DRAG-START] x={:.1} y={:.1} -> para={} offset={}", x, y, para, offset);
            if para >= 0 {
                let (cx, cy, _cw, ch) = sess.writer_query_cursor_rect(para as u64, offset as u64);
                s.writer_cursor_para = para;
                s.writer_cursor_offset = offset;
                s.writer_cursor_ideal_x = Some(cx);
                s.writer_sel_anchor_para = para;
                s.writer_sel_anchor_offset = offset;
                s.writer_dragging = true;
                s.writer_drag_query_count = 0;
                s.writer_drag_last_query_para = para;
                s.writer_drag_last_query_offset = offset;
                app.set_writer_cursor_x(cx as i32);
                app.set_writer_cursor_y(cy as i32);
                app.set_writer_cursor_h(ch as i32);
                app.set_writer_cursor_visible(true);
                // 刚开始拖拽，锚点=焦点，选区为空，清空高亮
                app.set_writer_selection_rects(slint::ModelRc::new(slint::VecModel::from(Vec::<SelectionRect>::new())));
            }
        }
    });

    let state_wdragmove = state.clone();
    let app_weak_wdragmove = app_weak.clone();
    app.on_writer_drag_move(move |x, y| {
        let app = match app_weak_wdragmove.upgrade() { Some(a) => a, None => return };
        let mut s = state_wdragmove.borrow_mut();
        if !s.writer_dragging {
            return;
        }
        let x_pt = x;
        use doc_core::CANVAS_OFFSET_PT;
        let y_pt = y - CANVAS_OFFSET_PT;
        if let Some((acid, ap, ao)) = s.writer_cell_sel_anchor {
            let mut res: Option<(i64, i64, (f32, f32, f32, f32), Option<String>)> = None;
            let mut fcid: u32 = acid;
            if let Some(sess) = s.session.as_mut() {
                let (ct_ex, cid_ex, para_ex, off_ex) = sess.writer_hit_test_ex(x_pt, y_pt);
                if ct_ex == 1 && cid_ex != acid && (cid_ex >> 16) == (acid >> 16) && para_ex >= 0 {
            eprintln!("[V318B-CROSS] anchor={} now={}", acid, cid_ex);
            fcid = cid_ex;
            let rect = sess.writer_query_cursor_rect_ex(1, cid_ex, para_ex as u32, off_ex as u32);
            let rj = sess.cell_range_rects(acid, cid_ex);
            res = Some((para_ex, off_ex, rect, rj));
        } else if ct_ex == 1 && cid_ex == acid && para_ex >= 0 {
                    let rect = sess.writer_query_cursor_rect_ex(1, cid_ex, para_ex as u32, off_ex as u32);
                    let rj = sess.cell_selection_rects(acid, ap as u32, ao as u32, para_ex as u32, off_ex as u32);
                    res = Some((para_ex, off_ex, rect, rj));
                }
            }
            if let Some((np, no, (cx, cy, _cw, ch), rj)) = res {
                s.writer_cell_cursor = Some((fcid, np, no));
                s.writer_cell_sel_focus = Some((fcid, np, no));
                app.set_writer_cursor_x(cx as i32);
                app.set_writer_cursor_y(cy as i32);
                app.set_writer_cursor_h(ch as i32);
                app.set_writer_cursor_visible(true);
                let mut items: Vec<SelectionRect> = Vec::new();
                if let Some(json) = rj {
                    if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&json) {
                        for r in arr {
                            let rx = r.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                            let ry = r.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32 + 24.0;
                            let rw = r.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                            let rh = r.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                            items.push(SelectionRect { rx, ry, rw, rh });
                        }
                    }
                }
                app.set_writer_selection_rects(slint::ModelRc::new(slint::VecModel::from(items)));
            }
            return;
        }

        // 先只用 sess 算完所有需要的东西，让这个可变借用尽快结束，
        // 之后再统一读写 s 的其他字段（避免同时借用s.session和s其他字段）。
        let throttle_enabled = s.writer_drag_throttle_enabled;
        let last_para = s.writer_drag_last_query_para;
        let last_offset = s.writer_drag_last_query_offset;
        let anchor_para = s.writer_sel_anchor_para;
        let anchor_offset = s.writer_sel_anchor_offset;

        let Some(sess) = s.session.as_mut() else { return };
        let (para, offset) = sess.writer_hit_test(x_pt, y_pt);
        eprintln!("[WRITER-DRAG-RAWXY] 收到鼠标坐标 x={:.1} y={:.1} (x_pt={:.1} y_pt={:.1}) -> para={} offset={}", x, y, x_pt, y_pt, para, offset);
        if para < 0 {
            return;
        }

        // 节流判断（AB测试用）：若开启节流且位置未变化，跳过后续查询
        if throttle_enabled && para == last_para && offset == last_offset {
            return;
        }

        // 埋点：计时本次query_selection_rects调用
        let t0 = std::time::Instant::now();
        let rects_json = sess.query_selection_rects(
            anchor_para as u32, anchor_offset as u32,
            para as u32, offset as u32,
        );
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

        // 光标坐标也要更新（焦点处显示光标跟随）
        let (cx, cy, _cw, ch) = sess.writer_query_cursor_rect(para as u64, offset as u64);

        // sess（对s.session的可变借用）到这里已经不再需要，
        // 后面全部是对s其他字段的读写，不再冲突。
        s.writer_cursor_para = para;
        s.writer_cursor_offset = offset;
        s.writer_drag_query_count += 1;
        s.writer_drag_last_query_para = para;
        s.writer_drag_last_query_offset = offset;
        eprintln!(
            "[WRITER-DRAG-MOVE] #{} para={} offset={} 耗时={:.3}ms 节流={}",
            s.writer_drag_query_count, para, offset, elapsed_ms, throttle_enabled
        );

        app.set_writer_cursor_x(cx as i32);
        app.set_writer_cursor_y(cy as i32);
        app.set_writer_cursor_h(ch as i32);

        // 解析选区矩形JSON，转换成SelectionRect数组
        let mut items: Vec<SelectionRect> = Vec::new();
        if let Some(json) = rects_json {
            if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&json) {
                // CANVAS_OFFSET_PT: roe_query_selection_rects 返回的y是"文档全局坐标"，
                // 与 doc-core::writer_query_cursor_rect 里对光标y做的转换同一个道理
                // （见session.rs 632-644行注释），需要加上画布偏移才是画布坐标。
                // x不需要同样处理：roe侧 selection_rects.rs（v103修复）已经把
                // margin_left + text_x_offset 加进返回的x里了，与
                // writer_query_cursor_rect 的x语义完全一致（都是"文档全局坐标"），
                // 前端两边可以共用同一套居中公式，不需要再额外加offset。
                use doc_core::CANVAS_OFFSET_PT;
                for r in arr {
                    let rx = r.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let ry = r.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32 + CANVAS_OFFSET_PT;
                    let rw = r.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let rh = r.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    items.push(SelectionRect { rx, ry, rw, rh });
                }
            } else {
                eprintln!("[WRITER-DRAG-MOVE] 选区矩形JSON解析失败: {}", json);
            }
        }
        if let Some(first) = items.first() {
            eprintln!(
                "[WRITER-DRAG-RECT] 首个矩形: rx={:.2} ry={:.2} rw={:.2} rh={:.2} (writer_cursor_x={} writer_cursor_y={})",
                first.rx, first.ry, first.rw, first.rh,
                cx as i32, cy as i32
            );
        }
        app.set_writer_selection_rects(slint::ModelRc::new(slint::VecModel::from(items)));
    });

    // ── Writer IME组字（v110路线A：删旧插新结算模式）───────────────────────────
    // 设计依据：本次调研证实fcitx5/Slint TextInput已经完整处理了IME组合
    // 生命周期（预编辑、候选切换、确认/放弃），本层不拦截、不判断任何
    // 特定按键（空格/回车等）语义——这些在候选激活期间根本不会传到
    // key-pressed。写法参考Calc cell-edit「无条件信任self.text当前值」
    // 的思路，但Writer是字符流插入（不像Calc单元格可整体替换字符串），
    // 所以用"先删除上次已写入的N个字符，再整体插入这次的新内容"来模拟
    // 整体替换的效果：任何一次edited都被当作"当前应该展示的最终值"，
    // 忠实同步进roe文档，多次的中间态变化会被后续变化自然覆盖收敛，
    // 不需要区分"组合中"还是"组合完成"。
    let state_wime = state.clone();
    let app_weak_wime = app_weak.clone();
    app.on_writer_ime_composed(move |text| {
        eprintln!("[WRITER-IME] composed text = {:?} (len={})", text.as_str(), text.len());
        let app = match app_weak_wime.upgrade() { Some(a) => a, None => return };
        let mut s = state_wime.borrow_mut();
        let para = s.writer_cursor_para;
        if para < 0 { return; }
        if s.session.as_ref().map(|sess| sess.get_doc_type() != "writer").unwrap_or(true) { return; }

        if let Some((cid, cp, coff)) = s.writer_cell_cursor {
            let (_, start_off) = *s.writer_ime_session_start.get_or_insert((cp, coff));
            let prev_len = s.writer_ime_session_prev_len;
            let range_start = start_off as u32;
            let range_end = range_start + prev_len;
            let text_str = text.as_str();
            let res = s.session.as_mut().and_then(|sess| sess.cell_replace_range(cid, cp as u32, range_start, range_end, text_str));
            let mut new_off = start_off;
            if let Some(o) = res {
                new_off = o;
                s.writer_ime_session_prev_len = text_str.len() as u32;
            }
            eprintln!("[V315-CELL-IME] cid={} para={} [{},{}) text={:?} -> off={}", cid, cp, range_start, range_end, text_str, new_off);
            s.writer_cell_cursor = Some((cid, cp, new_off));
            writer_cell_refresh(&app, &mut s);
            return;
        }
        // v112"编辑会话"模型：建立/复用会话起点，用绝对区间做一次原子
        // replace_range，替代旧的"循环delete_char撤回+insert_text插入"
        // 两步模拟（ROE_Progress_v112.md 三.1/三.5）。
        //
        // 若当前没有进行中的会话（None），用*当前真实光标位置*建立新会话
        // 起点，prev_len归零——这是本轮组合的第一次结算。
        // 先把 s.writer_cursor_offset 取到局部变量，避免在同一表达式里
        // 对 s 同时做可变借用（get_or_insert）和不可变借用（读字段），
        // Rust借用检查器不允许这种嵌套借用（E0502）。
        let cur_offset = s.writer_cursor_offset;
        let (session_para, session_start_offset) =
            *s.writer_ime_session_start.get_or_insert((para, cur_offset));
        // 会话起点所在段落必须和当前光标段落一致；理论上只要"结束会话"的
        // 三个时机点（方向键/点击/拖拽）逻辑正确，光标不会在会话进行中
        // 跨段落，这里加一层防御性校验，不一致时保守地丢弃旧会话重开一个
        // （避免把不同段落的内容错误地当同一区间处理，比静默沿用旧session
        // 更安全）。
        let (session_para, session_start_offset, prev_len) = if session_para != para {
            eprintln!("[WRITER-IME] 会话段落不一致(session_para={} != para={})，重开会话", session_para, para);
            s.writer_ime_session_start = Some((para, s.writer_cursor_offset));
            s.writer_ime_session_prev_len = 0;
            (para, s.writer_cursor_offset, 0u32)
        } else {
            (session_para, session_start_offset, s.writer_ime_session_prev_len)
        };

        let range_start = session_start_offset as u32;
        let range_end = range_start + prev_len;
        let text_str = text.as_str();

        let replace_result = s.session.as_mut().and_then(|sess| {
            sess.replace_range(session_para as u32, range_start, range_end, text_str)
        });

        let mut offset = session_start_offset; // 失败时的兜底：光标留在会话起点，不移动
        match replace_result {
            Some(new_loc_json) => {
                eprintln!(
                    "[WRITER-IME] replace_range para={} range=[{},{}) text={:?} -> {}",
                    session_para, range_start, range_end, text_str, new_loc_json
                );
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&new_loc_json) {
                    if let Some(new_offset) = v["char_offset"].as_i64() {
                        offset = new_offset;
                    }
                }
                // 记录这次实际写入的字节长度，供下一次结算算终点用。
                s.writer_ime_session_prev_len = text_str.len() as u32;
            }
            None => {
                eprintln!(
                    "[WRITER-IME] replace_range FAILED para={} range=[{},{}) text={:?}",
                    session_para, range_start, range_end, text_str
                );
            }
        }

        // 更新光标位置与选区锚点（折叠），并重新查询屏幕坐标显示光标，
        // 与insert_char/split_paragraph分支保持一致的模式。
        s.writer_cursor_offset = offset;
        s.writer_sel_anchor_para = para;
        s.writer_sel_anchor_offset = offset;
        if let Some(sess) = s.session.as_mut() {
            let (cx, cy, _cw, ch_h) = sess.writer_query_cursor_rect(para as u64, offset as u64);
            s.writer_cursor_ideal_x = Some(cx);
            app.set_writer_cursor_x(cx as i32);
            app.set_writer_cursor_y(cy as i32);
            app.set_writer_cursor_h(ch_h as i32);
            app.set_writer_cursor_visible(true);
        }
        // 重新渲染视口（与on_writer_key_input末尾逻辑保持一致）。
        let scroll_y = app.get_writer_scroll_y_pt();
        let vw = app.get_writer_view_width() as f32;
        let vh = app.get_writer_view_height() as f32;
        let doc_w = app.get_writer_doc_width_pt();
        if vw > 0.0 && vh > 0.0 {
            let rgba = s.session.as_mut().map(|sess| sess.render_writer_viewport(scroll_y, doc_w, vh)).unwrap_or_default();
            if !rgba.is_empty() {
                let pw = (doc_w * doc_core::doc_bitmap_scale()).round() as u32;
                let ph = (vh * doc_core::doc_bitmap_scale()).round() as u32;
                let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, pw, ph);
                app.set_writer_viewport_image(slint::Image::from_rgba8(buf));
                eprintln!("[WRITER-IME] rerender OK {}x{}px", pw, ph);
            } else {
                eprintln!("[WRITER-IME] rerender FAILED");
            }
        }
    });

    // ── Writer方向键光标移动（v95新增）──────────────────────────────────────────
    // 左右/Ctrl+左右 → roe_cursor_move_inline（路径1，单行内，不跨行）
    // 上下 → roe_cursor_move_line（路径2，跨行，需要ideal_x）
    // shift+方向键（扩展选区）本次未实现，先跑通纯光标移动主线。
    let state_wcursor = state.clone();
    let app_weak_wcursor = app_weak.clone();
    app.on_writer_cursor_key(move |key, _shift, ctrl| {
        let app = match app_weak_wcursor.upgrade() { Some(a) => a, None => return };
        // v300: 选中图片时，Delete/Backspace 删除图片，其他按键只取消选中
        {
            let sel = app.get_writer_obj_sel();
            if sel >= 0 {
                let is_del = key == slint::SharedString::from(slint::platform::Key::Delete)
                    || key == slint::SharedString::from(slint::platform::Key::Backspace);
                app.set_writer_obj_sel(-1);
                if is_del {
                    let mut s = state_wcursor.borrow_mut();
                    let res = s.session.as_ref().map(|x| x.writer_delete_object(sel as u32));
                    match res {
                        Some(Ok(())) => {
                            if s.writer_cursor_para > sel as i64 { s.writer_cursor_para -= 1; }
                            if s.writer_sel_anchor_para > sel as i64 { s.writer_sel_anchor_para -= 1; }
                            writer_refresh_canvas(&app, &mut s);
                            drop(s);
                            app.set_writer_needs_render(true);
                            app.set_status_message("已删除图片".into());
                        }
                        Some(Err(e)) => {
                            drop(s);
                            app.set_status_message(format!("删除图片失败: {}", e).into());
                        }
                        None => {}
                    }
                    return;
                }
            }
        }
        let mut s = state_wcursor.borrow_mut();
        let para = s.writer_cursor_para;
        let offset = s.writer_cursor_offset;
        if para < 0 {
            return;
        }

        // v112：方向键是"编辑会话"的结束时机点之一（ROE_Progress_v112.md
        // 三.4）。清空Slint侧IME输入框内容（走clear-buffer()防重入保护，
        // 不直接对self.text赋值，避免二.3描述的递归edited问题）+ 重置
        // Rust侧会话状态，确保下次IME组合从当前光标位置重新开始一轮新会话，
        // 不会延续到已经不再相邻的旧起点。
        app.invoke_writer_ime_clear_buffer();
        s.writer_ime_session_start = None;
        s.writer_ime_session_prev_len = 0;

        let is_left = key == slint::SharedString::from(slint::platform::Key::LeftArrow);
        let is_right = key == slint::SharedString::from(slint::platform::Key::RightArrow);
        let is_up = key == slint::SharedString::from(slint::platform::Key::UpArrow);
        let is_down = key == slint::SharedString::from(slint::platform::Key::DownArrow);
        let is_backspace = key == slint::SharedString::from(slint::platform::Key::Backspace);
        let is_delete = key == slint::SharedString::from(slint::platform::Key::Delete);

        // v318e: 有单元格选区时，第一次方向键只折叠选区，不移动
        if (is_left || is_right || is_up || is_down) && s.writer_cell_cursor.is_some() {
            if let (Some(a), Some(f)) = (s.writer_cell_sel_anchor, s.writer_cell_sel_focus) {
                if Some(f) == s.writer_cell_cursor && a != f {
                    let lo = if a <= f { a } else { f };
                    let hi = if a <= f { f } else { a };
                    let target = if is_left { lo } else if is_right { hi } else { f };
                    s.writer_cell_cursor = Some(target);
                    s.writer_cell_vert = None;
                    s.writer_cell_sel_anchor = None;
                    s.writer_cell_sel_focus = None;
                    writer_cell_refresh(&app, &mut s);
                    return;
                }
            }
        }

        // v316c：光标在单元格内时，上下键按 ideal_x 在单元格内找行
        if (is_up || is_down) && s.writer_cell_cursor.is_some() {
            if let Some((cid, cp, coff)) = s.writer_cell_cursor {
                let cur_x = s.session.as_mut().map(|sess| sess.writer_query_cursor_rect_ex(1, cid, cp as u32, coff as u32).0).unwrap_or(0.0);
                let ideal_x = match s.writer_cell_vert {
                    Some((vc, vp, vo, vx)) if vc == cid && vp == cp && vo == coff => vx,
                    _ => cur_x,
                };
                let dirv: u32 = if is_up { 0 } else { 1 };
                let res = s.session.as_mut().and_then(|sess| sess.cell_nav_line(cid, cp as u32, coff as u32, dirv, ideal_x));
                eprintln!("[V316C-CELL-LINE] cid={} para={} off={} dir={} ideal_x={:.1} -> {:?}", cid, cp, coff, dirv, ideal_x, res);
                if let Some((ncid, np, no)) = res {
                    s.writer_cell_cursor = Some((ncid, np, no));
                    s.writer_cell_vert = Some((ncid, np, no, ideal_x));
                }
            }
            writer_cell_refresh(&app, &mut s);
            return;
        }

        // v316b：光标在单元格内时，左右键走 cell_nav，上下键暂不处理
        if s.writer_cell_cursor.is_some() && (is_left || is_right || is_up || is_down) {
            if let Some((cid, cp, coff)) = s.writer_cell_cursor {
                if is_left || is_right {
                    let cmd: u32 = if is_left { 0 } else { 1 };
                    let res = s.session.as_mut().and_then(|sess| sess.cell_nav(cid, cp as u32, coff as u32, cmd));
                    eprintln!("[V316B-CELL-NAV] cid={} para={} off={} cmd={} -> {:?}", cid, cp, coff, cmd, res);
                    if let Some((ncid, np, no)) = res {
                        s.writer_cell_cursor = Some((ncid, np, no));
                    }
                }
            }
            writer_cell_refresh(&app, &mut s);
            return;
        }

        if is_left || is_right {
            // 路径1：单行内移动
            let command: u32 = match (is_left, ctrl) {
                (true, false) => 0,  // CharLeft
                (true, true) => 2,   // WordLeft
                (false, false) => 1, // CharRight
                (false, true) => 3,  // WordRight
            };
            let Some(sess) = s.session.as_mut() else { return };
            if sess.get_doc_type() != "writer" { return; }
            let Some(json) = sess.cursor_move_inline(para as u32, offset as u32, command) else {
                eprintln!("[WRITER-CURSOR-KEY] cursor_move_inline 调用失败");
                return;
            };
            let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
            let mut new_para = v.get("local_para_index").and_then(|x| x.as_i64()).unwrap_or(para);
            let mut new_offset = v.get("char_offset").and_then(|x| x.as_i64()).unwrap_or(offset);
            eprintln!("[WRITER-CURSOR-KEY] inline: para={} offset={} -> para={} offset={}", para, offset, new_para, new_offset);

            // v99新增：段落间行首/行尾跨段跳转。仅对Char级左右键生效（command 0/1），
            // WordLeft/WordRight（2/3，Ctrl+左右）本次不做跨段（西文分词跨段跳转是
            // 后续任务，见交接记录）。判定依据：cursor_move_inline 在到达段边界时
            // 原地不动（final文档五.3约定的原生语义），即 new_para==para 且
            // new_offset==offset，此时前端自行决定跳到相邻段的哪个位置：
            // 左键在段首 -> 跳到上一段末尾（该段总字节长度处）
            // 右键在段尾 -> 跳到下一段开头（offset=0）
            if (command == 0 || command == 1) && new_para == para && new_offset == offset {
                if command == 0 && para > 0 {
                    // 左键在段首，且不是文档第一段：查上一段长度，跳到其末尾
                    if let Some(info_json) = sess.get_paragraph_info((para - 1) as u32) {
                        let info: serde_json::Value = serde_json::from_str(&info_json).unwrap_or_default();
                        if let Some(prev_len) = info.get("para_len").and_then(|x| x.as_i64()) {
                            new_para = para - 1;
                            new_offset = prev_len;
                            eprintln!("[WRITER-CURSOR-KEY] 跨段跳转(左): para={} offset=0 -> para={} offset={}", para, new_para, new_offset);
                        }
                    }
                } else if command == 1 {
                    // 右键在段尾：先查总段数，确认不是文档最后一段再跳
                    if let Some(info_json) = sess.get_paragraph_info(para as u32) {
                        let info: serde_json::Value = serde_json::from_str(&info_json).unwrap_or_default();
                        if let Some(total) = info.get("total_para_count").and_then(|x| x.as_i64()) {
                            if para + 1 < total {
                                new_para = para + 1;
                                new_offset = 0;
                                eprintln!("[WRITER-CURSOR-KEY] 跨段跳转(右): para={} -> para={} offset=0", para, new_para);
                            }
                        }
                    }
                }
            }

            // v318j: 左右键越过内联图片/图表（它们是独立的 body 项，不能放文字光标）
            if (command == 0 || command == 1) && !(new_para == para && new_offset != offset) {
                let total: i64 = sess
                    .get_paragraph_info(para as u32)
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
                    .and_then(|v| v.get("total_para_count").and_then(|x| x.as_i64()))
                    .filter(|t| *t > 0)
                    .unwrap_or(i64::MAX);
                let step: i64 = if command == 0 { -1 } else { 1 };
                let start = if new_para != para { new_para } else { para + step };
                if start >= 0 && start < total && sess.writer_object_kind(start as u32) != 0 {
                    let mut t = start;
                    while t >= 0 && t < total && sess.writer_object_kind(t as u32) != 0 { t += step; }
                    let mut ok = false;
                    if t >= 0 && t < total {
                        if command == 0 {
                            let len = sess.get_paragraph_info(t as u32)
                                .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
                                .and_then(|v| v.get("para_len").and_then(|x| x.as_i64()));
                            if let Some(len) = len { new_para = t; new_offset = len; ok = true; }
                        } else {
                            new_para = t; new_offset = 0; ok = true;
                        }
                    }
                    if !ok { new_para = para; new_offset = offset; }
                    eprintln!("[V318J-SKIP-OBJ] para={} -> start={} target={} ok={} total={}", para, start, new_para, ok, total);
                }
            }

            let (cx, cy, _cw, ch) = sess.writer_query_cursor_rect(new_para as u64, new_offset as u64);
            eprintln!("[V96-DEBUG-ARROWKEY] inline移动后 query_cursor_rect(para={}, offset={}) -> x={:.1} y={:.1} h={:.1}", new_para, new_offset, cx, cy, ch);
            s.writer_cursor_para = new_para;
            s.writer_cursor_offset = new_offset;
            // 非上下移动：重新计算并覆盖 ideal_x（final文档五.2规则）
            // v95再修正：Writer坐标系pt直接当px用，不乘PT_TO_PX（与calc不同，见1305行注释）
            s.writer_cursor_ideal_x = Some(cx);
            // 未做选区扩展：折叠选区，锚点跟随焦点
            s.writer_sel_anchor_para = new_para;
            s.writer_sel_anchor_offset = new_offset;
            app.set_writer_cursor_x(cx as i32);
            app.set_writer_cursor_y(cy as i32);
            app.set_writer_cursor_h(ch as i32);
            app.set_writer_cursor_visible(true);
        } else if is_up || is_down {
            // 路径2：跨行移动，需要 ideal_x。
            // v95再修正：Writer坐标系pt直接当px用，ideal_x/actual_x全程不做PT_TO_PX换算
            // （与calc不同，见上方注释）；此前引入的"px<->pt来回换算"纯属误套用calc逻辑
            // 产生的多余复杂度，这里直接用同一套数值贯穿。
            let direction: u32 = if is_up { 0 } else { 1 };
            let ideal_x = match s.writer_cursor_ideal_x {
                Some(v) => v,
                None => {
                    let Some(sess) = s.session.as_mut() else { return };
                    let (cx, _cy, _cw, _ch) = sess.writer_query_cursor_rect(para as u64, offset as u64);
                    s.writer_cursor_ideal_x = Some(cx);
                    cx
                }
            };

            let Some(sess) = s.session.as_mut() else { return };
            if sess.get_doc_type() != "writer" { return; }
            let Some(json) = sess.cursor_move_line(para as u32, offset as u32, direction, ideal_x) else {
                eprintln!("[WRITER-CURSOR-KEY] cursor_move_line 调用失败");
                return;
            };
            let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
            let new_loc = v.get("new_loc");
            let new_para = new_loc.and_then(|l| l.get("local_para_index")).and_then(|x| x.as_i64()).unwrap_or(para);
            let new_offset = new_loc.and_then(|l| l.get("char_offset")).and_then(|x| x.as_i64()).unwrap_or(offset);
            eprintln!("[WRITER-CURSOR-KEY] line: para={} offset={} -> para={} offset={} (ideal_x={:.1})", para, offset, new_para, new_offset, ideal_x);

            let (_cx, cy, _cw, ch) = sess.writer_query_cursor_rect(new_para as u64, new_offset as u64);
            s.writer_cursor_para = new_para;
            s.writer_cursor_offset = new_offset;
            // 上下移动：ideal_x 保持不变，绝不用 actual_x 覆盖（final文档五.2明确强调的常见实现错误）
            s.writer_sel_anchor_para = new_para;
            s.writer_sel_anchor_offset = new_offset;
            let actual_x = v.get("actual_x").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            app.set_writer_cursor_x(actual_x as i32);
            app.set_writer_cursor_y(cy as i32);
            app.set_writer_cursor_h(ch as i32);
            app.set_writer_cursor_visible(true);
        } else if is_backspace || is_delete {
            // v100：删除/退格。direction 0=Backward(退格) 1=Forward(Delete键)，
            // 与 roe_api::text_edit::DeleteDirection 编码一致。
            let direction: u32 = if is_backspace { 0 } else { 1 };
            if let Some((cid, cp, coff)) = s.writer_cell_cursor {
                let mut cp = cp;
                let res = {
                    let r1 = s.session.as_mut().and_then(|sess| sess.cell_delete_char(cid, cp as u32, coff as u32, direction));
                    if r1.is_some() || (direction == 0 && coff != 0) {
                        r1
                    } else {
                        match s.session.as_mut().and_then(|sess| sess.cell_merge(cid, cp as u32, direction)) {
                            Some((np, no)) => { cp = np; Some(no) }
                            None => None,
                        }
                    }
                };
                eprintln!("[V315-CELL-DEL] cid={} para={} off={} dir={} -> {:?}", cid, cp, coff, direction, res);
                if let Some(o) = res {
                    s.writer_cell_cursor = Some((cid, cp, o));
                    s.writer_ime_session_start = None;
                    s.writer_ime_session_prev_len = 0;
                }
                writer_cell_refresh(&app, &mut s);
                return;
            }
            let Some(sess) = s.session.as_mut() else { return };
            if sess.get_doc_type() != "writer" { return; }
            // v252: 空列表项按退格 -> 先取消列表（不合并上一段）
            if is_backspace && offset == 0 {
                let empty_list_item = sess.get_paragraph_info(para as u32)
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
                    .map(|v| v["para_len"].as_i64() == Some(0) && v["is_list"].as_bool() == Some(true))
                    .unwrap_or(false);
                if empty_list_item {
                    match sess.writer_set_list(para as u32, 0, 0) {
                        Ok(()) => eprintln!("[WRITER-CURSOR-KEY] 空列表项退格 -> 取消列表 para={}", para),
                        Err(e) => eprintln!("[WRITER-CURSOR-KEY] 取消列表失败: {}", e),
                    }
                    let (cx, cy, _cw, ch_h) = sess.writer_query_cursor_rect(para as u64, 0);
                    s.writer_cursor_ideal_x = Some(cx);
                    s.writer_cursor_offset = 0;
                    s.writer_sel_anchor_para = para;
                    s.writer_sel_anchor_offset = 0;
                    app.set_writer_cursor_x(cx as i32);
                    app.set_writer_cursor_y(cy as i32);
                    app.set_writer_cursor_h(ch_h as i32);
                    app.set_writer_cursor_visible(true);
                    let scroll_y = app.get_writer_scroll_y_pt();
                    let vw = app.get_writer_view_width() as f32;
                    let vh = app.get_writer_view_height() as f32;
                    let doc_w = app.get_writer_doc_width_pt();
                    if vw > 0.0 && vh > 0.0 {
                        let rgba = s.session.as_mut().map(|sess| sess.render_writer_viewport(scroll_y, doc_w, vh)).unwrap_or_default();
                        if !rgba.is_empty() {
                            let pw = (doc_w * doc_core::doc_bitmap_scale()).round() as u32;
                            let ph = (vh * doc_core::doc_bitmap_scale()).round() as u32;
                            let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, pw, ph);
                            app.set_writer_viewport_image(slint::Image::from_rgba8(buf));
                            eprintln!("[WRITER-CURSOR-KEY] rerender OK (退格取消列表)");
                        }
                    }
                    return;
                }
            }
            let Some(sess) = s.session.as_mut() else { return };
            match sess.delete_char(para as u32, offset as u32, direction) {
                Some(json) => {
                    eprintln!("[WRITER-CURSOR-KEY] delete_char direction={} para={} offset={} -> {}", direction, para, offset, json);
                    let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                    let new_para = v.get("local_para_index").and_then(|x| x.as_i64()).unwrap_or(para);
                    let new_offset = v.get("char_offset").and_then(|x| x.as_i64()).unwrap_or(offset);
                    let (cx, cy, _cw, ch) = sess.writer_query_cursor_rect(new_para as u64, new_offset as u64);
                    s.writer_cursor_para = new_para;
                    s.writer_cursor_offset = new_offset;
                    // 删除是"非上下移动"操作，按final文档五.2规则，重新计算并覆盖ideal_x
                    s.writer_cursor_ideal_x = Some(cx);
                    s.writer_sel_anchor_para = new_para;
                    s.writer_sel_anchor_offset = new_offset;
                    app.set_writer_cursor_x(cx as i32);
                    app.set_writer_cursor_y(cy as i32);
                    app.set_writer_cursor_h(ch as i32);
                    app.set_writer_cursor_visible(true);
                }
                None => {
                    // v105：段首退格/段尾Delete这个边界原来是静默失败（不
                    // 支持跨段），现在自动回退调用 merge_paragraph 做跨段
                    // 合并（roe_split_paragraph 的逆操作）。这里不区分
                    // "真的是段首/段尾边界"还是"其他原因导致delete_char失
                    // 败"，直接尝试合并——如果不满足合并的前置条件（比如
                    // 已经是文档第一段/最后一段），merge_paragraph 自己会
                    // 返回 None，效果等价于原来的静默失败，不会有副作用。
                    eprintln!("[WRITER-CURSOR-KEY] delete_char 调用失败（可能是段首退格/段尾Delete边界），尝试 merge_paragraph direction={} para={} offset={}", direction, para, offset);
                    match sess.merge_paragraph(para as u32, offset as u32, direction) {
                        Some(json) => {
                            eprintln!("[WRITER-CURSOR-KEY] merge_paragraph direction={} para={} offset={} -> {}", direction, para, offset, json);
                            let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                            let new_para = v.get("local_para_index").and_then(|x| x.as_i64()).unwrap_or(para);
                            let new_offset = v.get("char_offset").and_then(|x| x.as_i64()).unwrap_or(offset);
                            let (cx, cy, _cw, ch) = sess.writer_query_cursor_rect(new_para as u64, new_offset as u64);
                            s.writer_cursor_para = new_para;
                            s.writer_cursor_offset = new_offset;
                            s.writer_cursor_ideal_x = Some(cx);
                            s.writer_sel_anchor_para = new_para;
                            s.writer_sel_anchor_offset = new_offset;
                            app.set_writer_cursor_x(cx as i32);
                            app.set_writer_cursor_y(cy as i32);
                            app.set_writer_cursor_h(ch as i32);
                            app.set_writer_cursor_visible(true);
                        }
                        None => {
                            // 真正的文档边界（已是第一段/最后一段），
                            // 静默失败，不崩溃、不改动文档、光标不动，
                            // 符合"光标停在文档边界"的直观预期。
                            eprintln!("[WRITER-CURSOR-KEY] merge_paragraph 也失败（已是文档边界）direction={} para={} offset={}", direction, para, offset);
                        }
                    }
                }
            }
        }

        // 重新渲染视口，让光标位置变化在下一帧生效（若滚动区域覆盖光标所在行）
        // v95修复：render_writer_viewport 内部按 scale=2.0（HiDPI）生成像素数据，
        // 返回的buf实际尺寸是 (doc_w*2) x (vh*2)，不是 vw x vh —— 这是照抄
        // on_writer_key_input 原版逻辑才发现的，此前误用vw/vh（未乘2）解释buffer，
        // 导致stride不匹配，画面变成黑色散点。pw用doc_w（不是vw）也是照抄原版，
        // 与on_writer_key_input保持一致，避免两处渲染逻辑不一致产生新的不一致bug。
        let scroll_y = app.get_writer_scroll_y_pt();
        let vw = app.get_writer_view_width() as f32;
        let vh = app.get_writer_view_height() as f32;
        let doc_w = app.get_writer_doc_width_pt();
        if vw > 0.0 && vh > 0.0 {
            let rgba = s.session.as_mut().map(|sess| sess.render_writer_viewport(scroll_y, doc_w, vh)).unwrap_or_default();
            if !rgba.is_empty() {
                let pw = (doc_w * doc_core::doc_bitmap_scale()).round() as u32;
                let ph = (vh * doc_core::doc_bitmap_scale()).round() as u32;
                let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, pw, ph);
                app.set_writer_viewport_image(slint::Image::from_rgba8(buf));
                eprintln!("[WRITER-CURSOR-KEY] rerender OK {}x{}px", pw, ph);
            } else {
                eprintln!("[WRITER-CURSOR-KEY] rerender FAILED");
            }
        }
    });

    // v115：Ctrl+Z/Ctrl+Shift+Z 撤销/重做，精确光标定位。
    // 取代v113方案A的"退化到段落开头"简化处理（ROE_Progress_v113/v114
    // 遗留的已知简化点）。roe_undo/roe_redo现在返回JSON，携带编辑发生
    // 前的锚点/焦点位置（ROE_Progress_v115.md 三.4方案），解析后直接
    // 应用到光标和选区锚点，不再需要"猜一个开头位置"。
    let state_wundo = state.clone();
    let app_weak_wundo = app_weak.clone();
    // v291: 气泡勾选图标 -> 切换完成；铅笔图标 -> 打开编辑对话框
    {
        let app_weak_cx = app_weak_wundo.clone();
        let state_cx = state_wundo.clone();
        app.on_writer_comment_toggle_done(move |cid| {
            let Some(app) = app_weak_cx.upgrade() else { return };
            let res = {
                let s = state_cx.borrow();
                let cur = s.comment_threads.get(&(cid as i64)).map(|t| t.3).unwrap_or(false);
                match s.session.as_ref() {
                    Some(sess) => sess.writer_set_comment_done(cid as u32, !cur),
                    None => return,
                }
            };
            match res {
                Ok(()) => app.set_writer_needs_render(true),
                Err(e) => app.set_status_message(format!("标记完成失败: {}", e).into()),
            }
        });
        let app_weak_ce = app_weak_wundo.clone();
        let state_ce = state_wundo.clone();
        app.on_writer_comment_edit(move |cid| {
            let Some(app) = app_weak_ce.upgrade() else { return };
            let text = {
                let s = state_ce.borrow();
                let mut found: Option<String> = s.comment_threads.get(&(cid as i64)).map(|t| t.2.clone());
                if found.is_none() {
                    // v294: cid 不是根批注，当作回复 id 反查
                    for (root_id, rids) in s.reply_ids.iter() {
                        if let Some(ri) = rids.iter().position(|r| *r == cid) {
                            found = s.comment_threads.get(root_id)
                                .and_then(|t| t.4.get(ri))
                                .map(|r| r.2.clone());
                            break;
                        }
                    }
                }
                found.unwrap_or_default()
            };
            app.set_cm_dlg_reply_to(-1);
            app.set_cm_dlg_edit_id(cid);
            app.set_cm_dlg_title("编辑批注".into());
            app.set_cm_dlg_text(text.into());
            app.set_cm_dlg_visible(true);
        });
    }

    // v292: 气泡「添加回复」-> 打开回复对话框
    {
        let app_weak_cr = app_weak_wundo.clone();
        app.on_writer_comment_reply(move |cid| {
            let Some(app) = app_weak_cr.upgrade() else { return };
            app.set_cm_dlg_edit_id(-1);
            app.set_cm_dlg_reply_to(cid);
            app.set_cm_dlg_title("添加回复".into());
            app.set_cm_dlg_text("".into());
            app.set_cm_dlg_visible(true);
        });
    }

    // v289: 气泡垃圾桶 -> 删除批注
    {
        let app_weak_cd = app_weak_wundo.clone();
        let state_cd = state_wundo.clone();
        app.on_writer_comment_delete(move |cid| {
            let Some(app) = app_weak_cd.upgrade() else { return };
            let res = {
                let s = state_cd.borrow();
                match s.session.as_ref() {
                    Some(sess) => sess.writer_delete_comment(cid as u32),
                    None => return,
                }
            };
            match res {
                Ok(()) => {
                    if app.get_writer_current_comment() == cid { app.set_writer_current_comment(-1); }
                    app.set_status_message("已删除批注".into());
                    app.set_writer_needs_render(true);
                }
                Err(e) => app.set_status_message(format!("删除批注失败: {}", e).into()),
            }
        });
    }

    app.on_writer_undo_redo(move |is_redo| {
        let app = match app_weak_wundo.upgrade() { Some(a) => a, None => return };
        let mut s = state_wundo.borrow_mut();
        if s.session.as_ref().map(|sess| sess.get_doc_type() != "writer").unwrap_or(true) { return; }

        let result_json = {
            let Some(sess) = s.session.as_ref() else { return };
            if is_redo { sess.redo() } else { sess.undo() }
        };

        let Some(json) = result_json else {
            eprintln!("[WRITER-UNDO-REDO] {} 失败（没有可{}的历史）", if is_redo { "redo" } else { "undo" }, if is_redo { "重做" } else { "撤销" });
            return;
        };
        eprintln!("[WRITER-UNDO-REDO] {} 成功，光标位置JSON={}", if is_redo { "redo" } else { "undo" }, json);

        // 撤销/重做会让Slint侧IME输入框里残留的组合中间态彻底失去意义
        // （文档内容已经不是那次组合面对的内容了），清空避免下次edited
        // 误把残留内容当作要提交的输入。
        app.invoke_writer_ime_clear_buffer();
        s.writer_ime_session_start = None;
        s.writer_ime_session_prev_len = 0;

        // v301: 撤销/重做后清掉图片选中框，并按新内容刷新画布高度
        app.set_writer_obj_sel(-1);
        writer_refresh_canvas(&app, &mut s);

        // v115：解析roe内核返回的精确光标位置。字段命名见
        // handle::EditCursorContext::to_json()（roe-api/src/handle.rs）。
        // 解析失败（理论上不应该发生，roe侧总是返回合法JSON或None）时
        // 退回旧的"当前段落开头"兜底，不让UI卡住。
        let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
        let para = v.get("local_para_index").and_then(|x| x.as_i64()).unwrap_or(s.writer_cursor_para.max(0));
        let anchor_offset = v.get("anchor_offset").and_then(|x| x.as_i64()).unwrap_or(0);
        let focus_offset = v.get("focus_offset").and_then(|x| x.as_i64()).unwrap_or(0);

        let offset = focus_offset;

        // v316b：撤销/重做前光标在单元格内，则校正后继续留在单元格内
        if let Some((ccid, ccp, _cco)) = s.writer_cell_cursor {
            let fixed = s.session.as_mut().and_then(|sess| sess.cell_nav(ccid, ccp as u32, focus_offset.max(0) as u32, 2));
            match fixed {
                Some((ncid, np, no)) => {
                    s.writer_cell_cursor = Some((ncid, np, no));
                    writer_cell_refresh(&app, &mut s);
                    return;
                }
                None => { s.writer_cell_cursor = None; }
            }
        }
        s.writer_cursor_para = para;
        s.writer_cursor_offset = offset;
        s.writer_cursor_ideal_x = None;
        s.writer_sel_anchor_para = para;
        s.writer_sel_anchor_offset = anchor_offset;

        if let Some(sess) = s.session.as_mut() {
            let (cx, cy, _cw, ch) = sess.writer_query_cursor_rect(para as u64, offset as u64);
            app.set_writer_cursor_x(cx as i32);
            app.set_writer_cursor_y(cy as i32);
            app.set_writer_cursor_h(ch as i32);
            app.set_writer_cursor_visible(true);
        }

        // 重新渲染视口：文档内容已经变化（撤销/重做的核心效果），照抄
        // on_writer_cursor_key末尾同一套渲染收尾模式，保持一致。
        let scroll_y = app.get_writer_scroll_y_pt();
        let vw = app.get_writer_view_width() as f32;
        let vh = app.get_writer_view_height() as f32;
        let doc_w = app.get_writer_doc_width_pt();
        if vw > 0.0 && vh > 0.0 {
            let rgba = s.session.as_mut().map(|sess| sess.render_writer_viewport(scroll_y, doc_w, vh)).unwrap_or_default();
            if !rgba.is_empty() {
                let pw = (doc_w * doc_core::doc_bitmap_scale()).round() as u32;
                let ph = (vh * doc_core::doc_bitmap_scale()).round() as u32;
                let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, pw, ph);
                app.set_writer_viewport_image(slint::Image::from_rgba8(buf));
                eprintln!("[WRITER-UNDO-REDO] rerender OK {}x{}px", pw, ph);
            } else {
                eprintln!("[WRITER-UNDO-REDO] rerender FAILED");
            }
        }
        // v286: 走标准渲染路径，让批注气泡随撤销/重做同步重拼
        app.set_writer_needs_render(true);
    });

    // ── AiPanel callbacks ─────────────────────────────────────────────────────
    fn ai_push(app: &AppWindow, kind: i32, text: String) {
        let m = app.get_ai_log();
        if let Some(vm) = m.as_any().downcast_ref::<slint::VecModel<AiMsg>>() {
            vm.push(AiMsg { kind, text: text.into() });
        } else {
            app.set_ai_log(slint::ModelRc::new(slint::VecModel::from(vec![AiMsg { kind, text: text.into() }])));
        }
    }
    fn ai_append_reply(app: &AppWindow, delta: &str) {
        let m = app.get_ai_log();
        if let Some(vm) = m.as_any().downcast_ref::<slint::VecModel<AiMsg>>() {
            let n = vm.row_count();
            if n > 0 {
                if let Some(last) = vm.row_data(n - 1) {
                    if last.kind == 5 {
                        vm.set_row_data(n - 1, AiMsg { kind: 5, text: format!("{}{}", last.text, delta).into() });
                        return;
                    }
                }
            }
        }
        ai_push(app, 5, delta.to_string());
    }
    let state_ai = state.clone();
    let app_weak2 = app_weak.clone();
    app.on_ai_fetch_index(move || {
        let idx = {
            let mut st = state_ai.borrow_mut();
            match st.session.as_mut() {
                Some(sess) => sess.get_index(),
                None => String::new(),
            }
        };
        if let Some(app) = app_weak2.upgrade() {
            if idx.is_empty() {
                ai_push(&app, 2, "没有打开的文档".into());
            } else {
                ai_push(&app, 0, format!("文档索引:\n{}", idx));
                app.set_ai_index_json(idx.into());
            }
        }
    });
    app.on_ai_copy_index(move || {});
    let app_weak_ai_set = app_weak.clone();
    // 设置页做出来之前:不再往对话流写消息(面板自己显示"设置页开发中")。
    // 设置页完成后在这里切 shell-view = "settings"。
    app.on_ai_open_settings(move || {
        if let Some(app) = app_weak_ai_set.upgrade() {
            // AI 面板只在文档里出现,所以从这里进设置页,返回时回文档
            app.set_settings_from_doc(true);
            app.set_shell_view("settings".into());
            app.set_show_start_page(false);
        }
    });

    // Impress 文档的 AI 脚本执行:内核 execute_script 已做快照/回滚;
    // 脚本可能增删页或改形状,所以重读页数、重绘并刷新全部缩略图
    fn run_ai_script_impress(ctx: &ribbon::RibbonCtx, script: &str) -> (bool, String) {
        let Some(app) = ctx.app_weak.upgrade() else { return (false, "窗口已关闭".into()) };
        let res = {
            let mut s = ctx.state.borrow_mut();
            let Some(sess) = s.session.as_mut() else { return (false, "没有打开的文档".into()) };
            sess.execute_script(script)
        };
        eprintln!("[ai] impress run_script 返回: {}", res);
        if !res.contains("\"ok\":true") {
            return (false, res);
        }
        // 内核判定为只读脚本时 dirty_pages 为空:没有改动,跳过重载与重绘
        if !res.contains("\"dirty_pages\":[]") {
            let cur = app.get_impress_current_slide();
            impress_reload_slides_v220(&app, &ctx.state, cur);
        }
        (true, res)
    }

    // AI 脚本执行:Calc / Writer / Impress 分流
    let ctx_ai = ribbon::RibbonCtx {
        state: state.clone(),
        app_weak: app_weak.clone(),
        current_sheet_idx: current_sheet_idx.clone(),
    };
    // Agent 脚本入口(UI 线程):Calc 复用 run_ai_script(执行+全套刷新)
    {
        struct AiScriptHandler { ctx: ribbon::RibbonCtx }
        impl AiScriptHandler {
            fn doc_supported(&self) -> Result<String, String> {
                let Some(app) = self.ctx.app_weak.upgrade() else { return Err("窗口已关闭".into()) };
                let dt = app.get_doc_type().to_string();
                match dt.as_str() {
                    "calc" | "writer" | "impress" => Ok(dt),
                    _ => Err(format!("暂不支持在 {} 文档中执行 AI 脚本", dt)),
                }
            }
        }
        impl agent_bridge::UiScriptHandler for AiScriptHandler {
            fn execute_raw(&self, script: &str) -> Result<String, String> {
                let dt = self.doc_supported()?;
                eprintln!("[ai] exec 脚本: {}", script.chars().take(400).collect::<String>());
                let (ok, raw) = if dt == "writer" {
                    run_ai_script_writer(&self.ctx, script)
                } else if dt == "impress" {
                    run_ai_script_impress(&self.ctx, script)
                } else {
                    ribbon::calc_home::run_ai_script(&self.ctx, script)
                };
                if ok { Ok(raw) } else { Err(raw) }
            }
            fn query_raw(&self, script: &str) -> Result<String, String> {
                self.doc_supported()?;
                eprintln!("[ai] query 脚本: {}", script.chars().take(400).collect::<String>());
                let st = self.ctx.state.borrow();
                match st.session.as_ref() {
                    Some(sess) => Ok(sess.query_script(script)),
                    None => Err("没有打开的文档".into()),
                }
            }
        }
        agent_bridge::install_ui_handler(std::rc::Rc::new(AiScriptHandler {
            ctx: ribbon::RibbonCtx {
                state: state.clone(),
                app_weak: app_weak.clone(),
                current_sheet_idx: current_sheet_idx.clone(),
            },
        }));
    }
    let app_weak_ai_run = app_weak.clone();
    app.on_ai_run_script(move |script| {
        let Some(app) = app_weak_ai_run.upgrade() else { return };
        let doc_type = app.get_doc_type().to_string();
        let preview: String = script.chars().take(200).collect();
        ai_push(&app, 3, preview);
        let (ok, raw) = if doc_type == "calc" {
            ribbon::calc_home::run_ai_script(&ctx_ai, script.as_str())
        } else if doc_type == "writer" {
            run_ai_script_writer(&ctx_ai, script.as_str())
        } else if doc_type == "impress" {
            run_ai_script_impress(&ctx_ai, script.as_str())
        } else {
            (false, format!("暂不支持在 {} 文档中执行 AI 脚本", doc_type))
        };
        if ok {
            ai_push(&app, 1, "执行成功,可点「撤销」回退".into());
            if let Some(t) = ribbon::calc_names::ai_return_text(&raw) {
                ai_push(&app, 1, format!("返回值: {}", t));
            }
        } else {
            ai_push(&app, 2, format!("执行失败,文档已回滚\n{}", raw));
        }
    });

    // 标题栏撤销/重做按钮:按文档类型转发到现有回调(Impress 由其他提交负责,此处不转发)
    {
        let aw = app_weak.clone();
        app.on_undo_clicked(move || {
            let Some(app) = aw.upgrade() else { return };
            match app.get_doc_type().as_str() {
                "calc" => app.invoke_calc_undo_redo(false),
                "writer" => app.invoke_writer_undo_redo(false),
                "impress" => app.invoke_impress_undo_redo(false),
                _ => {}
            }
        });
        let aw = app_weak.clone();
        app.on_redo_clicked(move || {
            let Some(app) = aw.upgrade() else { return };
            match app.get_doc_type().as_str() {
                "calc" => app.invoke_calc_undo_redo(true),
                "writer" => app.invoke_writer_undo_redo(true),
                "impress" => app.invoke_impress_undo_redo(true),
                _ => {}
            }
        });
    }

    // AI 撤销按钮:转发给现成的 Ctrl+Z 回调(on_calc_undo_redo / on_writer_undo_redo)
    let app_weak_ai_undo = app_weak.clone();
    app.on_ai_undo_script(move || {
        let Some(app) = app_weak_ai_undo.upgrade() else { return };
        let doc_type = app.get_doc_type().to_string();
        match doc_type.as_str() {
            "calc" => app.invoke_calc_undo_redo(false),
            "writer" => app.invoke_writer_undo_redo(false),
            "impress" => app.invoke_impress_undo_redo(false),
            _ => {
                ai_push(&app, 2, "当前文档类型暂不支持撤销".into());
                return;
            }
        }
        ai_push(&app, 0, "已撤销上一步操作".into());
    });

    // ══════════ Agent 模式：正式接入 roe-agent（取代此前的假数据 + echo 闭环骨架） ══════════
    //
    // 本区块所有回调统一只操作 slint 侧 agent_messages Model，不再维护
    // state.agent_messages 镜像（理由见上方 agent_runtime spawn 调用处的
    // 架构说明注释）。不再写入初始假数据——对话历史完全由用户输入 +
    // roe-agent 的真实事件流驱动，起始为空列表（UI 骨架本身已经能正确
    // 渲染空列表状态，不需要占位数据）。

    // thinking-toggle：翻转对应消息的 expanded 字段
    let app_weak_thinking = app_weak.clone();
    // 复制对话:Clipboard 必须在进程内保活,否则 Linux 上内容会随对象销毁而丢失
    fn agent_clip_set(text: &str) {
        thread_local! {
            static CLIP: std::cell::RefCell<Option<arboard::Clipboard>> = std::cell::RefCell::new(None);
        }
        CLIP.with(|c| {
            let mut c = c.borrow_mut();
            if c.is_none() {
                match arboard::Clipboard::new() {
                    Ok(cb) => *c = Some(cb),
                    Err(e) => { eprintln!("[agent] 剪贴板初始化失败: {e}"); return; }
                }
            }
            match c.as_mut().unwrap().set_text(text.to_string()) {
                Ok(()) => eprintln!("[agent] 已复制 {} 字符", text.chars().count()),
                Err(e) => eprintln!("[agent] 复制失败: {e}"),
            }
        });
    }
    let app_weak_cp1 = app_weak.clone();
    app.on_agent_copy_message(move |idx| {
        let Some(app) = app_weak_cp1.upgrade() else { return };
        if idx < 0 { return; }
        let msgs = app.get_agent_messages();
        let Some(m) = slint::Model::row_data(&msgs, idx as usize) else { return };
        let text = if m.kind.as_str() == "script" { m.script_code.to_string() } else if m.raw.is_empty() { m.text.to_string() } else { m.raw.to_string() };
        agent_clip_set(&text);
    });
    let app_weak_cp2 = app_weak.clone();
    app.on_agent_copy_all(move || {
        let Some(app) = app_weak_cp2.upgrade() else { return };
        let msgs = app.get_agent_messages();
        let mut parts: Vec<String> = Vec::new();
        for i in 0..slint::Model::row_count(&msgs) {
            let Some(m) = slint::Model::row_data(&msgs, i) else { continue };
            match m.kind.as_str() {
                "user" => parts.push(format!("用户：{}", m.text)),
                "assistant" => parts.push(format!("AI：{}", if m.raw.is_empty() { m.text.to_string() } else { m.raw.to_string() })),
                "script" => parts.push(format!("脚本：\n{}", m.script_code)),
                _ => {}
            }
        }
        agent_clip_set(&parts.join("\n\n"));
    });
    let app_weak_aicp = app_weak.clone();
    app.on_ai_copy_all(move || {
        let Some(app) = app_weak_aicp.upgrade() else { return };
        let log = app.get_ai_log();
        let mut parts: Vec<String> = Vec::new();
        for i in 0..slint::Model::row_count(&log) {
            let Some(m) = slint::Model::row_data(&log, i) else { continue };
            match m.kind {
                4 => parts.push(format!("用户：{}", m.text)),
                5 => parts.push(format!("AI：{}", m.text)),
                1 => parts.push(format!("成功：{}", m.text)),
                2 => parts.push(format!("失败：{}", m.text)),
                _ => {}
            }
        }
        agent_clip_set(&parts.join("\n\n"));
    });
    app.on_agent_thinking_toggle(move |idx| {
        if let Some(app) = app_weak_thinking.upgrade() {
            let existing = app.get_agent_messages();
            let mut msgs: Vec<AgentMessage> =
                (0..existing.row_count()).filter_map(|i| existing.row_data(i)).collect();
            if let Some(msg) = msgs.get_mut(idx as usize) {
                msg.expanded = !msg.expanded;
            }
            app.set_agent_messages(std::rc::Rc::new(slint::VecModel::from(msgs)).into());
        }
    });

    // script-confirm / script-discard：从消息的 shadow_handle 字段解析出
    // u64，发送真实的 UserConfirm/UserDiscard 动作给后台 task；同时先把
    // 这条消息标记为中间态（confirming/discarding）防止用户重复点击。
    // 最终的 confirmed/discarded 状态由 on_event 收到 ExecutionSuccess/
    // ExecutionError 后写定，这里不假设后台一定会成功。
    let app_weak_confirm = app_weak.clone();
    let agent_runtime_for_confirm = agent_runtime.clone();
    app.on_agent_script_confirm(move |idx| {
        if let Some(app) = app_weak_confirm.upgrade() {
            let existing = app.get_agent_messages();
            let mut msgs: Vec<AgentMessage> =
                (0..existing.row_count()).filter_map(|i| existing.row_data(i)).collect();
            if let Some(msg) = msgs.get_mut(idx as usize) {
                match msg.shadow_handle.parse::<u64>() {
                    Ok(shadow_handle) => {
                        msg.script_status = "confirming".into();
                        app.set_agent_messages(std::rc::Rc::new(slint::VecModel::from(msgs)).into());
                        agent_runtime_for_confirm
                            .send_action(agent_bridge::AgentAction::UserConfirm(shadow_handle));
                    }
                    Err(e) => {
                        eprintln!(
                            "[AGENT-SCRIPT-CONFIRM] idx={idx} shadow_handle 解析失败: {:?} ({e})",
                            msg.shadow_handle
                        );
                    }
                }
            }
        }
    });

    let app_weak_discard = app_weak.clone();
    let agent_runtime_for_discard = agent_runtime.clone();
    app.on_agent_script_discard(move |idx| {
        if let Some(app) = app_weak_discard.upgrade() {
            let existing = app.get_agent_messages();
            let mut msgs: Vec<AgentMessage> =
                (0..existing.row_count()).filter_map(|i| existing.row_data(i)).collect();
            if let Some(msg) = msgs.get_mut(idx as usize) {
                match msg.shadow_handle.parse::<u64>() {
                    Ok(shadow_handle) => {
                        msg.script_status = "discarding".into();
                        app.set_agent_messages(std::rc::Rc::new(slint::VecModel::from(msgs)).into());
                        agent_runtime_for_discard
                            .send_action(agent_bridge::AgentAction::UserDiscard(shadow_handle));
                    }
                    Err(e) => {
                        eprintln!(
                            "[AGENT-SCRIPT-DISCARD] idx={idx} shadow_handle 解析失败: {:?} ({e})",
                            msg.shadow_handle
                        );
                    }
                }
            }
        }
    });

    // script-preview：暂无预览面板，先占位打印，避免编译因未绑定回调而报警告
    app.on_agent_script_preview(move |idx| {
        eprintln!("[AGENT-SCRIPT-PREVIEW] idx={} （预览面板未实现，占位）", idx);
    });

    // send-message：真实接入 roe-agent。只把用户消息追加进 UI，并通过
    // send_action 转发给后台 task；assistant 的回复、thinking 状态、
    // 待确认脚本卡片全部由 agent_runtime 的 on_event 回调异步驱动，
    // 这里不再有任何占位假回复。
    let app_weak_send = app_weak.clone();
    let agent_runtime_for_cfg = vec![agent_runtime.clone(), panel_runtime.clone()];
    let agent_runtime_for_send = agent_runtime;
    let agent_runtime_for_new_chat = agent_runtime_for_send.clone();
    let app_weak_new_chat = app_weak.clone();
    app.on_agent_send_message(move |text| {
        if text.trim().is_empty() { return; }
        if let Some(app) = app_weak_send.upgrade() {
            let existing = app.get_agent_messages();
            let mut msgs: Vec<AgentMessage> =
                (0..existing.row_count()).filter_map(|i| existing.row_data(i)).collect();
            msgs.push(AgentMessage {
                raw: "".into(),
                kind: "user".into(),
                text: text.clone(),
                script_code: "".into(),
                script_status: "".into(),
                thinking_seconds: 0.0,
                expanded: false,
                shadow_handle: "".into(),
            });
            app.set_agent_messages(std::rc::Rc::new(slint::VecModel::from(msgs)).into());
            app.set_agent_input_text("".into());
        }

        agent_runtime_for_send.send_action(agent_bridge::AgentAction::UserQuery(text.to_string()));
    });

    // 新对话/新项目/历史选择/项目选择/模式切换：暂无后端状态，先占位不报错
    // 新对话:清空界面消息 + 清空后台对话历史(预览版对话用完即丢;/exports 里的文件保留)
    app.on_agent_new_chat_clicked(move || {
        if let Some(app) = app_weak_new_chat.upgrade() {
            app.set_agent_messages(std::rc::Rc::new(slint::VecModel::<AgentMessage>::default()).into());
            app.set_agent_input_text("".into());
        }
        agent_runtime_for_new_chat.send_action(agent_bridge::AgentAction::ResetHistory);
    });
    // 项目目录：用系统文件管理器打开 Agent 工作区(路径与 agent 后台共用同一个函数)
    app.on_agent_open_workspace_clicked(move || {
        let root = agent_bridge::agent_workspace_root();
        if let Err(e) = std::fs::create_dir_all(&root) {
            eprintln!("[AGENT-OPEN-WORKSPACE] 创建目录失败 {}: {e}", root.display());
            return;
        }
        let mut cmd = if cfg!(target_os = "windows") {
            std::process::Command::new("explorer")
        } else if cfg!(target_os = "macos") {
            std::process::Command::new("open")
        } else {
            std::process::Command::new("xdg-open")
        };
        // explorer 成功时退出码也可能是 1,所以只关心 spawn 本身是否失败
        if let Err(e) = cmd.arg(&root).spawn() {
            eprintln!("[AGENT-OPEN-WORKSPACE] 拉起文件管理器失败 {}: {e}", root.display());
        }
    });
    // "+"号:选文件 -> 校验并复制进工作区 inputs/ -> 对话区追加提示行(只是界面反馈,不进后台历史)
    let app_weak_add = app_weak.clone();
    app.on_agent_add_clicked(move || {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("文档与图片", &["xlsx", "docx", "pptx", "csv", "png", "jpg", "jpeg"])
            .pick_file() else { return };
        let notice = match agent_bridge::upload_to_inputs(&agent_bridge::agent_workspace_root(), &path) {
            Ok(name) => format!("已添加 {name}"),
            Err(e) => format!("添加失败:{e}"),
        };
        if let Some(app) = app_weak_add.upgrade() {
            let existing = app.get_agent_messages();
            let mut msgs: Vec<AgentMessage> =
                (0..existing.row_count()).filter_map(|i| existing.row_data(i)).collect();
            msgs.push(AgentMessage {
                raw: "".into(),
                kind: "notice".into(),
                text: notice.into(),
                script_code: "".into(),
                script_status: "".into(),
                thinking_seconds: 0.0,
                expanded: false,
                shadow_handle: "".into(),
            });
            app.set_agent_messages(std::rc::Rc::new(slint::VecModel::from(msgs)).into());
        }
    });
    app.on_agent_new_project_clicked(move || {
        eprintln!("[AGENT-NEW-PROJECT] 占位，未实现新项目逻辑");
    });
    app.on_agent_conversation_selected(move |id| {
        eprintln!("[AGENT-CONV-SELECTED] id={} 占位，未实现历史对话切换", id);
    });
    app.on_agent_project_selected(move |id| {
        eprintln!("[AGENT-PROJECT-SELECTED] id={} 占位，未实现项目切换", id);
    });
    app.on_agent_mode_changed(move |autonomous| {
        eprintln!("[AGENT-MODE-CHANGED] autonomous={} （UI已自行双向绑定，此处仅记录）", autonomous);
    });
    app.on_ai_confirm_shadow(move || {});
    app.on_ai_discard_shadow(move || {});
    let app_weak3 = app_weak.clone();
    let panel_runtime_send = panel_runtime.clone();
    app.on_ai_send_message(move |msg| {
        let Some(app) = app_weak3.upgrade() else { return };
        let text = msg.to_string();
        if text.trim().is_empty() { return; }
        ai_push(&app, 4, text.clone());
        app.set_ai_sending(true);
        panel_runtime_send.send_action(agent_bridge::AgentAction::UserQuery(text));
    });
    // ── AI 配置:持久化 + 同步到界面 ────────────────────────────────────────
    fn ai_sync_ui(app: &AppWindow, cfg: &ai_config::AiConfig) {
        let profs: Vec<AiProfile> = cfg.profiles.iter().map(|p| AiProfile {
            name: p.name.clone().into(),
            provider: p.provider,
            endpoint: p.endpoint.clone().into(),
            key: p.key.clone().into(),
            model: p.model.clone().into(),
        }).collect();
        let names: Vec<slint::SharedString> = cfg.profiles.iter().map(|p| p.name.clone().into()).collect();
        app.set_ai_profiles(slint::ModelRc::new(slint::VecModel::from(profs)));
        app.set_ai_profile_names(slint::ModelRc::new(slint::VecModel::from(names)));
        app.set_ai_active_profile(cfg.active);
        app.set_ai_configured(
            cfg.is_configured() && cfg.to_agent_config().map_or(false, |c| c.provider == 3),
        );
    }
    // 配置变化后通知 Agent 重建;未配置完整时发空 Key,Agent 页会提示去设置
    fn agent_reconfigure(rts: &[agent_bridge::AgentRuntimeHandle], cfg: &ai_config::AiConfig) {
        let c = cfg.to_agent_config().unwrap_or(agent_bridge::AgentConfig {
            provider: 3,
            endpoint: String::new(),
            key: String::new(),
            model: String::new(),
        });
        for rt in rts {
            rt.send_action(agent_bridge::AgentAction::Reconfigure(c.clone()));
        }
    }
    let ai_cfg = Rc::new(RefCell::new(ai_config::AiConfig::load()));
    {
        let c = ai_cfg.borrow();
        app.set_ai_hf_cache_dir(c.hf_cache_dir.clone().into());
        app.set_ui_language_index(ai_config::effective_language(c.language));
        if std::env::var("ROE_LANG").is_err() { apply_ui_language(ai_config::effective_language(c.language)); }
        app.set_ui_theme_index(c.theme);
        ai_sync_ui(&app, &c);
    }

    let cfg_act = ai_cfg.clone();
    let rt_act = agent_runtime_for_cfg.clone();
    let aw_act = app_weak.clone();
    app.on_ai_activate_profile(move |idx| {
        let Some(app) = aw_act.upgrade() else { return };
        app.set_ai_save_status("".into());
        let mut c = cfg_act.borrow_mut();
        if idx >= 0 && (idx as usize) < c.profiles.len() {
            c.active = idx;
        }
        agent_reconfigure(&rt_act, &c);
        if let Err(e) = c.save() {
            ai_push(&app, 2, format!("保存配置失败: {}", e));
        }
        ai_sync_ui(&app, &c);
    });

    let cfg_save = ai_cfg.clone();
    let rt_save = agent_runtime_for_cfg.clone();
    let aw_save = app_weak.clone();
    app.on_ai_save_profile(move |idx, name, endpoint, key, model, provider| {
        let Some(app) = aw_save.upgrade() else { return };
        app.set_ai_save_status("".into());
        let name = name.trim().to_string();
        if name.is_empty() { return; }
        let p = ai_config::Profile {
            name,
            provider,
            endpoint: endpoint.trim().to_string(),
            key: key.trim().to_string(),
            model: model.trim().to_string(),
        };
        let mut c = cfg_save.borrow_mut();
        let i = if idx >= 0 && (idx as usize) < c.profiles.len() {
            c.profiles[idx as usize] = p;
            idx
        } else {
            c.profiles.push(p);
            (c.profiles.len() - 1) as i32
        };
        c.active = i; // 保存即激活
        agent_reconfigure(&rt_save, &c);
        if let Err(e) = c.save() {
            ai_push(&app, 2, format!("保存配置失败: {}", e));
        }
        ai_sync_ui(&app, &c);
    });

    let cfg_del = ai_cfg.clone();
    let rt_del = agent_runtime_for_cfg.clone();
    let aw_del = app_weak.clone();
    app.on_ai_delete_profile(move |idx| {
        let Some(app) = aw_del.upgrade() else { return };
        let mut c = cfg_del.borrow_mut();
        app.set_ai_save_status("".into());
        if idx < 0 || (idx as usize) >= c.profiles.len() { return; }
        c.profiles.remove(idx as usize);
        if c.active == idx {
            c.active = -1;
        } else if c.active > idx {
            c.active -= 1;
        }
        agent_reconfigure(&rt_del, &c);
        if let Err(e) = c.save() {
            ai_push(&app, 2, format!("保存配置失败: {}", e));
        }
        ai_sync_ui(&app, &c);
    });

    let cfg_misc = ai_cfg.clone();
    let aw_misc = app_weak.clone();
    app.on_ai_save_misc(move |hf, lang, theme| {
        let Some(app) = aw_misc.upgrade() else { return };
        let mut c = cfg_misc.borrow_mut();
        apply_ui_language(lang as i32);
        c.hf_cache_dir = hf.to_string();
        c.language = lang;
        c.theme = theme;
        if let Err(e) = c.save() {
            ai_push(&app, 2, format!("保存配置失败: {}", e));
        }
    });

    // ── 插入图表弹窗 ────────────────────────────────────────────────────────
    let state_chart = state.clone();
    let app_weak_chart = app_weak.clone();
    app.on_chart_type_changed(move |type_idx| {
        let app = match app_weak_chart.upgrade() { Some(a) => a, None => return };
        let kind = match type_idx {
            0 => "Bar",
            1 => "HorizontalBar",
            2 => "Line",
            3 => "Pie",
            4 => "Scatter",
            5 => "Radar",
            _ => "Bar",
        };
        // 用示例数据渲染预览
        let chart_json = format!(
            r#"{{"kind":"{}","title":"预览","width":480,"height":360,"x_labels":["A","B","C","D"],"series":[{{"name":"系列1","data":[120.0,200.0,150.0,80.0]}},{{"name":"系列2","data":[60.0,90.0,180.0,120.0]}}]}}"#,
            kind
        );
        let mut s = state_chart.borrow_mut();
        if let Some(sess) = s.session.as_mut() {
            if let Some((rgba, w, h)) = sess.render_chart_preview(&chart_json) {
                let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w, h);
                app.set_chart_preview_image(slint::Image::from_rgba8(buf));
            }
        }
        eprintln!("[CHART] type_changed idx={} kind={}", type_idx, kind);
    });

    let state_chart2 = state.clone();
    let app_weak_chart2 = app_weak.clone();
    let sheet_idx_chart = current_sheet_idx.clone();
    app.on_chart_confirmed(move |type_idx, title| {
        let app = match app_weak_chart2.upgrade() { Some(a) => a, None => return };
        eprintln!("[CHART] confirmed idx={} title={}", type_idx, title);

        let kind = match type_idx {
            0 => "Bar",
            1 => "HorizontalBar",
            2 => "Line",
            3 => "Pie",
            4 => "Scatter",
            5 => "Radar",
            _ => "Bar",
        };

        let mut s = state_chart2.borrow_mut();
        let sess = match s.session.as_mut() { Some(s) => s, None => return };
        let sheet_idx = sheet_idx_chart.get();

        let scroll_x = app.get_calc_scroll_x();
        let scroll_y = app.get_calc_scroll_y();
        let vw_pt = app.get_calc_view_width() as f32;
        let vh_pt = app.get_calc_view_height() as f32;
        let chart_w_pt = 560.0_f32;
        let chart_h_pt = 340.0_f32;
        let topleft_x = scroll_x + vw_pt / 2.0 - chart_w_pt / 2.0;
        let topleft_y = scroll_y + vh_pt / 2.0 - chart_h_pt / 2.0;
        let anchor_row = sess.get_row_at_offset(sheet_idx, topleft_y.max(0.0)).max(0) as u32;
        let anchor_col = sess.get_col_at_offset(sheet_idx, topleft_x.max(0.0)).max(0) as u32;
        let data_row = app.get_calc_selected_row().max(0) as u32;
        let data_col = app.get_calc_selected_col().max(0) as u32;

        // 以选中单元格为左上角，读取一小片区域(最多6行 x 4列)组装图表数据源
        // 第1列作为x_labels，第2列起每列作为一个系列
        const MAX_ROWS: u32 = 6;
        const MAX_COLS: u32 = 4;
        let mut x_labels: Vec<String> = Vec::new();
        let mut series_cols: Vec<Vec<f64>> = vec![Vec::new(); MAX_COLS.saturating_sub(1) as usize];

        for r in 0..MAX_ROWS {
            let label = sess.calc_get_cell(sheet_idx, data_row + r, data_col);
            if label.trim().is_empty() { break; }
            x_labels.push(label.replace('"', "\\\""));
            for c in 1..MAX_COLS {
                let raw = sess.calc_get_cell(sheet_idx, data_row + r, data_col + c);
                let v: f64 = raw.trim().parse().unwrap_or(0.0);
                series_cols[(c - 1) as usize].push(v);
            }
        }

        if x_labels.is_empty() {
            eprintln!("[CHART] 选中单元格 row={} col={} 无数据，取消插入", data_row, data_col);
            app.set_chart_dialog_visible(false);
            app.set_status_message("插入图表失败：所选起始单元格为空，请先选中含数据的区域左上角".into());
            return;
        }

        let x_labels_json = format!("[{}]", x_labels.iter().map(|s| format!("\"{}\"", s)).collect::<Vec<_>>().join(","));
        let series_json = series_cols.iter().enumerate()
            .filter(|(_, col)| col.iter().any(|v| *v != 0.0))
            .map(|(i, col)| {
                let data_json = format!("[{}]", col.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","));
                format!(r#"{{"name":"系列{}","data":{}}}"#, i + 1, data_json)
            })
            .collect::<Vec<_>>()
            .join(",");

        let title_escaped = title.replace('"', "\\\"");
        let chart_json = format!(
            r#"{{"kind":"{}","title":"{}","width":480,"height":360,"x_labels":{},"series":[{}]}}"#,
            kind, title_escaped, x_labels_json, series_json
        );
        eprintln!("[CHART] chart_json={}", chart_json);

        let chart_json_escaped = chart_json.replace('\\', "\\\\").replace('"', "\\\"");
        // v70：insert_chart 在 Rhai 层返回 i64（图表id，0=失败），此前
        // roe_execute_script 经 run_calc 只捕获脏区域丢弃表达式返回值，
        // ROE 已修复（return_value 字段），这里直接读取该字段拿真实id，
        // 不再需要 v58 那个 let ok=...; throw 的 bool 包装写法。
        let script = format!(
            "insert_chart({}, {}, {}, \"{}\")",
            sheet_idx, anchor_row, anchor_col, chart_json_escaped
        );
        let result = sess.execute_script(&script);
        eprintln!("[CHART] insert_chart script result={}", result);
        app.set_chart_dialog_visible(false);

        let parsed: Option<serde_json::Value> = serde_json::from_str(&result).ok();
        let chart_id: u64 = parsed
            .as_ref()
            .filter(|v| v.get("ok").and_then(|o| o.as_bool()) == Some(true))
            .and_then(|v| v.get("return_value"))
            .and_then(|rv| rv.as_i64())
            .filter(|id| *id > 0)
            .map(|id| id as u64)
            .unwrap_or(0);

        if chart_id == 0 {
            app.set_status_message(format!("插入图表失败: {}", result).into());
            return;
        }

        // 拿真实id后立刻查一次权威矩形（anchor_row/col 在ROE侧会累加成实际
        // pt坐标，不能拿前台算的topleft_x/y凑数）。rect_json 是owned String，
        // 提前解析好，避免后面在借用sess做viewport刷新时和写chart_rects的
        // 可变借用产生冲突（sess在下面render_sheet_viewport处仍有用到，
        // 借用会一直存活到那里，所以chart_rects的写入必须放在最后）。
        let rect_json_opt = sess.get_chart_rect(sheet_idx, chart_id);
        let rect_parsed: Option<(f32, f32, f32, f32)> = rect_json_opt.as_ref().and_then(|rect_json| {
            serde_json::from_str::<serde_json::Value>(rect_json).ok().map(|rv| {
                let x_pt = rv.get("x_pt").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                let y_pt = rv.get("y_pt").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                let w_pt = rv.get("width_pt").and_then(|x| x.as_f64()).unwrap_or(chart_w_pt as f64) as f32;
                let h_pt = rv.get("height_pt").and_then(|x| x.as_f64()).unwrap_or(chart_h_pt as f64) as f32;
                (x_pt, y_pt, w_pt, h_pt)
            })
        });
        if rect_parsed.is_none() {
            eprintln!("[CHART] 插入成功但get_chart_rect查询/解析失败 id={} raw={:?}", chart_id, rect_json_opt);
        }

        // execute_script不会自动触发viewport重绘，这里手动补一次主动刷新验证
        // （这是sess在本次闭包里的最后一次使用，之后才能安全地再次可变借用s）
        let scale = app.window().scale_factor();
        let vw = app.get_calc_view_width() as u32;
        let vh = app.get_calc_view_height() as u32;
        if vw > 0 && vh > 0 {
            let buf = sess.render_sheet_viewport(0.0, 0.0, vw, vh);
            if !buf.is_empty() {
                let (phys_w, phys_h) = doc_core::calc_phys_size(vw, vh);
                let pixel_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&buf, phys_w, phys_h);
                app.set_calc_render_scroll_x(0.0);
                app.set_calc_render_scroll_y(0.0);
                app.set_calc_viewport_image(slint::Image::from_rgba8(pixel_buf));
                app.window().request_redraw(); // transform 层不会自动重绘
                eprintln!("[CHART] 插入后主动刷新 OK {}x{}px", phys_w, phys_h);
            } else {
                eprintln!("[CHART] 插入后主动刷新失败：render_sheet_viewport返回空");
            }
        }

        // sess 的使用已全部结束:走覆盖层选中路径(float_sel),侧边栏由 sync_chart_sidebar 推导
        let _ = rect_parsed;
        {
            let sheet_u = sheet_idx as u32;
            refresh_float_objs(&app, &mut *s, sheet_u);
            select_float_obj(&app, &mut *s, sheet_u, chart_id);
        }

    });

    // ── 图表：点击命中判断（临时方案，等ROE的hit_test_floating接口，见回复md第四节）──
    // 前台自行维护 chart_rects 映射（chart_id -> 最近已知的pt矩形），逐个跟点击坐标比对。
    // v70更新：insert_chart 的 return_value 链路已打通（见上方插入逻辑），chart_rects
    // 现在会在插入成功后正常写入，此handler对新插入的图表可以正常命中；
    // 仅历史遗留图表（本次会话启动前已存在于文档中的图表）不在chart_rects里，
    // 暂不处理，这类图表需要点击才能命中的能力等ROE的hit_test_floating接口。
    // ── 图表：拖拽移动 ──────────────────────────────────────────────────────
    let state_chart_move_start = state.clone();
    let app_weak_chart_move_start = app_weak.clone();
    // ── Calc 动态扩展滚动范围：滚到边缘时给当前 sheet 加长一档 ──
    {
        let app_weak_ext = app_weak.clone();
        let state_ext = state.clone();
        let sheet_idx_ext = current_sheet_idx.clone();
        app.on_calc_extend_total(move |is_height| {
            let app = match app_weak_ext.upgrade() { Some(a) => a, None => return };
            let sheet = sheet_idx_ext.get() as u32;
            let s = state_ext.borrow();
            let sess = match s.session.as_ref() { Some(v) => v, None => return };
            sess.extend_sheet_total(sheet, is_height);
            app.set_calc_sheet_total_height((sess.get_sheet_total_height(sheet)) * (96.0 / 72.0));
            app.set_calc_sheet_total_width((sess.get_sheet_total_width(sheet)) * (96.0 / 72.0));
        });
    }
    // ── Calc 拖拽填充：等差数值续填 + 复制填充（公式相对引用平移、格式一并复制） ──
    {
        use std::{cell::RefCell, rc::Rc};
        let fill: Rc<RefCell<Option<(u32, u32, u32, u32, u32, u32)>>> = Rc::new(RefCell::new(None));
        {
            let app_weak = app_weak.clone();
            let fill = fill.clone();
            app.on_calc_fill_start(move || {
                let app = match app_weak.upgrade() { Some(a) => a, None => return };
                let (sc, sr) = (app.get_calc_selected_col(), app.get_calc_selected_row());
                if sc < 0 || sr < 0 { return; }
                let (ec, er) = (app.get_calc_sel_end_col(), app.get_calc_sel_end_row());
                let (ec, er) = if ec < 0 || er < 0 { (sc, sr) } else { (ec, er) };
                let (r1, r2) = (sr.min(er) as u32, sr.max(er) as u32);
                let (c1, c2) = (sc.min(ec) as u32, sc.max(ec) as u32);
                *fill.borrow_mut() = Some((r1, c1, r2, c2, r2, c2));
            });
        }
        {
            let app_weak = app_weak.clone();
            let fill = fill.clone();
            let state = state.clone();
            let sheet_idx = current_sheet_idx.clone();
            app.on_calc_fill_move(move |mx, my| {
                let app = match app_weak.upgrade() { Some(a) => a, None => return };
                let Some((r1, c1, r2, c2, _, _)) = *fill.borrow() else { return };
                let mut s = state.borrow_mut();
                let Some(sess) = s.session.as_mut() else { return };
                let sheet = sheet_idx.get() as u32;
                let (sx, sy) = (app.get_calc_scroll_x(), app.get_calc_scroll_y());
                let col = sess.get_col_at_offset(sheet, ((mx - 30.0) + sx) / PT_TO_PX);
                let row = sess.get_row_at_offset(sheet, ((my - 18.0) + sy) / PT_TO_PX);
                if col < 0 || row < 0 { return; }
                let (col, row) = (col as u32, row as u32);
                let dr = row.saturating_sub(r2);
                let dc = col.saturating_sub(c2);
                let (tr, tc) = if dr == 0 && dc == 0 { (r2, c2) } else if dr >= dc { (row, c2) } else { (r2, col) };
                *fill.borrow_mut() = Some((r1, c1, r2, c2, tr, tc));
                let x0 = sess.get_col_offset_pt(sheet, c1) * PT_TO_PX;
                let x1 = (sess.get_col_offset_pt(sheet, tc) + sess.get_column_width_pt(sheet, tc)) * PT_TO_PX;
                let y0 = sess.get_row_offset_pt(sheet, r1) * PT_TO_PX;
                let y1 = (sess.get_row_offset_pt(sheet, tr) + sess.get_row_height_pt(sheet, tr)) * PT_TO_PX;
                app.set_calc_fill_x(x0 - sx + 30.0);
                app.set_calc_fill_y(y0 - sy + 18.0);
                app.set_calc_fill_w(x1 - x0);
                app.set_calc_fill_h(y1 - y0);
                app.set_calc_fill_show(tr != r2 || tc != c2);
            });
        }
        {
            let app_weak = app_weak.clone();
            let fill = fill.clone();
            let state = state.clone();
            let sheet_idx = current_sheet_idx.clone();
            app.on_calc_fill_end(move || {
                let app = match app_weak.upgrade() { Some(a) => a, None => return };
                app.set_calc_fill_show(false);
                let Some((r1, c1, r2, c2, tr, tc)) = fill.borrow_mut().take() else { return };
                if tr == r2 && tc == c2 { return; }
                let mut s = state.borrow_mut();
                let merges = s.calc_merges.clone();
                let Some(sess) = s.session.as_mut() else { return };
                let sheet = sheet_idx.get() as u32;
                let down = tr > r2;
                let n: u32 = if down { r2 - r1 + 1 } else { c2 - c1 + 1 };
                let (lo_t, hi_t) = if down { (r2 + 1, tr) } else { (c2 + 1, tc) };
                let lines: Vec<u32> = if down { (c1..=c2).collect() } else { (r1..=r2).collect() };
                if (hi_t - lo_t + 1) as u64 * lines.len() as u64 > 200_000 {
                    app.set_status_message("填充范围过大（上限 20 万格）".into());
                    return;
                }
                let esc = |t: &str| t.replace('\\', "\\\\").replace('"', "\\\"");
                let parse = |t: &str| t.trim().parse::<f64>().ok();
                let fnum = |v: f64| format!("{}", (v * 1e9).round() / 1e9);
                let mut script = String::new();
                for line in lines {
                    let mut src: Vec<(String, bool, Option<String>)> = Vec::new();
                    for i in 0..n {
                        let (row, col) = if down { (r1 + i, line) } else { (line, c1 + i) };
                        let f = sess.calc_get_cell_formula(sheet, row, col);
                        let (txt, isf) = if !f.is_empty() { (format!("={}", f), true) } else { (sess.calc_get_cell(sheet, row, col), false) };
                        let fv = sess.calc_get_cell_format(sheet, row, col);
                        let fmt = fv.as_object().filter(|o| !o.is_empty()).map(|_| fv.to_string());
                        src.push((txt, isf, fmt));
                    }
                    let series = if n == 2 && !src[0].1 && !src[1].1 {
                        match (parse(&src[0].0), parse(&src[1].0)) {
                            (Some(a), Some(b)) => calc_fill::series_step(&[a, b]).map(|st| (a, st)),
                            _ => None,
                        }
                    } else { None };
                    for t in lo_t..=hi_t {
                        let idx = t - if down { r1 } else { c1 };
                        let k = (idx % n) as usize;
                        let (row, col) = if down { (t, line) } else { (line, t) };
                        let off = idx as i32 - k as i32;
                        let (dr, dc) = if down { (off, 0) } else { (0, off) };
                        let (txt, isf, fmt) = &src[k];
                        if let Some((a, st)) = series {
                            script.push_str(&format!("set_cell({}, {}, {}, \"{}\");\n", sheet, row, col, fnum(a + st * idx as f64)));
                        } else if *isf {
                            let sh = calc_fill::shift_formula(txt, dr, dc);
                            script.push_str(&format!("set_formula({}, {}, {}, \"{}\");\n", sheet, row, col, esc(sh.trim_start_matches('='))));
                        } else if txt.is_empty() {
                            script.push_str(&format!("clear_cell({}, {}, {});\n", sheet, row, col));
                        } else {
                            script.push_str(&format!("set_cell({}, {}, {}, \"{}\");\n", sheet, row, col, esc(txt)));
                        }
                        if let Some(f) = fmt {
                            script.push_str(&format!("set_cell_format({}, {}, {}, \"{}\");\n", sheet, row, col, esc(f)));
                        }
                    }
                }
                let ok = sess.calc_run_script(&script);
                eprintln!("[CALC-FILL] src=({},{})-({},{}) target_end=({},{}) down={} ok={}", r1, c1, r2, c2, tr, tc, down, ok);
                if !ok {
                    app.set_status_message("填充失败".into());
                    return;
                }
                calc_selection::select_anchor(&app, sess, &merges, sheet, r1, c1);
                calc_selection::drag_to(&app, sess, &merges, sheet, (r1, c1), tr.max(r2), tc.max(c2));
                app.set_calc_sel_end_row(tr.max(r2) as i32);
                app.set_calc_sel_end_col(tc.max(c2) as i32);
                let (sx, sy) = (app.get_calc_scroll_x(), app.get_calc_scroll_y());
                let vw = app.get_calc_view_width();
                let vh = app.get_calc_view_height();
                let img = sess.render_sheet_viewport(sx, sy, vw as u32, vh as u32);
                if !img.is_empty() {
                    let (phys_w, phys_h) = doc_core::calc_phys_size(vw as u32, vh as u32);
                    if phys_w > 0 && phys_h > 0 && img.len() == (phys_w * phys_h * 4) as usize {
                        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&img, phys_w, phys_h);
                        app.set_calc_render_scroll_x(sx);
                        app.set_calc_render_scroll_y(sy);
                        app.set_calc_viewport_image(slint::Image::from_rgba8(buf));
                        app.window().request_redraw();
                    }
                }
            });
        }
    }
    // ── 图表：样式面板 callback ──────────────────────────────────────────────
    fn apply_chart_style_patch(
        app: &AppWindow,
        s: &mut std::cell::RefMut<AppState>,
        sheet_idx: u32,
        chart_id: u64,
        style_json: &str,
    ) -> bool {
        if chart_id == 0 { return false; }
        let ok = match s.session.as_mut() {
            Some(sess) => sess.set_chart_style(sheet_idx, chart_id, style_json),
            None => return false,
        };
        eprintln!("[CHART-STYLE-DEBUG] chart_id={} json={} set_chart_style_ok={}", chart_id, style_json, ok);
        if ok {
            // 样式改动需要重绘viewport才能在贴图上看到效果（图表是渲染进
            // Calc viewport bitmap的，光改内存模型不会自动刷新画面）。
            // 复用已经借用的s（不能在这里再borrow_mut，否则RefCell重复借用panic）。
            let scale = app.window().scale_factor();
            let vw = app.get_calc_view_width() as u32;
            let vh = app.get_calc_view_height() as u32;
            let scroll_x = app.get_calc_scroll_x();
            let scroll_y = app.get_calc_scroll_y();
            eprintln!("[CHART-STYLE-DEBUG2] vw={} vh={} scroll_x={} scroll_y={} session_is_some={}", vw, vh, scroll_x, scroll_y, s.session.is_some());
            if vw > 0 && vh > 0 {
                if let Some(sess) = s.session.as_mut() {
                    eprintln!("[CHART-STYLE-DEBUG2] entering render_sheet_viewport call");
                    let buf = sess.render_sheet_viewport(scroll_x, scroll_y, vw, vh);
                    eprintln!("[CHART-STYLE-DEBUG2] buf.len={}", buf.len());
                    if !buf.is_empty() {
                        let (phys_w, phys_h) = doc_core::calc_phys_size(vw, vh);
                        let pixel_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&buf, phys_w, phys_h);
                        app.set_calc_render_scroll_x(scroll_x);
                        app.set_calc_render_scroll_y(scroll_y);
                        app.set_calc_viewport_image(slint::Image::from_rgba8(pixel_buf));
                        app.window().request_redraw(); // transform 层不会自动重绘
                    }
                }
            }
        }
        ok
    }

    let state_chart_legend = state.clone();
    let app_weak_chart_legend = app_weak.clone();
    let sheet_idx_chart_legend = current_sheet_idx.clone();
    app.on_chart_style_legend_toggled(move |v| {
        eprintln!("[CHART-STYLE-DEBUG] legend_toggled callback fired, v={}", v);
        let app = match app_weak_chart_legend.upgrade() { Some(a) => a, None => return };
        let mut s = state_chart_legend.borrow_mut();
        let id = s.chart_selected_id;
        let sheet_idx = sheet_idx_chart_legend.get() as u32;
        let json = format!("{{\"show_legend\":{}}}", v);
        if !apply_chart_style_patch(&app, &mut s, sheet_idx, id, &json) {
            drop(s);
            app.set_status_message("修改图例显示失败".into());
        }
    });

    let state_chart_grid = state.clone();
    let app_weak_chart_grid = app_weak.clone();
    let sheet_idx_chart_grid = current_sheet_idx.clone();
    app.on_chart_style_grid_toggled(move |v| {
        let app = match app_weak_chart_grid.upgrade() { Some(a) => a, None => return };
        let mut s = state_chart_grid.borrow_mut();
        let id = s.chart_selected_id;
        let sheet_idx = sheet_idx_chart_grid.get() as u32;
        let json = format!("{{\"show_grid\":{}}}", v);
        if !apply_chart_style_patch(&app, &mut s, sheet_idx, id, &json) {
            drop(s);
            app.set_status_message("修改网格显示失败".into());
        }
    });

    let state_chart_xlabel = state.clone();
    let app_weak_chart_xlabel = app_weak.clone();
    let sheet_idx_chart_xlabel = current_sheet_idx.clone();
    app.on_chart_style_x_label_edited(move |text| {
        let app = match app_weak_chart_xlabel.upgrade() { Some(a) => a, None => return };
        let mut s = state_chart_xlabel.borrow_mut();
        let id = s.chart_selected_id;
        let sheet_idx = sheet_idx_chart_xlabel.get() as u32;
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        let json = format!("{{\"x_axis_label\":\"{}\"}}", escaped);
        if !apply_chart_style_patch(&app, &mut s, sheet_idx, id, &json) {
            drop(s);
            app.set_status_message("修改X轴标题失败".into());
        }
    });

    let state_chart_ylabel = state.clone();
    let app_weak_chart_ylabel = app_weak.clone();
    let sheet_idx_chart_ylabel = current_sheet_idx.clone();
    app.on_chart_style_y_label_edited(move |text| {
        let app = match app_weak_chart_ylabel.upgrade() { Some(a) => a, None => return };
        let mut s = state_chart_ylabel.borrow_mut();
        let id = s.chart_selected_id;
        let sheet_idx = sheet_idx_chart_ylabel.get() as u32;
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        let json = format!("{{\"y_axis_label\":\"{}\"}}", escaped);
        if !apply_chart_style_patch(&app, &mut s, sheet_idx, id, &json) {
            drop(s);
            app.set_status_message("修改Y轴标题失败".into());
        }
    });

    // kind_index -> ChartKind字符串，与Ribbon下拉/Sidebar下拉的编码保持一致：
    // 0=Bar 1=HorizontalBar 2=Line 3=Pie 4=Scatter 5=Radar
    fn chart_kind_index_to_str(idx: i32) -> &'static str {
        match idx {
            0 => "Bar",
            1 => "HorizontalBar",
            2 => "Line",
            3 => "Pie",
            4 => "Scatter",
            5 => "Radar",
            6 => "Area",
            7 => "Doughnut",
            _ => "Line",
        }
    }

    let state_chart_kind = state.clone();
    let app_weak_chart_kind = app_weak.clone();
    let sheet_idx_chart_kind = current_sheet_idx.clone();
    app.on_chart_style_kind_changed(move |idx| {
        let app = match app_weak_chart_kind.upgrade() { Some(a) => a, None => return };
        let mut s = state_chart_kind.borrow_mut();
        let id = s.chart_selected_id;
        let sheet_idx = sheet_idx_chart_kind.get() as u32;
        let kind_str = chart_kind_index_to_str(idx);
        let json = format!("{{\"kind\":\"{}\"}}", kind_str);
        if !apply_chart_style_patch(&app, &mut s, sheet_idx, id, &json) {
            drop(s);
            app.set_status_message("修改图表类型失败".into());
        }
    });

    let state_chart_title = state.clone();
    let app_weak_chart_title = app_weak.clone();
    let sheet_idx_chart_title = current_sheet_idx.clone();
    app.on_chart_style_title_edited(move |text| {
        let app = match app_weak_chart_title.upgrade() { Some(a) => a, None => return };
        let mut s = state_chart_title.borrow_mut();
        let id = s.chart_selected_id;
        let sheet_idx = sheet_idx_chart_title.get() as u32;
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        let json = if text.is_empty() { "{\"title\":null}".to_string() } else { format!("{{\"title\":\"{}\"}}", escaped) };
        if !apply_chart_style_patch(&app, &mut s, sheet_idx, id, &json) {
            drop(s);
            app.set_status_message("修改图表标题失败".into());
        }
    });

    // hex颜色字符串 -> slint Color，供色块预览用；解析失败时返回灰色兜底
    fn parse_hex_to_slint_color(hex: &str) -> slint::Color {
        let h = hex.trim().trim_start_matches('#');
        if h.len() != 6 {
            return slint::Color::from_rgb_u8(0xcc, 0xcc, 0xcc);
        }
        let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(0xcc);
        let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(0xcc);
        let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(0xcc);
        slint::Color::from_rgb_u8(r, g, b)
    }

    let state_chart_bcolor = state.clone();
    let app_weak_chart_bcolor = app_weak.clone();
    let sheet_idx_chart_bcolor = current_sheet_idx.clone();
    app.on_chart_style_border_color_edited(move |text| {
        let app = match app_weak_chart_bcolor.upgrade() { Some(a) => a, None => return };
        let mut s = state_chart_bcolor.borrow_mut();
        let id = s.chart_selected_id;
        let sheet_idx = sheet_idx_chart_bcolor.get() as u32;
        let json = format!("{{\"border_color\":\"{}\"}}", text);
        if apply_chart_style_patch(&app, &mut s, sheet_idx, id, &json) {
            drop(s);
            app.set_chart_border_color_brush(parse_hex_to_slint_color(&text));
        } else {
            drop(s);
            app.set_status_message("修改边框颜色失败".into());
        }
    });

    let state_chart_bwidth = state.clone();
    let app_weak_chart_bwidth = app_weak.clone();
    let sheet_idx_chart_bwidth = current_sheet_idx.clone();
    app.on_chart_style_border_width_edited(move |v| {
        let app = match app_weak_chart_bwidth.upgrade() { Some(a) => a, None => return };
        let mut s = state_chart_bwidth.borrow_mut();
        let id = s.chart_selected_id;
        let sheet_idx = sheet_idx_chart_bwidth.get() as u32;
        let json = format!("{{\"border_width\":{:.2}}}", v);
        if !apply_chart_style_patch(&app, &mut s, sheet_idx, id, &json) {
            drop(s);
            app.set_status_message("修改边框宽度失败".into());
        }
    });

    // ── Calc 插入图片 / 文本框:放在当前视口左上偏移 40px 处,插入后选中并刷新 ──
    {
        let st = state.clone(); let aw = app_weak.clone(); let si = current_sheet_idx.clone();
        app.on_calc_insert_image(move || {
            let Some(app) = aw.upgrade() else { return };
            let Some(path) = rfd::FileDialog::new().add_filter("图片", &["png", "jpg", "jpeg"]).pick_file() else { return };
            let sheet = si.get() as u32;
            let (sx, sy) = (app.get_calc_scroll_x(), app.get_calc_scroll_y());
            let kk = 96.0f32 / 72.0;
            let (x_pt, y_pt) = ((sx + 40.0) / kk, (sy + 40.0) / kk);
            let new_id = {
                let mut s = st.borrow_mut();
                match s.session.as_mut() {
                    Some(sess) => {
                        sess.begin_edit();
                        let r = sess.insert_image(sheet, &path.to_string_lossy(), x_pt, y_pt, 0.0, 0.0);
                        if r.is_some() { sess.commit_edit(); } else { sess.discard_edit(); }
                        r
                    }
                    None => None,
                }
            };
            match new_id {
                None => app.set_status_message("插入图片失败(仅支持 PNG/JPEG,不超过 10MB)".into()),
                Some(id) => {
                    { let mut s = st.borrow_mut(); refresh_float_objs(&app, &mut s, sheet); select_float_obj(&app, &mut s, sheet, id); }
                    app.invoke_calc_render_viewport(sx, sy);
                }
            }
        });

        let st = state.clone(); let aw = app_weak.clone(); let si = current_sheet_idx.clone();
        app.on_calc_insert_textbox(move || {
            let Some(app) = aw.upgrade() else { return };
            let sheet = si.get() as u32;
            let (sx, sy) = (app.get_calc_scroll_x(), app.get_calc_scroll_y());
            let kk = 96.0f32 / 72.0;
            let (x_pt, y_pt) = ((sx + 40.0) / kk, (sy + 40.0) / kk);
            let new_id = {
                let mut s = st.borrow_mut();
                match s.session.as_mut() {
                    Some(sess) => {
                        sess.begin_edit();
                        let r = sess.insert_textbox(sheet, x_pt, y_pt, 200.0, 60.0, "");
                        if r.is_some() { sess.commit_edit(); } else { sess.discard_edit(); }
                        r
                    }
                    None => None,
                }
            };
            match new_id {
                None => app.set_status_message("插入文本框失败".into()),
                Some(id) => {
                    { let mut s = st.borrow_mut(); refresh_float_objs(&app, &mut s, sheet); select_float_obj(&app, &mut s, sheet, id); }
                    app.invoke_calc_render_viewport(sx, sy);
                }
            }
        });
    }

    // ── Calc 插入形状:菜单点选后直接插入默认大小,放在视口左上偏移 40px 处 ──
    {
        let st = state.clone(); let aw = app_weak.clone(); let si = current_sheet_idx.clone();
        app.on_calc_insert_shape(move |kind, is_line| {
            let Some(app) = aw.upgrade() else { return };
            let sheet = si.get() as u32;
            let (sx, sy) = (app.get_calc_scroll_x(), app.get_calc_scroll_y());
            let kk = 96.0f32 / 72.0;
            let (x_pt, y_pt) = ((sx + 40.0) / kk, (sy + 40.0) / kk);
            let h_pt = if is_line { 40.0 } else { 80.0 };
            let fill = if is_line { "" } else { "#cfe0f5" };
            let new_id = {
                let mut s = st.borrow_mut();
                match s.session.as_mut() {
                    Some(sess) => {
                        sess.begin_edit();
                        let r = sess.insert_shape(sheet, kind.as_str(), x_pt, y_pt, 120.0, h_pt, fill);
                        if r.is_some() { sess.commit_edit(); } else { sess.discard_edit(); }
                        r
                    }
                    None => None,
                }
            };
            match new_id {
                None => app.set_status_message("插入形状失败".into()),
                Some(id) => {
                    { let mut s = st.borrow_mut(); refresh_float_objs(&app, &mut s, sheet); select_float_obj(&app, &mut s, sheet, id); }
                    app.invoke_calc_render_viewport(sx, sy);
                }
            }
        });
    }

    // ── Calc 图形/文本框双击编辑文字(弹框) ──
    {
        let state_to = state.clone();
        let app_weak_to = app_weak.clone();
        let sheet_idx_to = current_sheet_idx.clone();
        app.on_calc_obj_text_open(move || {
            let app = match app_weak_to.upgrade() { Some(a) => a, None => return };
            let sheet = sheet_idx_to.get() as u32;
            let text = {
                let mut s = state_to.borrow_mut();
                let id = s.float_sel;
                if id == 0 || s.float_sel_sheet != sheet || !s.float_store.is_shape(sheet, id) { return; }
                let sess = match s.session.as_mut() { Some(x) => x, None => return };
                sess.list_objects(sheet)
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
                    .and_then(|arr| arr.as_array().and_then(|a| a.iter().find(|o| o.get("id").and_then(|x| x.as_u64()) == Some(id)).cloned()))
                    .and_then(|o| o.get("text").and_then(|x| x.as_str()).map(|t| t.to_string()))
                    .unwrap_or_default()
            };
            app.set_calc_text_value(text.into());
            app.set_calc_text_visible(true);
        });
        let state_ta = state.clone();
        let app_weak_ta = app_weak.clone();
        let sheet_idx_ta = current_sheet_idx.clone();
        app.on_calc_obj_text_apply(move |text| {
            let app = match app_weak_ta.upgrade() { Some(a) => a, None => return };
            let sheet = sheet_idx_ta.get() as u32;
            let ok = {
                let mut s = state_ta.borrow_mut();
                let id = s.float_sel;
                if id == 0 || s.float_sel_sheet != sheet { return; }
                let sess = match s.session.as_mut() { Some(x) => x, None => return };
                sess.begin_edit();
                let done = sess.set_object_text(sheet, id, text.as_str());
                if done { sess.commit_edit(); } else { sess.discard_edit(); }
                done
            };
            if !ok {
                app.set_status_message("设置文字失败".into());
                return;
            }
            let (sx, sy) = (app.get_calc_scroll_x(), app.get_calc_scroll_y());
            app.invoke_calc_render_viewport(sx, sy);
        });
    }

    // ── Calc 图形/文本框填充与边框(形状格式页签):fill:#rrggbb fill:none line:#rrggbb line:none width:n ──
    {
        let state_ss = state.clone();
        let app_weak_ss = app_weak.clone();
        let sheet_idx_ss = current_sheet_idx.clone();
        app.on_calc_shape_style(move |cmd| {
            let app = match app_weak_ss.upgrade() { Some(a) => a, None => return };
            let cmd = cmd.to_string();
            let (k, v) = match cmd.split_once(':') { Some(p) => p, None => return };
            if v.starts_with("grad:") {
                app.set_status_message("Calc 图形暂不支持渐变填充".into());
                return;
            }
            let sheet = sheet_idx_ss.get() as u32;
            let ok = {
                let mut s = state_ss.borrow_mut();
                let id = s.float_sel;
                if id == 0 || s.float_sel_sheet != sheet { return; }
                let sess = match s.session.as_mut() { Some(x) => x, None => return };
                let cur = sess.list_objects(sheet)
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
                    .and_then(|arr| arr.as_array().and_then(|a| a.iter().find(|o| o.get("id").and_then(|x| x.as_u64()) == Some(id)).cloned()));
                let (cur_color, cur_w) = cur.as_ref()
                    .and_then(|o| o.get("stroke"))
                    .map(|st| (
                        st.get("color").and_then(|x| x.as_str()).unwrap_or("#000000").to_string(),
                        st.get("width").and_then(|x| x.as_f64()).unwrap_or(1.0) as f32,
                    ))
                    .unwrap_or(("#000000".to_string(), 1.0));
                sess.begin_edit();
                let done = match k {
                    "fill" => sess.set_object_fill(sheet, id, v),
                    "line" => sess.set_object_stroke(sheet, id, v, cur_w),
                    "width" => match v.parse::<f32>() {
                        Ok(w) => sess.set_object_stroke(sheet, id, &cur_color, w),
                        Err(_) => false,
                    },
                    _ => false,
                };
                if done { sess.commit_edit(); } else { sess.discard_edit(); }
                done
            };
            if !ok {
                app.set_status_message("设置图形样式失败".into());
                return;
            }
            let (sx, sy) = (app.get_calc_scroll_x(), app.get_calc_scroll_y());
            app.invoke_calc_render_viewport(sx, sy);
        });
    }

    // ── Calc 浮动对象删除(Delete/Backspace):普通图和统计图通用,读 float_sel ──
    let state_obj_del = state.clone();
    let app_weak_obj_del = app_weak.clone();
    let sheet_idx_obj_del = current_sheet_idx.clone();
    app.on_calc_obj_delete_clicked(move || {
        let app = match app_weak_obj_del.upgrade() { Some(a) => a, None => return };
        let sheet = sheet_idx_obj_del.get() as u32;
        {
            let mut s = state_obj_del.borrow_mut();
            let id = s.float_sel;
            eprintln!("[OBJ-DEL] key fired float_sel={} float_sel_sheet={} sheet={} stat={}", id, s.float_sel_sheet, sheet, s.float_store.is_stat(sheet, id));
            if id == 0 || s.float_sel_sheet != sheet { return; }
            let ok = match s.session.as_mut() { Some(sess) => sess.delete_chart(sheet, id) || sess.delete_object(sheet, id), None => false };
            eprintln!("[OBJ-DEL] delete_chart ok={}", ok);
            if !ok { return; }
            s.float_store.remove(sheet, id);
            s.float_sel = 0;
            s.chart_selected_id = 0;
            s.chart_rects.remove(&id);
        }
        app.set_calc_obj_id(-1);
        if app.get_sidebar_kind().as_str() == "chart" { app.set_sidebar_kind("none".into()); }
        let scale = app.window().scale_factor();
        let vw = app.get_calc_view_width() as u32;
        let vh = app.get_calc_view_height() as u32;
        let (sx, sy) = (app.get_calc_scroll_x(), app.get_calc_scroll_y());
        if vw > 0 && vh > 0 {
            let mut s = state_obj_del.borrow_mut();
            if let Some(sess) = s.session.as_mut() {
                let buf = sess.render_sheet_viewport(sx, sy, vw, vh);
                if !buf.is_empty() {
                    let (pw, ph) = doc_core::calc_phys_size(vw, vh);
                    let pb = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&buf, pw, ph);
                    app.set_calc_render_scroll_x(sx);
                    app.set_calc_render_scroll_y(sy);
                    app.set_calc_viewport_image(slint::Image::from_rgba8(pb));
                    app.window().request_redraw(); // transform 层不会自动重绘
                }
            }
        }
    });

    // ── 图表：删除 ──────────────────────────────────────────────────────────
    let state_chart_delete = state.clone();
    let app_weak_chart_delete = app_weak.clone();
    let sheet_idx_chart_delete = current_sheet_idx.clone();
    app.on_chart_panel_delete_clicked(move || {
        let app = match app_weak_chart_delete.upgrade() { Some(a) => a, None => return };
        let mut s = state_chart_delete.borrow_mut();
        let id = s.chart_selected_id;
        if id == 0 { return; }
        let sheet_idx = sheet_idx_chart_delete.get() as u32;
        let sess = match s.session.as_mut() { Some(x) => x, None => return };
        let ok = sess.delete_chart(sheet_idx, id);
        if ok {
            s.chart_rects.remove(&id);
            s.chart_selected_id = 0;
            s.float_store.remove(sheet_idx, id);
            s.float_sel = 0;
            drop(s);
            app.set_calc_obj_id(-1);
            if app.get_sidebar_kind().as_str() == "chart" { app.set_sidebar_kind("none".into()); }
            // 删除后触发一次viewport重绘
            let scale = app.window().scale_factor();
            let vw = app.get_calc_view_width() as u32;
            let vh = app.get_calc_view_height() as u32;
            let scroll_x = app.get_calc_scroll_x();
            let scroll_y = app.get_calc_scroll_y();
            if vw > 0 && vh > 0 {
                let mut s2 = state_chart_delete.borrow_mut();
                if let Some(sess2) = s2.session.as_mut() {
                    let buf = sess2.render_sheet_viewport(scroll_x, scroll_y, vw, vh);
                    if !buf.is_empty() {
                        let (phys_w, phys_h) = doc_core::calc_phys_size(vw, vh);
                        let pixel_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&buf, phys_w, phys_h);
                        app.set_calc_render_scroll_x(scroll_x);
                        app.set_calc_render_scroll_y(scroll_y);
                        app.set_calc_viewport_image(slint::Image::from_rgba8(pixel_buf));
                        app.window().request_redraw(); // transform 层不会自动重绘
                    }
                }
            }
        } else {
            drop(s);
            app.set_status_message("删除图表失败".into());
        }
    });

    // ── Calc 新建Sheet ──────────────────────────────────────────────────────
    let state_add_sheet = state.clone();
    let app_weak_add_sheet = app_weak.clone();
    let sheet_idx_add = current_sheet_idx.clone();
    app.on_calc_add_sheet_requested(move || {
        let app = match app_weak_add_sheet.upgrade() { Some(a) => a, None => return };
        let mut s = state_add_sheet.borrow_mut();
        let Some(sess) = s.session.as_mut() else { return };

        // 新sheet名称：基于当前数量生成 SheetN，避免和已有名称冲突
        let existing_names = sess.get_all_sheet_names();
        let mut n = existing_names.len() + 1;
        let mut new_name = format!("Sheet{}", n);
        while existing_names.iter().any(|s| s == &new_name) {
            n += 1;
            new_name = format!("Sheet{}", n);
        }

        let new_idx = sess.add_sheet(&new_name);
        if new_idx < 0 {
            eprintln!("[CALC-ADD-SHEET] failed to add sheet");
            return;
        }
        eprintln!("[CALC-ADD-SHEET] added '{}' at idx={}", new_name, new_idx);
        // 已知问题：roe_add_sheet新建Sheet默认列宽/行高是sheet0的2倍，见roe-bug-new-sheet-default-size.md，等ROE修复

        // 更新SheetTabBar名称列表
        let updated_names = sess.get_all_sheet_names();
        let names_model: Vec<slint::SharedString> = updated_names.iter().map(|s| s.as_str().into()).collect();
        app.set_calc_sheet_names(slint::ModelRc::new(slint::VecModel::from(names_model)));
        app.set_calc_active_sheet(new_name.as_str().into());

        // 切换当前sheet索引并重渲染viewport（新sheet默认空白，从(0,0)开始显示）
        sheet_idx_add.set(new_idx as u32);
        sess.set_current_sheet(new_idx);
        {
            let (r, c) = (app.get_calc_selected_row(), app.get_calc_selected_col());
            if r >= 0 && c >= 0 {
                ribbon::calc_data::dv_list_refresh(&app, sess, new_idx as u32, r as u32, c as u32);
            } else {
                app.set_dv_list_active(false);
                app.set_dv_list_open(false);
            }
        }
        let scale = app.window().scale_factor();
        let vw = (app.get_calc_view_width() as f32) as u32;
        let vh = (app.get_calc_view_height() as f32) as u32;
        eprintln!("[CALC-ADD-SHEET] vw={} vh={} scale={}", vw, vh, scale);
        if vw == 0 || vh == 0 { eprintln!("[CALC-ADD-SHEET] skip render: vw/vh=0"); return; }
        let buf = sess.render_sheet_viewport(0.0, 0.0, vw, vh);
        eprintln!("[CALC-ADD-SHEET] render buf.len={}", buf.len());
        if !buf.is_empty() {
            let (phys_w, phys_h) = doc_core::calc_phys_size(vw, vh);
            let pixel_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &buf, phys_w, phys_h,
            );
            app.set_calc_render_scroll_x(0.0);
            app.set_calc_render_scroll_y(0.0);
            app.set_calc_viewport_image(slint::Image::from_rgba8(pixel_buf));
            app.window().request_redraw(); // transform 层不会自动重绘
        }
        app.set_calc_address("A1".into());
        app.set_calc_range_x(0.0);
        app.set_calc_range_y(0.0);
        app.set_calc_range_w(0.0);
        app.set_calc_range_h(0.0);
        app.set_calc_sel_end_row(-1);
        app.set_calc_sel_end_col(-1);
    });

    // ── 创建数据透视表：打开弹窗时自动检测数据范围（简化版：整sheet有数据矩形）───
    let state_pivot_open = state.clone();
    let app_weak_pivot_open = app_weak.clone();
    let sheet_idx_pivot_open = current_sheet_idx.clone();
    let pivot_row_fields_open = pivot_row_fields.clone();
    let pivot_col_fields_open = pivot_col_fields.clone();
    let pivot_value_fields_open = pivot_value_fields.clone();
    let pivot_filter_fields_open = pivot_filter_fields.clone();
    let pivot_source_sheet_idx_open = pivot_source_sheet_idx.clone();
    app.on_open_pivot_dialog(move || {
        let app = match app_weak_pivot_open.upgrade() { Some(a) => a, None => return };
        let mut s = state_pivot_open.borrow_mut();
        let sheet_idx = sheet_idx_pivot_open.get();
        // 锁定本次透视表操作的数据源sheet，后续生成/追加字段均使用这个值，不受生成新sheet后current_sheet_idx被切换的影响
        pivot_source_sheet_idx_open.set(sheet_idx);
        let (range_str, header_fields) = if let Some(sess) = s.session.as_mut() {
            let row_count = sess.get_row_count(sheet_idx).max(0);
            let col_count = sess.get_col_count(sheet_idx).max(0);
            let range_str = if row_count > 0 && col_count > 0 {
                let last_col_letter = col_index_to_letter((col_count - 1) as u32);
                format!("$A$1:${}${}", last_col_letter, row_count)
            } else {
                "$A$1".to_string()
            };
            // 读取第一行作为字段名（表头），空sheet时返回空列表
            let mut fields = Vec::new();
            if col_count > 0 {
                for c in 0..col_count as u32 {
                    let v = sess.calc_get_cell(sheet_idx, 0, c).trim().to_string();
                    let label = if v.is_empty() { format!("列{}", c + 1) } else { v };
                    fields.push(label);
                }
            }
            (range_str, fields)
        } else {
            ("A1".to_string(), Vec::new())
        };
        eprintln!("[PIVOT-OPEN] detected range={} fields={:?}", range_str, header_fields);

        // 重置字段选择状态（每次重新打开都清空上一次的选择）
        pivot_row_fields_open.borrow_mut().clear();
        pivot_col_fields_open.borrow_mut().clear();
        pivot_value_fields_open.borrow_mut().clear();
        pivot_filter_fields_open.borrow_mut().clear();

        let fields_model: Vec<slint::SharedString> = header_fields.iter().map(|s| s.as_str().into()).collect();
        app.set_pivot_available_fields(slint::ModelRc::new(slint::VecModel::from(fields_model)));
        app.set_pivot_row_fields(slint::ModelRc::new(slint::VecModel::from(Vec::<slint::SharedString>::new())));
        app.set_pivot_col_fields(slint::ModelRc::new(slint::VecModel::from(Vec::<slint::SharedString>::new())));
        app.set_pivot_value_fields(slint::ModelRc::new(slint::VecModel::from(Vec::<slint::SharedString>::new())));
        app.set_pivot_filter_fields(slint::ModelRc::new(slint::VecModel::from(Vec::<slint::SharedString>::new())));

        app.set_pivot_source_initial_range(range_str.into());
        app.set_pivot_source_dialog_visible(true);
    });

    // ── 创建数据透视表 第一步：选范围确认后，暂存参数 + 打开字段选择弹窗 ─────────
    let pivot_pending_source_range = pivot_pending_source.clone();
    let app_weak_pivot_range = app_weak.clone();
    app.on_pivot_source_range_accepted(move |range, new_sheet, location| {
        let app = match app_weak_pivot_range.upgrade() { Some(a) => a, None => return };
        *pivot_pending_source_range.borrow_mut() = (range.to_string(), new_sheet, location.to_string());
        if !new_sheet {
            eprintln!("[PIVOT-RANGE] 现有工作表模式暂未实现，请先选择新建工作表");
            return;
        }
        app.set_sidebar_kind("pivot".into());
    });

    // ── 创建数据透视表：字段选择弹窗内 添加/移除字段 回调 ────────────────────────
    let pivot_row_fields_add = pivot_row_fields.clone();
    let pivot_col_fields_add = pivot_col_fields.clone();
    let pivot_value_fields_add = pivot_value_fields.clone();
    let pivot_filter_fields_add = pivot_filter_fields.clone();
    let app_weak_pivot_add = app_weak.clone();
    // ── 透视表字段归属：统一收敛入口（技术债记录：目前Row/Col/Value/Filter一律互斥，
    //    不支持同一字段多次进入Value区做多聚合方式，未来对接ROE新pivot渲染时再重构为完整状态机）──
    fn set_field_zone_exclusive(
        name: &str,
        target: Option<&str>,
        row_fields: &Rc<std::cell::RefCell<Vec<String>>>,
        col_fields: &Rc<std::cell::RefCell<Vec<String>>>,
        value_fields: &Rc<std::cell::RefCell<Vec<String>>>,
        filter_fields: &Rc<std::cell::RefCell<Vec<String>>>,
    ) {
        for target_vec in [row_fields, col_fields, value_fields, filter_fields] {
            target_vec.borrow_mut().retain(|x| x != name);
        }
        if let Some(zone) = target {
            let dest: &Rc<std::cell::RefCell<Vec<String>>> = match zone {
                "row" => row_fields,
                "col" => col_fields,
                "value" => value_fields,
                "filter" => filter_fields,
                _ => return,
            };
            dest.borrow_mut().push(name.to_string());
        }
    }

    app.on_pivot_add_field_to_zone(move |name, zone| {
        let app = match app_weak_pivot_add.upgrade() { Some(a) => a, None => return };
        let name = name.to_string();
        // 勾选框点击时zone固定传"row"：若该字段已存在于任意区域，视为取消勾选，整体移除；
        // 否则（含四个按钮点击）统一走set_field_zone_exclusive，保证不会同时归属多个区域。
        let already_checked = pivot_row_fields_add.borrow().iter().any(|x| x == &name)
            || pivot_col_fields_add.borrow().iter().any(|x| x == &name)
            || pivot_value_fields_add.borrow().iter().any(|x| x == &name)
            || pivot_filter_fields_add.borrow().iter().any(|x| x == &name);

        if zone.as_str() == "row" && already_checked {
            set_field_zone_exclusive(&name, None, &pivot_row_fields_add, &pivot_col_fields_add, &pivot_value_fields_add, &pivot_filter_fields_add);
        } else {
            set_field_zone_exclusive(&name, Some(zone.as_str()), &pivot_row_fields_add, &pivot_col_fields_add, &pivot_value_fields_add, &pivot_filter_fields_add);
        }

        sync_pivot_fields_to_ui(&app, &pivot_row_fields_add, &pivot_col_fields_add, &pivot_value_fields_add, &pivot_filter_fields_add);
        app.invoke_pivot_fields_confirmed();
    });

    let pivot_row_fields_menu = pivot_row_fields.clone();
    let pivot_col_fields_menu = pivot_col_fields.clone();
    let pivot_value_fields_menu = pivot_value_fields.clone();
    let pivot_filter_fields_menu = pivot_filter_fields.clone();
    let app_weak_pivot_menu = app_weak.clone();
    // 下拉菜单"添加到XX"专用入口：与勾选框语义完全独立，永远是"精确移动到目标zone"，
    // 不含"若已勾选则取消"的特殊判断（该判断是勾选框独有的，复用add-field-to-zone会导致
    // 下拉菜单选"添加到行"在字段已在其他区域时被误判成"取消勾选"而被清空，这是本次修复的bug）
    app.on_pivot_menu_add_field_to_zone(move |name, zone| {
        let app = match app_weak_pivot_menu.upgrade() { Some(a) => a, None => return };
        let name = name.to_string();
        set_field_zone_exclusive(&name, Some(zone.as_str()), &pivot_row_fields_menu, &pivot_col_fields_menu, &pivot_value_fields_menu, &pivot_filter_fields_menu);
        sync_pivot_fields_to_ui(&app, &pivot_row_fields_menu, &pivot_col_fields_menu, &pivot_value_fields_menu, &pivot_filter_fields_menu);
        app.invoke_pivot_fields_confirmed();
    });

    let pivot_row_fields_rm = pivot_row_fields.clone();
    let pivot_col_fields_rm = pivot_col_fields.clone();
    let pivot_value_fields_rm = pivot_value_fields.clone();
    let pivot_filter_fields_rm = pivot_filter_fields.clone();
    let app_weak_pivot_rm = app_weak.clone();
    // ── 拖拽感知 步骤A：仅打印日志，验证手势能否正常触发 ──
    app.on_pivot_field_drag_started(move |name, x, y| {
        eprintln!("[PIVOT-DRAG] started name={} x={} y={}", name, x, y);
    });
    app.on_pivot_field_drag_moved(move |x, y| {
        eprintln!("[PIVOT-DRAG] moved x={} y={}", x, y);
    });
    app.on_pivot_field_drag_ended(move |x, y| {
        eprintln!("[PIVOT-DRAG] ended x={} y={}", x, y);
    });

    // ── 步骤C：拖拽跨zone移动/拖出删除 统一入口（复用set_field_zone_exclusive）──
    let pivot_row_fields_move = pivot_row_fields.clone();
    let pivot_col_fields_move = pivot_col_fields.clone();
    let pivot_value_fields_move = pivot_value_fields.clone();
    let pivot_filter_fields_move = pivot_filter_fields.clone();
    let app_weak_pivot_move = app_weak.clone();
    app.on_pivot_move_field_to_zone(move |name, zone| {
        let app = match app_weak_pivot_move.upgrade() { Some(a) => a, None => return };
        let name = name.to_string();
        // zone为空字符串表示拖到了无效区域外，视为删除；否则移动到目标zone（先移除旧归属再加入）
        let target = if zone.is_empty() { None } else { Some(zone.as_str()) };
        set_field_zone_exclusive(&name, target, &pivot_row_fields_move, &pivot_col_fields_move, &pivot_value_fields_move, &pivot_filter_fields_move);
        sync_pivot_fields_to_ui(&app, &pivot_row_fields_move, &pivot_col_fields_move, &pivot_value_fields_move, &pivot_filter_fields_move);
        app.invoke_pivot_fields_confirmed();
    });

    app.on_pivot_remove_field(move |name| {
        let app = match app_weak_pivot_rm.upgrade() { Some(a) => a, None => return };
        let name = name.to_string();
        set_field_zone_exclusive(&name, None, &pivot_row_fields_rm, &pivot_col_fields_rm, &pivot_value_fields_rm, &pivot_filter_fields_rm);
        sync_pivot_fields_to_ui(&app, &pivot_row_fields_rm, &pivot_col_fields_rm, &pivot_value_fields_rm, &pivot_filter_fields_rm);
        app.invoke_pivot_fields_confirmed();
    });

    // ── 创建数据透视表 第二步：字段确认后执行 df_load_sheet + df_pivot_to_sheet ──
    let state_pivot_accept = state.clone();
    let app_weak_pivot_accept = app_weak.clone();
    let sheet_idx_pivot_accept = current_sheet_idx.clone();
    let pivot_pending_source_accept = pivot_pending_source.clone();
    let pivot_row_fields_accept = pivot_row_fields.clone();
    let pivot_col_fields_accept = pivot_col_fields.clone();
    let pivot_value_fields_accept = pivot_value_fields.clone();
    let pivot_filter_fields_accept = pivot_filter_fields.clone();
    let pivot_source_sheet_idx_accept = pivot_source_sheet_idx.clone();
    let pivot_last_result_sheet_idx_accept = pivot_last_result_sheet_idx.clone();
    app.on_pivot_fields_confirmed(move || {
        let app = match app_weak_pivot_accept.upgrade() { Some(a) => a, None => return };
        let (range, new_sheet, location) = pivot_pending_source_accept.borrow().clone();
        let _ = (&range, &location); // 简化版：range仅用于UI展示，实际整sheet导入；location（现有工作表场景）暂未实现
        if !new_sheet {
            eprintln!("[PIVOT-ACCEPT] 现有工作表模式暂未实现，请先选择新建工作表");
            return;
        }
        let mut s = state_pivot_accept.borrow_mut();
        let Some(sess) = s.session.as_mut() else { return };

        // 当前 sheet 已是透视表:就地改配置(保留筛选/排序),不删除重建
        let cur_sheet = sheet_idx_pivot_accept.get();
        if sess.is_pivot_sheet(cur_sheet) {
            let rows = pivot_row_fields_accept.borrow().clone();
            let cols = pivot_col_fields_accept.borrow().clone();
            let values = pivot_value_fields_accept.borrow().clone();
            let filters = pivot_filter_fields_accept.borrow().clone();
            sess.begin_edit();
            let ok = sess.pivot_set_config(cur_sheet, &rows, &cols, &values, &filters);
            if ok { sess.commit_edit(); } else { sess.discard_edit(); }
            eprintln!("[PIVOT-ACCEPT] 就地更新 sheet={} rows={:?} cols={:?} values={:?} filters={:?} ok={}", cur_sheet, rows, cols, values, filters, ok);
            if ok {
                let vw = (app.get_calc_view_width() as f32) as u32;
                let vh = (app.get_calc_view_height() as f32) as u32;
                if vw > 0 && vh > 0 {
                    let buf = sess.render_sheet_viewport(0.0, 0.0, vw, vh);
                    if !buf.is_empty() {
                        let (phys_w, phys_h) = doc_core::calc_phys_size(vw, vh);
                        let pixel_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&buf, phys_w, phys_h);
                        app.set_calc_render_scroll_x(0.0);
                        app.set_calc_render_scroll_y(0.0);
                        app.set_calc_viewport_image(slint::Image::from_rgba8(pixel_buf));
                        app.window().request_redraw();
                    }
                }
            }
            return;
        }

        let source_sheet_idx = pivot_source_sheet_idx_accept.get();

        // 若上次已经生成过透视表sheet，先删除，避免每次都累加新sheet（原地覆盖效果）
        if let Some(last_idx) = pivot_last_result_sheet_idx_accept.get() {
            let removed = sess.remove_sheet(last_idx);
            eprintln!("[PIVOT-ACCEPT] 删除上次生成的sheet idx={} removed={}", last_idx, removed);
        }

        // 从用户拖拽结果生成PivotConfig（agg暂固定sum，后续如需per-field聚合方式再扩展）
        let rows_json = json_string_array(&pivot_row_fields_accept.borrow());
        let cols_json = json_string_array(&pivot_col_fields_accept.borrow());
        let values_json = json_string_array(&pivot_value_fields_accept.borrow());
        // filters：每个筛选字段取该列在源数据sheet中的真实唯一值列表，全部塞入数组
        // （效果等同于"全选"，即不实际过滤任何数据）。这样传给ROE的是真实候选值而非空数组，
        // 便于其筛选按钮渲染显示具体候选项。具体勾选值待筛选下拉菜单UI排期开发后再改为用户实际选择。
        let filters_json = {
            let filter_fields = pivot_filter_fields_accept.borrow();
            if filter_fields.is_empty() {
                "{}".to_string()
            } else {
                let entries: Vec<String> = filter_fields.iter()
                    .map(|f| {
                        let values = sess.pivot_unique_values(source_sheet_idx, f);
                        let values_json = json_string_array(&values);
                        format!("\"{}\":{}", f.replace('\\', "\\\\").replace('"', "\\\""), values_json)
                    })
                    .collect();
                format!("{{{}}}", entries.join(","))
            }
        };
        let config_json = format!(
            r#"{{"rows":{},"cols":{},"values":{},"agg":"sum","filters":{}}}"#,
            rows_json, cols_json, values_json, filters_json
        );
        eprintln!("[PIVOT-ACCEPT] config_json={}", config_json);

        let existing_names = sess.get_all_sheet_names();
        let mut n = existing_names.len() + 1;
        let mut sheet_name = format!("数据透视表{}", n);
        while existing_names.iter().any(|s| s == &sheet_name) {
            n += 1;
            sheet_name = format!("数据透视表{}", n);
        }

        // 已迁移至 ROE v85 生产路径 roe_pivot_sheet（pivot_sheet_into_workbook_v2，树状表头）。
        // 不再走 df_load_sheet+df_pivot_to_sheet 这条Rhai旧路径（该路径曾阻塞于ROE侧
        // roe_execute_script未绑定calc_df_api的bug，详见v51结论）。
        // 新接口是纯C-ABI直调，直接返回准确的新sheet索引，不再需要"sheet_count-1"猜索引。
        let new_idx = match sess.pivot_sheet(source_sheet_idx, &config_json, &sheet_name) {
            Some(idx) => {
                eprintln!("[PIVOT-ACCEPT] roe_pivot_sheet 成功，新sheet idx={}", idx);
                idx
            }
            None => {
                eprintln!("[PIVOT-ACCEPT] roe_pivot_sheet 执行失败，中止");
                return;
            }
        };
        pivot_last_result_sheet_idx_accept.set(Some(new_idx));
        app.set_pivot_result_sheet(new_idx as i32);

        // 刷新SheetTabBar并切换到新sheet
        let updated_names = sess.get_all_sheet_names();
        let names_model: Vec<slint::SharedString> = updated_names.iter().map(|s| s.as_str().into()).collect();
        app.set_calc_sheet_names(slint::ModelRc::new(slint::VecModel::from(names_model)));
        if let Some(name) = updated_names.get(new_idx as usize) {
            app.set_calc_active_sheet(name.as_str().into());
        }
        sheet_idx_pivot_accept.set(new_idx as u32);
        sess.set_current_sheet(new_idx as i32);

        let pivot_scale = app.window().scale_factor();
        let vw = (app.get_calc_view_width() as f32) as u32;
        let vh = (app.get_calc_view_height() as f32) as u32;
        if vw > 0 && vh > 0 {
            let buf = sess.render_sheet_viewport(0.0, 0.0, vw, vh);
            if !buf.is_empty() {
                let (phys_w, phys_h) = doc_core::calc_phys_size(vw, vh);
                let pixel_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &buf, phys_w, phys_h,
                );
                app.set_calc_render_scroll_x(0.0);
                app.set_calc_render_scroll_y(0.0);
                app.set_calc_viewport_image(slint::Image::from_rgba8(pixel_buf));
                app.window().request_redraw(); // transform 层不会自动重绘
            }
        }
        app.set_calc_address("A1".into());
    });

    app.on_chart_cancelled(move || {});

    // ===== 标题栏窗口控制回调（最小化/最大化/关闭/双击，拖拽本次搁置）=====
    let app_weak_win_min = app_weak.clone();
    app.on_window_minimize_clicked(move || {
        let app = match app_weak_win_min.upgrade() { Some(a) => a, None => return };
        app.window().set_minimized(true);
    });

    let app_weak_win_max = app_weak.clone();
    app.on_window_maximize_clicked(move || {
        let app = match app_weak_win_max.upgrade() { Some(a) => a, None => return };
        let currently_maximized = app.window().with_winit_window(|w| w.is_maximized()).unwrap_or(false);
        app.window().with_winit_window(|winit_win| {
            eprintln!("[MAXIMIZE] calling winit set_maximized({})", !currently_maximized);
            winit_win.set_maximized(!currently_maximized);
        });
    });

    let app_weak_win_close = app_weak.clone();
    app.on_window_close_clicked(move || {
        let app = match app_weak_win_close.upgrade() { Some(a) => a, None => return };
        app.window().hide().ok();
    });

    let app_weak_win_dbl = app_weak.clone();
    app.on_window_double_clicked(move || {
        let app = match app_weak_win_dbl.upgrade() { Some(a) => a, None => return };
        let currently_maximized = app.window().with_winit_window(|w| w.is_maximized()).unwrap_or(false);
        app.window().with_winit_window(|winit_win| {
            eprintln!("[MAXIMIZE-DBL] calling winit set_maximized({})", !currently_maximized);
            winit_win.set_maximized(!currently_maximized);
        });
    });

    // ===== 原生拖动 / 八向缩放（Wayland 下 set_position 不可用，改走合成器） =====
    let app_weak_win_drag = app_weak.clone();
    app.on_window_drag_moved(move |_dx, _dy| {
        let app = match app_weak_win_drag.upgrade() { Some(a) => a, None => return };
        let r = app.window().with_winit_window(|w| w.drag_window());
        match r {
            Some(Ok(())) => release_pointer_wayland(app_weak_win_drag.clone()),
            Some(Err(e)) => eprintln!("[WIN-DRAG] drag_window 失败: {e}"),
            None => eprintln!("[WIN-DRAG] 没有 winit 窗口"),
        }
    });

    let app_weak_win_resize = app_weak.clone();
    app.on_window_resize_started(move |d| {
        use i_slint_backend_winit::winit::window::ResizeDirection as RD;
        let app = match app_weak_win_resize.upgrade() { Some(a) => a, None => return };
        let win_max = app.window().with_winit_window(|w| w.is_maximized()).unwrap_or(false);
        if win_max { return; }
        let dir = match d {
            0 => RD::North, 1 => RD::South, 2 => RD::West, 3 => RD::East,
            4 => RD::NorthWest, 5 => RD::NorthEast, 6 => RD::SouthWest,
            _ => RD::SouthEast,
        };
        let r = app.window().with_winit_window(|w| w.drag_resize_window(dir));
        match r {
            Some(Ok(())) => release_pointer_wayland(app_weak_win_resize.clone()),
            Some(Err(e)) => eprintln!("[WIN-RESIZE] drag_resize_window 失败: {e}"),
            None => eprintln!("[WIN-RESIZE] 没有 winit 窗口"),
        }
    });

    // Writer 缩放:状态栏按钮(视口中心锚点)与 Ctrl+滚轮(鼠标位置锚点)共用 writer_apply_zoom。
    {
        // anchor_pt: 缩放前锚点距视口顶部的距离(pt);None=视口中心。缩放后该点保持在屏幕同一位置。
        fn writer_apply_zoom(app: &AppWindow, z_req: f32, anchor_pt: Option<f32>) {
            if app.get_doc_type().as_str() != "writer" { return; }
            let old_zoom = app.get_writer_zoom();
            let old_scale = doc_core::PX_PER_PT * old_zoom;
            let vw_old = app.get_writer_view_width();
            let vh_old = app.get_writer_view_height();
            let doc_w = app.get_writer_doc_width_pt();
            if old_scale <= 0.0 || vw_old <= 0.0 || vh_old <= 0.0 || doc_w <= 0.0 { return; }
            // 上限:页宽刚好填满窗口(Writer 暂无横向滚动),至少 100%,至多 500%;下限 35%。
            let fit = (vw_old * old_scale) / (doc_w * doc_core::PX_PER_PT);
            let max_zoom = fit.clamp(1.0, 5.0);
            let z = ((z_req * 100.0).round() / 100.0).clamp(0.35, max_zoom);
            app.set_zoom_level(z);
            if (z - old_zoom).abs() < 1e-4 { return; }
            let scale = doc_core::PX_PER_PT * z;
            let ratio = old_scale / scale;
            let scroll_old = app.get_writer_scroll_y_pt();
            let anchor_old = anchor_pt.unwrap_or(vh_old / 2.0).clamp(0.0, vh_old);
            let anchor_doc = scroll_old + anchor_old;
            let anchor_new = anchor_old * ratio;
            let vh_new = vh_old * ratio;
            let vw_new = vw_old * ratio;
            let canvas_h = app.get_writer_canvas_height_pt();
            let max_scroll = (canvas_h - vh_new).max(0.0);
            let target = (anchor_doc - anchor_new).clamp(0.0, max_scroll);
            doc_core::set_writer_zoom(z);
            app.set_writer_zoom(z);
            app.set_writer_view_width(vw_new);
            app.set_writer_view_height(vh_new);
            app.set_writer_scroll_y_pt(target);
            app.invoke_scroll_writer_to(target);
            app.invoke_writer_render_viewport(target);
            eprintln!("[ZOOM] {:.0}% -> {:.0}% (上限 {:.0}%) scroll_y {:.1}", old_zoom * 100.0, z * 100.0, max_zoom * 100.0, target);
        }

        
        // Impress 缩放(批3):范围 25%~300%;anchor 为画布容器坐标(px),None=视口中心。
        fn impress_apply_zoom(app: &AppWindow, z_req: f32, anchor: Option<(f32, f32)>) {
            if app.get_doc_type().as_str() != "impress" { return; }
            // z_req < 0:复位为适应窗口(渲染回调在 user-zoomed=false 时自动按窗口 contain)
            if z_req < 0.0 {
                app.set_impress_user_zoomed(false);
                app.set_impress_pan_x(0.0);
                app.set_impress_pan_y(0.0);
                app.invoke_impress_render_viewport(0.0);
                eprintln!("[IMPRESS-ZOOM] 复位为适应窗口");
                return;
            }
            let vw = app.get_impress_view_width() as f32;
            let vh = app.get_impress_view_height() as f32;
            let cw = app.get_impress_canvas_width_pt();
            let ch = app.get_impress_canvas_height_pt();
            if vw <= 0.0 || vh <= 0.0 || cw <= 0.0 || ch <= 0.0 { return; }
            app.set_impress_user_zoomed(true);
            let old = app.get_impress_scale();
            let z = ((z_req * 100.0).round() / 100.0).clamp(0.25, 3.0);
            app.set_zoom_level(z);
            if (z - old).abs() < 1e-4 { return; }
            let (w_old, h_old) = (cw * old, ch * old);
            let (cw_old, ch_old) = (vw.max(w_old), vh.max(h_old));
            let px_old = app.get_impress_pan_x().clamp(0.0, (cw_old - vw).max(0.0));
            let py_old = app.get_impress_pan_y().clamp(0.0, (ch_old - vh).max(0.0));
            let (ax, ay) = anchor.unwrap_or((vw / 2.0 + px_old, vh / 2.0 + py_old));
            let p_x = (ax - (cw_old - w_old) / 2.0) / old;
            let p_y = (ay - (ch_old - h_old) / 2.0) / old;
            let (vpx, vpy) = (ax - px_old, ay - py_old);
            let (w_new, h_new) = (cw * z, ch * z);
            let (cw_new, ch_new) = (vw.max(w_new), vh.max(h_new));
            let px_new = ((cw_new - w_new) / 2.0 + p_x * z - vpx).clamp(0.0, (cw_new - vw).max(0.0));
            let py_new = ((ch_new - h_new) / 2.0 + p_y * z - vpy).clamp(0.0, (ch_new - vh).max(0.0));
            doc_core::set_impress_scale(z);
            app.set_impress_scale(z);
            app.set_impress_pan_x(px_new);
            app.set_impress_pan_y(py_new);
            app.invoke_impress_render_viewport(0.0);
            eprintln!("[IMPRESS-ZOOM] {:.0}% -> {:.0}% pan=({:.0},{:.0})", old * 100.0, z * 100.0, px_new, py_new);
        }

        // Calc 缩放(批2):档位梯子 + 锚点保持;目标值经内核夹取,以实际生效值 calc_scale() 为准。
        const CALC_ZOOM_LADDER: [f32; 16] = [0.25, 0.30, 0.35, 0.42, 0.50, 0.60, 0.70, 0.85, 1.0, 1.2, 1.4, 1.7, 2.0, 2.4, 2.8, 3.0];
        fn calc_ladder_step(cur: f32, up: bool) -> f32 {
            if up {
                CALC_ZOOM_LADDER.iter().copied().find(|&l| l > cur + 0.01).unwrap_or(cur)
            } else {
                CALC_ZOOM_LADDER.iter().rev().copied().find(|&l| l < cur - 0.01).unwrap_or(cur)
            }
        }
        // anchor 为 calc-canvas 局部坐标(100% 坐标系);None = 视口中心
        fn calc_apply_zoom(app: &AppWindow, target: f32, anchor: Option<(f32, f32)>) {
            if app.get_doc_type().as_str() != "calc" { return; }
            let old = doc_core::calc_scale();
            doc_core::set_calc_scale(target);
            let new = doc_core::calc_scale();
            if (new - old).abs() < 1e-4 { return; }
            let vw = app.get_calc_view_width() as f32;
            let vh = app.get_calc_view_height() as f32;
            // 锚点固定为可见区域左上角(A1 可见时即 A1):滚动值不变,仅按新范围夹取
            let _ = anchor;
            let (mx, my) = (0.0_f32, 0.0_f32);
            let ratio = old / new;
            let max_x = (app.get_calc_sheet_total_width() - vw * ratio).max(0.0);
            let max_y = (app.get_calc_sheet_total_height() - vh * ratio).max(0.0);
            let sx = (app.get_calc_scroll_x() + mx * (1.0 - ratio)).clamp(0.0, max_x);
            let sy = (app.get_calc_scroll_y() + my * (1.0 - ratio)).clamp(0.0, max_y);
            app.set_calc_scroll_x(sx);
            app.set_calc_scroll_y(sy);
            app.set_calc_scale(new);
            eprintln!("[CALC-ZOOM] {:.0}% -> {:.0}% scroll=({:.0},{:.0})", old * 100.0, new * 100.0, sx, sy);
        }
        fn calc_button_zoom(app: &AppWindow, z_req: f32) {
            let cur = doc_core::calc_scale();
            let target = if (z_req - 1.0).abs() < 1e-4 {
                1.0
            } else if z_req > cur {
                calc_ladder_step(cur, true)
            } else {
                calc_ladder_step(cur, false)
            };
            calc_apply_zoom(app, target, None);
        }

        let app_weak_zoom = app_weak.clone();
        app.on_zoom_changed(move |z_req| {
            let Some(app) = app_weak_zoom.upgrade() else { return };
            if app.get_doc_type().as_str() == "impress" {
                impress_apply_zoom(&app, z_req, None);
            } else if app.get_doc_type().as_str() == "calc" {
                calc_button_zoom(&app, z_req);
            } else {
                writer_apply_zoom(&app, z_req, None);
            }
        });

        let app_weak_izoom = app_weak.clone();
        app.on_impress_wheel_zoom(move |delta, mx, my| {
            let Some(app) = app_weak_izoom.upgrade() else { return };
            if app.get_doc_type().as_str() != "impress" { return; }
            let step = (delta / 60.0).clamp(-1.0, 1.0);
            if step == 0.0 { return; }
            let old = app.get_impress_scale();
            let mut z = old * 1.1_f32.powf(step);
            if (z - old).abs() < 0.01 { z = old + 0.01 * step.signum(); }
            impress_apply_zoom(&app, z, Some((mx, my)));
        });


        let app_weak_cwheel = app_weak.clone();
        let calc_wheel_notches = std::rc::Rc::new(std::cell::Cell::new(0.0_f32));
        let calc_wheel_last = std::rc::Rc::new(std::cell::Cell::new(None::<std::time::Instant>));
        app.on_calc_wheel_zoom(move |delta, mx, my| {
            let Some(app) = app_weak_cwheel.upgrade() else { return };
            if app.get_doc_type().as_str() != "calc" { return; }
            let now = std::time::Instant::now();
            if let Some(t) = calc_wheel_last.get() {
                if now.duration_since(t).as_secs_f32() > 0.75 { calc_wheel_notches.set(0.0); }
            }
            calc_wheel_last.set(Some(now));
            let mut n = (calc_wheel_notches.get() + delta / 60.0).clamp(-8.0, 8.0);
            let mut target = doc_core::calc_scale();
            while n >= 0.6 { target = calc_ladder_step(target, true); n -= 1.0; }
            while n <= -0.6 { target = calc_ladder_step(target, false); n += 1.0; }
            calc_wheel_notches.set(n);
            if (target - doc_core::calc_scale()).abs() > 1e-4 {
                calc_apply_zoom(&app, target, Some((mx, my)));
            }
        });

        let app_weak_wheel = app_weak.clone();
        app.on_writer_wheel_zoom(move |delta, anchor_pt| {
            let Some(app) = app_weak_wheel.upgrade() else { return };
            if app.get_doc_type().as_str() != "writer" { return; }
            eprintln!("[ZOOM-WHEEL] delta={} anchor_pt={:.1}", delta, anchor_pt);
            // 每个事件最多 ×1.1(delta 按 60 归一化,触控板的小 delta 按比例缩小)
            let step = (delta / 60.0).clamp(-1.0, 1.0);
            if step == 0.0 { return; }
            let old = app.get_writer_zoom();
            let mut z = old * 1.1_f32.powf(step);
            if (z - old).abs() < 0.01 { z = old + 0.01 * step.signum(); }
            writer_apply_zoom(&app, z, Some(anchor_pt));
        });
    }

    // Impress 缩放(批1:仅用环境变量 ROE_IMPRESS_SCALE 测试对齐,默认 1.0;状态栏接入在批3)。
    if let Ok(s) = std::env::var("ROE_IMPRESS_SCALE") {
        if let Ok(v) = s.trim().parse::<f32>() {
            if v > 0.1 && v <= 5.0 {
                doc_core::set_impress_scale(v);
                app.set_impress_scale(v);
                app.set_impress_user_zoomed(true);
                eprintln!("[IMPRESS-SCALE] 测试缩放 {}", v);
            }
        }
    }

    // 设备像素比同步:启动时一次 + 200ms 轮询(窗口换屏时 scale_factor 会变)。
    // Slint 的 device-ppp 取 doc_core::device_ppp()(含 ROE_PPP 覆盖),保证位图与控件宽度同源。
    if let Ok(sv) = std::env::var("ROE_CALC_SCALE") {
        if let Ok(v) = sv.trim().parse::<f32>() { doc_core::set_calc_scale(v); }
    }
    let ppp_timer = slint::Timer::default();
    {
        let app_weak_ppp = app_weak.clone();
        let sync_ppp = Rc::new(move || {
            if let Some(app) = app_weak_ppp.upgrade() {
                let changed = doc_core::set_device_ppp(app.window().scale_factor());
                let cur = doc_core::device_ppp();
                if (app.get_device_ppp() - cur).abs() > f32::EPSILON {
                    app.set_device_ppp(cur);
                }
                let cs = doc_core::calc_scale();
                if (app.get_calc_scale() - cs).abs() > 1e-4 {
                    eprintln!("[CALC-SCALE] calc-scale -> {}", cs);
                    app.set_calc_scale(cs);
                }
                if changed {
                    eprintln!("[PPP] 设备像素比变为 {}，重渲染 Writer 视口", cur);
                    app.invoke_writer_render_viewport(app.get_writer_scroll_y_pt());
                }
            }
        });
        sync_ppp();
        let sync_ppp_t = sync_ppp.clone();
        ppp_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(200), move || sync_ppp_t());
    }

    let run_result = app.run();
    // v319r: close the document before main returns, while TLS is still alive
    if let Ok(mut s) = state.try_borrow_mut() {
        if let Some(sess) = s.session.as_mut() { sess.close_document(); }
    }
    run_result
}

// Wayland 下原生拖动/缩放一旦开始，合成器接管鼠标，应用收不到松开事件，
// Slint 会认为 TouchArea 一直被按住，导致后续按钮全部失效。这里补发一个窗外的左键松开。
// X11 / 其他平台不需要，按 WAYLAND_DISPLAY 判断。
fn release_pointer_wayland(weak: slint::Weak<AppWindow>) {
    if std::env::var("WAYLAND_DISPLAY").is_err() {
        return;
    }
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(a) = weak.upgrade() {
            a.window().dispatch_event(slint::platform::WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(-1.0, -1.0),
                button: slint::platform::PointerEventButton::Left,
            });
        }
    });
}

fn col_to_letter(col: u32) -> String {
    let mut n = col + 1;
    let mut result = String::new();
    while n > 0 {
        n -= 1;
        result.insert(0, (b'A' + (n % 26) as u8) as char);
        n /= 26;
    }
    result
}


/// Writer 文档的 AI 脚本执行:内核 execute_script 已做快照/回滚/布局缓存失效,前台只需触发重绘
fn run_ai_script_writer(ctx: &ribbon::RibbonCtx, script: &str) -> (bool, String) {
    let Some(app) = ctx.app_weak.upgrade() else { return (false, "窗口已关闭".into()) };
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return (false, "没有打开的文档".into()) };
    let res = sess.execute_script(script);
    eprintln!("[ai] writer run_script 返回: {}", res);
    if !res.contains("\"ok\":true") {
        return (false, res);
    }
    // 内核判定为只读脚本时 dirty_pages 为空:没有改动,不置脏
    if !res.contains("\"dirty_pages\":[]") {
        // 脚本可能改变了页数(如插入长文本):同步画布高度与总页数,否则只显示旧的一页
        writer_refresh_canvas(&app, &mut s);
        app.set_writer_needs_render(true);
    }
    (true, res)
}
