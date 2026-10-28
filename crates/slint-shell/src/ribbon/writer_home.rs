//! Writer「开始」页按钮接线（v231）：粗体 / 斜体 / 下划线。
use super::RibbonCtx;

/// v317c：单元格内哪些命令走「单元格格式目标」路由
fn cell_fmt_supported(sub: &str) -> bool {
    const EXACT: &[&str] = &[
        "bold", "italic", "underline", "strike", "super", "sub", "clear_format", "font_grow", "font_shrink",
        "align_left", "align_center", "align_right", "align_justify",
    ];
    const PREFIX: &[&str] = &[
        "font_family:", "font_size:", "fg_color:", "bg_color:", "shade_color:",
        "line_rule:", "line_spacing:", "space_before:", "space_after:",
    ];
    EXACT.contains(&sub) || PREFIX.iter().any(|p| sub.starts_with(p))
}

/// v317c：单元格内暂不支持、且会读写正文段落的命令（先提示，不误改正文）
fn cell_unsupported(sub: &str) -> bool {
    const EXACT: &[&str] = &[
        "indent_inc", "indent_dec",
        "copy", "cut", "paste", "format_painter", "painter_apply",
        "page_break", "soft_break", "column_break", "insert_image",
        "comment_dialog", "comment_apply", "footnote_dialog", "endnote_dialog",
        "sort_dialog",
    ];
    const PREFIX: &[&str] = &[
        "list_bullet:", "list_number:", "para_style:",
        "change_case:", "sort:",
    ];
    EXACT.contains(&sub) || PREFIX.iter().any(|p| sub.starts_with(p))
}

/// v317c：入口。光标在单元格内且命令受支持时设置格式目标，动作结束后一定清回 None
pub fn handle(sub: &str, ctx: &RibbonCtx) {
    let (cell, v318_anchor, v318_focus) = { let s = ctx.state.borrow(); (s.writer_cell_cursor, s.writer_cell_sel_anchor, s.writer_cell_sel_focus) };
    let mut v318_segs: Vec<(u32, u32, u32, u32)> = Vec::new();
    if let (Some(cur), Some(a), Some(f)) = (cell, v318_anchor, v318_focus) {
        if cur == f && a != f && cell_fmt_supported(sub) {
            if a.0 != f.0 { say(ctx, "跨单元格选区的格式暂未支持"); return; }
            let (lo, hi) = if (a.1, a.2) <= (f.1, f.2) { ((a.1, a.2), (f.1, f.2)) } else { ((f.1, f.2), (a.1, a.2)) };
            for p in lo.0..=hi.0 {
                let st = if p == lo.0 { lo.1.max(0) as u32 } else { 0 };
                let en = if p == hi.0 { hi.1.max(0) as u32 } else { u32::MAX };
                v318_segs.push((f.0, p.max(0) as u32, st, en));
            }
        }
    }
    let target = match cell {
        Some((cid, cp, _)) if cp >= 0 && cell_fmt_supported(sub) => Some((cid, cp as u32)),
        _ => None,
    };
    if cell.is_some() && cell_unsupported(sub) {
        say(ctx, "单元格内暂不支持该操作");
        return;
    }
    if v318_segs.is_empty() {
        doc_core::TileCacheRust::set_cell_fmt_target(target);
        handle_inner(sub, ctx);
        doc_core::TileCacheRust::set_cell_fmt_target(None);
        return;
    }
    for (cid, p, st, en) in v318_segs {
        doc_core::TileCacheRust::set_cell_fmt_target(Some((cid, p)));
        doc_core::TileCacheRust::set_cell_fmt_range(Some((st, en)));
        handle_inner(sub, ctx);
    }
    doc_core::TileCacheRust::set_cell_fmt_range(None);
    doc_core::TileCacheRust::set_cell_fmt_target(None);
}

fn handle_inner(sub: &str, ctx: &RibbonCtx) {
    if let Some(n) = sub.strip_prefix("font_family:") { return set_font_family(n, ctx); }
    if let Some(v) = sub.strip_prefix("font_size:") { return set_font_size(v, ctx); }
    if let Some(v) = sub.strip_prefix("fg_color:") { return set_color("fg", v, ctx); }
    if let Some(v) = sub.strip_prefix("bg_color:") { return set_color("bg", v, ctx); }
    if let Some(v) = sub.strip_prefix("shade_color:") { return set_shade(v, ctx); }
    if let Some(v) = sub.strip_prefix("line_rule:") { return set_line_rule(v, ctx); }
    if let Some(v) = sub.strip_prefix("sort:") { return do_sort(v, ctx); }
    if let Some(v) = sub.strip_prefix("page_setup:") { return set_page_setup(v, ctx); }
    if let Some(v) = sub.strip_prefix("list_bullet:") { return set_list(1, v, ctx); }
    if let Some(v) = sub.strip_prefix("list_number:") { return set_list(2, v, ctx); }
    if let Some(v) = sub.strip_prefix("para_style:") { return set_style(v, ctx); }
    if let Some(v) = sub.strip_prefix("change_case:") { return change_case(v.parse().unwrap_or(0), ctx); }
    if let Some(v) = sub.strip_prefix("columns:") { return set_columns(v.parse().unwrap_or(1), ctx); }
    if let Some(v) = sub.strip_prefix("line_spacing:") { return set_spacing(0, v.parse().unwrap_or(-1.0), ctx); }
    if let Some(v) = sub.strip_prefix("space_before:") { return set_spacing(1, v.parse().unwrap_or(-1.0), ctx); }
    if let Some(v) = sub.strip_prefix("space_after:") { return set_spacing(2, v.parse().unwrap_or(-1.0), ctx); }
    match sub {
        "indent_inc" => return change_indent(1, ctx),
        "indent_dec" => return change_indent(-1, ctx),
        "font_grow" => return step_font_size(1.0, ctx),
        "clear_format" => return clear_format(ctx),
        "font_shrink" => return step_font_size(-1.0, ctx),
        "font_list" => return load_font_list(ctx),
        "sync" => return sync(ctx),
        "show_marks" => return toggle_marks(ctx),
        "page_break" => return page_break(ctx),
        "soft_break" => return soft_break(ctx),
        "line_spacing_dialog" => return open_line_dialog(ctx),
        "page_setup_dialog" => return open_page_setup(ctx),
        "header_footer_dialog" => return open_header_footer(ctx),
        "header_footer_apply" => return apply_header_footer(ctx),
        "comment_dialog" => return open_comment_dialog(ctx),
        "comment_apply" => return apply_comment(ctx),
        "footnote_dialog" => return open_note_dialog(ctx, 0),
        "endnote_dialog" => return open_note_dialog(ctx, 1),
        "comment_delete_current" => return delete_current_comment(ctx),
        "comment_prev" => return step_comment(ctx, -1),
        "comment_next" => return step_comment(ctx, 1),
        "sort_dialog" => return open_sort_dialog(ctx),
        "delete_footnote" => return delete_note(ctx, 0),
        "delete_endnote" => return delete_note(ctx, 1),
        "column_break" => return column_break(ctx),
        "insert_image" => return insert_image(ctx),
        _ => {}
    }
    match sub {
        "bold" => toggle("bold", ctx),
        "italic" => toggle("italic", ctx),
        "underline" => toggle("underline", ctx),
        "strike" => toggle("strike", ctx),
        "super" => toggle("super", ctx),
        "sub" => toggle("sub", ctx),
        "align_left" => align(0, ctx),
        "align_center" => align(1, ctx),
        "align_right" => align(2, ctx),
        "align_justify" => align(3, ctx),
        "find" => open_find("find", ctx),
        "replace" => open_find("replace", ctx),
        "copy" => clip_copy(false, ctx),
        "cut" => clip_copy(true, ctx),
        "paste" => clip_paste(ctx),
        "format_painter" => painter_toggle(ctx),
        "painter_apply" => painter_apply(ctx),
        _ => eprintln!("[ribbon] writer 未接线的命令: {}", sub),
    }
}

