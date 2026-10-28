//! Calc「获取数据(Polars)」:前台选择 parquet 文件夹 -> 加载为命名数据集 -> 只读分页预览。
//! 数据不写入工作表。分页状态(当前数据集名、offset)放在 thread_local,不进 AppState。

use std::cell::{Cell, RefCell};

use slint::{ModelRc, SharedString, VecModel};

use super::RibbonCtx;

const PAGE: u32 = 100;

thread_local! {
    static CUR_NAME: RefCell<String> = RefCell::new(String::new());
    static CUR_OFFSET: Cell<u64> = Cell::new(0);
}

fn status(app: &crate::AppWindow, msg: &str) {
    app.set_status_message(msg.into());
}

/// 文件夹名 -> 合法数据集名(字母/数字/下划线,其余换成下划线;空则 data)。
fn make_name(dir: &std::path::Path) -> String {
    let raw = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let n: String = raw.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if n.is_empty() { "data".to_string() } else { n }
}

/// 取一页并刷新对话框属性。
fn show(app: &crate::AppWindow, sess: &doc_core::TileCacheRust, name: &str, offset: u64) -> Result<(), String> {
    let p = sess.df_page(name, offset, PAGE)?;
    let cols: Vec<SharedString> = p.columns.iter().map(|s| SharedString::from(s.as_str())).collect();
    let rows: Vec<ModelRc<SharedString>> = p
        .rows
        .iter()
        .map(|r| {
            let cells: Vec<SharedString> = r.iter().map(|s| SharedString::from(s.as_str())).collect();
            ModelRc::new(VecModel::from(cells))
        })
        .collect();
    let shown = p.rows.len() as u64;
    let from = if shown == 0 { 0 } else { p.offset + 1 };
    let to = p.offset + shown;
    app.set_df_title(format!("数据集:{}", name).into());
    app.set_df_info(format!("第 {}–{} 行 / 共 {} 行,{} 列", from, to, p.total_rows, p.columns.len()).into());
    app.set_df_columns(ModelRc::new(VecModel::from(cols)));
    app.set_df_rows(ModelRc::new(VecModel::from(rows)));
    app.set_df_can_prev(p.offset > 0);
    app.set_df_can_next(to < p.total_rows);
    CUR_OFFSET.with(|o| o.set(p.offset));
    Ok(())
}

pub fn open(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    // 先短暂借用判断文档类型,随后放掉:文件对话框是阻塞调用,不能持有 borrow_mut
    {
        let s = ctx.state.borrow();
        match s.session.as_ref() {
            Some(sess) if sess.get_doc_type() == "calc" => {}
            _ => {
                status(&app, "请先打开或新建一个表格(Calc)文档");
                return;
            }
        }
    }
    let Some(dir) = rfd::FileDialog::new().set_title("选择 parquet 文件夹(如 Hive 表目录)").pick_folder() else {
        return;
    };
    let name = make_name(&dir);
    let path = dir.to_string_lossy().into_owned();

    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    match sess.df_open_parquet(&path, &name) {
        Ok((rows, cols)) => eprintln!("[ribbon] calc.df_open name={} rows={} cols={}", name, rows, cols.len()),
        Err(e) => {
            eprintln!("[ribbon] calc.df_open 失败: {}", e);
            status(&app, &format!("加载失败: {}", e));
            return;
        }
    }
    if let Err(e) = show(&app, sess, &name, 0) {
        status(&app, &format!("分页失败: {}", e));
        return;
    }
    CUR_NAME.with(|n| *n.borrow_mut() = name);
    app.set_df_visible(true);
}

fn page_to(ctx: &RibbonCtx, offset: u64) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let name = CUR_NAME.with(|n| n.borrow().clone());
    if name.is_empty() { return; }
    let s = ctx.state.borrow();
    let Some(sess) = s.session.as_ref() else { return };
    if let Err(e) = show(&app, sess, &name, offset) {
        status(&app, &format!("分页失败: {}", e));
    }
}

pub fn prev(ctx: &RibbonCtx) {
    let off = CUR_OFFSET.with(|o| o.get());
    page_to(ctx, off.saturating_sub(PAGE as u64));
}

pub fn next(ctx: &RibbonCtx) {
    let off = CUR_OFFSET.with(|o| o.get());
    page_to(ctx, off + PAGE as u64);
}