/// v263: 行距/段前/段后。kind 0=行距倍数 1=段前pt 2=段后pt
fn set_spacing(kind: u32, value: f32, ctx: &RibbonCtx) {
    let cell_mode = doc_core::TileCacheRust::cell_fmt_target().is_some();
    let mut s = ctx.state.borrow_mut();
    let a = s.writer_sel_anchor_para;
    let c = s.writer_cursor_para;
    if !cell_mode && (a < 0 || c < 0) { return; }
    let (p1, p2) = if a <= c { (a, c) } else { (c, a) };
    let Some(sess) = s.session.as_mut() else { return };
    let mut err: Option<String> = None;
    if !cell_mode { sess.writer_undo_group(true); }
    for p in (if cell_mode { 0..=0i64 } else { p1..=p2 }) {
        if let Err(e) = sess.writer_set_para_spacing(p as u32, kind, value) {
            err = Some(e);
            break;
        }
    }
    if !cell_mode { if let Some(sess) = s.session.as_mut() { sess.writer_undo_group(false); } }
    drop(s);
    match err {
        Some(e) => say(ctx, &format!("设置间距失败: {}", e)),
        None => {
            refresh_canvas(ctx);
            if let Some(app) = ctx.app_weak.upgrade() {
                app.set_writer_needs_render(true);
            }
            refresh_selection(ctx);
        }
    }
}

/// v265: 切换段落标记显示
fn toggle_marks(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let on = !app.get_writer_show_marks();
    let res = {
        let s = ctx.state.borrow();
        match s.session.as_ref() { Some(sess) => sess.writer_set_show_marks(on), None => Err("没有打开的文档".to_string()) }
    };
    match res {
        Ok(()) => { app.set_writer_show_marks(on); app.set_writer_needs_render(true); }
        Err(e) => say(ctx, &format!("切换段落标记失败: {}", e)),
    }
}

/// v266: 更改大小写
fn change_case(mode: u32, ctx: &RibbonCtx) {
    let mut s = ctx.state.borrow_mut();
    let a = (s.writer_sel_anchor_para, s.writer_sel_anchor_offset);
    let c = (s.writer_cursor_para, s.writer_cursor_offset);
    if a == c {
        drop(s);
        say(ctx, "请先选中文字");
        return;
    }
    let (start, end) = if a <= c { (a, c) } else { (c, a) };
    if start.0 < 0 || start.1 < 0 { return; }
    let Some(sess) = s.session.as_mut() else { return };
    let ranges = ranges_of(&*sess, start, end, 1_000_000_000);
    let mut err: Option<String> = None;
    sess.writer_undo_group(true);
    for &(p, so, eo) in &ranges {
        if let Err(e) = sess.writer_change_case(p, so, eo, mode) {
            err = Some(e);
            break;
        }
    }
    if let Some(sess) = s.session.as_mut() { sess.writer_undo_group(false); }
    drop(s);
    match err {
        Some(e) => say(ctx, &format!("更改大小写失败: {}", e)),
        None => {
            if let Some(app) = ctx.app_weak.upgrade() {
                app.set_writer_needs_render(true);
            }
            refresh_selection(ctx);
        }
    }
}

/// v267: 在光标处插入分页符
fn page_break(ctx: &RibbonCtx) {
    let (p, res) = {
        let mut s = ctx.state.borrow_mut();
        let p = s.writer_cursor_para;
        let o = s.writer_cursor_offset;
        if p < 0 || o < 0 { return; }
        let r = s.session.as_mut().map(|x| x.writer_insert_page_break(p as u32, o as u32));
        (p, r)
    };
    match res {
        Some(Ok(split)) => {
            refresh_canvas(ctx);
            place_cursor(ctx, if split { p + 1 } else { p }, 0);
            say(ctx, "已插入分页符");
        }
        Some(Err(e)) => say(ctx, &format!("插入分页符失败: {}", e)),
        None => {}
    }
}

/// v268: 页面高度变化后（分页符/行距），重取文档尺寸刷新画布高度与页数
fn refresh_canvas(ctx: &RibbonCtx) {
    let size = {
        let mut s = ctx.state.borrow_mut();
        s.session.as_mut().and_then(|sess| sess.get_writer_document_size())
    };
    if let Some((doc_w, canvas_h, page_count)) = size {
        if let Some(app) = ctx.app_weak.upgrade() {
            app.set_writer_doc_width_pt(doc_w);
            app.set_writer_canvas_height_pt(canvas_h);
            app.set_page_total(page_count);
        }
    }
}

/// v270: Shift+Enter 段内手动换行（插入 \n，不拆段）
fn soft_break(ctx: &RibbonCtx) {
    let (p, o, res) = {
        let mut s = ctx.state.borrow_mut();
        let p = s.writer_cursor_para;
        let o = s.writer_cursor_offset;
        if p < 0 || o < 0 { return; }
        let r = s.session.as_mut().and_then(|x| x.insert_char(p as u32, o as u32, '\n' as u32));
        (p, o, r)
    };
    match res {
        Some(_) => {
            place_cursor(ctx, p, o + 1);
            if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
        }
        None => say(ctx, "插入换行失败"),
    }
}

/// v272: 段落底纹（hex 为空=清除）
fn set_shade(hex: &str, ctx: &RibbonCtx) {
    let cell_mode = doc_core::TileCacheRust::cell_fmt_target().is_some();
    let rgb: Option<u32> = if hex.is_empty() {
        None
    } else if hex.len() == 7 && hex.starts_with('#') {
        match u32::from_str_radix(&hex[1..], 16) {
            Ok(v) => Some(v),
            Err(_) => { say(ctx, "颜色无效"); return; }
        }
    } else {
        say(ctx, "颜色无效");
        return;
    };
    let mut s = ctx.state.borrow_mut();
    let a = s.writer_sel_anchor_para;
    let c = s.writer_cursor_para;
    if !cell_mode && (a < 0 || c < 0) { return; }
    let (p1, p2) = if a <= c { (a, c) } else { (c, a) };
    let Some(sess) = s.session.as_mut() else { return };
    let mut err: Option<String> = None;
    if !cell_mode { sess.writer_undo_group(true); }
    for p in (if cell_mode { 0..=0i64 } else { p1..=p2 }) {
        if let Err(e) = sess.writer_set_para_shading(p as u32, rgb) {
            err = Some(e);
            break;
        }
    }
    if !cell_mode { if let Some(sess) = s.session.as_mut() { sess.writer_undo_group(false); } }
    drop(s);
    match err {
        Some(e) => say(ctx, &format!("设置底纹失败: {}", e)),
        None => {
            if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
            refresh_selection(ctx);
        }
    }
}

/// v273: 打开行距选项对话框，按光标所在段回显
fn open_line_dialog(ctx: &RibbonCtx) {
    let info: Option<serde_json::Value> = {
        let s = ctx.state.borrow();
        let p = s.writer_cursor_para;
        if p < 0 { None } else {
            s.session.as_ref()
                .and_then(|x| x.get_paragraph_info(p as u32))
                .and_then(|j| serde_json::from_str(&j).ok())
        }
    };
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let (mut mode, mut value, mut before, mut after) = (0i32, "1.3".to_string(), String::new(), String::new());
    if let Some(v) = info {
        if let (Some(abs), Some(ex)) = (v["line_abs_pt"].as_f64(), v["line_exact"].as_bool()) {
            mode = if ex { 1 } else { 2 };
            value = format!("{}", abs);
        } else if let Some(m) = v["line_height"].as_f64() {
            value = format!("{}", m);
        }
        if let Some(b) = v["space_before"].as_f64() { before = format!("{}", b); }
        if let Some(a) = v["space_after"].as_f64() { after = format!("{}", a); }
    }
    app.set_line_dlg_mode(mode);
    app.set_line_dlg_value(value.into());
    app.set_line_dlg_before(before.into());
    app.set_line_dlg_after(after.into());
    app.set_line_dlg_visible(true);
}

/// v273: 应用行距对话框结果。参数 "模式:数值:段前:段后"，段前/段后留空=不改
fn set_line_rule(arg: &str, ctx: &RibbonCtx) {
    let cell_mode = doc_core::TileCacheRust::cell_fmt_target().is_some();
    let parts: Vec<&str> = arg.splitn(4, ':').collect();
    if parts.len() < 4 { say(ctx, "行距参数无效"); return; }
    let mode: u32 = parts[0].trim().parse().unwrap_or(0);
    let Ok(value) = parts[1].trim().parse::<f32>() else { say(ctx, "请输入有效的行距数值"); return; };
    let before: Option<f32> = parts[2].trim().parse().ok();
    let after: Option<f32> = parts[3].trim().parse().ok();
    let mut s = ctx.state.borrow_mut();
    let a = s.writer_sel_anchor_para;
    let c = s.writer_cursor_para;
    if !cell_mode && (a < 0 || c < 0) { return; }
    let (p1, p2) = if a <= c { (a, c) } else { (c, a) };
    let Some(sess) = s.session.as_mut() else { return };
    let mut err: Option<String> = None;
    if !cell_mode { sess.writer_undo_group(true); }
    for p in (if cell_mode { 0..=0i64 } else { p1..=p2 }) {
        if let Err(e) = sess.writer_set_para_line_rule(p as u32, mode, value) { err = Some(e); break; }
        if let Some(b) = before {
            if let Err(e) = sess.writer_set_para_spacing(p as u32, 1, b) { err = Some(e); break; }
        }
        if let Some(af) = after {
            if let Err(e) = sess.writer_set_para_spacing(p as u32, 2, af) { err = Some(e); break; }
        }
    }
    if !cell_mode { if let Some(sess) = s.session.as_mut() { sess.writer_undo_group(false); } }
    drop(s);
    match err {
        Some(e) => say(ctx, &format!("设置行距失败: {}", e)),
        None => {
            if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
            refresh_canvas(ctx);
            refresh_selection(ctx);
        }
    }
}

/// v274: 打开排序对话框。需要选中多个段落
fn open_sort_dialog(ctx: &RibbonCtx) {
    {
        let s = ctx.state.borrow();
        let (a, c) = (s.writer_sel_anchor_para, s.writer_cursor_para);
        if a < 0 || c < 0 || a == c {
            drop(s);
            say(ctx, "请选择多个段落");
            return;
        }
    }
    let Some(app) = ctx.app_weak.upgrade() else { return };
    app.set_sort_dlg_kind(0);
    app.set_sort_dlg_desc(false);
    app.set_sort_dlg_visible(true);
}

/// v274: 应用排序。参数 "类型:降序(0/1)"
fn do_sort(arg: &str, ctx: &RibbonCtx) {
    let parts: Vec<&str> = arg.splitn(2, ':').collect();
    if parts.len() < 2 { say(ctx, "排序参数无效"); return; }
    let kind: u32 = parts[0].trim().parse().unwrap_or(0);
    let desc = parts[1].trim() == "1";
    let mut s = ctx.state.borrow_mut();
    let a = s.writer_sel_anchor_para;
    let c = s.writer_cursor_para;
    if a < 0 || c < 0 || a == c {
        drop(s);
        say(ctx, "请选择多个段落");
        return;
    }
    let (p1, p2) = if a <= c { (a, c) } else { (c, a) };
    let Some(sess) = s.session.as_mut() else { return };
    let res = sess.writer_sort_paragraphs(p1 as u32, p2 as u32, kind, desc);
    drop(s);
    match res {
        Err(e) => say(ctx, &format!("排序失败: {}", e)),
        Ok(()) => {
            if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
            refresh_canvas(ctx);
            refresh_selection(ctx);
        }
    }
}

const PAGE_PAPERS: [(f32, f32); 5] = [(595.28, 841.89), (841.89, 1190.55), (419.53, 595.28), (612.0, 792.0), (612.0, 1008.0)];
const PT_PER_CM: f32 = 28.3465;

/// v275: 打开页面设置对话框，按当前文档回显
fn open_page_setup(ctx: &RibbonCtx) {
    let info: Option<serde_json::Value> = {
        let s = ctx.state.borrow();
        s.session.as_ref()
            .and_then(|x| x.writer_get_page_setup())
            .and_then(|j| serde_json::from_str(&j).ok())
    };
    let Some(v) = info else { say(ctx, "无法读取页面设置"); return; };
    let g = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
    let (w, h) = (g("width"), g("height"));
    let (lo, hi) = (w.min(h), w.max(h));
    let mut paper = 5i32;
    for (i, (pw, ph)) in PAGE_PAPERS.iter().enumerate() {
        if (lo - pw).abs() < 2.0 && (hi - ph).abs() < 2.0 { paper = i as i32; break; }
    }
    let orient = if w > h { 1 } else { 0 };
    let cm = |pt: f32| format!("{:.2}", pt / PT_PER_CM);
    let Some(app) = ctx.app_weak.upgrade() else { return };
    app.set_page_dlg_paper(paper);
    app.set_page_dlg_orient(orient);
    app.set_page_dlg_top(cm(g("top")).into());
    app.set_page_dlg_bottom(cm(g("bottom")).into());
    app.set_page_dlg_left(cm(g("left")).into());
    app.set_page_dlg_right(cm(g("right")).into());
    app.set_page_dlg_curw(format!("{}", w).into());
    app.set_page_dlg_curh(format!("{}", h).into());
    app.set_page_dlg_visible(true);
}

/// v275: 应用页面设置。参数 "纸张:方向:上:下:左:右:当前宽:当前高"（边距单位 cm）
fn set_page_setup(arg: &str, ctx: &RibbonCtx) {
    let p: Vec<&str> = arg.splitn(8, ':').collect();
    if p.len() < 8 { say(ctx, "页面设置参数无效"); return; }
    let paper: i32 = p[0].trim().parse().unwrap_or(5);
    let orient: i32 = p[1].trim().parse().unwrap_or(0);
    let mut m = [0f32; 4];
    for i in 0..4 {
        match p[2 + i].trim().parse::<f32>() {
            Ok(x) if x >= 0.0 => m[i] = x * PT_PER_CM,
            _ => { say(ctx, "请输入有效的页边距（厘米）"); return; }
        }
    }
    let (cw, ch) = (p[6].trim().parse::<f32>().unwrap_or(595.28), p[7].trim().parse::<f32>().unwrap_or(841.89));
    let (lo, hi) = if (0..5).contains(&paper) {
        let (a, b) = PAGE_PAPERS[paper as usize];
        (a, b)
    } else {
        (cw.min(ch), cw.max(ch))
    };
    let (w, h) = if orient == 1 { (hi, lo) } else { (lo, hi) };
    let res = {
        let s = ctx.state.borrow();
        match s.session.as_ref() {
            Some(sess) => sess.writer_set_page_setup(w, h, m[0], m[1], m[2], m[3]),
            None => return,
        }
    };
    match res {
        Err(e) => say(ctx, &format!("设置页面失败: {}", e)),
        Ok(()) => {
            if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
            refresh_canvas(ctx);
            refresh_selection(ctx);
        }
    }
}

/// v278: 打开页眉页脚对话框，按当前文档回显
fn open_header_footer(ctx: &RibbonCtx) {
    let info: Option<serde_json::Value> = {
        let s = ctx.state.borrow();
        s.session.as_ref()
            .and_then(|x| x.writer_get_header_footer())
            .and_then(|j| serde_json::from_str(&j).ok())
    };
    let Some(v) = info else { say(ctx, "无法读取页眉页脚"); return; };
    let Some(app) = ctx.app_weak.upgrade() else { return };
    app.set_hf_dlg_header(v["header"].as_str().unwrap_or("").into());
    app.set_hf_dlg_footer(v["footer"].as_str().unwrap_or("").into());
    app.set_hf_dlg_tab(0);
    app.set_hf_dlg_halign(v["header_align"].as_i64().unwrap_or(1) as i32);
    app.set_hf_dlg_falign(v["footer_align"].as_i64().unwrap_or(1) as i32);
    app.set_hf_dlg_numpos(v["num_pos"].as_i64().unwrap_or(0) as i32);
    app.set_hf_dlg_mode(v["num_mode"].as_i64().unwrap_or(0) as i32);
    app.set_hf_dlg_numalign(v["num_align"].as_i64().unwrap_or(1) as i32);
    app.set_hf_dlg_title(v["title_page"].as_bool().unwrap_or(false));
    app.set_hf_dlg_evenodd(v["even_odd"].as_bool().unwrap_or(false));
    app.set_hf_dlg_visible(true);
}