/// 关闭:清空当前页数据(释放 UI 侧内存)。数据集本身仍挂在文档句柄上,随文档关闭释放。
pub fn close(ctx: &RibbonCtx) {
    CUR_NAME.with(|n| n.borrow_mut().clear());
    CUR_OFFSET.with(|o| o.set(0));
    if let Some(app) = ctx.app_weak.upgrade() {
        app.set_df_rows(ModelRc::new(VecModel::from(Vec::<ModelRc<SharedString>>::new())));
        app.set_df_columns(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
    }
}


// ══════════ 通用文件导入:概览 -> 确认注册 -> 写预览 sheet ══════════

/// 预览 sheet 写入的行数。
const SHEET_HEAD_ROWS: u32 = 50;

thread_local! {
    /// 待确认的导入:(路径, 类型)。取消或确认后清空。
    static PENDING: RefCell<Option<(String, String)>> = RefCell::new(None);
}

fn jcell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) if n.is_f64() => {
            let f = n.as_f64().unwrap_or(0.0);
            let t = format!("{:.4}", f);
            t.trim_end_matches('0').trim_end_matches('.').to_string()
        }
        other => other.to_string(),
    }
}

fn to_model_rows(rows: Vec<Vec<String>>) -> ModelRc<ModelRc<SharedString>> {
    let v: Vec<ModelRc<SharedString>> = rows
        .into_iter()
        .map(|r| {
            let cells: Vec<SharedString> = r.into_iter().map(SharedString::from).collect();
            ModelRc::new(VecModel::from(cells))
        })
        .collect();
    ModelRc::new(VecModel::from(v))
}