/// v278: 应用页眉页脚（内容从对话框属性读取）
fn apply_header_footer(ctx: &RibbonCtx) {
    let (h, f, ha, fa, np, nm, na, tp, eo) = {
        let Some(app) = ctx.app_weak.upgrade() else { return };
        (
            app.get_hf_dlg_header().to_string(),
            app.get_hf_dlg_footer().to_string(),
            app.get_hf_dlg_halign(),
            app.get_hf_dlg_falign(),
            app.get_hf_dlg_numpos(),
            app.get_hf_dlg_mode(),
            app.get_hf_dlg_numalign(),
            app.get_hf_dlg_title(),
            app.get_hf_dlg_evenodd(),
        )
    };
    let res = {
        let s = ctx.state.borrow();
        match s.session.as_ref() {
            Some(sess) => sess.writer_set_header_footer(&h, &f, ha, fa, np, nm, na, tp, eo),
            None => return,
        }
    };
    match res {
        Err(e) => say(ctx, &format!("设置页眉页脚失败: {}", e)),
        Ok(()) => {
            if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
            refresh_canvas(ctx);
            refresh_selection(ctx);
        }
    }
}

/// v284: 取当前选区 -> (起点段, 起点偏移, 终点段, 终点偏移)；无选区返回 None
fn comment_span(ctx: &RibbonCtx) -> Option<(u32, u32, u32, u32)> {
    let s = ctx.state.borrow();
    let a = (s.writer_sel_anchor_para, s.writer_sel_anchor_offset);
    let c = (s.writer_cursor_para, s.writer_cursor_offset);
    if a == c || a.0 < 0 || a.1 < 0 || c.0 < 0 || c.1 < 0 { return None; }
    let (start, end) = if a <= c { (a, c) } else { (c, a) };
    let sess = s.session.as_ref()?;
    let ranges = ranges_of(sess, start, end, 1_000_000_000);
    let first = ranges.first()?;
    let last = ranges.last()?;
    Some((first.0 as u32, first.1 as u32, last.0 as u32, last.2 as u32))
}

/// v284: 打开新建批注对话框（需要先选中文字）
fn open_comment_dialog(ctx: &RibbonCtx) {
    if comment_span(ctx).is_none() {
        say(ctx, "请先选中文字");
        return;
    }
    let Some(app) = ctx.app_weak.upgrade() else { return };
    app.set_cm_dlg_edit_id(-1);
    app.set_cm_dlg_reply_to(-1);
    app.set_cm_dlg_title("新建批注".into());
    app.set_cm_dlg_text("".into());
    app.set_cm_dlg_visible(true);
}

/// v284: 应用新建批注
fn apply_comment(ctx: &RibbonCtx) {
    let text = {
        let Some(app) = ctx.app_weak.upgrade() else { return };
        app.get_cm_dlg_text().to_string()
    };
    if text.trim().is_empty() { say(ctx, "批注内容不能为空"); return; }
    let edit_id = ctx.app_weak.upgrade().map(|a| a.get_cm_dlg_edit_id()).unwrap_or(-1);
    if edit_id >= 0 {
        let res = {
            let s = ctx.state.borrow();
            match s.session.as_ref() {
                Some(sess) => sess.writer_edit_comment(edit_id as u32, &text),
                None => return,
            }
        };
        if let Some(app) = ctx.app_weak.upgrade() {
            app.set_cm_dlg_edit_id(-1);
            app.set_cm_dlg_title("新建批注".into());
        }
        match res {
            Err(e) => say(ctx, &format!("编辑批注失败: {}", e)),
            Ok(()) => {
                if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
                refresh_canvas(ctx);
                refresh_selection(ctx);
            }
        }
        return;
    }
    let note_code = ctx.app_weak.upgrade().map(|a| a.get_cm_dlg_reply_to()).unwrap_or(-1);
    if note_code == -2 || note_code == -3 {
        let kind: u32 = if note_code == -2 { 0 } else { 1 };
        let (para, off) = {
            let s = ctx.state.borrow();
            (s.writer_cursor_para, s.writer_cursor_offset)
        };
        if para < 0 || off < 0 { say(ctx, "请先在正文中点击要插入的位置"); return; }
        let res = {
            let s = ctx.state.borrow();
            match s.session.as_ref() {
                Some(sess) => sess.writer_insert_note(para as u32, off as u32, kind, &text),
                None => return,
            }
        };
        if let Some(app) = ctx.app_weak.upgrade() {
            app.set_cm_dlg_reply_to(-1);
            app.set_cm_dlg_title("新建批注".into());
        }
        match res {
            Err(e) => say(ctx, &format!("插入注释失败: {}", e)),
            Ok(()) => {
                if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
                refresh_canvas(ctx);
                refresh_selection(ctx);
            }
        }
        return;
    }
    let reply_to = ctx.app_weak.upgrade().map(|a| a.get_cm_dlg_reply_to()).unwrap_or(-1);
    if reply_to >= 0 {
        let res = {
            let s = ctx.state.borrow();
            match s.session.as_ref() {
                Some(sess) => sess.writer_reply_comment(reply_to as u32, "用户", &text),
                None => return,
            }
        };
        if let Some(app) = ctx.app_weak.upgrade() {
            app.set_cm_dlg_reply_to(-1);
            app.set_cm_dlg_title("新建批注".into());
        }
        match res {
            Err(e) => say(ctx, &format!("添加回复失败: {}", e)),
            Ok(()) => {
                if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
                refresh_canvas(ctx);
                refresh_selection(ctx);
            }
        }
        return;
    }
    let Some((pa, oa, pb, ob)) = comment_span(ctx) else { say(ctx, "请先选中文字"); return; };
    let res = {
        let s = ctx.state.borrow();
        match s.session.as_ref() {
            Some(sess) => sess.writer_insert_comment(pa, oa, pb, ob, "用户", &text),
            None => return,
        }
    };
    match res {
        Err(e) => say(ctx, &format!("插入批注失败: {}", e)),
        Ok(_) => {
            if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
            refresh_canvas(ctx);
            refresh_selection(ctx);
        }
    }
}

/// v290: 删除当前选中（点击过气泡）的批注
fn delete_current_comment(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let id = app.get_writer_current_comment();
    if id < 0 { say(ctx, "请先点击要删除的批注气泡"); return; }
    let res = {
        let s = ctx.state.borrow();
        match s.session.as_ref() {
            Some(sess) => sess.writer_delete_comment(id as u32),
            None => return,
        }
    };
    match res {
        Err(e) => say(ctx, &format!("删除批注失败: {}", e)),
        Ok(()) => {
            app.set_writer_current_comment(-1);
            say(ctx, "已删除批注");
            app.set_writer_needs_render(true);
            refresh_canvas(ctx);
            refresh_selection(ctx);
        }
    }
}

/// v290: 在当前可见气泡之间切换选中。dir=1 下一条，dir=-1 上一条
fn step_comment(ctx: &RibbonCtx, dir: i32) {
    use slint::Model;
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut list: Vec<(f32, i32)> = app
        .get_writer_comment_bubbles()
        .iter()
        .map(|b| (b.anchor_ry as f32, b.comment_id))
        .collect();
    if list.is_empty() { say(ctx, "当前视图没有批注"); return; }
    list.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let cur = app.get_writer_current_comment();
    let n = list.len() as i32;
    let next = match list.iter().position(|x| x.1 == cur) {
        Some(i) => (((i as i32) + dir) % n + n) % n,
        None => if dir > 0 { 0 } else { n - 1 },
    };
    app.set_writer_current_comment(list[next as usize].1);
}

/// v295: 打开脚注/尾注输入弹窗（复用批注弹窗，reply_to=-2 脚注 / -3 尾注）
fn open_note_dialog(ctx: &RibbonCtx, kind: i32) {
    {
        let s = ctx.state.borrow();
        if s.writer_cursor_para < 0 || s.writer_cursor_offset < 0 {
            drop(s);
            say(ctx, "请先在正文中点击要插入的位置");
            return;
        }
    }
    let Some(app) = ctx.app_weak.upgrade() else { return };
    app.set_cm_dlg_edit_id(-1);
    app.set_cm_dlg_reply_to(-2 - kind);
    app.set_cm_dlg_title(if kind == 0 { "插入脚注" } else { "插入尾注" }.into());
    app.set_cm_dlg_text("".into());
    app.set_cm_dlg_visible(true);
}

fn say(ctx: &RibbonCtx, msg: &str) {
    if let Some(app) = ctx.app_weak.upgrade() {
        app.set_status_message(msg.into());
    }
}

fn toggle(kind: &str, ctx: &RibbonCtx) {
    let cell_mode = doc_core::TileCacheRust::cell_fmt_target().is_some();
    let mut s = ctx.state.borrow_mut();
    let a = (s.writer_sel_anchor_para, s.writer_sel_anchor_offset);
    let c = (s.writer_cursor_para, s.writer_cursor_offset);
    if a == c && !cell_mode {
        drop(s);
        say(ctx, "请先选中文字");
        return;
    }
    let (start, end) = if a <= c { (a, c) } else { (c, a) };
    if !cell_mode && (start.0 < 0 || start.1 < 0) { return; }
    let Some(sess) = s.session.as_mut() else { return };

    // 按段拆分出 (para, start, end) 区间
    let mut ranges: Vec<(u32, u32, u32)> = Vec::new();
    if cell_mode { ranges.push((0, 0, 0)); }
    for p in start.0..=(if cell_mode { start.0 - 1 } else { end.0 }) {
        let len = sess
            .get_paragraph_info(p as u32)
            .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
            .and_then(|v| v["para_len"].as_u64());
        let Some(len) = len else { continue };
        let so = if p == start.0 { start.1 as u64 } else { 0 };
        let eo = if p == end.0 { end.1 as u64 } else { len };
        if so < eo {
            ranges.push((p as u32, so as u32, eo as u32));
        }
    }
    if ranges.is_empty() {
        return;
    }
    let all_on = ranges.iter().all(|&(p, so, eo)| match kind {
        "strike" => sess.writer_get_strike(p, so, eo) == 1,
        "super" => sess.writer_get_script(p, so, eo, 1) == 1,
        "sub" => sess.writer_get_script(p, so, eo, 2) == 1,
        _ => sess.writer_get_char_format(kind, p, so, eo) == 1,
    });
    let mut err: Option<String> = None;
    if !cell_mode { sess.writer_undo_group(true); }
    for &(p, so, eo) in &ranges {
        let r = match kind {
            "strike" => sess.writer_set_strike(p, so, eo, if all_on { 0 } else { 1 }),
            "super" => sess.writer_set_script(p, so, eo, if all_on { 0 } else { 1 }),
            "sub" => sess.writer_set_script(p, so, eo, if all_on { 0 } else { 2 }),
            _ => sess.writer_set_char_format(kind, p, so, eo, if all_on { 0 } else { 1 }),
        };
        if let Err(e) = r {
            err = Some(e);
            break;
        }
    }
    if !cell_mode { if let Some(sess) = s.session.as_mut() { sess.writer_undo_group(false); } }
    drop(s);
    match err {
        Some(e) => say(ctx, &format!("设置格式失败: {}", e)),
        None => {
            if let Some(app) = ctx.app_weak.upgrade() {
                app.set_writer_needs_render(true);
            }
            refresh_selection(ctx);
        }
    }
}

/// v253: 增减缩进。有选区作用于选中的所有段，无选区作用于光标所在段
fn change_indent(delta: i32, ctx: &RibbonCtx) {
    let mut s = ctx.state.borrow_mut();
    let a = s.writer_sel_anchor_para;
    let c = s.writer_cursor_para;
    if a < 0 || c < 0 {
        return;
    }
    let (p1, p2) = if a <= c { (a, c) } else { (c, a) };
    let Some(sess) = s.session.as_mut() else { return };
    let mut err: Option<String> = None;
    sess.writer_undo_group(true);
    for p in p1..=p2 {
        if let Err(e) = sess.writer_change_indent(p as u32, delta) {
            err = Some(e);
            break;
        }
    }
    if let Some(sess) = s.session.as_mut() { sess.writer_undo_group(false); }
    drop(s);
    match err {
        Some(e) => say(ctx, &format!("调整缩进失败: {}", e)),
        None => {
            if let Some(app) = ctx.app_weak.upgrade() {
                app.set_writer_needs_render(true);
            }
            refresh_selection(ctx);
        }
    }
}

/// 段落对齐：有选区作用于选中的所有段，无选区作用于光标所在段
fn align(value: u32, ctx: &RibbonCtx) {
    let cell_mode = doc_core::TileCacheRust::cell_fmt_target().is_some();
    let mut s = ctx.state.borrow_mut();
    let a = s.writer_sel_anchor_para;
    let c = s.writer_cursor_para;
    if !cell_mode && (a < 0 || c < 0) { return; }
    let (p1, p2) = if a <= c { (a, c) } else { (c, a) };
    let Some(sess) = s.session.as_mut() else { return };
    let mut err: Option<String> = None;
    if !cell_mode { sess.writer_undo_group(true); }
    for p in (if cell_mode { 0..=0i64 } else { p1..=p2 }) {
        if let Err(e) = sess.writer_set_alignment(p as u32, value) {
            err = Some(e);
            break;
        }
    }
    if !cell_mode { if let Some(sess) = s.session.as_mut() { sess.writer_undo_group(false); } }
    drop(s);
    match err {
        Some(e) => say(ctx, &format!("设置对齐失败: {}", e)),
        None => {
            if let Some(app) = ctx.app_weak.upgrade() {
                app.set_writer_needs_render(true);
            }
            refresh_selection(ctx);
        }
    }
}


// ───────────── v235：字体 / 字号 ─────────────

fn para_len(sess: &doc_core::TileCacheRust, p: i64) -> Option<u64> {
    sess.get_paragraph_info(p as u32)
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v["para_len"].as_u64())
}

/// 把选区按段拆成 (para, start, end)；max_paras 限制最多处理多少段（轮询时用小值）
fn ranges_of(sess: &doc_core::TileCacheRust, start: (i64, i64), end: (i64, i64), max_paras: i64) -> Vec<(u32, u32, u32)> {
    let mut out = Vec::new();
    let last = end.0.min(start.0 + max_paras);
    for p in start.0..=last {
        let Some(len) = para_len(sess, p) else { continue };
        let so = if p == start.0 { start.1 as u64 } else { 0 };
        let eo = if p == end.0 { end.1 as u64 } else { len };
        if so < eo {
            out.push((p as u32, so as u32, eo as u32));
        }
    }
    out
}