/// kind: "csv" | "parquet" | "xlsx"。选文件 -> 预览概览 -> 弹对话框(此时不注册任何东西)。
pub fn open_file(ctx: &RibbonCtx, kind: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    {
        let s = ctx.state.borrow();
        match s.session.as_ref() {
            Some(sess) if sess.get_doc_type() == "calc" => {}
            _ => {
                status(&app, "请先打开或新建一个表格(Calc)文档");
                return;
            }
        }
    }
    let picked = match kind {
        "csv" => rfd::FileDialog::new()
            .set_title("选择 CSV 文件")
            .add_filter("CSV / 文本", &["csv", "tsv", "txt"])
            .add_filter("所有文件", &["*"])
            .pick_file(),
        "xlsx" => rfd::FileDialog::new()
            .set_title("选择 Excel 文件")
            .add_filter("Excel / ODS", &["xlsx", "xls", "ods"])
            .add_filter("所有文件", &["*"])
            .pick_file(),
        _ => rfd::FileDialog::new().set_title("选择 parquet 文件夹(如 Hive 表目录)").pick_folder(),
    };
    let Some(p) = picked else { return };
    let path = p.to_string_lossy().into_owned();

    let pv = {
        let s = ctx.state.borrow();
        let Some(sess) = s.session.as_ref() else { return };
        let r = sess.df_preview_file(&path, kind, r#"{"head":10}"#);
        r
    };
    let v = match pv {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[ribbon] calc.df_open_file 预览失败: {}", e);
            status(&app, &format!("读取失败: {}", e));
            return;
        }
    };
    let rows = v.get("rows").and_then(|x| x.as_u64()).unwrap_or(0);
    let ncols = v.get("cols").and_then(|x| x.as_u64()).unwrap_or(0);
    let col_info: Vec<Vec<String>> = v
        .get("columns")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .map(|c| {
                    ["name", "dtype", "nulls", "min", "max", "mean"]
                        .iter()
                        .map(|k| c.get(*k).map(jcell).unwrap_or_default())
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default();
    let head_cols: Vec<SharedString> = v["head"]["columns"]
        .as_array()
        .map(|a| a.iter().map(|c| SharedString::from(jcell(c))).collect())
        .unwrap_or_default();
    let head_rows: Vec<Vec<String>> = v["head"]["rows"]
        .as_array()
        .map(|rs| {
            rs.iter()
                .map(|r| r.as_array().map(|cs| cs.iter().map(jcell).collect()).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();

    let fname = std::path::Path::new(&path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let mut info = format!("共 {} 行,{} 列。确定后注册数据集并写入前 {} 行预览", rows, ncols, SHEET_HEAD_ROWS);
    if kind == "xlsx" {
        info.push_str("(Excel 仅读取第一个工作表)");
    }
    app.set_dfo_title(format!("导入概览:{}", fname).into());
    app.set_dfo_info(info.into());
    app.set_dfo_col_info(to_model_rows(col_info));
    app.set_dfo_head_cols(ModelRc::new(VecModel::from(head_cols)));
    app.set_dfo_head_rows(to_model_rows(head_rows));
    PENDING.with(|p| *p.borrow_mut() = Some((path, kind.to_string())));
    app.set_dfo_visible(true);
}

/// 确定:注册 df_N,写前 SHEET_HEAD_ROWS 行到新 sheet,切到新 sheet 并重绘。
pub fn confirm(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some((path, kind)) = PENDING.with(|p| p.borrow_mut().take()) else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    let (name, rows, cols) = match sess.df_register_file(&path, &kind, "") {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[ribbon] calc.df_confirm 注册失败: {}", e);
            status(&app, &format!("导入失败: {}", e));
            return;
        }
    };
    let idx = match sess.df_sheet_head(&name, SHEET_HEAD_ROWS) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[ribbon] calc.df_confirm 写预览失败: {}", e);
            status(&app, &format!("已注册 {},但写入预览失败: {}", name, e));
            return;
        }
    };
    let names = sess.get_all_sheet_names();
    let model: Vec<SharedString> = names.iter().map(|n| SharedString::from(n.as_str())).collect();
    app.set_calc_sheet_names(ModelRc::new(VecModel::from(model)));
    ctx.current_sheet_idx.set(idx as u32);
    sess.set_current_sheet(idx as i32);
    if let Some(n) = names.get(idx as usize) {
        app.set_calc_active_sheet(n.as_str().into());
    }
    super::calc_home::refresh_view(&app, sess);
    eprintln!("[ribbon] calc.df_confirm name={} rows={} cols={} sheet_idx={}", name, rows, cols.len(), idx);
    status(
        &app,
        &format!("已导入 {}({} 行,{} 列),前 {} 行预览写入新 sheet", name, rows, cols.len(), SHEET_HEAD_ROWS),
    );
}

/// 取消:丢弃待确认状态,不注册任何东西。
pub fn cancel(_ctx: &RibbonCtx) {
    PENDING.with(|p| *p.borrow_mut() = None);
}


// ══════════ SQL 查询:运行预览 -> 保存为新数据集 ══════════

/// 打开 SQL 对话框(此时不运行、不注册任何东西)。
pub fn open_sql(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let names = {
        let s = ctx.state.borrow();
        let Some(sess) = s.session.as_ref() else { return };
        if sess.get_doc_type() != "calc" {
            status(&app, "请先打开或新建一个表格(Calc)文档");
            return;
        }
        let r = sess.df_list_datasets();
        r
    };
    let names = match names {
        Ok(v) => v,
        Err(e) => {
            status(&app, &format!("读取数据集失败: {}", e));
            return;
        }
    };
    if names.is_empty() {
        status(&app, "还没有数据集,请先用「获取数据(Polars)」导入");
        return;
    }
    let chips: Vec<SharedString> = names.iter().map(|(n, _)| SharedString::from(n.as_str())).collect();
    if app.get_sql_text().trim().is_empty() {
        app.set_sql_text(format!("SELECT * FROM {} LIMIT 100", names[0].0).into());
    }
    app.set_sql_ds_names(ModelRc::new(VecModel::from(chips)));
    app.set_sql_cols(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
    app.set_sql_rows(to_model_rows(Vec::new()));
    app.set_sql_status("".into());
    app.set_sql_visible(true);
}

/// 运行:只预览(前 50 行),不注册、不改文档。
pub fn run_sql(ctx: &RibbonCtx, sql: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let pv = {
        let s = ctx.state.borrow();
        let Some(sess) = s.session.as_ref() else { return };
        let r = sess.df_sql_preview(sql);
        r
    };
    match pv {
        Err(e) => app.set_sql_status(format!("错误: {}", e).into()),
        Ok(v) => {
            let cols: Vec<SharedString> = v["columns"]
                .as_array()
                .map(|a| a.iter().map(|c| SharedString::from(jcell(c))).collect())
                .unwrap_or_default();
            let rows: Vec<Vec<String>> = v["rows"]
                .as_array()
                .map(|rs| {
                    rs.iter()
                        .map(|r| r.as_array().map(|cs| cs.iter().map(jcell).collect()).unwrap_or_default())
                        .collect()
                })
                .unwrap_or_default();
            let total = v["total_rows"].as_u64().unwrap_or(0);
            let msg = format!("共 {} 行,{} 列;下表显示前 {} 行", total, cols.len(), rows.len());
            app.set_sql_cols(ModelRc::new(VecModel::from(cols)));
            app.set_sql_rows(to_model_rows(rows));
            app.set_sql_status(msg.into());
        }
    }
}

/// 保存:结果注册为 df_N+1,写前 SHEET_HEAD_ROWS 行到新 sheet,切过去并关闭对话框。失败则对话框保持打开。
pub fn save_sql(ctx: &RibbonCtx, sql: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    let (name, rows, cols) = match sess.df_sql_save(sql) {
        Ok(t) => t,
        Err(e) => {
            app.set_sql_status(format!("保存失败: {}", e).into());
            return;
        }
    };
    let idx = match sess.df_sheet_head(&name, SHEET_HEAD_ROWS) {
        Ok(i) => i,
        Err(e) => {
            status(&app, &format!("已保存为 {},但写入预览失败: {}", name, e));
            app.set_sql_visible(false);
            return;
        }
    };
    let names = sess.get_all_sheet_names();
    let model: Vec<SharedString> = names.iter().map(|n| SharedString::from(n.as_str())).collect();
    app.set_calc_sheet_names(ModelRc::new(VecModel::from(model)));
    ctx.current_sheet_idx.set(idx as u32);
    sess.set_current_sheet(idx as i32);
    if let Some(n) = names.get(idx as usize) {
        app.set_calc_active_sheet(n.as_str().into());
    }
    super::calc_home::refresh_view(&app, sess);
    eprintln!("[ribbon] calc.sql_save name={} rows={} cols={} sheet_idx={}", name, rows, cols.len(), idx);
    status(
        &app,
        &format!("SQL 结果已存为 {}({} 行,{} 列),前 {} 行预览写入新 sheet", name, rows, cols.len(), SHEET_HEAD_ROWS),
    );
    app.set_sql_visible(false);
}


// ══════════ DF 链式查询:运行预览 -> 保存为 chain_N ══════════

fn strs(v: &[String]) -> ModelRc<SharedString> {
    let m: Vec<SharedString> = v.iter().map(|s| SharedString::from(s.as_str())).collect();
    ModelRc::new(VecModel::from(m))
}

thread_local! {
    /// DF 链对话框当前列名 / 已选列(按点击顺序)。状态在 Rust,Slint 只显示。
    static CHAIN_COLS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static CHAIN_SEL: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn chain_push_on(app: &crate::AppWindow) {
    let on: Vec<bool> = CHAIN_COLS.with(|c| {
        CHAIN_SEL.with(|s| {
            let s = s.borrow();
            c.borrow().iter().map(|n| s.contains(n)).collect()
        })
    });
    app.set_chain_col_on(ModelRc::new(VecModel::from(on)));
}

/// 换一组列名:清空选择,刷新列 chip。
fn chain_set_cols(app: &crate::AppWindow, cols: &[String]) {
    CHAIN_COLS.with(|c| *c.borrow_mut() = cols.to_vec());
    CHAIN_SEL.with(|s| s.borrow_mut().clear());
    app.set_chain_col_names(strs(cols));
    chain_push_on(app);
}

/// 列 chip 点选/取消。
pub fn toggle_chain_col(ctx: &RibbonCtx, i: i32) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let Some(name) = CHAIN_COLS.with(|c| c.borrow().get(i as usize).cloned()) else { return };
    CHAIN_SEL.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(p) = s.iter().position(|x| *x == name) {
            s.remove(p);
        } else {
            s.push(name);
        }
    });
    chain_push_on(&app);
}

/// 模板按钮:用已选列 + 参数生成一行;失败写状态栏并返回空串。
pub fn build_line(ctx: &RibbonCtx, kind: &str, n: &str, desc: bool, agg: &str) -> String {
    let Some(app) = ctx.app_weak.upgrade() else { return String::new() };
    let sel = CHAIN_SEL.with(|s| s.borrow().clone());
    match super::chain_line::build_chain_line(kind, &sel, n, desc, agg) {
        Ok(l) => {
            app.set_chain_status("".into());
            l
        }
        Err(e) => {
            app.set_chain_status(e.into());
            String::new()
        }
    }
}

/// 打开 DF 链对话框(不运行、不注册)。默认列出第一个数据集的列名。
pub fn open_chain(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let (names, first_cols) = {
        let s = ctx.state.borrow();
        let Some(sess) = s.session.as_ref() else { return };
        if sess.get_doc_type() != "calc" {
            status(&app, "请先打开或新建一个表格(Calc)文档");
            return;
        }
        let names = match sess.df_list_datasets() {
            Ok(v) => v,
            Err(e) => {
                status(&app, &format!("读取数据集失败: {}", e));
                return;
            }
        };
        let cols = names.first().and_then(|(n, _)| sess.df_columns(n).ok()).unwrap_or_default();
        (names, cols)
    };
    if names.is_empty() {
        status(&app, "还没有数据集,请先用「获取数据(Polars)」导入");
        return;
    }
    let ds: Vec<String> = names.iter().map(|(n, _)| n.clone()).collect();
    app.set_chain_ds_names(strs(&ds));
    chain_set_cols(&app, &first_cols);
    app.set_chain_cols(strs(&[]));
    app.set_chain_rows(to_model_rows(Vec::new()));
    app.set_chain_status("".into());
    app.set_chain_visible(true);
}

/// 点数据集 chip:刷新列名 chip。
pub fn pick_chain_ds(ctx: &RibbonCtx, name: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let cols = {
        let s = ctx.state.borrow();
        let Some(sess) = s.session.as_ref() else { return };
        sess.df_columns(name)
    };
    match cols {
        Ok(c) => chain_set_cols(&app, &c),
        Err(e) => app.set_chain_status(format!("读取列名失败: {}", e).into()),
    }
}

/// 运行:只预览(前 50 行),不注册、不改文档。
pub fn run_chain(ctx: &RibbonCtx, code: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let pv = {
        let s = ctx.state.borrow();
        let Some(sess) = s.session.as_ref() else { return };
        sess.df_chain_preview(code)
    };
    match pv {
        Err(e) => app.set_chain_status(format!("错误: {}", e).into()),
        Ok(v) => {
            let cols: Vec<String> = v["columns"]
                .as_array()
                .map(|a| a.iter().map(jcell).collect())
                .unwrap_or_default();
            let rows: Vec<Vec<String>> = v["rows"]
                .as_array()
                .map(|rs| {
                    rs.iter()
                        .map(|r| r.as_array().map(|cs| cs.iter().map(jcell).collect()).unwrap_or_default())
                        .collect()
                })
                .unwrap_or_default();
            let total = v["total_rows"].as_u64().unwrap_or(0);
            let msg = format!("共 {} 行,{} 列;下表显示前 {} 行", total, cols.len(), rows.len());
            app.set_chain_cols(strs(&cols));
            app.set_chain_rows(to_model_rows(rows));
            app.set_chain_status(msg.into());
        }
    }
}

/// 保存:注册 chain_N,写前 SHEET_HEAD_ROWS 行到新 sheet,切过去并关闭对话框。失败则保持打开。
pub fn save_chain(ctx: &RibbonCtx, code: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(sess) = s.session.as_mut() else { return };
    let (name, rows, cols) = match sess.df_chain_save(code) {
        Ok(t) => t,
        Err(e) => {
            app.set_chain_status(format!("保存失败: {}", e).into());
            return;
        }
    };
    let idx = match sess.df_sheet_head(&name, SHEET_HEAD_ROWS) {
        Ok(i) => i,
        Err(e) => {
            status(&app, &format!("已保存为 {},但写入预览失败: {}", name, e));
            app.set_chain_visible(false);
            return;
        }
    };
    let names = sess.get_all_sheet_names();
    let model: Vec<SharedString> = names.iter().map(|n| SharedString::from(n.as_str())).collect();
    app.set_calc_sheet_names(ModelRc::new(VecModel::from(model)));
    ctx.current_sheet_idx.set(idx as u32);
    sess.set_current_sheet(idx as i32);
    if let Some(n) = names.get(idx as usize) {
        app.set_calc_active_sheet(n.as_str().into());
    }
    super::calc_home::refresh_view(&app, sess);
    eprintln!("[ribbon] calc.chain_save name={} rows={} cols={} sheet_idx={}", name, rows, cols.len(), idx);
    status(
        &app,
        &format!("DF 链结果已存为 {}({} 行,{} 列),前 {} 行预览写入新 sheet", name, rows, cols.len(), SHEET_HEAD_ROWS),
    );
    app.set_chain_visible(false);
}