/// 对当前选区的每个段内区间执行 f；无选区提示
fn for_selection<F>(ctx: &RibbonCtx, mut f: F)
where
    F: FnMut(&doc_core::TileCacheRust, u32, u32, u32) -> Result<(), String>,
{
    let cell_mode = doc_core::TileCacheRust::cell_fmt_target().is_some();
    let mut s = ctx.state.borrow_mut();
    let a = (s.writer_sel_anchor_para, s.writer_sel_anchor_offset);
    let c = (s.writer_cursor_para, s.writer_cursor_offset);
    if a == c && !cell_mode {
        drop(s);
        say(ctx, "请先选中文字");
        return;
    }
    let (start, end) = if a <= c { (a, c) } else { (c, a) };
    if !cell_mode && (start.0 < 0 || start.1 < 0) { return; }
    let Some(sess) = s.session.as_mut() else { return };
    let ranges = if cell_mode { vec![(0u32, 0u32, 0u32)] } else { ranges_of(&*sess, start, end, 1_000_000_000) };
    let mut err: Option<String> = None;
    if !cell_mode { sess.writer_undo_group(true); }
    for &(p, so, eo) in &ranges {
        if let Err(e) = f(&*sess, p, so, eo) {
            err = Some(e);
            break;
        }
    }
    if !cell_mode { if let Some(sess) = s.session.as_mut() { sess.writer_undo_group(false); } }
    drop(s);
    match err {
        Some(e) => say(ctx, &format!("设置格式失败: {}", e)),
        None => {
            if let Some(app) = ctx.app_weak.upgrade() {
                app.set_writer_needs_render(true);
            }
            refresh_selection(ctx);
        }
    }
}

fn set_font_family(name: &str, ctx: &RibbonCtx) {
    for_selection(ctx, |sess, p, so, eo| sess.writer_set_font_family(p, so, eo, Some(name)));
}

fn set_font_size(text: &str, ctx: &RibbonCtx) {
    let Ok(size) = text.trim().parse::<f32>() else {
        say(ctx, "字号无效");
        return;
    };
    if !(1.0..=400.0).contains(&size) {
        say(ctx, "字号超出范围 (1-400)");
        return;
    }
    for_selection(ctx, |sess, p, so, eo| sess.writer_set_font_size(p, so, eo, size));
}

fn step_font_size(delta: f32, ctx: &RibbonCtx) {
    for_selection(ctx, |sess, p, so, eo| {
        let cur = sess.writer_get_font_size(p, so, eo);
        let base = if cur >= 0.0 { cur } else { 11.0 };
        let next = (base + delta).clamp(1.0, 400.0);
        sess.writer_set_font_size(p, so, eo, next)
    });
}

/// v318x: 读取本机已装字体（fc-list）。失败返回空，调用方退回内置列表。
fn system_font_names_v318x() -> Vec<String> {
    let out = match std::process::Command::new("fc-list").arg(":").arg("family").output() {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut set = std::collections::BTreeSet::new();
    for line in text.lines() {
        let first = line.split(',').next().unwrap_or("").trim().replace('\\', "");
        if first.is_empty() || first.contains("Nerd Font") || first.starts_with("icomoon") { continue; }
        set.insert(first);
    }
    set.into_iter().take(300).collect()
}

/// 字体下拉列表：Calc 已加载过则复用，否则用内置常用字体
fn load_font_list(ctx: &RibbonCtx) {
    use slint::Model;
    let Some(app) = ctx.app_weak.upgrade() else { return };
    if app.get_font_menu_list().row_count() > 0 {
        return;
    }
    let names = [
        "等线", "宋体", "黑体", "楷体", "仿宋", "微软雅黑",
        "Noto Sans CJK SC", "Noto Serif CJK SC",
        "Arial", "Times New Roman", "Calibri", "Carlito",
        "Courier New", "Liberation Sans", "Liberation Serif", "DejaVu Sans",
    ];
    let sys_names = system_font_names_v318x();
    let names: Vec<&str> = if sys_names.is_empty() { names.to_vec() } else { sys_names.iter().map(|s| s.as_str()).collect() };
    let items: Vec<crate::FontItem> = names
        .iter()
        .map(|n| crate::FontItem {
            label: if *n == "Carlito" { "Carlito (默认)".into() } else { (*n).into() },
            value: (*n).into(),
        })
        .collect();
    app.set_font_menu_list(slint::ModelRc::new(slint::VecModel::from(items)));
}

/// 轮询：把选区（无选区则取光标前一个字符）的字体名/字号回显到功能区
fn sync(ctx: &RibbonCtx) {
    let Ok(mut s) = ctx.state.try_borrow_mut() else { return };
    let a = (s.writer_sel_anchor_para, s.writer_sel_anchor_offset);
    let c = (s.writer_cursor_para, s.writer_cursor_offset);
    if a.0 < 0 || a.1 < 0 || c.0 < 0 || c.1 < 0 {
        return;
    }
    let Some(sess) = s.session.as_mut() else { return };
    let is_sel = a != c;
    let ranges: Vec<(u32, u32, u32)> = if is_sel {
        let (st, en) = if a <= c { (a, c) } else { (c, a) };
        ranges_of(&*sess, st, en, 30)
    } else {
        let mut v = Vec::new();
        if let Some(len) = para_len(&*sess, c.0) {
            let off = c.1 as u64;
            if len > 0 {
                if off > 0 && off <= len {
                    v.push((c.0 as u32, (off - 1) as u32, off as u32));
                } else if off < len {
                    v.push((c.0 as u32, off as u32, (off + 1) as u32));
                }
            }
        }
        v
    };
    if ranges.is_empty() {
        return;
    }
    let sizes: Vec<f32> = ranges.iter().map(|&(p, so, eo)| sess.writer_get_font_size(p, so, eo)).collect();
    if sizes.iter().any(|v| *v < -1.5) {
        return;
    }
    let first = sizes[0];
    let size_text = if first >= 0.0 && sizes.iter().all(|v| (*v - first).abs() < 0.01) {
        if first.fract() == 0.0 { format!("{}", first as i64) } else { format!("{}", first) }
    } else if !is_sel && first < 0.0 {
        "11".to_string()
    } else {
        String::new()
    };
    let fams: Vec<Option<String>> = ranges.iter().map(|&(p, so, eo)| sess.writer_get_font_family(p, so, eo)).collect();
    let f0 = fams[0].clone();
    let name = if fams.iter().all(|f| *f == f0) {
        match f0 {
            Some(n) if n == "__MIXED__" => String::new(),
            Some(n) if n == "Carlito" => "Carlito (默认)".to_string(),
            Some(n) => n,
            None => "等线 (中文正文)".to_string(),
        }
    } else {
        String::new()
    };
    drop(s);
    if let Some(app) = ctx.app_weak.upgrade() {
        if app.get_writer_font_name().as_str() != name {
            app.set_writer_font_name(name.into());
        }
        if app.get_writer_font_size_text().as_str() != size_text {
            app.set_writer_font_size_text(size_text.into());
        }
    }
}

/// v235b：格式改动后重新计算选区高亮和光标位置（字号/字体变化会改变文字宽度和行高）
fn refresh_selection(ctx: &RibbonCtx) {
    let mut s = ctx.state.borrow_mut();
    if s.writer_cell_cursor.is_some() {
        if let Some(app) = ctx.app_weak.upgrade() {
            crate::writer_cell_refresh(&app, &mut s);
        }
        return;
    }
    let a = (s.writer_sel_anchor_para, s.writer_sel_anchor_offset);
    let c = (s.writer_cursor_para, s.writer_cursor_offset);
    if a.0 < 0 || a.1 < 0 || c.0 < 0 || c.1 < 0 {
        return;
    }
    let Some(sess) = s.session.as_mut() else { return };
    let rects_json = if a != c {
        sess.query_selection_rects(a.0 as u32, a.1 as u32, c.0 as u32, c.1 as u32)
    } else {
        None
    };
    let (cx, cy, _cw, ch) = sess.writer_query_cursor_rect(c.0 as u64, c.1 as u64);
    drop(s);

    use doc_core::CANVAS_OFFSET_PT;
    let mut items: Vec<crate::SelectionRect> = Vec::new();
    if let Some(json) = rects_json {
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&json) {
            for r in arr {
                let rx = r.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let ry = r.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32 + CANVAS_OFFSET_PT;
                let rw = r.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let rh = r.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                items.push(crate::SelectionRect { rx, ry, rw, rh });
            }
        }
    }
    if let Some(app) = ctx.app_weak.upgrade() {
        app.set_writer_cursor_x(cx as i32);
        app.set_writer_cursor_y(cy as i32);
        app.set_writer_cursor_h(ch as i32);
        app.set_writer_selection_rects(slint::ModelRc::new(slint::VecModel::from(items)));
    }
}

/// v237：清除格式——把选区内的字符内联覆盖全部清掉
fn clear_format(ctx: &RibbonCtx) {
    for_selection(ctx, |sess, p, so, eo| sess.writer_clear_char_format(p, so, eo));
}

/// v238：文字颜色 / 背景色。hex="#RRGGBB"，空串=清除
fn set_color(which: &str, hex: &str, ctx: &RibbonCtx) {
    let rgb: Option<u32> = if hex.is_empty() {
        None
    } else if hex.len() == 7 && hex.starts_with('#') {
        match u32::from_str_radix(&hex[1..], 16) {
            Ok(v) => Some(v),
            Err(_) => { say(ctx, "颜色无效"); return; }
        }
    } else {
        say(ctx, "颜色无效");
        return;
    };
    for_selection(ctx, |sess, p, so, eo| {
        let r = sess.writer_set_color(which, p, so, eo, rgb);
        r
    });
}


// ───────────── v242：项目符号 / 编号 ─────────────
/// variant 为菜单下标：0=无（取消），1.. 对应具体样式
fn set_list(kind: u32, variant: &str, ctx: &RibbonCtx) {
    let v: u32 = variant.parse().unwrap_or(0);
    let (k, v) = if v == 0 { (0u32, 0u32) } else { (kind, v) };
    let mut s = ctx.state.borrow_mut();
    let a = s.writer_sel_anchor_para;
    let c = s.writer_cursor_para;
    if a < 0 || c < 0 {
        return;
    }
    let (p1, p2) = if a <= c { (a, c) } else { (c, a) };
    let Some(sess) = s.session.as_mut() else { return };
    let mut err: Option<String> = None;
    sess.writer_undo_group(true);
    for p in p1..=p2 {
        if let Err(e) = sess.writer_set_list(p as u32, k, v) {
            err = Some(e);
            break;
        }
    }
    if let Some(sess) = s.session.as_mut() { sess.writer_undo_group(false); }
    drop(s);
    match err {
        Some(e) => say(ctx, &format!("设置列表失败: {}", e)),
        None => {
            if let Some(app) = ctx.app_weak.upgrade() {
                app.set_writer_needs_render(true);
            }
            refresh_selection(ctx);
        }
    }
}


// ───────────── v244：段落样式 ─────────────
fn set_style(kind: &str, ctx: &RibbonCtx) {
    let k: u32 = kind.parse().unwrap_or(0);
    let mut s = ctx.state.borrow_mut();
    let a = s.writer_sel_anchor_para;
    let c = s.writer_cursor_para;
    if a < 0 || c < 0 {
        return;
    }
    let (p1, p2) = if a <= c { (a, c) } else { (c, a) };
    let Some(sess) = s.session.as_mut() else { return };
    let mut err: Option<String> = None;
    sess.writer_undo_group(true);
    for p in p1..=p2 {
        if let Err(e) = sess.writer_set_style(p as u32, k) {
            err = Some(e);
            break;
        }
    }
    if let Some(sess) = s.session.as_mut() { sess.writer_undo_group(false); }
    drop(s);
    match err {
        Some(e) => say(ctx, &format!("设置样式失败: {}", e)),
        None => {
            if let Some(app) = ctx.app_weak.upgrade() {
                app.set_writer_needs_render(true);
            }
            refresh_selection(ctx);
        }
    }
}


// ───────────── v245：打开查找 / 替换弹窗 ─────────────
fn open_find(tab: &str, ctx: &RibbonCtx) {
    if let Some(app) = ctx.app_weak.upgrade() {
        app.set_find_bar_tab(tab.into());
        app.set_find_bar_visible(true);
    }
}


// ───────────── v247：剪切 / 复制 / 粘贴（应用内，字符级）─────────────
fn place_cursor(ctx: &RibbonCtx, p: i64, o: i64) {
    let mut s = ctx.state.borrow_mut();
    s.writer_cursor_para = p;
    s.writer_cursor_offset = o;
    s.writer_sel_anchor_para = p;
    s.writer_sel_anchor_offset = o;
    let rect = s.session.as_mut().map(|sess| sess.writer_query_cursor_rect(p as u64, o as u64));
    if let Some((cx, _cy, _cw, _ch)) = rect {
        s.writer_cursor_ideal_x = Some(cx);
    }
    drop(s);
    if let Some(app) = ctx.app_weak.upgrade() {
        if let Some((cx, cy, _cw, ch)) = rect {
            app.set_writer_cursor_x(cx as i32);
            app.set_writer_cursor_y(cy as i32);
            app.set_writer_cursor_h(ch as i32);
            app.set_writer_cursor_visible(true);
        }
        app.set_writer_needs_render(true);
    }
    refresh_selection(ctx);
}

fn clip_copy(cut: bool, ctx: &RibbonCtx) {
    let (st, res) = {
        let mut s = ctx.state.borrow_mut();
        let a = (s.writer_sel_anchor_para, s.writer_sel_anchor_offset);
        let c = (s.writer_cursor_para, s.writer_cursor_offset);
        if a == c {
            drop(s);
            say(ctx, "请先选中文字");
            return;
        }
        if a.0 < 0 || a.1 < 0 || c.0 < 0 || c.1 < 0 { return; }
        let (x, y) = if a <= c { (a, c) } else { (c, a) };
        let Some(sess) = s.session.as_mut() else { return };
        if sess.get_doc_type() != "writer" { return; }
        let mut r = sess.writer_clip_copy(x.0 as u32, x.1 as u32, y.0 as u32, y.1 as u32);
        if cut && r.is_ok() {
            r = sess.writer_clip_delete(x.0 as u32, x.1 as u32, y.0 as u32, y.1 as u32);
        }
        (x, r)
    };
    match res {
        Err(e) => say(ctx, &format!("{}失败: {}", if cut { "剪切" } else { "复制" }, e)),
        Ok(()) => {
            if cut {
                place_cursor(ctx, st.0, st.1);
                say(ctx, "已剪切");
            } else {
                say(ctx, "已复制");
            }
        }
    }
}

fn clip_paste(ctx: &RibbonCtx) {
    let outcome = {
        let mut s = ctx.state.borrow_mut();
        let a = (s.writer_sel_anchor_para, s.writer_sel_anchor_offset);
        let c = (s.writer_cursor_para, s.writer_cursor_offset);
        if a.0 < 0 || a.1 < 0 || c.0 < 0 || c.1 < 0 { return; }
        let (x, y) = if a <= c { (a, c) } else { (c, a) };
        let Some(sess) = s.session.as_mut() else { return };
        if sess.get_doc_type() != "writer" { return; }
        if !sess.writer_clip_has() {
            drop(s);
            say(ctx, "剪贴板为空");
            return;
        }
        sess.writer_undo_group(true);
        let mut r: Result<(), String> = Ok(());
        if a != c {
            r = sess.writer_clip_delete(x.0 as u32, x.1 as u32, y.0 as u32, y.1 as u32);
        }
        let pr = match r {
            Ok(()) => sess.writer_clip_paste(x.0 as u32, x.1 as u32),
            Err(e) => Err(e),
        };
        sess.writer_undo_group(false);
        pr
    };
    match outcome {
        Err(e) => say(ctx, &format!("粘贴失败: {}", e)),
        Ok((p, o)) => {
            place_cursor(ctx, p as i64, o as i64);
            say(ctx, "已粘贴");
        }
    }
}


// ───────────── v249：格式刷 ─────────────
struct WPainter { mode: u8, last_click: Option<std::time::Instant> }
thread_local! {
    static WPAINTER: std::cell::RefCell<WPainter> = std::cell::RefCell::new(WPainter { mode: 0, last_click: None });
}

fn painter_set_active(ctx: &RibbonCtx, on: bool) {
    if let Some(app) = ctx.app_weak.upgrade() {
        app.set_writer_painter_active(on);
    }
}

fn painter_reset(ctx: &RibbonCtx) {
    WPAINTER.with(|p| {
        let mut p = p.borrow_mut();
        p.mode = 0;
        p.last_click = None;
    });
    painter_set_active(ctx, false);
}

fn painter_toggle(ctx: &RibbonCtx) {
    let now = std::time::Instant::now();
    let (mode, last) = WPAINTER.with(|p| { let p = p.borrow(); (p.mode, p.last_click) });
    let is_double = last.map_or(false, |t| now.duration_since(t).as_millis() < 400);
    if mode == 1 && is_double {
        WPAINTER.with(|p| p.borrow_mut().mode = 2);
        painter_set_active(ctx, true);
        say(ctx, "格式刷：连续模式（再点按钮退出）");
        return;
    }
    if mode != 0 {
        painter_reset(ctx);
        say(ctx, "已退出格式刷");
        return;
    }
    let picked: Result<(), String> = {
        let mut s = ctx.state.borrow_mut();
        let a = (s.writer_sel_anchor_para, s.writer_sel_anchor_offset);
        let c = (s.writer_cursor_para, s.writer_cursor_offset);
        if a.0 < 0 || a.1 < 0 || c.0 < 0 || c.1 < 0 { return; }
        let pos = if a != c {
            if a <= c { a } else { c }
        } else if c.1 > 0 {
            (c.0, c.1 - 1)
        } else {
            c
        };
        let Some(sess) = s.session.as_mut() else { return };
        if sess.get_doc_type() != "writer" { return; }
        sess.writer_painter_pick(pos.0 as u32, pos.1 as u32)
    };
    match picked {
        Err(e) => say(ctx, &format!("格式刷取格式失败: {}", e)),
        Ok(()) => {
            WPAINTER.with(|p| {
                let mut p = p.borrow_mut();
                p.mode = 1;
                p.last_click = Some(now);
            });
            painter_set_active(ctx, true);
            say(ctx, "格式刷：请拖选要应用的文字");
        }
    }
}

/// 鼠标在文档区抬起时调用（只有刷状态开着才会被触发）。没有选区则不处理。
fn painter_apply(ctx: &RibbonCtx) {
    let mode = WPAINTER.with(|p| p.borrow().mode);
    if mode == 0 { return; }
    let mut s = ctx.state.borrow_mut();
    let a = (s.writer_sel_anchor_para, s.writer_sel_anchor_offset);
    let c = (s.writer_cursor_para, s.writer_cursor_offset);
    if a == c { return; }
    let (start, end) = if a <= c { (a, c) } else { (c, a) };
    if start.0 < 0 || start.1 < 0 { return; }
    let Some(sess) = s.session.as_mut() else { return };
    let ranges = ranges_of(&*sess, start, end, 1_000_000_000);
    let mut err: Option<String> = None;
    sess.writer_undo_group(true);
    for &(p, so, eo) in &ranges {
        if let Err(e) = sess.writer_painter_apply(p, so, eo) {
            err = Some(e);
            break;
        }
    }
    if let Some(sess) = s.session.as_mut() { sess.writer_undo_group(false); }
    drop(s);
    match err {
        Some(e) => say(ctx, &format!("格式刷应用失败: {}", e)),
        None => {
            if mode == 1 { painter_reset(ctx); }
            if let Some(app) = ctx.app_weak.upgrade() {
                app.set_writer_needs_render(true);
            }
            refresh_selection(ctx);
            say(ctx, "已应用格式");
        }
    }
}

/// v298: 删除光标所在段落的脚注/尾注（kind 0=脚注 1=尾注）
fn delete_note(ctx: &RibbonCtx, kind: u32) {
    let (p, o) = {
        let s = ctx.state.borrow();
        (s.writer_cursor_para, s.writer_cursor_offset)
    };
    if p < 0 || o < 0 { say(ctx, "请先点击脚注/尾注引用所在的位置"); return; }
    let res = {
        let s = ctx.state.borrow();
        match s.session.as_ref() {
            Some(sess) => sess.writer_delete_note(p as u32, o as u32, kind),
            None => return,
        }
    };
    match res {
        Err(e) => say(ctx, &format!("删除失败: {}", e)),
        Ok(()) => {
            say(ctx, if kind == 0 { "已删除脚注" } else { "已删除尾注" });
            if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
            refresh_canvas(ctx);
            refresh_selection(ctx);
        }
    }
}

/// v298: 在光标处插入分栏符
fn column_break(ctx: &RibbonCtx) {
    let (p, res) = {
        let mut s = ctx.state.borrow_mut();
        let p = s.writer_cursor_para;
        let o = s.writer_cursor_offset;
        if p < 0 || o < 0 { return; }
        let r = s.session.as_mut().map(|x| x.writer_insert_column_break(p as u32, o as u32));
        (p, r)
    };
    match res {
        Some(Ok(())) => {
            if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
            refresh_canvas(ctx);
            place_cursor(ctx, p + 1, 0);
            say(ctx, "已插入分栏符");
        }
        Some(Err(e)) => say(ctx, &format!("插入分栏符失败: {}", e)),
        None => {}
    }
}

/// v298: 设置栏数
fn set_columns(n: u32, ctx: &RibbonCtx) {
    let res = {
        let s = ctx.state.borrow();
        match s.session.as_ref() {
            Some(sess) => sess.writer_set_columns(n, 28.35),
            None => return,
        }
    };
    match res {
        Err(e) => say(ctx, &format!("设置分栏失败: {}", e)),
        Ok(()) => {
            say(ctx, &format!("已设置为 {} 栏", n));
            if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
            refresh_canvas(ctx);
            refresh_selection(ctx);
        }
    }
}

/// v298: 插入图片（选文件，96dpi 换算 pt，限制在页宽内）
fn insert_image(ctx: &RibbonCtx) {
    let (p, o) = {
        let s = ctx.state.borrow();
        (s.writer_cursor_para, s.writer_cursor_offset)
    };
    if p < 0 || o < 0 { say(ctx, "请先在正文中点击要插入图片的位置"); return; }
    let file = rfd::FileDialog::new().add_filter("图片", &["png", "jpg", "jpeg"]).pick_file();
    let Some(path) = file else { return; };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => { say(ctx, &format!("读取图片失败: {}", e)); return; }
    };
    let (iw, ih) = match doc_core::TileCacheRust::impress_image_dimensions(&bytes) {
        Some(v) if v.0 > 0 && v.1 > 0 => v,
        _ => { say(ctx, "无法识别图片（仅支持 PNG/JPEG）"); return; }
    };
    let is_png = path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("png")).unwrap_or(false);
    let mime = if is_png { "image/png" } else { "image/jpeg" };
    let page_w: f32 = ctx.app_weak.upgrade().map(|a| a.get_writer_doc_width_pt() as f32).unwrap_or(595.0);
    let mut w = iw as f32 * 0.75;
    let mut h = ih as f32 * 0.75;
    let max_w = (page_w - 144.0).max(100.0);
    if w > max_w { let k = max_w / w; w *= k; h *= k; }
    if h > 600.0 { let k = 600.0 / h; w *= k; h *= k; }
    let res = {
        let s = ctx.state.borrow();
        s.session.as_ref().map(|x| x.writer_insert_image(p as u32, o as u32, &bytes, mime, w, h))
    };
    match res {
        Some(Ok(())) => {
            if let Some(app) = ctx.app_weak.upgrade() { app.set_writer_needs_render(true); }
            refresh_canvas(ctx);
            if o == 0 { place_cursor(ctx, p + 1, 0); } else { place_cursor(ctx, p, o); }
            say(ctx, "已插入图片");
        }
        Some(Err(e)) => say(ctx, &format!("插入图片失败: {}", e)),
        None => {}
    }
}
