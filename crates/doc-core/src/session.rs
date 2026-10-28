#![allow(unused_variables)]
#![allow(dead_code)]

//! doc-core / DocSession：office-rs2 的文档会话核心逻辑。
//!
//! 这份代码的前身是 app-bridge（cxx桥接给QML用的旧实现，已废弃），现剥离为不依赖cxx/Qt的纯Rust crate。
//!
//! 关键特性：
//! - 无 actor 线程，所有调用同步直达 roe-api（C-ABI / thread_local 句柄表）
//! - Writer/Impress 不再做 tile 裁剪缓存，render_page 整页渲染一次性返回
//! - 文档打开/保存改为 bytes in/out，调用方负责读写文件，这里不碰文件系统
//! - save_document_to_bytes 目前仅支持 KAI(1/2/3)，OOXML/ODF 写出待 roe-api 后续支持

use std::ffi::{c_void, CStr};
use std::os::raw::c_char;

use roe_api::*;

fn stub_json(msg: &str) -> String {
    format!(r#"{{"ok":false,"error":"roe-backend stub: {msg}"}}"#)
}

fn err_json(msg: &str) -> String {
    format!(r#"{{"ok":false,"error":"{msg}"}}"#)
}

/// 把 roe-api 返回的 *mut c_char 转成 String 并释放原内存。
unsafe fn take_roe_string(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
    roe_free_string(ptr);
    Some(s)
}

/// roe_save_document_to_bytes 的回调：把数据追加到 userdata 指向的 Vec<u8>。
extern "C" fn collect_bytes_callback(data: *const u8, len: usize, userdata: *mut c_void) {
    if data.is_null() || userdata.is_null() {
        return;
    }
    unsafe {
        let out = &mut *(userdata as *mut Vec<u8>);
        let slice = std::slice::from_raw_parts(data, len);
        out.extend_from_slice(slice);
    }
}

/// df_page 返回的一页数据(单元格已转成显示用字符串)。
#[derive(Debug, Clone, Default)]
pub struct DfPage {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub total_rows: u64,
    pub offset: u64,
}

/// 数据集名:非空,仅字母/数字/下划线(含中文)。名称会拼进 Rhai 脚本,必须杜绝引号等字符。
pub fn df_valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// 把任意文本转成 Rhai 普通双引号字符串字面量(转义 \ " 换行 制表;其余控制字符换成空格)。
/// 不用反引号:Rhai 反引号字符串支持 ${...} 插值,SQL 里出现会被当表达式。
fn rhai_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c.is_control() => o.push(' '),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// 剥 roe_* / query_script 的 JSON 外壳,返回内层值。
/// 兼容 {"ok":true,"value":X} 与 return_value;X 为字符串化 JSON 时再解析一层;"err:" 前缀视为错误。
fn df_unwrap(raw: &str) -> Result<serde_json::Value, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("返回值解析失败: {e}"))?;
    if v.get("ok").and_then(|x| x.as_bool()) == Some(false) {
        return Err(v.get("error").and_then(|e| e.as_str()).unwrap_or("未知错误").to_string());
    }
    let inner = v
        .get("value")
        .or_else(|| v.get("return_value"))
        .cloned()
        .ok_or_else(|| "返回值缺少 value".to_string())?;
    match inner {
        serde_json::Value::String(t) => {
            if let Some(e) = t.strip_prefix("err:") {
                return Err(e.to_string());
            }
            serde_json::from_str(&t).map_err(|e| format!("内层 JSON 解析失败: {e}"))
        }
        other => Ok(other),
    }
}

/// 单元格 JSON 值 -> 显示字符串(null 显示为空)。
fn df_cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(t) => t.clone(),
        other => other.to_string(),
    }
}

/// TileCacheRust 持有 ROE 文档句柄（同步调用，无 actor 线程）
pub struct TileCacheRust {
    handle: u64,
    /// "writer" / "calc" / "impress" / ""（未打开）
    doc_type: String,
    /// Writer/Impress: >=1；Calc: -1（概念不适用，原样传递自 roe_get_page_count）
    page_count: i32,
    page_w_pt: f32,
    page_h_pt: f32,
    /// Calc 当前 sheet 索引；Impress 复用此字段表示当前 slide 索引
    current_sheet: i32,
    /// UI 层维护的"当前文件路径"概念，与 ROE 内核无关
    original_doc_url: String,
}

pub fn new_tile_cache() -> Box<TileCacheRust> {
    Box::new(TileCacheRust {
        handle: 0,
        doc_type: String::new(),
        page_count: -1,
        page_w_pt: 0.0,
        page_h_pt: 0.0,
        current_sheet: 0,
        original_doc_url: String::new(),
    })
}

impl Drop for TileCacheRust {
    fn drop(&mut self) {
        if self.handle != 0 {
            roe_close_document(self.handle);
        }
    }
}

impl TileCacheRust {
    // ================= 生命周期 =================

    /// 打开文档。返回非0句柄表示成功（与 roe_open_document_from_bytes 一致）。
    /// 若之前已打开过文档，先关闭旧的。
    pub fn open_document_from_bytes(&mut self, data: &[u8], format_hint: i32) -> u64 {
        if self.handle != 0 {
            roe_close_document(self.handle);
            self.handle = 0;
        }
        self.doc_type.clear();
        self.page_count = -1;
        self.page_w_pt = 0.0;
        self.page_h_pt = 0.0;
        self.current_sheet = 0;

        let handle = roe_open_document_from_bytes(data.as_ptr(), data.len(), format_hint);
        if handle == 0 {
            return 0;
        }
        self.handle = handle;
        self.refresh_doc_meta();
        handle
    }

    /// 新建空白文档。doc_type: 1=Writer 2=Calc 3=Impress。
    /// 返回非0句柄表示成功；0 表示 doc_type 非法。
    /// 与 open_document_from_bytes 平行：若之前已打开过文档，先关闭旧的。
    pub fn new_document(&mut self, doc_type: i32) -> u64 {
        if self.handle != 0 {
            roe_close_document(self.handle);
            self.handle = 0;
        }
        self.doc_type.clear();
        self.page_count = -1;
        self.page_w_pt = 0.0;
        self.page_h_pt = 0.0;
        self.current_sheet = 0;

        let handle = roe_api::document::roe_new_document(doc_type);
        if handle == 0 {
            return 0;
        }
        self.handle = handle;

        // Writer 新建后 body 为空，没有任何段落，光标/输入/命中测试都无处落脚
        // （roe_query_layout_rect 对 para_index=0 会返回 Err）。与 Word 的行为
        // 对齐：新建 Writer 文档自带一个空段落。走 Rhai append_paragraph，
        // 然后清空 undo/redo 栈——否则用户新建后第一次 Ctrl+Z 会把这个
        // 初始段落撤销掉，回到无段落的坏状态。
        if doc_type == 1 {
            let r = self.execute_script(r#"append_paragraph("");"#);
            if !r.contains("\"ok\":true") && !r.contains("\"ok\": true") {
                eprintln!("[new_document] 初始化空段落返回: {}", r);
            }
            roe_api::handle::with_entry(handle, |entry| {
                entry.undo_stack.clear();
                entry.redo_stack.clear();
            });
        }

        self.refresh_doc_meta();
        handle
    }

    pub fn close_document(&mut self) {
        if self.handle != 0 {
            roe_close_document(self.handle);
            self.handle = 0;
        }
        self.doc_type.clear();
        self.page_count = -1;
        self.page_w_pt = 0.0;
        self.page_h_pt = 0.0;
        self.current_sheet = 0;
    }

    /// 把当前内存模型序列化为字节。
    /// 注意：roe_save_document_to_bytes 目前仅支持 KAI（format_hint 1/2/3），
    /// 传入其他值会按 roe-api 的实际行为失败并返回空 Vec（错误信息见 roe_last_error_message）。
    pub fn save_document_to_bytes(&mut self, format_hint: i32) -> Vec<u8> {
        if self.handle == 0 {
            return Vec::new();
        }
        let mut out: Vec<u8> = Vec::new();
        let userdata = &mut out as *mut Vec<u8> as *mut c_void;
        let code = roe_save_document_to_bytes(self.handle, format_hint, collect_bytes_callback, userdata);
        if code == RoeErrorCode::Ok as i32 {
            out
        } else {
            Vec::new()
        }
    }

    /// 把当前内存模型序列化为 OOXML 字节(format_hint: 4=docx 5=xlsx 6=pptx,
    /// 必须与文档类型匹配)。失败返回空 Vec,错误信息见 roe_last_error_message。
    pub fn save_document_to_ooxml_bytes(&mut self, format_hint: i32) -> Vec<u8> {
        if self.handle == 0 {
            return Vec::new();
        }
        let mut out: Vec<u8> = Vec::new();
        let userdata = &mut out as *mut Vec<u8> as *mut c_void;
        let code = roe_api::document::roe_save_document_to_ooxml_bytes(
            self.handle, format_hint, collect_bytes_callback, userdata,
        );
        if code == RoeErrorCode::Ok as i32 { out } else { Vec::new() }
    }

    fn refresh_doc_meta(&mut self) {
        if self.handle == 0 {
            return;
        }
        if let Some(idx_json) = unsafe { take_roe_string(roe_get_index(self.handle)) } {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&idx_json) {
                if let Some(t) = v.get("type").and_then(|x| x.as_str()) {
                    self.doc_type = t.to_string();
                }
            }
        }
        self.page_count = roe_get_page_count(self.handle);
        if let Some(size_json) = unsafe { take_roe_string(roe_get_document_size(self.handle)) } {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&size_json) {
                self.page_w_pt = v.get("width_pt").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                self.page_h_pt = v.get("height_pt").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            }
        }
    }

    pub fn get_doc_type(&self) -> String {
        self.doc_type.clone()
    }

    // ================= 页面/尺寸查询 =================

    /// JSON {"width_pt":.., "height_pt":..}；句柄无效时返回 roe-api 的错误占位 JSON。
    pub fn get_document_size(&mut self) -> String {
        if self.handle == 0 {
            return err_json("no document open");
        }
        match unsafe { take_roe_string(roe_get_document_size(self.handle)) } {
            Some(s) => s,
            None => err_json("roe_get_document_size failed"),
        }
    }

    /// Writer/Impress: 页数/slide数；Calc: -1（概念不适用，原样来自 roe_get_page_count）。
    pub fn get_page_count(&mut self) -> i32 {
        if self.handle == 0 {
            return -1;
        }
        roe_get_page_count(self.handle)
    }

    /// Impress 动画：推进 delta_ms 毫秒，返回 roe_tick 的原始 JSON 字符串。
    pub fn tick(&mut self, slide_idx: u64, delta_ms: f64) -> String {
        if self.handle == 0 {
            return err_json("tick: invalid handle");
        }
        match unsafe { take_roe_string(roe_tick(self.handle, slide_idx, delta_ms)) } {
            Some(s) => s,
            None => err_json("roe_tick failed"),
        }
    }

    /// Impress 动画：触发一次点击（shape_id 为 -1 表示空白点击/推进序列）。
    pub fn on_click(&mut self, slide_idx: u64, shape_id: i64) -> String {
        if self.handle == 0 {
            return err_json("on_click: invalid handle");
        }
        match unsafe { take_roe_string(roe_on_click(self.handle, slide_idx, shape_id)) } {
            Some(s) => s,
            None => err_json("roe_on_click failed"),
        }
    }

    // ================= 渲染 =================

    /// 2026-06-19: 改用 roe_get_render_page_size 先查精确尺寸再渲染，一次成功，
    /// 不再需要"渲染失败->解析报错信息->重试"的绕过方案（ROE接口变更说明_2026-06-18.md 方案A）。
    /// 注意：传入的 height_px 不再使用（ROE按width反推），仅保留参数兼容cxx::bridge签名。
    pub fn render_page(&mut self, page_idx: i32, width_px: i32, height_px: i32) -> Vec<u8> {
        if self.handle == 0 || page_idx < 0 || width_px <= 0 || height_px <= 0 {
            return Vec::new();
        }
        let (pw, ph) = match unsafe {
            take_roe_string(roe_get_render_page_size(self.handle, page_idx as u32, width_px as u32))
        } {
            Some(size_json) => {
                match serde_json::from_str::<serde_json::Value>(&size_json) {
                    Ok(v) => {
                        let w = v.get("width").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                        let h = v.get("height").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                        if w == 0 || h == 0 {
                            eprintln!("[RUST-DEBUG] render_page: roe_get_render_page_size returned zero size, json={}", size_json);
                            return Vec::new();
                        }
                        (w, h)
                    }
                    Err(e) => {
                        eprintln!("[RUST-DEBUG] render_page: failed to parse size json '{}': {}", size_json, e);
                        return Vec::new();
                    }
                }
            }
            None => {
                let last_err = unsafe {
                    let ptr = roe_api::roe_last_error_message();
                    if ptr.is_null() { "(no error message)".to_string() }
                    else {
                        let s = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
                        roe_api::roe_free_string(ptr);
                        s
                    }
                };
                eprintln!("[RUST-DEBUG] render_page: roe_get_render_page_size failed for page_idx={} width={}, last_error={}", page_idx, width_px, last_err);
                return Vec::new();
            }
        };
        match self.render_page_once(page_idx, pw as i32, ph as i32) {
            Ok(buf) => buf,
            Err(err_msg) => {
                eprintln!("[RUST-DEBUG] render_page failed even with queried size {}x{}: {}", pw, ph, err_msg);
                Vec::new()
            }
        }
    }

    /// 单次调用 roe_render_page，成功返回Ok(buf)，失败返回Err(错误信息字符串)。
    fn render_page_once(&mut self, page_idx: i32, width_px: i32, height_px: i32) -> Result<Vec<u8>, String> {
        let mut buf = vec![0u8; (width_px as usize) * (height_px as usize) * 4];
        let code = roe_render_page(
            self.handle,
            page_idx as u32,
            width_px as u32,
            height_px as u32,
            buf.as_mut_ptr(),
            buf.len(),
        );
        if code == RoeErrorCode::Ok as i32 {
            Ok(buf)
        } else {
            let msg = unsafe {
                let ptr = roe_api::roe_last_error_message();
                if ptr.is_null() {
                    "(no error message)".to_string()
                } else {
                    let s = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
                    roe_api::roe_free_string(ptr);
                    s
                }
            };
            Err(msg)
        }
    }


    pub fn render_sheet_viewport(&mut self, scroll_x_pt: f32, scroll_y_pt: f32, vw: u32, vh: u32) -> Vec<u8> {
        if self.handle == 0 || vw == 0 || vh == 0 {
            return Vec::new();
        }
        // 位图倍数与 main.rs 共用 calc_zoom_pct()/calc_phys_size(),保证两边尺寸同源
        let zoom_pct: u32 = calc_zoom_pct();
        let (phys_w, phys_h) = calc_phys_size(vw, vh);
        // 标题固定 30x18 逻辑px(内核按 zoom 放大成物理px)
        let header_col_w_pt: f32 = 30.0;
        let header_row_h_pt: f32 = 18.0;
        let buf_len = (phys_w * phys_h * 4) as usize;
        let mut buf = vec![0u8; buf_len];
        let code = roe_api::roe_render_sheet_viewport(
            self.handle,
            self.current_sheet as u32,
            scroll_x_pt,
            scroll_y_pt,
            phys_w,
            phys_h,
            zoom_pct,
            header_col_w_pt,
            header_row_h_pt,
            buf.as_mut_ptr(),
            buf.len(),
        );
        if code == RoeErrorCode::Ok as i32 {
            buf
        } else {
            eprintln!("[CALC-VIEWPORT] render_sheet_viewport failed code={}", code);
            Vec::new()
        }
    }

    /// v51更新：ROE接口变更（2026-06-29回复），height字段已移除，改为canvas_height
    /// （含顶部留白24pt+页间距16pt+底部留白48pt+所有页面高度）。
    /// 返回 (doc_width_pt, canvas_height_pt, page_count)。
    pub fn get_writer_document_size(&mut self) -> Option<(f32, f32, i32)> {
        if self.handle == 0 { return None; }
        match unsafe { take_roe_string(roe_api::roe_get_writer_document_size(self.handle)) } {
            Some(json) => {
                let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                let w = v.get("width").and_then(|x| x.as_f64()).unwrap_or(595.28) as f32;
                let canvas_h = v.get("canvas_height").and_then(|x| x.as_f64()).unwrap_or(841.89 + 24.0 + 48.0) as f32;
                let page_count = v.get("page_count").and_then(|x| x.as_i64()).unwrap_or(1) as i32;
                Some((w, canvas_h, page_count))
            }
            None => None,
        }
    }

    /// v264: Writer 每页顶部 y（画布 pt，与滚动坐标同一坐标系）
    pub fn get_writer_page_tops(&mut self) -> Vec<f32> {
        if self.handle == 0 { return Vec::new(); }
        match unsafe { take_roe_string(roe_api::roe_get_writer_document_size(self.handle)) } {
            Some(json) => {
                let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                v.get("pages").and_then(|p| p.as_array())
                    .map(|a| a.iter().map(|x| x.get("y").and_then(|y| y.as_f64()).unwrap_or(0.0) as f32).collect())
                    .unwrap_or_default()
            }
            None => Vec::new(),
        }
    }

    pub fn render_writer_viewport(&mut self, scroll_y_pt: f32, vw_pt: f32, vh_pt: f32) -> Vec<u8> {
        if self.handle == 0 || vw_pt <= 0.0 || vh_pt <= 0.0 { return Vec::new(); }
        let scale: f32 = doc_bitmap_scale(); // HiDPI
        let pixel_w = (vw_pt * scale).round() as u32;
        let pixel_h = (vh_pt * scale).round() as u32;
        let buf_len = (pixel_w * pixel_h * 4) as usize;
        let mut buf = vec![0u8; buf_len];
        let code = roe_api::roe_render_writer_viewport(
            self.handle,
            0.0, scroll_y_pt, vw_pt, vh_pt,
            pixel_w, pixel_h,
            buf.as_mut_ptr(), buf.len(),
        );
        if code == 0 {
            buf
        } else {
            eprintln!("[WRITER-VIEWPORT] render failed code={}", code);
            Vec::new()
        }
    }

    /// v51新增：Impress虚拟视口，用法与Writer视口一致。
    /// 返回 (canvas_width_pt, canvas_height_pt, slide_count)。
    pub fn get_impress_document_size(&mut self) -> Option<(f32, f32, i32, Vec<(f32, f32, f32)>)> {
        if self.handle == 0 { return None; }
        match unsafe { take_roe_string(roe_api::roe_get_impress_document_size(self.handle)) } {
            Some(json) => {
                let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                let w = v.get("width").and_then(|x| x.as_f64()).unwrap_or(960.0) as f32;
                let canvas_h = v.get("canvas_height").and_then(|x| x.as_f64()).unwrap_or(540.0 + 48.0) as f32;
                let slide_count = v.get("slide_count").and_then(|x| x.as_i64()).unwrap_or(1) as i32;
                let slides: Vec<(f32, f32, f32)> = v.get("slides")
                    .and_then(|s| s.as_array())
                    .map(|arr| arr.iter().map(|s| {
                        let y = s.get("y").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                        let sw = s.get("width").and_then(|x| x.as_f64()).unwrap_or(w as f64) as f32;
                        let sh = s.get("height").and_then(|x| x.as_f64()).unwrap_or(540.0) as f32;
                        (y, sw, sh)
                    }).collect())
                    .unwrap_or_default();
                Some((w, canvas_h, slide_count, slides))
            }
            None => None,
        }
    }

    /// Impress：命中测试，坐标为幻灯片内 pt。返回最顶层形状 id，未命中或失败返回 None。
    pub fn impress_hit_test(&mut self, slide_idx: u32, x_pt: f32, y_pt: f32) -> Option<u32> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_impress_hit_test(self.handle, slide_idx, x_pt, y_pt)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        v.get("shape_id").and_then(|x| x.as_u64()).map(|x| x as u32)
    }

    /// Impress：查询形状矩形，返回 (x, y, w, h, rotation)，rotation 为 null 时按 0 处理。
    pub fn impress_query_shape_rect(&mut self, slide_idx: u32, shape_id: u32) -> Option<(f32, f32, f32, f32, f32)> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_impress_query_shape_rect(self.handle, slide_idx, shape_id)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        let g = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
        Some((g("x_pt"), g("y_pt"), g("width_pt"), g("height_pt"), g("rotation")))
    }

    /// Impress：查询形状是否为两点直线及其翻转，返回 (is_line, flip_h, flip_v)。
    pub fn impress_query_line_info(&mut self, slide_idx: u32, shape_id: u32) -> Option<(bool, bool, bool)> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_impress_query_shape_rect(self.handle, slide_idx, shape_id)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        Some((b("is_line"), b("flip_h"), b("flip_v")))
    }

    /// Impress：按两个端点设置直线（起点 x1,y1，终点 x2,y2，pt）。
    pub fn impress_set_line_endpoints(&mut self, slide_idx: u32, shape_id: u32, x1: f32, y1: f32, x2: f32, y2: f32) -> bool {
        self.handle != 0 && roe_api::roe_impress_set_line_endpoints(self.handle, slide_idx, shape_id, x1, y1, x2, y2)
    }

    /// Impress：一次性设置形状矩形（拖动结束后的最终矩形）。
    pub fn impress_set_shape_rect(&mut self, slide_idx: u32, shape_id: u32, x_pt: f32, y_pt: f32, w_pt: f32, h_pt: f32) -> bool {
        self.handle != 0 && roe_api::roe_impress_set_shape_rect(self.handle, slide_idx, shape_id, x_pt, y_pt, w_pt, h_pt)
    }

    /// Impress：调整叠放次序。op: "front"=置顶 "forward"=上移一层 "backward"=下移一层 "back"=置底。
    pub fn impress_shape_order(&mut self, slide_idx: u32, shape_id: u32, op: &str) -> bool {
        if self.handle == 0 { return false; }
        match op {
            "front" => roe_api::roe_impress_shape_bring_to_front(self.handle, slide_idx, shape_id),
            "forward" => roe_api::roe_impress_shape_move_forward(self.handle, slide_idx, shape_id),
            "backward" => roe_api::roe_impress_shape_move_backward(self.handle, slide_idx, shape_id),
            "back" => roe_api::roe_impress_shape_send_to_back(self.handle, slide_idx, shape_id),
            _ => false,
        }
    }

    /// Impress：在指定幻灯片插入形状（pt 坐标）。geometry 为 "rect"/"ellipse"/"line" 或 OOXML 预设名。
    /// 成功返回新形状 id，失败返回 -1。
    pub fn impress_insert_shape(&mut self, slide_idx: u32, geometry: &str, x: f32, y: f32, w: f32, h: f32) -> i32 {
        if self.handle == 0 { return -1; }
        let c = match std::ffi::CString::new(geometry) { Ok(c) => c, Err(_) => return -1 };
        roe_api::roe_impress_insert_shape(self.handle, slide_idx, c.as_ptr(), x, y, w, h)
    }

    /// Impress：在指定幻灯片插入文本框（pt 坐标，无填充无边框，内容为一个空段落）。
    /// vertical=true 为竖排。成功返回新形状 id，失败返回 -1。
    pub fn impress_insert_textbox(&mut self, slide_idx: u32, x: f32, y: f32, w: f32, h: f32, vertical: bool) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_impress_insert_textbox(self.handle, slide_idx, x, y, w, h, vertical)
    }

    /// Impress 文本编辑（2b）：插入文字，返回 {"para","offset"}（插入后光标）。
    pub fn impress_text_insert(&mut self, slide: u32, shape: u32, para: u32, offset: u32, text: &str) -> Option<String> {
        if self.handle == 0 || text.is_empty() { return None; }
        let b = text.as_bytes();
        let ptr = roe_api::roe_impress_text_insert(self.handle, slide, shape, para, offset, b.as_ptr(), b.len() as u32);
        unsafe { take_roe_string(ptr) }
    }

    /// Impress：退格（backward=true）/ Delete（false）。已在边界返回 None。
    pub fn impress_text_delete(&mut self, slide: u32, shape: u32, para: u32, offset: u32, backward: bool) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_text_delete(self.handle, slide, shape, para, offset, if backward { 0 } else { 1 });
        unsafe { take_roe_string(ptr) }
    }

    // v319t: Impress table cell editing
    pub fn impress_cell_at(&self, slide: u32, shape: u32, x: f32, y: f32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_cell_at(self.handle, slide, shape, x, y);
        unsafe { take_roe_string(ptr) }
    }
    pub fn impress_cell_hit(&self, slide: u32, shape: u32, row: u32, col: u32, x: f32, y: f32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_cell_hit(self.handle, slide, shape, row, col, x, y);
        unsafe { take_roe_string(ptr) }
    }
    pub fn impress_cell_caret(&self, slide: u32, shape: u32, row: u32, col: u32, offset: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_cell_caret(self.handle, slide, shape, row, col, offset);
        unsafe { take_roe_string(ptr) }
    }
    pub fn impress_cell_insert(&mut self, slide: u32, shape: u32, row: u32, col: u32, offset: u32, text: &str) -> Option<String> {
        if self.handle == 0 || text.is_empty() { return None; }
        let b = text.as_bytes();
        let ptr = roe_api::roe_impress_cell_insert(self.handle, slide, shape, row, col, offset, b.as_ptr(), b.len() as u32);
        unsafe { take_roe_string(ptr) }
    }
    pub fn impress_cell_delete(&mut self, slide: u32, shape: u32, row: u32, col: u32, offset: u32, backward: bool) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_cell_delete(self.handle, slide, shape, row, col, offset, if backward { 0 } else { 1 });
        unsafe { take_roe_string(ptr) }
    }

    // v319u: caret move in a table cell (dir 0=left 1=right) -> new offset
    pub fn impress_cell_move(&self, slide: u32, shape: u32, row: u32, col: u32, offset: u32, dir: u32) -> Option<u32> {
        if self.handle == 0 { return None; }
        let v = roe_api::roe_impress_cell_move(self.handle, slide, shape, row, col, offset, dir);
        if v < 0 { None } else { Some(v as u32) }
    }
    // v319u3: fit table shape height to measured height -> new height pt
    pub fn impress_table_fit(&self, slide: u32, shape: u32) -> Option<f32> {
        if self.handle == 0 { return None; }
        let v = roe_api::roe_impress_table_fit(self.handle, slide, shape);
        if v < 0.0 { None } else { Some(v) }
    }
    // v319u4: settle table row heights after resize -> final height pt
    pub fn impress_table_settle(&self, slide: u32, shape: u32) -> Option<f32> {
        if self.handle == 0 { return None; }
        let v = roe_api::roe_impress_table_settle(self.handle, slide, shape);
        if v < 0.0 { None } else { Some(v) }
    }
    // v319u: is the shape a table
    pub fn impress_is_table(&self, slide: u32, shape: u32) -> Option<bool> {
        if self.handle == 0 { return None; }
        match roe_api::roe_impress_is_table(self.handle, slide, shape) {
            1 => Some(true),
            0 => Some(false),
            _ => None,
        }
    }

    pub fn impress_text_copy(&self, slide: u32, shape: u32, p1: u32, o1: u32, p2: u32, o2: u32) -> bool {
        self.handle != 0 && roe_api::roe_impress_text_copy(self.handle, slide, shape, p1, o1, p2, o2) == 0
    }

    pub fn impress_text_paste(&mut self, slide: u32, shape: u32, para: u32, offset: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_text_paste(self.handle, slide, shape, para, offset);
        unsafe { take_roe_string(ptr) }
    }

    pub fn impress_text_extent(&self, slide: u32, shape: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_text_extent(self.handle, slide, shape);
        unsafe { take_roe_string(ptr) }
    }

    /// Impress：回车拆段，返回 {"para","offset"}。
    pub fn impress_text_split(&mut self, slide: u32, shape: u32, para: u32, offset: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_text_split(self.handle, slide, shape, para, offset);
        unsafe { take_roe_string(ptr) }
    }

    /// Impress：光标线段，slide 坐标 pt，返回 {"x1","y1","x2","y2"}。
    pub fn impress_text_caret(&self, slide: u32, shape: u32, para: u32, offset: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_text_caret(self.handle, slide, shape, para, offset);
        unsafe { take_roe_string(ptr) }
    }

    /// Impress（v318s）：选区矩形 JSON 数组 [{x,y,w,h}]（slide pt）。
    pub fn impress_text_sel_rects(&self, slide: u32, shape: u32, p1: u32, o1: u32, p2: u32, o2: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_text_sel_rects(self.handle, slide, shape, p1, o1, p2, o2);
        unsafe { take_roe_string(ptr) }
    }

    /// Impress（v318y）：段落样式。p1==u32::MAX 表示整个形状。cmd 如 align:center。
    pub fn impress_set_para_style(&mut self, slide_idx: u32, shape_id: u32, p1: u32, p2: u32, cmd: &str) -> bool {
        if self.handle == 0 { return false; }
        let c = match std::ffi::CString::new(cmd) { Ok(c) => c, Err(_) => return false };
        roe_api::roe_impress_set_para_style(self.handle, slide_idx, shape_id, p1, p2, c.as_ptr())
    }

    /// Impress（v318x）：查询字体名/字号 JSON。p1==u32::MAX 表示整个形状。
    pub fn impress_text_style_query(&self, slide: u32, shape: u32, p1: u32, o1: u32, p2: u32, o2: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_text_style_query(self.handle, slide, shape, p1, o1, p2, o2);
        unsafe { take_roe_string(ptr) }
    }

    /// Impress：点击点（slide pt）转文字位置，返回 {"para","offset"}（2c 使用）。
    pub fn impress_text_hit(&self, slide: u32, shape: u32, x: f32, y: f32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_text_hit(self.handle, slide, shape, x, y);
        unsafe { take_roe_string(ptr) }
    }

    /// Impress：形状文字是否为空。Some(true)=空，Some(false)=非空，None=非文字形状/失败。
    pub fn impress_text_is_empty(&self, slide: u32, shape: u32) -> Option<bool> {
        if self.handle == 0 { return None; }
        match roe_api::roe_impress_text_is_empty(self.handle, slide, shape) {
            1 => Some(true),
            0 => Some(false),
            _ => None,
        }
    }

    /// Impress：光标按字符左右移动（dir 0=左 1=右），返回 {"para","offset"}。
    pub fn impress_text_move(&self, slide: u32, shape: u32, para: u32, offset: u32, dir: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_impress_text_move(self.handle, slide, shape, para, offset, dir);
        unsafe { take_roe_string(ptr) }
    }

    /// Impress：形状缩略图路径（SVG 路径串，w x h 框内）。失败返回空串。
    pub fn impress_shape_preview_path(&self, prst: &str, w: f32, h: f32) -> String {
        let c = match std::ffi::CString::new(prst) { Ok(c) => c, Err(_) => return String::new() };
        let p = roe_api::roe_impress_shape_preview_path(c.as_ptr(), w, h);
        if p.is_null() { return String::new(); }
        let s = unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned();
        roe_api::roe_free_string(p);
        s
    }

    /// 读取 PNG/JPEG 图片原始像素宽高。失败返回 None。
    pub fn impress_image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
        if bytes.is_empty() { return None; }
        let (mut w, mut h) = (0u32, 0u32);
        if roe_api::roe_image_dimensions(bytes.as_ptr(), bytes.len(), &mut w, &mut h) {
            Some((w, h))
        } else {
            None
        }
    }

    /// Impress：在指定幻灯片插入图片（pt 坐标）。mime 传 None 时由后端按字节头识别。
    /// 成功返回新形状 id，失败返回 -1。
    pub fn impress_insert_image(&mut self, slide_idx: u32, bytes: &[u8], x: f32, y: f32, w: f32, h: f32) -> i32 {
        if self.handle == 0 || bytes.is_empty() { return -1; }
        roe_api::roe_impress_insert_image(self.handle, slide_idx, bytes.as_ptr(), bytes.len(), std::ptr::null(), x, y, w, h)
    }

    /// Impress（v221）：插入图表，chart_json 为 ChartModel 的 JSON。成功返回形状 id，失败 -1。
    pub fn impress_insert_chart(&mut self, slide_idx: u32, chart_json: &str, x: f32, y: f32, w: f32, h: f32) -> i32 {
        if self.handle == 0 { return -1; }
        let c = match std::ffi::CString::new(chart_json) { Ok(c) => c, Err(_) => return -1 };
        roe_api::roe_impress_insert_chart(self.handle, slide_idx, c.as_ptr(), x, y, w, h)
    }

    /// Impress（v222）：插入统计类图表，json 为完整 StatChartModel。成功返回形状 id，失败 -1。
    pub fn impress_insert_stat_chart(&mut self, slide_idx: u32, json: &str, x: f32, y: f32, w: f32, h: f32) -> i32 {
        if self.handle == 0 { return -1; }
        let c = match std::ffi::CString::new(json) { Ok(c) => c, Err(_) => return -1 };
        roe_api::roe_impress_insert_stat_chart(self.handle, slide_idx, c.as_ptr(), x, y, w, h)
    }

    /// Impress（v221）：读取图表形状的完整 ChartModel JSON。
    pub fn impress_get_chart_json(&mut self, slide_idx: u32, shape_id: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        unsafe { take_roe_string(roe_api::roe_impress_get_chart_json(self.handle, slide_idx, shape_id)) }
    }

    /// Impress（v221）：用新的 ChartModel JSON 整体替换图表内容（保留 id 与位置尺寸）。
    pub fn impress_set_chart_json(&mut self, slide_idx: u32, shape_id: u32, chart_json: &str) -> bool {
        if self.handle == 0 { return false; }
        let c = match std::ffi::CString::new(chart_json) { Ok(c) => c, Err(_) => return false };
        roe_api::roe_impress_set_chart_json(self.handle, slide_idx, shape_id, c.as_ptr())
    }

    /// Impress（v318o）：改形状样式。fill/stroke: ""不改 "none"清除 "#rrggbb"；width<0 不改。
    pub fn impress_set_shape_style(&mut self, slide_idx: u32, shape_id: u32, fill: &str, stroke: &str, width: f32) -> bool {
        if self.handle == 0 { return false; }
        let f = match std::ffi::CString::new(fill) { Ok(c) => c, Err(_) => return false };
        let st = match std::ffi::CString::new(stroke) { Ok(c) => c, Err(_) => return false };
        roe_api::roe_impress_set_shape_style(self.handle, slide_idx, shape_id, f.as_ptr(), st.as_ptr(), width)
    }

    /// Impress（v318r）：在本页插入 rows×cols 空表格，返回新形状 id，失败 -1。
    pub fn impress_insert_table(&mut self, slide_idx: u32, rows: u32, cols: u32, x: f32, y: f32, w: f32, h: f32) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_impress_insert_table(self.handle, slide_idx, rows, cols, x, y, w, h)
    }

    /// Impress（v318s）：只改选区文字样式。
    pub fn impress_set_text_style_range(&mut self, slide_idx: u32, shape_id: u32, p1: u32, o1: u32, p2: u32, o2: u32, cmd: &str) -> bool {
        if self.handle == 0 { return false; }
        let c = match std::ffi::CString::new(cmd) { Ok(c) => c, Err(_) => return false };
        roe_api::roe_impress_set_text_style_range(self.handle, slide_idx, shape_id, p1, o1, p2, o2, c.as_ptr())
    }

    /// Impress（v318q）：形状文字样式。cmd 如 bold:toggle / size:+2 / color:#rrggbb / clear:1。
    pub fn impress_set_text_style(&mut self, slide_idx: u32, shape_id: u32, cmd: &str) -> bool {
        if self.handle == 0 { return false; }
        let c = match std::ffi::CString::new(cmd) { Ok(c) => c, Err(_) => return false };
        roe_api::roe_impress_set_text_style(self.handle, slide_idx, shape_id, c.as_ptr())
    }

    /// Impress（v318o）：本页背景设为图片（拉伸铺满）。
    pub fn impress_set_slide_background_image(&mut self, slide_idx: u32, bytes: &[u8], mime: &str) -> bool {
        if self.handle == 0 || bytes.is_empty() { return false; }
        let m = match std::ffi::CString::new(mime) { Ok(c) => c, Err(_) => return false };
        roe_api::roe_impress_set_slide_background_image(self.handle, slide_idx, bytes.as_ptr(), bytes.len(), m.as_ptr())
    }

    /// Impress（v318o）：本页背景。color: "#rrggbb" 或 "none"（回退母版/白色）。
    pub fn impress_set_slide_background(&mut self, slide_idx: u32, color: &str) -> bool {
        if self.handle == 0 { return false; }
        let c = match std::ffi::CString::new(color) { Ok(c) => c, Err(_) => return false };
        roe_api::roe_impress_set_slide_background(self.handle, slide_idx, c.as_ptr())
    }

    /// Impress 母版视图：在母版/版式里插入图片，返回新形状 id，失败 -1。
    pub fn impress_master_insert_image(&mut self, master: u32, layout: i32, bytes: &[u8], x: f32, y: f32, w: f32, h: f32) -> i32 {
        if self.handle == 0 || bytes.is_empty() { return -1; }
        roe_api::roe_impress_master_insert_image(self.handle, master, layout, bytes.as_ptr(), bytes.len(), std::ptr::null(), x, y, w, h)
    }

    /// Impress 母版视图：母版/版式背景设为图片。
    pub fn impress_master_set_background_image(&mut self, master: u32, layout: i32, bytes: &[u8], mime: &str) -> bool {
        if self.handle == 0 || bytes.is_empty() { return false; }
        let m = match std::ffi::CString::new(mime) { Ok(c) => c, Err(_) => return false };
        roe_api::roe_impress_master_set_background_image(self.handle, master, layout, bytes.as_ptr(), bytes.len(), m.as_ptr())
    }

    /// Impress 母版视图：母版/版式背景色，color 为 "#rrggbb" 或 "none"。
    pub fn impress_master_set_background(&mut self, master: u32, layout: i32, color: &str) -> bool {
        if self.handle == 0 { return false; }
        let c = match std::ffi::CString::new(color) { Ok(c) => c, Err(_) => return false };
        roe_api::roe_impress_master_set_background(self.handle, master, layout, c.as_ptr())
    }

    /// Impress 母版视图：每个版式被幻灯片使用的次数（下标即版式下标）。
    pub fn impress_master_layout_usage(&mut self, master: u32) -> Vec<u32> {
        if self.handle == 0 { return Vec::new(); }
        let j = unsafe { take_roe_string(roe_api::roe_impress_master_layout_usage(self.handle, master)) };
        match j {
            Some(j) => j
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .filter_map(|x| x.trim().parse::<u32>().ok())
                .collect(),
            None => Vec::new(),
        }
    }

    /// Impress 母版视图：删除母版/版式内的形状。
    pub fn impress_master_delete_shape(&mut self, master: u32, layout: i32, shape_id: u32) -> bool {
        self.handle != 0 && roe_api::roe_impress_master_delete_shape(self.handle, master, layout, shape_id)
    }

    /// Impress 母版视图：删除版式（有幻灯片在用则失败）。
    pub fn impress_master_delete_layout(&mut self, master: u32, layout: u32) -> bool {
        self.handle != 0 && roe_api::roe_impress_master_delete_layout(self.handle, master, layout)
    }

    /// Impress 母版视图：复制版式到原版式之后。成功返回新版式下标，失败 -1。
    pub fn impress_master_duplicate_layout(&mut self, master: u32, layout: u32) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_impress_master_duplicate_layout(self.handle, master, layout)
    }

    /// Impress 母版视图：重命名版式。成功返回 true。
    pub fn impress_master_rename_layout(&mut self, master: u32, layout: u32, name: &str) -> bool {
        if self.handle == 0 { return false; }
        let c = match std::ffi::CString::new(name) { Ok(c) => c, Err(_) => return false };
        roe_api::roe_impress_master_rename_layout(self.handle, master, layout, c.as_ptr())
    }

    /// Impress 母版视图：设置母版/版式内形状的绝对旋转角度（度）。
    pub fn impress_master_set_shape_rotation(&mut self, master: u32, layout: i32, shape_id: u32, degrees: f32) -> bool {
        if self.handle == 0 { return false; }
        roe_api::roe_impress_master_set_shape_rotation(self.handle, master, layout, shape_id, degrees)
    }

    /// Impress 母版视图：给版式加占位符。kind: title/body/date/footer/slide_number。成功返回新形状 id，失败 -1。
    pub fn impress_master_insert_placeholder(&mut self, master: u32, layout: u32, kind: &str) -> i32 {
        if self.handle == 0 { return -1; }
        let c = match std::ffi::CString::new(kind) { Ok(c) => c, Err(_) => return -1 };
        roe_api::roe_impress_master_insert_placeholder(self.handle, master, layout, c.as_ptr())
    }

    /// Impress 母版视图：版式里已有的占位符类型（title/body/dt/ftr/sldNum）。
    pub fn impress_master_layout_placeholders(&mut self, master: u32, layout: u32) -> Vec<String> {
        if self.handle == 0 { return Vec::new(); }
        let j = unsafe { take_roe_string(roe_api::roe_impress_master_layout_placeholders(self.handle, master, layout)) };
        match j {
            Some(j) => j
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|x| x.trim().trim_matches('"').to_string())
                .filter(|x| !x.is_empty())
                .collect(),
            None => Vec::new(),
        }
    }

    /// Impress：在 after_idx 之后新建空白页（沿用该页母版/背景）。成功返回新页下标，失败 -1。
    pub fn impress_insert_slide(&mut self, after_idx: u32) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_impress_insert_slide(self.handle, after_idx)
    }

    /// Impress：删除第 idx 页（只剩一页时失败）。
    pub fn impress_delete_slide(&mut self, idx: u32) -> bool {
        self.handle != 0 && roe_api::roe_impress_delete_slide(self.handle, idx) >= 0
    }

    /// Impress：复制第 idx 页到其后。成功返回新页下标，失败 -1。
    pub fn impress_duplicate_slide(&mut self, idx: u32) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_impress_duplicate_slide(self.handle, idx)
    }

    /// Impress: insert a new slide after after_idx with the given layout. Returns new index, -1 on failure.
    pub fn impress_insert_slide_with_layout(&mut self, after_idx: u32, layout_idx: u32) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_impress_insert_slide_with_layout(self.handle, after_idx, layout_idx)
    }

    /// Impress: apply a layout to an existing slide (adds missing placeholders).
    pub fn impress_set_slide_layout(&mut self, slide_idx: u32, layout_idx: u32) -> bool {
        self.handle != 0 && roe_api::roe_impress_set_slide_layout(self.handle, slide_idx, layout_idx) >= 0
    }

    /// Impress: layouts of the master used by the slide, as JSON.
    pub fn impress_list_layouts(&self, slide_idx: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        unsafe { take_roe_string(roe_api::roe_impress_list_layouts(self.handle, slide_idx)) }
    }

    /// Impress: layout index of the slide. -1 on failure, -2 if the slide has no layout.
    pub fn impress_slide_layout(&self, slide_idx: u32) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_impress_slide_layout(self.handle, slide_idx)
    }

    /// Impress 母版视图：母版与版式两级列表（JSON）。
    pub fn impress_master_list(&self) -> Option<String> {
        if self.handle == 0 { return None; }
        unsafe { take_roe_string(roe_api::roe_impress_master_list(self.handle)) }
    }

    /// Impress 母版视图：渲染母版（layout=-1）或版式，返回 (RGBA, 宽, 高)。
    pub fn impress_master_render(&mut self, master: u32, layout: i32, width_px: i32) -> Option<(Vec<u8>, u32, u32)> {
        if self.handle == 0 || width_px <= 0 { return None; }
        let size_json = unsafe { take_roe_string(roe_api::roe_impress_master_render_size(self.handle, width_px as u32)) }?;
        let v = serde_json::from_str::<serde_json::Value>(&size_json).ok()?;
        let w = v.get("width").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        let h = v.get("height").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        if w == 0 || h == 0 { return None; }
        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let rc = roe_api::roe_impress_render_master(self.handle, master, layout, width_px as u32, buf.as_mut_ptr(), buf.len());
        if rc != 0 { return None; }
        Some((buf, w, h))
    }

    /// Impress 母版视图：母版/版式内命中测试，返回最顶层形状 id。
    pub fn impress_master_hit_test(&mut self, master: u32, layout: i32, x_pt: f32, y_pt: f32) -> Option<u32> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_impress_master_hit_test(self.handle, master, layout, x_pt, y_pt)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        v.get("shape_id").and_then(|x| x.as_u64()).map(|x| x as u32)
    }

    /// Impress 母版视图：查询母版/版式内形状矩形 (x, y, w, h, rotation)。
    pub fn impress_master_query_shape_rect(&mut self, master: u32, layout: i32, shape_id: u32) -> Option<(f32, f32, f32, f32, f32)> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_impress_master_query_shape_rect(self.handle, master, layout, shape_id)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        let g = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
        Some((g("x_pt"), g("y_pt"), g("width_pt"), g("height_pt"), g("rotation")))
    }

    /// Impress 母版视图：一次性设置母版/版式内形状矩形。
    pub fn impress_master_set_shape_rect(&mut self, master: u32, layout: i32, shape_id: u32, x_pt: f32, y_pt: f32, w_pt: f32, h_pt: f32) -> bool {
        self.handle != 0 && roe_api::roe_impress_master_set_shape_rect(self.handle, master, layout, shape_id, x_pt, y_pt, w_pt, h_pt)
    }

    /// Impress 母版视图：把两级列表展开成平铺项 (母版序号, 版式序号或-1, 名称)。
    pub fn impress_master_items(&self) -> Vec<(u32, i32, String)> {
        let mut out: Vec<(u32, i32, String)> = Vec::new();
        let Some(j) = self.impress_master_list() else { return out; };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&j) else { return out; };
        if let Some(ms) = v.as_array() {
            for m in ms {
                let mi = m.get("master").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let name = m.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                out.push((mi, -1, name));
                if let Some(ls) = m.get("layouts").and_then(|x| x.as_array()) {
                    for l in ls {
                        let li = l.get("index").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
                        let ln = l.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        out.push((mi, li, ln));
                    }
                }
            }
        }
        out
    }

    /// Impress 母版视图：每行（与 impress_master_items 同序）的悬停提示文本。
    pub fn impress_master_tips(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let Some(j) = self.impress_master_list() else { return out; };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&j) else { return out; };
        if let Some(ms) = v.as_array() {
            for m in ms {
                let name = m.get("name").and_then(|x| x.as_str()).unwrap_or("");
                out.push(format!("母版：{}", name));
                if let Some(ls) = m.get("layouts").and_then(|x| x.as_array()) {
                    for l in ls {
                        let ln = l.get("name").and_then(|x| x.as_str()).unwrap_or("");
                        let used: Vec<String> = l.get("used_by").and_then(|x| x.as_array())
                            .map(|a| a.iter().filter_map(|n| n.as_u64()).map(|n| n.to_string()).collect())
                            .unwrap_or_default();
                        if used.is_empty() {
                            out.push(format!("{}\n未被使用", ln));
                        } else {
                            out.push(format!("{}\n应用于：第 {} 页", ln, used.join("、")));
                        }
                    }
                }
            }
        }
        out
    }

    /// Impress：按 shape_id 删除形状。
    pub fn impress_delete_shape(&mut self, slide_idx: u32, shape_id: u32) -> bool {
        self.handle != 0 && roe_api::roe_impress_delete_shape(self.handle, slide_idx, shape_id)
    }

    /// Impress：设置形状旋转角（绝对值，度）。
    pub fn impress_set_shape_rotation(&mut self, slide_idx: u32, shape_id: u32, degrees: f32) -> bool {
        self.handle != 0 && roe_api::roe_impress_set_shape_rotation(self.handle, slide_idx, shape_id, degrees)
    }

    /// v51新增：渲染Impress视口。vp_x/vp_y是画布坐标（pt），vw_pt/vh_pt是视口尺寸（pt）。
    pub fn render_impress_viewport(&mut self, vp_x: f32, vp_y: f32, vw_pt: f32, vh_pt: f32) -> Vec<u8> {
        if self.handle == 0 || vw_pt <= 0.0 || vh_pt <= 0.0 { return Vec::new(); }
        let scale: f32 = impress_bitmap_scale(); // HiDPI
        let pixel_w = (vw_pt * scale).round() as u32;
        let pixel_h = (vh_pt * scale).round() as u32;
        let buf_len = (pixel_w * pixel_h * 4) as usize;
        let mut buf = vec![0u8; buf_len];
        let code = roe_api::roe_render_impress_viewport(
            self.handle,
            vp_x, vp_y, vw_pt, vh_pt,
            pixel_w, pixel_h,
            buf.as_mut_ptr(), buf.len(),
        );
        if code == 0 {
            buf
        } else {
            eprintln!("[IMPRESS-VIEWPORT] render failed code={}", code);
            Vec::new()
        }
    }

    pub fn render_slide_thumbnail(&self, n_part: i32, width: i32, height: i32) -> Vec<u8> {
        if self.handle == 0 || width <= 0 || height <= 0 || n_part < 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; (width as usize) * (height as usize) * 4];
        let code = roe_render_page(self.handle, n_part as u32, width as u32, height as u32, buf.as_mut_ptr(), buf.len());
        if code == RoeErrorCode::Ok as i32 {
            buf
        } else {
            Vec::new()
        }
    }

    // ================= 脚本执行 =================

    /// 替代 post_uno_command / query_format_state / get_command_values。
    /// 返回 roe_execute_script 的原始 JSON 字符串（{"ok":true,"dirty_pages":[...],...} 或 {"ok":false,"error":...}）。
    pub fn execute_script(&mut self, script: &str) -> String {
        if self.handle == 0 {
            return err_json("no document open");
        }
        let c_script = match std::ffi::CString::new(script) {
            Ok(s) => s,
            Err(_) => return err_json("script contains NUL byte"),
        };
        match unsafe { take_roe_string(roe_execute_script(self.handle, c_script.as_ptr())) } {
            Some(s) => s,
            None => err_json("roe_execute_script returned null"),
        }
    }

    /// 只读执行 Rhai 脚本，不触发 dirty/重渲染。
    /// 返回 {"ok":true,"value":"..."} 或 {"ok":false,"error":"..."}
    pub fn query_script(&self, script: &str) -> String {
        if self.handle == 0 {
            return err_json("no document open");
        }
        let c_script = match std::ffi::CString::new(script) {
            Ok(s) => s,
            Err(_) => return err_json("script contains NUL byte"),
        };
        match unsafe { take_roe_string(roe_api::roe_query_script(self.handle, c_script.as_ptr())) } {
            Some(s) => s,
            None => err_json("roe_query_script returned null"),
        }
    }

    // ================= 前台数据集(parquet 惰性分页) =================

    /// 前台手动把 parquet 文件/目录加载为命名数据集(一期物化进句柄数据集容器)。
    /// 路径由用户在前台选择,不经脚本。成功返回 (行数, 列名)。
    pub fn df_open_parquet(&mut self, path: &str, name: &str) -> Result<(u64, Vec<String>), String> {
        if self.handle == 0 {
            return Err("no document open".to_string());
        }
        if !df_valid_name(name) {
            return Err("数据集名称只能含字母、数字、下划线".to_string());
        }
        let c_path = std::ffi::CString::new(path).map_err(|_| "路径含 NUL 字符".to_string())?;
        let c_name = std::ffi::CString::new(name).map_err(|_| "名称含 NUL 字符".to_string())?;
        let raw = unsafe {
            take_roe_string(roe_api::script::roe_df_open_parquet(
                self.handle,
                c_path.as_ptr(),
                c_name.as_ptr(),
            ))
        }
        .ok_or_else(|| "roe_df_open_parquet returned null".to_string())?;
        let v = df_unwrap(&raw)?;
        let rows = v.get("rows").and_then(|x| x.as_u64()).unwrap_or(0);
        let cols: Vec<String> = v
            .get("cols")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
            .unwrap_or_default();
        Ok((rows, cols))
    }

    /// 取命名数据集的一页(惰性分页,只读引擎)。df_use 与 df_page 在同一次脚本里执行。
    pub fn df_page(&self, name: &str, offset: u64, limit: u32) -> Result<DfPage, String> {
        if self.handle == 0 {
            return Err("no document open".to_string());
        }
        if !df_valid_name(name) {
            return Err("数据集名称只能含字母、数字、下划线".to_string());
        }
        let limit = limit.clamp(1, 1000);
        let raw = self.query_script(&format!("df_use(\"{}\"); df_page({}, {})", name, offset, limit));
        let v = df_unwrap(&raw)?;
        let columns: Vec<String> = v
            .get("columns")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().map(df_cell).collect())
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = v
            .get("rows")
            .and_then(|x| x.as_array())
            .map(|rs| {
                rs.iter()
                    .map(|r| r.as_array().map(|cs| cs.iter().map(df_cell).collect()).unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        Ok(DfPage {
            columns,
            rows,
            total_rows: v.get("total_rows").and_then(|x| x.as_u64()).unwrap_or(0),
            offset: v.get("offset").and_then(|x| x.as_u64()).unwrap_or(offset),
        })
    }

    // ================= 前台数据集:通用文件导入(csv/parquet/xlsx) =================

    /// 预览外部文件(概览 JSON:rows/cols/columns/head),不注册、不碰文档。
    /// kind: "csv" | "parquet" | "xlsx";opts_json 如 {"delimiter":",","has_header":true,"head":10}。
    pub fn df_preview_file(&self, path: &str, kind: &str, opts_json: &str) -> Result<serde_json::Value, String> {
        let c_path = std::ffi::CString::new(path).map_err(|_| "路径含 NUL 字符".to_string())?;
        let c_kind = std::ffi::CString::new(kind).map_err(|_| "类型含 NUL 字符".to_string())?;
        let c_opts = std::ffi::CString::new(opts_json).map_err(|_| "选项含 NUL 字符".to_string())?;
        let raw = unsafe {
            take_roe_string(roe_api::script::roe_df_preview_file(c_path.as_ptr(), c_kind.as_ptr(), c_opts.as_ptr()))
        }
        .ok_or_else(|| "roe_df_preview_file returned null".to_string())?;
        df_unwrap(&raw)
    }

    /// 确认后把外部文件注册为数据集 df_N,返回 (数据集名, 行数, 列名)。
    pub fn df_register_file(&mut self, path: &str, kind: &str, opts_json: &str) -> Result<(String, u64, Vec<String>), String> {
        if self.handle == 0 {
            return Err("no document open".to_string());
        }
        let c_path = std::ffi::CString::new(path).map_err(|_| "路径含 NUL 字符".to_string())?;
        let c_kind = std::ffi::CString::new(kind).map_err(|_| "类型含 NUL 字符".to_string())?;
        let c_opts = std::ffi::CString::new(opts_json).map_err(|_| "选项含 NUL 字符".to_string())?;
        let raw = unsafe {
            take_roe_string(roe_api::script::roe_df_register_file(
                self.handle, c_path.as_ptr(), c_kind.as_ptr(), c_opts.as_ptr(),
            ))
        }
        .ok_or_else(|| "roe_df_register_file returned null".to_string())?;
        let v = df_unwrap(&raw)?;
        let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let rows = v.get("rows").and_then(|x| x.as_u64()).unwrap_or(0);
        let cols: Vec<String> = v
            .get("cols")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if name.is_empty() {
            return Err("注册结果缺少数据集名".to_string());
        }
        Ok((name, rows, cols))
    }

    /// 把数据集前 n 行写成新 sheet(预览),返回新 sheet 索引。重名时后端自动加 _2、_3 后缀。
    pub fn df_sheet_head(&mut self, name: &str, n: u32) -> Result<i64, String> {
        if self.handle == 0 {
            return Err("no document open".to_string());
        }
        if !df_valid_name(name) {
            return Err("数据集名称只能含字母、数字、下划线".to_string());
        }
        let raw = self.execute_script(&format!("df_to_sheet_head(\"{}\", {})", name, n));
        let v = df_unwrap(&raw)?;
        match v.as_i64() {
            Some(i) if i >= 0 => Ok(i),
            Some(_) => Err("写入预览 sheet 失败(数据集不存在或写入出错)".to_string()),
            None => Err(format!("意外的返回值: {v}")),
        }
    }

    // ================= 前台数据集:SQL 查询 =================

    /// 列出会话数据集,返回 (名称, 列数) 列表(只读)。Lazy 数据集无行数,故不返回行数。
    pub fn df_list_datasets(&self) -> Result<Vec<(String, u64)>, String> {
        if self.handle == 0 {
            return Err("no document open".to_string());
        }
        let raw = self.query_script("df_list_datasets()");
        let v = df_unwrap(&raw)?;
        let arr = v.as_array().ok_or_else(|| format!("意外的返回值: {v}"))?;
        Ok(arr
            .iter()
            .filter_map(|x| {
                let n = x.get("name")?.as_str()?.to_string();
                let c = x.get("cols").and_then(|c| c.as_u64()).unwrap_or(0);
                Some((n, c))
            })
            .collect())
    }

    /// 多数据集 SQL 预览(只读,不注册、不改文档)。返回 {"columns","rows","total_rows"}(最多 50 行)。
    pub fn df_sql_preview(&self, sql: &str) -> Result<serde_json::Value, String> {
        if self.handle == 0 {
            return Err("no document open".to_string());
        }
        if sql.trim().is_empty() {
            return Err("SQL 不能为空".to_string());
        }
        let raw = self.query_script(&format!("df_sql_ds({})", rhai_str(sql)));
        df_unwrap(&raw)
    }

    /// 多数据集 SQL 结果存为新数据集 df_N,返回 (数据集名, 行数, 列名)。
    pub fn df_sql_save(&mut self, sql: &str) -> Result<(String, u64, Vec<String>), String> {
        if self.handle == 0 {
            return Err("no document open".to_string());
        }
        if sql.trim().is_empty() {
            return Err("SQL 不能为空".to_string());
        }
        let raw = self.execute_script(&format!("df_sql_save({})", rhai_str(sql)));
        let v = df_unwrap(&raw)?;
        let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let rows = v.get("rows").and_then(|x| x.as_u64()).unwrap_or(0);
        let cols: Vec<String> = v
            .get("cols")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if name.is_empty() {
            return Err("保存结果缺少数据集名".to_string());
        }
        Ok((name, rows, cols))
    }

    // ================= 前台数据集:DF 链 =================

    /// 数据集的列名(只读,走惰性概览)。
    pub fn df_columns(&self, name: &str) -> Result<Vec<String>, String> {
        if self.handle == 0 {
            return Err("no document open".to_string());
        }
        if !df_valid_name(name) {
            return Err("数据集名称只能含字母、数字、下划线".to_string());
        }
        let raw = self.query_script(&format!("df_dataset_overview(\"{}\", 0)", name));
        let v = df_unwrap(&raw)?;
        Ok(v.get("columns")
            .and_then(|c| c.as_array())
            .map(|a| a.iter().filter_map(|c| c.get("name")?.as_str().map(String::from)).collect())
            .unwrap_or_default())
    }

    /// DF 链预览(只读,不注册):逐行包装后执行,返回 {"columns","rows","total_rows"}(前 50 行)。
    /// 链里某行失败 -> Err("...第N行: ...")。
    pub fn df_chain_preview(&self, code: &str) -> Result<serde_json::Value, String> {
        if self.handle == 0 {
            return Err("no document open".to_string());
        }
        if code.trim().is_empty() {
            return Err("代码不能为空".to_string());
        }
        let script = format!("{}\ndf_preview(50)", roe_api::script::wrap_df_chain(code));
        df_unwrap(&self.query_script(&script))
    }

    /// DF 链结果存为新数据集 chain_N,返回 (名称, 行数, 列名)。
    pub fn df_chain_save(&mut self, code: &str) -> Result<(String, u64, Vec<String>), String> {
        if self.handle == 0 {
            return Err("no document open".to_string());
        }
        if code.trim().is_empty() {
            return Err("代码不能为空".to_string());
        }
        let script = format!("{}\ndf_save_result()", roe_api::script::wrap_df_chain(code));
        let v = df_unwrap(&self.execute_script(&script))?;
        let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let rows = v.get("rows").and_then(|x| x.as_u64()).unwrap_or(0);
        let cols: Vec<String> = v
            .get("cols")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if name.is_empty() {
            return Err("保存结果缺少数据集名".to_string());
        }
        Ok((name, rows, cols))
    }

    // ================= 编辑事务（替代 shadow 机制） =================

    pub fn begin_edit(&mut self) -> bool {
        self.handle != 0 && roe_begin_edit(self.handle) == RoeErrorCode::Ok as i32
    }
    pub fn commit_edit(&mut self) -> bool {
        self.handle != 0 && roe_commit_edit(self.handle) == RoeErrorCode::Ok as i32
    }
    pub fn discard_edit(&mut self) {
        if self.handle != 0 {
            roe_discard_edit(self.handle);
        }
    }
    pub fn get_index(&mut self) -> String {
        if self.handle == 0 {
            return err_json("no document open");
        }
        match unsafe { take_roe_string(roe_get_index(self.handle)) } {
            Some(s) => s,
            None => err_json("roe_get_index returned null"),
        }
    }

    // ================= Calc 几何查询 =================

    /// 返回第 col 列的列宽（pt）。失败返回默认值 72.0pt（≈96px）。
    pub fn get_column_width_pt(&mut self, sheet_idx: u32, col: u32) -> f32 {
        if self.handle == 0 { return 72.0; }
        let v = roe_api::roe_get_column_width(self.handle, sheet_idx, col);
        if v <= 0.0 { 72.0 } else { v }
    }

    /// 返回第 row 行的行高（pt）。失败返回默认值 18.0pt（≈24px）。
    pub fn get_row_height_pt(&mut self, sheet_idx: u32, row: u32) -> f32 {
        if self.handle == 0 { return 18.0; }
        let v = roe_api::roe_get_row_height(self.handle, sheet_idx, row);
        if v <= 0.0 { 18.0 } else { v }
    }

    /// 返回第 row 行的起始 y 坐标（pt）。失败返回 -1.0。
    pub fn get_row_offset_pt(&self, sheet_idx: u32, row: u32) -> f32 {
        if self.handle == 0 { return -1.0; }
        roe_api::roe_get_row_offset(self.handle, sheet_idx, row)
    }

    /// 返回第 col 列的起始 x 坐标（pt）。失败返回 -1.0。
    pub fn get_col_offset_pt(&self, sheet_idx: u32, col: u32) -> f32 {
        if self.handle == 0 { return -1.0; }
        roe_api::roe_get_col_offset(self.handle, sheet_idx, col)
    }

    /// 给定 y_pt，返回对应行号（0-based）。失败返回 -1。
    pub fn get_row_at_offset(&self, sheet_idx: u32, y_pt: f32) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_get_row_at_offset(self.handle, sheet_idx, y_pt)
    }

    /// 给定 x_pt，返回对应列号（0-based）。失败返回 -1。
    pub fn get_col_at_offset(&self, sheet_idx: u32, x_pt: f32) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_get_col_at_offset(self.handle, sheet_idx, x_pt)
    }

    /// 返回 sheet 实际使用的行列数。失败返回 (0, 0)。
    pub fn get_used_range(&self, sheet_idx: u32) -> (u32, u32) {
        if self.handle == 0 { return (0, 0); }
        let mut max_row: u32 = 0;
        let mut max_col: u32 = 0;
        let ok = roe_api::roe_get_used_range(self.handle, sheet_idx, &mut max_row, &mut max_col);
        if ok == 1 { (max_row, max_col) } else { (0, 0) }
    }

    /// 前台动态扩展：给指定 sheet 的总高度/总宽度加长一档（内核值不变，只在前台叠加）。
    pub fn extend_sheet_total(&self, sheet_idx: u32, is_height: bool) {
        calc_extra_add(self.handle, sheet_idx, is_height);
    }

    /// 返回 sheet 总高度（pt），用于 Flickable content-height。失败返回 -1.0。
    pub fn get_sheet_total_height(&self, sheet_idx: u32) -> f32 {
        if self.handle == 0 { return -1.0; }
        { let b = roe_api::roe_get_sheet_total_height(self.handle, sheet_idx); if b < 0.0 { b } else { b + calc_extra_get(self.handle, sheet_idx).0 } }
    }

    /// 返回 sheet 总宽度（pt），用于 Flickable content-width。失败返回 -1.0。
    pub fn get_sheet_total_width(&self, sheet_idx: u32) -> f32 {
        if self.handle == 0 { return -1.0; }
        { let b = roe_api::roe_get_sheet_total_width(self.handle, sheet_idx); if b < 0.0 { b } else { b + calc_extra_get(self.handle, sheet_idx).1 } }
    }

    // ================= Calc sheet / Impress slide 索引管理 =================

    pub fn set_current_sheet(&mut self, index: i32) {
        self.current_sheet = index;
    }
    pub fn get_current_sheet(&mut self) -> i32 {
        self.current_sheet
    }
    pub fn get_slide_count(&self) -> i32 {
        if self.doc_type == "impress" {
            self.page_count.max(0)
        } else {
            0
        }
    }
    pub fn get_current_slide(&self) -> i32 {
        self.current_sheet // 复用同一字段表示当前 slide 索引
    }
    pub fn set_current_slide(&mut self, n_part: i32) {
        self.current_sheet = n_part;
    }
    pub fn get_slide_name(&self, _n_part: i32) -> String {
        // 占位：ROE 侧暂无 slide 名称字段查询，沿用旧版遗留问题
        String::new()
    }

    // ================= 文档元信息（UI层维护，与内核无关） =================

    pub fn get_original_doc_url(&self) -> String {
        self.original_doc_url.clone()
    }
    pub fn set_original_doc_url(&mut self, url: &str) {
        self.original_doc_url = url.to_string();
    }

    // ================= Polars/SQL（占位保留，待 roe-polars-bridge 签名确认） =================

    pub fn run_pivot_on_sheet(&mut self, src_sheet: i32, config_json: &str) -> String { stub_json("run_pivot_on_sheet") }
    pub fn get_unique_values_for_field(&mut self, src_sheet: i32, field: &str) -> String { stub_json("get_unique_values_for_field") }
    pub fn paste_tsv(&mut self, sheet: i32, tsv: &str) -> bool { false }

    // ================= 图表预览 =================
    /// 返回 (rgba_bytes, width, height)，由调用方转成 slint::Image
    pub fn render_chart_preview(&self, chart_json: &str) -> Option<(Vec<u8>, u32, u32)> {
        use std::ffi::CString;
        let c_json = CString::new(chart_json).ok()?;
        let mut out_len: u32 = 0;
        let ptr = unsafe { roe_api::roe_render_chart_to_png(c_json.as_ptr(), &mut out_len) };
        if ptr.is_null() || out_len == 0 {
            eprintln!("[CHART] roe_render_chart_to_png failed");
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr, out_len as usize) }.to_vec();
        unsafe { roe_api::roe_free_bytes(ptr, out_len); }
        // PNG → RGBA 字节
        let img = image::load_from_memory(&bytes).ok()?;
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        Some((rgba.into_raw(), w, h))
    }
    /// v223: 统计图大预览。返回 (rgba, w, h)。
    pub fn render_stat_chart_preview(&self, stat_json: &str, w: u32, h: u32) -> Option<(Vec<u8>, u32, u32)> {
        use std::ffi::CString;
        let c_json = CString::new(stat_json).ok()?;
        let mut out_len: u32 = 0;
        let ptr = unsafe { roe_api::roe_render_stat_chart_to_rgba(c_json.as_ptr(), w, h, &mut out_len) };
        if ptr.is_null() || out_len == 0 {
            eprintln!("[CHART] roe_render_stat_chart_to_rgba failed");
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr, out_len as usize) }.to_vec();
        unsafe { roe_api::roe_free_bytes(ptr, out_len); }
        if bytes.len() != (w as usize) * (h as usize) * 4 { return None; }
        Some((bytes, w, h))
    }
    pub fn import_file_via_polars(&self, path: &str) -> String { stub_json("import_file_via_polars") }
    pub fn df_info_cached(&self) -> String { stub_json("df_info_cached") }
    pub fn df_sql(&mut self, sql: &str) -> String { stub_json("df_sql") }
    pub fn df_sql_to_new_sheet(&mut self, sql: &str, sheet_name: &str) -> String { stub_json("df_sql_to_new_sheet") }

    // ================= 字体（保留） =================

    pub fn get_font_name(&mut self) -> String { String::new() }
    pub fn get_font_list(&mut self) -> String { stub_json("get_font_list") }

    /// Writer点击命中测试：(x_pt, y_pt) -> {para_index, char_offset}
    /// 返回(para_index, char_offset)，失败返回(-1, 0)
    pub fn writer_hit_test(&mut self, x_pt: f32, y_pt: f32) -> (i64, i64) {
        if self.handle == 0 { return (-1, 0); }
        match unsafe { take_roe_string(roe_api::roe_hit_test(self.handle, x_pt as f64, y_pt as f64)) } {
            Some(json) => {
                let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                let para = v.get("para_index").and_then(|x| x.as_i64()).unwrap_or(-1);
                let offset = v.get("char_offset").and_then(|x| x.as_i64()).unwrap_or(0);
                (para, offset)
            }
            None => (-1, 0),
        }
    }

    /// 查询光标位置：(para_index, char_offset) -> {x, y, w, h}（pt）
    /// 返回(x, y, w, h)，失败返回全0
    /// v96调试用：暴露当前handle值，方便日志核对是否跨句柄串了数据
    pub fn debug_handle(&self) -> u64 { self.handle }

    pub fn writer_query_cursor_rect(&mut self, para_index: u64, char_offset: u64) -> (f32, f32, f32, f32) {
        if self.handle == 0 { return (0.0, 0.0, 0.0, 0.0); }
        match unsafe { take_roe_string(roe_api::roe_query_layout_rect(self.handle, para_index, char_offset)) } {
            Some(json) => {
                let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                let x = v.get("x").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                // v95修复：roe_query_layout_rect 返回的y是"文档全局坐标"（doc coordinate，
                // 与 arena.total_height()/page_global_y() 同一套坐标系），而前端画布坐标系
                // （渲染视口、光标显示用的坐标）的原点是画布顶部，二者相差一个固定偏移量
                // canvas_offset = ruler高度(未启用ruler时为0) + CANVAS_TOP_PT(24.0)。
                // 对照 roe-api/src/tile.rs::roe_render_writer_viewport 里的
                // doc_vp_y = vp_y - canvas_offset，这里是其反向操作。
                // 当前硬编码24.0：本次测试文档ruler未启用，corresponds to CANVAS_TOP_PT。
                // TODO: ruler功能启用后，这里需要改为从roe查询真实canvas_offset
                // （可复用 roe_get_writer_document_size 返回的 pages[0].y，因为
                // pages[0].y = canvas_ruler_pt + CANVAS_TOP_PT + doc_y(page0)，
                // 对第一页 doc_y(page0)=0，故 pages[0].y 本身就等于 canvas_offset）。
                let y = v.get("y").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32 + CANVAS_OFFSET_PT;
                let w = v.get("w").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                let h = v.get("h").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                (x, y, w, h)
            }
            None => (0.0, 0.0, 0.0, 0.0),
        }
    }

    /// v315：单元格内区间替换。返回新 off。
    pub fn cell_replace_range(&mut self, cid: u32, para: u32, start: u32, end: u32, text: &str) -> Option<i64> {
        if self.handle == 0 { return None; }
        let b = text.as_bytes();
        let json = unsafe { take_roe_string(roe_api::roe_cell_replace_range(self.handle, cid, para, start, end, b.as_ptr(), b.len() as u32)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        v.get("off").and_then(|x| x.as_i64())
    }

    /// v315：单元格内退格(0)/Delete(1)。返回新 off。
    pub fn cell_delete_char(&mut self, cid: u32, para: u32, off: u32, direction: u32) -> Option<i64> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_cell_delete_char(self.handle, cid, para, off, direction)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        v.get("off").and_then(|x| x.as_i64())
    }

    /// v316：单元格内回车拆段，返回 (新段落下标, off)。
    pub fn cell_split(&mut self, cid: u32, para: u32, off: u32) -> Option<(i64, i64)> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_cell_split_paragraph(self.handle, cid, para, off)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        Some((v.get("para")?.as_i64()?, v.get("off")?.as_i64()?))
    }

    /// v316：Tab/Shift+Tab 跳格（表尾自动加行），返回 (新 cid, para, off)。
    pub fn cell_tab(&mut self, cid: u32, back: bool) -> Option<(u32, i64, i64)> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_cell_tab(self.handle, cid, if back { 1 } else { 0 })) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        Some((v.get("cid")?.as_u64()? as u32, v.get("para")?.as_i64()?, v.get("off")?.as_i64()?))
    }

    /// v316b：单元格内段落合并。dir=0 段首退格并入上一段，dir=1 段尾 Delete 并入下一段。返回 (para, off)。
    pub fn cell_merge(&mut self, cid: u32, para: u32, dir: u32) -> Option<(i64, i64)> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_cell_merge_paragraph(self.handle, cid, para, dir)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        Some((v.get("para")?.as_i64()?, v.get("off")?.as_i64()?))
    }

    /// v316b：单元格内光标移动。cmd 0=左 1=右 2=仅校正。返回 (cid, para, off)。
    pub fn cell_nav(&mut self, cid: u32, para: u32, off: u32, cmd: u32) -> Option<(u32, i64, i64)> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_cell_nav(self.handle, cid, para, off, cmd)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        Some((v.get("cid")?.as_u64()? as u32, v.get("para")?.as_i64()?, v.get("off")?.as_i64()?))
    }

    /// v316c：单元格内上(0)/下(1)移动。返回 (cid, para, off)。
    pub fn cell_nav_line(&mut self, cid: u32, para: u32, off: u32, dir: u32, ideal_x: f32) -> Option<(u32, i64, i64)> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_cell_nav_line(self.handle, cid, para, off, dir, ideal_x as f64)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        Some((v.get("cid")?.as_u64()? as u32, v.get("para")?.as_i64()?, v.get("off")?.as_i64()?))
    }

    /// v317：单元格格式目标（线程内状态）。ribbon 在执行格式动作前设置：Some((cid, 单元格段落下标)) 表示作用于单元格段落，None 表示正文。
    fn cell_fmt_target_rw(set: Option<Option<(u32, u32)>>) -> Option<(u32, u32)> {
        thread_local! { static T: std::cell::Cell<Option<(u32, u32)>> = std::cell::Cell::new(None); }
        T.with(|c| { if let Some(v) = set { c.set(v); } c.get() })
    }
    pub fn set_cell_fmt_target(t: Option<(u32, u32)>) { Self::cell_fmt_target_rw(Some(t)); }
    fn cell_fmt_range_rw(set: Option<Option<(u32, u32)>>) -> Option<(u32, u32)> {
        thread_local! { static R: std::cell::Cell<Option<(u32, u32)>> = std::cell::Cell::new(None); }
        R.with(|c| { if let Some(v) = set { c.set(v); } c.get() })
    }
    /// v318c：单元格格式的字节区间覆盖（None=整段）
    pub fn set_cell_fmt_range(r: Option<(u32, u32)>) { Self::cell_fmt_range_rw(Some(r)); }
    pub fn cell_fmt_target() -> Option<(u32, u32)> { Self::cell_fmt_target_rw(None) }

    fn cell_fmt_char(&self, cid: u32, para: u32, start: u32, end: u32, kind: u32, a: u32, b: u32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let (start, end) = Self::cell_fmt_range_rw(None).unwrap_or((start, end));
        let ptr = roe_api::roe_cell_fmt_char(self.handle, cid, para, start, end, kind, a, b);
        if ptr.is_null() { return Err(Self::formula_last_err()); }
        let _ = unsafe { take_roe_string(ptr) };
        Ok(())
    }
    fn cell_fmt_font(&self, cid: u32, para: u32, name: Option<&str>) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let c = match name {
            Some(n) => Some(std::ffi::CString::new(n).map_err(|_| "字体名含NUL".to_string())?),
            None => None,
        };
        let ptr = c.as_ref().map_or(std::ptr::null(), |x| x.as_ptr());
        let (fs0, fe0) = Self::cell_fmt_range_rw(None).unwrap_or((0, u32::MAX));
        let r = roe_api::roe_cell_fmt_font(self.handle, cid, para, fs0, fe0, ptr);
        if r.is_null() { return Err(Self::formula_last_err()); }
        let _ = unsafe { take_roe_string(r) };
        Ok(())
    }
    fn cell_fmt_para(&self, cid: u32, para: u32, kind: u32, value: f32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let ptr = roe_api::roe_cell_fmt_para(self.handle, cid, para, kind, value);
        if ptr.is_null() { return Err(Self::formula_last_err()); }
        let _ = unsafe { take_roe_string(ptr) };
        Ok(())
    }
    fn cell_fmt_get(&self, cid: u32, para: u32, kind: u32) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_cell_fmt_get(self.handle, cid, para, kind)
    }

    /// v314：扩展命中测试。返回 (ct, cid, para, off)；ct=1 表示命中表格单元格。失败返回 (-1,0,-1,0)。
    pub fn writer_hit_test_ex(&mut self, x_pt: f32, y_pt: f32) -> (i64, u32, i64, i64) {
        if self.handle == 0 { return (-1, 0, -1, 0); }
        match unsafe { take_roe_string(roe_api::roe_hit_test_ex(self.handle, x_pt as f64, y_pt as f64)) } {
            Some(json) => {
                let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                let ct = v.get("ct").and_then(|x| x.as_i64()).unwrap_or(-1);
                let cid = v.get("cid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let para = v.get("para").and_then(|x| x.as_i64()).unwrap_or(-1);
                let off = v.get("off").and_then(|x| x.as_i64()).unwrap_or(0);
                (ct, cid, para, off)
            }
            None => (-1, 0, -1, 0),
        }
    }

    /// v314：扩展光标矩形，返回 (x, y, w, h)，y 已加画布偏移（与旧接口一致）。
    pub fn writer_query_cursor_rect_ex(&mut self, ct: u32, cid: u32, para: u32, off: u32) -> (f32, f32, f32, f32) {
        if self.handle == 0 { return (0.0, 0.0, 0.0, 0.0); }
        match unsafe { take_roe_string(roe_api::roe_query_layout_rect_ex(self.handle, ct, cid, para, off)) } {
            Some(json) => {
                let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                let x = v.get("x").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                let y = v.get("y").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32 + CANVAS_OFFSET_PT;
                let w = v.get("w").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                let h = v.get("h").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                (x, y, w, h)
            }
            None => (0.0, 0.0, 0.0, 0.0),
        }
    }

    /// v318a：单元格内选区矩形 JSON（y 为文档坐标，调用方自行加画布偏移）。
    pub fn cell_selection_rects(&self, cid: u32, sp: u32, so: u32, ep: u32, eo: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_query_cell_selection_rects(self.handle, cid, sp, so, ep, eo);
        unsafe { take_roe_string(ptr) }
    }

    /// v318b：两个单元格围成的矩形范围内所有整格矩形 JSON（y 为文档坐标）。
    pub fn cell_range_rects(&self, cid_a: u32, cid_b: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_query_cell_range_rects(self.handle, cid_a, cid_b);
        unsafe { take_roe_string(ptr) }
    }

    /// Writer 页面左边距（pt）。暂用默认值，后续从 ROE 接口读取。
    pub fn writer_margin_left(&self) -> f32 { 70.87 }

    /// Writer 页面上边距（pt）。暂用默认值，后续从 ROE 接口读取。
    pub fn writer_margin_top(&self) -> f32 { 56.69 }


    /// 读取 Calc 单元格值（字符串），失败返回空字符串。
    pub fn calc_get_cell(&mut self, sheet: u32, row: u32, col: u32) -> String {
        let script = format!("get_cell({}, {}, {})", sheet, row, col);
        let s = self.query_script(&script);
        if s.contains("\"ok\":true") {
            let v: serde_json::Value = serde_json::from_str(&s).unwrap_or_default();
            v.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string()
        } else {
            String::new()
        }
    }

    /// 读取 Calc 单元格的公式原文（不含前导"="）。非公式单元格返回空串。
    /// 2026-07-13新增：此前公式栏/双击编辑框统一复用 calc_get_cell（取
    /// 显示值），导致公式提交后一旦求值完成，公式栏/编辑框只能看到计算
    /// 结果或错误码，公式原文实质上不可见也不可再编辑。本函数专供公式
    /// 栏/编辑场景使用，与 calc_get_cell（tile渲染显示值用）职责区分。
    /// 读取单元格格式（roe_model::calc::CellFormat 的 JSON 对象）。
    /// 读不到/解析失败返回空对象 {}。
    /// get_cell_format 返回的是"JSON 字符串"，经 query_script 包装后可能是
    /// 已展开的对象，也可能是被当成字符串嵌在 value 里，这里两种形态都兼容。
    pub fn calc_get_cell_format(&mut self, sheet: u32, row: u32, col: u32) -> serde_json::Value {
        let script = format!("get_cell_format({}, {}, {})", sheet, row, col);
        let s = self.query_script(&script);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap_or_default();
        if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
            return serde_json::json!({});
        }
        match v.get("value") {
            Some(serde_json::Value::Object(o)) => serde_json::Value::Object(o.clone()),
            Some(serde_json::Value::String(t)) => {
                serde_json::from_str(t).unwrap_or_else(|_| serde_json::json!({}))
            }
            _ => serde_json::json!({}),
        }
    }

    /// 对矩形区域 [r1..=r2] x [c1..=c2] 的每个单元格设置加粗/取消加粗。
    /// set_cell_format 是整体覆盖（不是合并），所以逐格"读格式 -> 改 bold -> 写回"，
    /// 其余字段原样保留。所有写入拼成一个脚本一次执行，只产生一条撤销记录。
    /// 加粗的薄封装,保持原有调用点不变。
    pub fn calc_apply_bold(&mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32, bold: bool) -> bool {
        self.calc_apply_format_flag(sheet, r1, c1, r2, c2, "bold", bold)
    }

    /// 对区域内每个格子设置布尔型格式字段(bold/italic/underline/wrap_text 等)。
    /// set_cell_format 是整格覆盖,所以逐格"读整格→改一个字段→写回",
    /// 整个区域拼成一个脚本一次 execute_script,只产生一条 undo 记录。
    pub fn calc_apply_format_flag(&mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32, field: &str, value: bool) -> bool {
        self.calc_apply_format_field(sheet, r1, c1, r2, c2, field, serde_json::Value::Bool(value))
    }

    /// 字符串型格式字段(align_h/align_v 等)。value 为 None 表示清除该字段(写 null)。
    pub fn calc_apply_format_num(&mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32, field: &str, value: f64) -> bool {
        self.calc_apply_format_field(sheet, r1, c1, r2, c2, field, serde_json::json!(value))
    }
    pub fn calc_apply_format_str(&mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32, field: &str, value: Option<&str>) -> bool {
        let v = match value {
            Some(t) => serde_json::Value::String(t.to_string()),
            None => serde_json::Value::Null,
        };
        self.calc_apply_format_field(sheet, r1, c1, r2, c2, field, v)
    }

    /// 列宽自适应 [start_col, end_col)(右开区间,start_col < end_col)。
    pub fn calc_auto_fit_columns(&mut self, sheet: u32, start_col: u32, end_col: u32) -> bool {
        if self.handle == 0 { return false; }
        roe_auto_fit_columns(self.handle, sheet, start_col, end_col)
    }

    /// 边框预设写入:mode = top/bottom/left/right/none/all/outside/inside/inside_h/inside_v。
    /// 由 roe 的 apply_borders 完成(保留其余格式字段、同步邻格对边),一个脚本一条 undo。
    /// color 为空串表示黑色。mode/style/color 由调用方保证不含引号。
    pub fn calc_apply_borders(&mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32, mode: &str, style: &str, color: &str) -> bool {
        let script = format!(
            "apply_borders({}, {}, {}, {}, {}, \"{}\", \"{}\", \"{}\");",
            sheet, r1, c1, r2, c2, mode, style, color
        );
        let result = self.execute_script(&script);
        result.contains("\"ok\":true")
    }

    /// 条件格式:给选区加一条规则。kind_json 为 roe ConditionalFormatKind 的 serde JSON
    /// (如 {"DataBar":{...}}),不含反引号。一个脚本一条 undo(快照是整个文档)。
    pub fn calc_add_cf_rule(&mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32, kind_json: &str) -> bool {
        let range = format!(r#"{{"sheet":"","start":[{},{}],"end":[{},{}]}}"#, r1, c1, r2, c2);
        let script = format!("add_cf_rule({}, `{}`, `{}`, false);", sheet, range, kind_json);
        self.execute_script(&script).contains("\"ok\":true")
    }

    /// 条件格式(可视化类:数据条/色阶/图标集):先清除选区内相交的可视化规则,
    /// 再加一条新规则。同一脚本、一条 undo;不影响突出显示类规则。
    pub fn calc_replace_cf_visual(&mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32, kind_json: &str) -> bool {
        let range = format!(r#"{{"sheet":"","start":[{},{}],"end":[{},{}]}}"#, r1, c1, r2, c2);
        let script = format!(
            "clear_cf_visual({0}, `{1}`);\nadd_cf_rule({0}, `{1}`, `{2}`, false);",
            sheet, range, kind_json
        );
        self.execute_script(&script).contains("\"ok\":true")
    }

    /// 迷你图:按"数据范围 + 位置范围"新建一组(类型 line/column/stacked)。一条 undo。
    pub fn calc_add_sparkline_group(&mut self, sheet: u32, kind: &str, data: (u32, u32, u32, u32), loc: (u32, u32, u32, u32)) -> bool {
        let rj = |r: (u32, u32, u32, u32)| format!(r#"{{"sheet":"","start":[{},{}],"end":[{},{}]}}"#, r.0, r.1, r.2, r.3);
        let script = format!("add_sparkline_group({}, `{}`, `{}`, `{}`);", sheet, kind, rj(data), rj(loc));
        self.execute_script(&script).contains("\"ok\":true")
    }

    /// 迷你图:清除落在选区内的迷你图(按单元格,组空整组删)。
    pub fn calc_clear_sparklines(&mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32) -> bool {
        let range = format!(r#"{{"sheet":"","start":[{},{}],"end":[{},{}]}}"#, r1, c1, r2, c2);
        let script = format!("clear_sparklines({}, `{}`);", sheet, range);
        self.execute_script(&script).contains("\"ok\":true")
    }

    /// 条件格式:删除与选区相交的全部规则(相交即整条删,不拆分)。
    pub fn calc_clear_cf(&mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32) -> bool {
        let range = format!(r#"{{"sheet":"","start":[{},{}],"end":[{},{}]}}"#, r1, c1, r2, c2);
        let script = format!("clear_conditional_formats({}, `{}`);", sheet, range);
        self.execute_script(&script).contains("\"ok\":true")
    }

    /// 一次写入多个格式字段(单元格样式用)。fields 是 CellFormat 字段名到值的 JSON 对象,
    /// 给出的字段覆盖,未给出的保留原值。border 为 Some((mode, style, color)) 时在同一脚本末尾
    /// 追加 apply_borders。整区一个脚本、一条 undo。
    /// 格式刷:把源格完整 CellFormat(JSON,含边框)整体替换写入区域内每个格。
    /// set_cell_format 是整体替换,所以目标格原有格式不会残留。一次 execute_script = 一条 undo。
    pub fn calc_paint_format(
        &mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32,
        fmt: &serde_json::Value,
    ) -> bool {
        let escaped = fmt.to_string().replace('\\', "\\\\").replace('"', "\\\"");
        let mut script = String::new();
        for r in r1..=r2 {
            for c in c1..=c2 {
                script.push_str(&format!("set_cell_format({}, {}, {}, \"{}\");\n", sheet, r, c, escaped));
            }
        }
        self.execute_script(&script).contains("\"ok\":true")
    }

    pub fn calc_apply_format_multi(
        &mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32,
        fields: &serde_json::Map<String, serde_json::Value>,
        borders: &[(&str, &str, &str)],
    ) -> bool {
        let mut script = String::new();
        for r in r1..=r2 {
            for c in c1..=c2 {
                let mut fmt = self.calc_get_cell_format(sheet, r, c);
                if let Some(obj) = fmt.as_object_mut() {
                    for (k, v) in fields {
                        obj.insert(k.clone(), v.clone());
                    }
                }
                let escaped = fmt.to_string().replace('\\', "\\\\").replace('"', "\\\"");
                script.push_str(&format!("set_cell_format({}, {}, {}, \"{}\");\n", sheet, r, c, escaped));
            }
        }
        for (mode, style, color) in borders.iter().copied() {
            script.push_str(&format!(
                "apply_borders({}, {}, {}, {}, {}, \"{}\", \"{}\", \"{}\");\n",
                sheet, r1, c1, r2, c2, mode, style, color
            ));
        }
        self.execute_script(&script).contains("\"ok\":true")
    }

    /// 单元格样式:一次写 fg/bg/bold/italic(None=清除),可选边框。一条 undo。
    pub fn calc_apply_style_fields(
        &mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32,
        fg: Option<&str>, bg: Option<&str>, bold: bool, italic: bool,
        size: Option<f32>, borders: &[(&str, &str, &str)],
    ) -> bool {
        let opt = |v: Option<&str>| match v {
            Some(t) => serde_json::Value::String(t.to_string()),
            None => serde_json::Value::Null,
        };
        let mut m = serde_json::Map::new();
        m.insert("fg_color".into(), opt(fg));
        m.insert("bg_color".into(), opt(bg));
        m.insert("bold".into(), serde_json::Value::Bool(bold));
        m.insert("italic".into(), serde_json::Value::Bool(italic));
        m.insert("font_size".into(), match size { Some(z) => serde_json::json!(z), None => serde_json::Value::Null });
        self.calc_apply_format_multi(sheet, r1, c1, r2, c2, &m, borders)
    }

    /// 通用实现:逐格"读整格→改一个字段→写回",整区一个脚本、一条 undo。
    fn calc_apply_format_field(&mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32, field: &str, value: serde_json::Value) -> bool {
        let mut script = String::new();
        for r in r1..=r2 {
            for c in c1..=c2 {
                let mut fmt = self.calc_get_cell_format(sheet, r, c);
                if let Some(obj) = fmt.as_object_mut() {
                    obj.insert(field.to_string(), value.clone());
                }
                let json = fmt.to_string();
                // Rhai 字符串字面量转义：反斜杠与双引号
                let escaped = json.replace('\\', "\\\\").replace('"', "\\\"");
                script.push_str(&format!("set_cell_format({}, {}, {}, \"{}\");\n", sheet, r, c, escaped));
            }
        }
        let result = self.execute_script(&script);
        result.contains("\"ok\":true")
    }

    /// 读取指定 sheet 的全部表格(get_tables 的 JSON 数组项)。读不到返回空 Vec。
    /// 每项含 idx/name/range:[r1,c1,r2,c2]/style/has_header/... 兼容 value 为数组或字符串两种形态。
    pub fn calc_get_tables(&self, sheet: u32) -> Vec<serde_json::Value> {
        let s = self.query_script(&format!("get_tables({})", sheet));
        let v: serde_json::Value = serde_json::from_str(&s).unwrap_or_default();
        if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
            return Vec::new();
        }
        match v.get("value") {
            Some(serde_json::Value::Array(a)) => a.clone(),
            Some(serde_json::Value::String(t)) => serde_json::from_str::<Vec<serde_json::Value>>(t).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// 套用表格样式:选区与已有表格相交 -> 只改该表样式(set_table_style);
    /// 否则把选区建成新表(create_table,多行带表头、单行无表头)。
    /// style_name 为空串表示清除样式。一个脚本一次 execute_script(沿用 calc_apply_* 的 undo 约定)。
    /// 成功返回 Ok(表名或"ok"),失败返回 Err(原因)。
    pub fn calc_apply_table_style(
        &mut self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32, style_name: &str,
    ) -> Result<String, String> {
        let esc = style_name.replace('\\', "\\\\").replace('"', "\\\"");
        let hit = self.calc_get_tables(sheet).into_iter().find(|t| {
            let Some(r) = t.get("range").and_then(|x| x.as_array()) else { return false };
            let g = |i: usize| r.get(i).and_then(|x| x.as_u64()).unwrap_or(0) as u32;
            g(0) <= r2 && g(2) >= r1 && g(1) <= c2 && g(3) >= c1
        });
        let script = match &hit {
            Some(t) => {
                let idx = t.get("idx").and_then(|x| x.as_u64()).unwrap_or(0);
                format!("set_table_style({}, {}, \"{}\")", sheet, idx, esc)
            }
            None => {
                if style_name.is_empty() {
                    return Err("选区不在表格内,无需清除".to_string());
                }
                format!("create_table({}, {}, {}, {}, {}, {}, \"{}\")", sheet, r1, c1, r2, c2, r2 > r1, esc)
            }
        };
        let raw = self.execute_script(&script);
        let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| format!("解析脚本结果失败: {e}"))?;
        if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
            let e = v.get("error").and_then(|x| x.as_str()).unwrap_or("脚本执行失败");
            return Err(e.to_string());
        }
        let rv = match v.get("return_value") {
            Some(serde_json::Value::String(t)) => t.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        };
        let rv = rv.trim_matches('"').to_string();
        if let Some(e) = rv.strip_prefix("err:") {
            return Err(e.to_string());
        }
        Ok(rv)
    }

    /// 读取指定 sheet 的全部合并区域,每项为 (r1, c1, r2, c2)(0-based,闭区间)。
    /// get_merged_cells 返回 JSON 数组字符串,经 query_script 包装后可能已展开为
    /// 数组,也可能作为字符串嵌在 value 里,两种形态都兼容。读不到返回空 Vec。
    pub fn calc_get_merged_cells(&mut self, sheet: u32) -> Vec<(u32, u32, u32, u32)> {
        let script = format!("get_merged_cells({})", sheet);
        let s = self.query_script(&script);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap_or_default();
        if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
            return Vec::new();
        }
        let arr = match v.get("value") {
            Some(serde_json::Value::Array(a)) => a.clone(),
            Some(serde_json::Value::String(t)) => {
                serde_json::from_str::<Vec<serde_json::Value>>(t).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        arr.iter()
            .filter_map(|m| {
                let g = |k: &str| m.get(k).and_then(|x| x.as_u64()).map(|x| x as u32);
                Some((g("r1")?, g("c1")?, g("r2")?, g("c2")?))
            })
            .collect()
    }

    pub fn calc_get_cell_formula(&mut self, sheet: u32, row: u32, col: u32) -> String {
        let script = format!("get_cell_formula({}, {}, {})", sheet, row, col);
        let s = self.query_script(&script);
        if s.contains("\"ok\":true") {
            let v: serde_json::Value = serde_json::from_str(&s).unwrap_or_default();
            let f = v.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string();
            // roe 的 Formula.source 带前导"="(xlsx 导入即如此),前台调用方统一再拼一个"=",
            // 这里去掉一个前导"=",避免公式栏显示 "==A4"。
            f.strip_prefix('=').unwrap_or(&f).to_string()
        } else {
            String::new()
        }
    }

    /// 清空 Calc 单元格内容（Delete/Backspace），真正置为 Empty，
    /// 区别于 calc_set_cell 传空字符串会存成 CellValue::Text("")。
    pub fn calc_clear_cell(&mut self, sheet: u32, row: u32, col: u32) -> bool {
        let script = format!("clear_cell({}, {}, {})", sheet, row, col);
        let result = self.execute_script(&script);
        result.contains("\"ok\":true")
    }

    /// 写入 Calc 单元格值，返回是否成功。
    pub fn calc_set_cell(&mut self, sheet: u32, row: u32, col: u32, value: &str) -> bool {
        // 2026-07-10发现：此前无论输入什么都调set_cell，该Rhai函数只识别
        // 数字/布尔/文本三种类型（roe-script/calc_api.rs），"="开头的公式
        // 字符串会被当成纯文本存入单元格，从不触发计算——公式功能此前
        // 完全无效。这里补上分流："="开头调set_formula（ROE_Formula_Engine_
        // Interface_v1.md写明公式字符串内部约定不带前导"="，故去除后再传），
        // 否则维持原有set_cell路径不变。
        // fix: 此前只转义了双引号，没转义反斜杠本身，导致输入以 ' 开头再跟 \\
        // 时(如 '\\ , '\\\\\\ )反斜杠数量为奇数，拼出的Rhai脚本字符串里
        // \\" 被解析成"转义后的引号"而非"字符串结束"，脚本语法错误，
        // set_cell返回ok=false。bug记录见roe-ext_progress_v211起。
        // 必须先转义反斜杠再转义双引号，顺序不能反。
        fn escape_rhai_string(s: &str) -> String {
            s.replace('\\', "\\\\").replace('"', "\\\"")
        }
        let script = if let Some(formula_body) = value.strip_prefix('=') {
            let escaped = escape_rhai_string(formula_body);
            format!("set_formula({}, {}, {}, \"{}\")", sheet, row, col, escaped)
        } else {
            let escaped = escape_rhai_string(value);
            format!("set_cell({}, {}, {}, \"{}\")", sheet, row, col, escaped)
        };
        let result = self.execute_script(&script);
        result.contains("\"ok\":true")
    }

    // ================= 图表操作（ROE_Chart_API_v1，2026-07-07） =================
    // 均以 (sheet_idx, chart_id) 定位，不用数组下标。chart_id 来自 insert_chart 返回值。

    /// 列出指定 sheet 的全部浮动图表(底到顶叠放序;统计图追加在普通图之后)。
    /// JSON 数组 [{id,kind,x_pt,y_pt,width_pt,height_pt}],统计图额外带 "stat":true。
    /// 无图表返回 "[]";失败返回 None。统计图暂不支持 get_chart_config/set_chart_style。
    pub fn list_charts(&mut self, sheet_idx: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        unsafe { take_roe_string(roe_list_charts(self.handle, sheet_idx)) }
    }

    /// 在 Calc 指定 sheet 插入统计图(chartEx)。stat_json 为完整 StatChartModel JSON;
    /// x/y/w/h 单位 pt。成功返回新图 id,失败返回 None(详情见 last_error_message)。
    pub fn insert_calc_stat_chart(&mut self, sheet_idx: u32, stat_json: &str, x_pt: f32, y_pt: f32, w_pt: f32, h_pt: f32) -> Option<u64> {
        if self.handle == 0 { return None; }
        let cs = std::ffi::CString::new(stat_json).ok()?;
        let id = roe_calc_insert_stat_chart(self.handle, sheet_idx, cs.as_ptr(), x_pt, y_pt, w_pt, h_pt);
        if id > 0 { Some(id as u64) } else { None }
    }

    /// 读取 Calc 矩形区域(闭区间)的 JSON:{"rows":[[{row,col,value|cached_value|...}]]}。失败返回 None。
    pub fn get_range_json(&mut self, sheet_idx: u32, r1: u32, c1: u32, r2: u32, c2: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        unsafe { take_roe_string(roe_get_range_json(self.handle, sheet_idx, r1, c1, r2, c2, false)) }
    }

    /// 查询图表位置/尺寸。JSON {"x_pt":..,"y_pt":..,"width_pt":..,"height_pt":..}。
    /// 失败（句柄无效/chart_id不存在等）返回 None，详情在 last_error_message 里。
    pub fn get_chart_rect(&mut self, sheet_idx: u32, chart_id: u64) -> Option<String> {
        if self.handle == 0 { return None; }
        unsafe { take_roe_string(roe_get_chart_rect(self.handle, sheet_idx, chart_id)) }
    }

    /// 查询图表完整配置（kind/title/rect/legend/grid/轴标题/边框），供侧边栏面板初始化用。
    /// 失败返回 None。
    pub fn get_chart_config(&mut self, sheet_idx: u32, chart_id: u64) -> Option<String> {
        if self.handle == 0 { return None; }
        unsafe { take_roe_string(roe_get_chart_config(self.handle, sheet_idx, chart_id)) }
    }

    /// 返回公式函数列表（JSON数组字符串，name/args/desc/category字段）。
    /// 2026-07-10提需求时ROE仅硬编码34个常用函数；ROE已完成升级，现从
    /// formualizer-eval 0.5.9 builtins 自动生成，共376个内置函数全量注册表
    /// （见ROE roe-api/src/formula_function_list.rs）。前端自动补全据此为
    /// 完整清单，不再是"常用函数"级别的简化版。不依赖handle（纯静态列表，
    /// 与具体文档无关）。失败返回 None。
    pub fn get_formula_function_list(&mut self) -> Option<String> {
        unsafe { take_roe_string(roe_get_formula_function_list()) }
    }

    /// 修改图表尺寸（绝对值，非增量）。成功返回 true。
    pub fn resize_chart(&mut self, sheet_idx: u32, chart_id: u64, width_pt: f32, height_pt: f32) -> bool {
        if self.handle == 0 { return false; }
        roe_resize_chart(self.handle, sheet_idx, chart_id, width_pt, height_pt)
    }

    /// 修改图表位置（绝对值，非增量）。成功返回 true。
    pub fn move_chart(&mut self, sheet_idx: u32, chart_id: u64, x_pt: f32, y_pt: f32) -> bool {
        if self.handle == 0 { return false; }
        roe_move_chart(self.handle, sheet_idx, chart_id, x_pt, y_pt)
    }

    /// 原子设置图表完整矩形（位置+尺寸，绝对值，非增量）。成功返回 true。
    /// 2026-07-10：替代此前 resize_chart + move_chart 两次调用的workaround
    /// （见本函数调用方 main.rs chart-drag-ended 的历史注释），ROE已提供
    /// 原子接口一次性设置 x/y/w/h 四个字段，避免两次调用间的状态不一致。
    pub fn set_chart_rect(&mut self, sheet_idx: u32, chart_id: u64, x_pt: f32, y_pt: f32, width_pt: f32, height_pt: f32) -> bool {
        if self.handle == 0 { return false; }
        roe_set_chart_rect(self.handle, sheet_idx, chart_id, x_pt, y_pt, width_pt, height_pt)
    }

    /// 局部 patch 修改图表格式字段（show_legend/show_grid/x_axis_label/y_axis_label/
    /// border_color/border_width，均可选，未提供的字段保持原值）。成功返回 true。
    pub fn set_chart_style(&mut self, sheet_idx: u32, chart_id: u64, style_json: &str) -> bool {
        if self.handle == 0 { return false; }
        let Ok(cstr) = std::ffi::CString::new(style_json) else { return false; };
        roe_set_chart_style(self.handle, sheet_idx, chart_id, cstr.as_ptr())
    }

    /// 删除图表。成功返回 true，chart_id 不存在（含重复删除）返回 false。
    pub fn delete_chart(&mut self, sheet_idx: u32, chart_id: u64) -> bool {
        if self.handle == 0 { return false; }
        roe_delete_chart(self.handle, sheet_idx, chart_id)
    }

    /// 列出指定 sheet 的全部浮动对象(图表、统计图、图片、图形),JSON 数组,
    /// 每项含 id/kind/x_pt/y_pt/width_pt/height_pt/hidden。失败返回 None。
    pub fn list_objects(&mut self, sheet_idx: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        unsafe { take_roe_string(roe_calc_list_objects(self.handle, sheet_idx)) }
    }

    /// 设置任意浮动对象的矩形(绝对值,pt)。成功返回 true。
    pub fn set_object_rect(&mut self, sheet_idx: u32, object_id: u64, x_pt: f32, y_pt: f32, width_pt: f32, height_pt: f32) -> bool {
        if self.handle == 0 { return false; }
        roe_calc_set_object_rect(self.handle, sheet_idx, object_id, x_pt, y_pt, width_pt, height_pt)
    }

    /// 删除任意浮动对象。成功返回 true,id 不存在返回 false。
    pub fn delete_object(&mut self, sheet_idx: u32, object_id: u64) -> bool {
        if self.handle == 0 { return false; }
        roe_calc_delete_object(self.handle, sheet_idx, object_id)
    }

    /// 整体替换图形/文本框文字(\n 分段)。
    pub fn set_object_text(&mut self, sheet_idx: u32, object_id: u64, text: &str) -> bool {
        if self.handle == 0 { return false; }
        let c = match std::ffi::CString::new(text) { Ok(c) => c, Err(_) => return false };
        roe_calc_set_object_text(self.handle, sheet_idx, object_id, c.as_ptr())
    }

    /// 设置图形/文本框填充。fill 为 #rrggbb;空串或 none 表示无填充。
    pub fn set_object_fill(&mut self, sheet_idx: u32, object_id: u64, fill: &str) -> bool {
        if self.handle == 0 { return false; }
        let c = match std::ffi::CString::new(fill) { Ok(c) => c, Err(_) => return false };
        roe_calc_set_object_fill(self.handle, sheet_idx, object_id, c.as_ptr())
    }

    /// 设置图形/文本框边框。color 为 #rrggbb;空串或 none 表示无边框;width_pt 范围 (0,100]。
    pub fn set_object_stroke(&mut self, sheet_idx: u32, object_id: u64, color: &str, width_pt: f32) -> bool {
        if self.handle == 0 { return false; }
        let c = match std::ffi::CString::new(color) { Ok(c) => c, Err(_) => return false };
        roe_calc_set_object_stroke(self.handle, sheet_idx, object_id, c.as_ptr(), width_pt)
    }

    /// 插入浮动图片。source 为本地路径或 base64:/data:image/ 开头;w/h 传 0 按原比例。成功返回新对象 id。
    pub fn insert_image(&mut self, sheet_idx: u32, source: &str, x_pt: f32, y_pt: f32, w_pt: f32, h_pt: f32) -> Option<u64> {
        if self.handle == 0 { return None; }
        let cs = std::ffi::CString::new(source).ok()?;
        let id = roe_calc_insert_image(self.handle, sheet_idx, cs.as_ptr(), x_pt, y_pt, w_pt, h_pt);
        if id > 0 { Some(id as u64) } else { None }
    }

    /// 插入浮动图形。geometry 为 rect/ellipse/line 或 OOXML 预设名;fill 为 #rrggbb 或空串。
    pub fn insert_shape(&mut self, sheet_idx: u32, geometry: &str, x_pt: f32, y_pt: f32, w_pt: f32, h_pt: f32, fill: &str) -> Option<u64> {
        if self.handle == 0 { return None; }
        let g = std::ffi::CString::new(geometry).ok()?;
        let f = std::ffi::CString::new(fill).ok()?;
        let id = roe_calc_insert_shape(self.handle, sheet_idx, g.as_ptr(), x_pt, y_pt, w_pt, h_pt, f.as_ptr());
        if id > 0 { Some(id as u64) } else { None }
    }

    /// 插入文本框(无填充矩形,\n 分段)。成功返回新对象 id。
    pub fn insert_textbox(&mut self, sheet_idx: u32, x_pt: f32, y_pt: f32, w_pt: f32, h_pt: f32, text: &str) -> Option<u64> {
        if self.handle == 0 { return None; }
        let t = std::ffi::CString::new(text).ok()?;
        let id = roe_calc_insert_textbox(self.handle, sheet_idx, x_pt, y_pt, w_pt, h_pt, t.as_ptr());
        if id > 0 { Some(id as u64) } else { None }
    }


    /// 复制一块矩形单元格区域（含格式）到剪贴板。坐标为闭区间 [r1,r2]x[c1,c2]。
    /// 直接走 roe-api，不经 Rhai（roe-script 尚未注册 copy/paste，走脚本层无意义）。
    pub fn calc_copy_range(&mut self, sheet_idx: u32, r1: u32, c1: u32, r2: u32, c2: u32) -> bool {
        if self.handle == 0 { return false; }
        roe_copy_calc_range(self.handle, sheet_idx, r1, c1, r2, c2) == 0
    }

    /// 把剪贴板中的 Calc 内容粘贴到指定位置（左上角 dest_row, dest_col）。
    /// 会覆盖目标矩形区域内现有内容，与主流表格软件行为一致。
    pub fn calc_paste_range(&mut self, sheet_idx: u32, dest_row: u32, dest_col: u32) -> bool {
        if self.handle == 0 { return false; }
        roe_paste_calc_range(self.handle, sheet_idx, dest_row, dest_col) == 0
    }

    /// 执行一段 Rhai 脚本，成功返回 true（多条写入在同一脚本里只产生一条撤销记录）。
    pub fn calc_run_script(&mut self, script: &str) -> bool {
        self.execute_script(script).contains("\"ok\":true")
    }

    /// 查询剪贴板是否有内容，前端用来决定"粘贴"操作是否可执行。
    pub fn calc_clipboard_has_content(&self) -> bool {
        roe_clipboard_has_content() != 0
    }

}

// 独立函数：与文档内核无关的工具函数
pub fn get_file_size(path: &str) -> i64 {
    std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(-1)
}

/// 读取文件原始字节，供 open_document_from_bytes 使用。
/// 失败（文件不存在/无权限等）返回空 Vec，调用方需自行判断长度是否为0。
pub fn read_file_bytes(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}

pub fn load_ai_config() -> String {
    "{}".to_string()
}

pub fn save_ai_config(json: &str) -> String {
    let _ = json;
    String::new()
}

/// 从形如 "...expected 14272944 bytes (1588x2247x4), got 14266592" 的错误信息里
/// 解析出 ROE 内部真正期望的 (width, height)。解析失败返回 None。
/// 临时绕过方案的一部分，见 render_page 内的注释，待ROE侧口径确认后整体移除。
fn parse_expected_size_from_error(msg: &str) -> Option<(u32, u32)> {
    let start = msg.find('(')?;
    let end = msg.find(')')?;
    if end <= start {
        return None;
    }
    let inner = &msg[start + 1..end]; // "1588x2247x4"
    let parts: Vec<&str> = inner.split('x').collect();
    if parts.len() != 3 {
        return None;
    }
    let w: u32 = parts[0].parse().ok()?;
    let h: u32 = parts[1].parse().ok()?;
    Some((w, h))
}

// ================= v261: Calc 区域探测与批量读（图表真实数据用） =================

/// 取 query_script 返回里的 value,兼容"已展开的对象/数组"和"字符串化 JSON"两种形态。
fn query_value(raw: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    if v.get("ok").and_then(|x| x.as_bool()) != Some(true) { return None; }
    match v.get("value")? {
        serde_json::Value::String(t) => serde_json::from_str(t).ok(),
        other => Some(other.clone()),
    }
}

/// 解析 detect_current_region 的返回。无数据(direction=none)或格式不对返回 None。
pub fn parse_region_json(raw: &str) -> Option<(u32, u32, u32, u32)> {
    let v = query_value(raw)?;
    let st = v.get("start")?.as_array()?;
    let en = v.get("end")?.as_array()?;
    let g = |a: &Vec<serde_json::Value>, i: usize| a.get(i).and_then(|x| x.as_u64()).map(|x| x as u32);
    Some((g(st, 0)?, g(st, 1)?, g(en, 0)?, g(en, 1)?))
}

/// 解析 get_row_texts 的返回(字符串数组)。失败返回 None。
pub fn parse_row_texts_json(raw: &str) -> Option<Vec<String>> {
    let v = query_value(raw)?;
    let arr = v.as_array()?;
    Some(arr.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect())
}

impl TileCacheRust {
    /// 从 (row,col) 出发探测当前数据区(等价 Excel Ctrl+*)。返回 (r1,c1,r2,c2)。
    pub fn calc_current_region(&self, sheet: u32, row: u32, col: u32) -> Option<(u32, u32, u32, u32)> {
        let raw = self.query_script(&format!("detect_current_region({}, {}, {})", sheet, row, col));
        parse_region_json(&raw)
    }

    /// 批量读区域的显示文本(行优先)。最多读 max_rows 行,避免大区域卡死;
    /// 某行读取失败时该行补空串,保证矩阵形状规整。
    pub fn calc_get_rows_text(&self, sheet: u32, r1: u32, c1: u32, r2: u32, c2: u32, max_rows: u32) -> Vec<Vec<String>> {
        if r2 < r1 || c2 < c1 { return Vec::new(); }
        let width = (c2 - c1 + 1) as usize;
        let last = r2.min(r1.saturating_add(max_rows.saturating_sub(1)));
        (r1..=last).map(|r| {
            let raw = self.query_script(&format!("get_row_texts({}, {}, {}, {})", sheet, r, c1, c2));
            let mut row = parse_row_texts_json(&raw).unwrap_or_default();
            row.resize(width, String::new());
            row
        }).collect()
    }
}

// ================= Calc 多Sheet管理（v49新增） =================

impl TileCacheRust {
    /// 返回当前文档的Sheet数量。失败返回-1。
    pub fn get_sheet_count(&self) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_get_sheet_count(self.handle)
    }

    /// 返回指定Sheet的名称。失败返回空字符串。
    pub fn get_sheet_name(&self, sheet_idx: u32) -> String {
        if self.handle == 0 { return String::new(); }
        let ptr = roe_api::roe_get_sheet_name(self.handle, sheet_idx);
        if ptr.is_null() { return String::new(); }
        let json = unsafe { take_roe_string(ptr) }.unwrap_or_default();
        // json形如 {"name":"Sheet1"}，简单提取，避免引入serde_json依赖
        json.split("\"name\":\"").nth(1)
            .and_then(|s| s.split('"').next())
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    /// 对 sheet_idx 执行数据透视表计算，结果作为新 sheet 追加到 workbook 末尾。
    /// config_json 格式：{"rows":[...],"cols":[...],"values":[...],"agg":"sum","filters":{...}}
    /// 对应 ROE v85 生产路径 pivot_sheet_into_workbook_v2（树状表头，非旧版压平字符串表头）。
    /// 成功返回新 sheet 的准确索引；失败返回 None，详情可用 roe_last_error_message 查询。
    /// 注：这是纯 C-ABI 直调，不经过 Rhai/execute_script，不依赖"sheet_count-1"这类猜测式索引推导。
    pub fn pivot_sheet(&mut self, sheet_idx: u32, config_json: &str, result_sheet_name: &str) -> Option<u32> {
        if self.handle == 0 { return None; }
        let c_config = std::ffi::CString::new(config_json).ok()?;
        let c_name = std::ffi::CString::new(result_sheet_name).ok()?;
        let idx = roe_api::roe_pivot_sheet(self.handle, sheet_idx, c_config.as_ptr(), c_name.as_ptr());
        if idx == u32::MAX { None } else { Some(idx) }
    }

    /// 取 sheet_idx 中 field 列的唯一值列表，供透视表筛选字段的下拉候选使用。
    /// 返回值形如 ["华东","华南","华北"]；失败或无数据返回空 Vec。
    /// 现状（v63）：筛选下拉菜单UI尚未排期开发，此方法先行接入备用——
    /// 在下拉菜单UI就绪前，筛选字段的语义等同于"已选中全部候选值"（即不过滤）。
    /// 待下拉菜单UI开发时，可直接调用本方法获取候选列表用于渲染勾选项。
    pub fn pivot_unique_values(&self, sheet_idx: u32, field: &str) -> Vec<String> {
        if self.handle == 0 { return Vec::new(); }
        let Ok(c_field) = std::ffi::CString::new(field) else { return Vec::new(); };
        let ptr = roe_api::roe_get_pivot_unique_values(self.handle, sheet_idx, c_field.as_ptr());
        if ptr.is_null() { return Vec::new(); }
        let json = unsafe { take_roe_string(ptr) }.unwrap_or_default();
        // json形如 ["华东","华南"]，用serde_json解析（doc-core已依赖serde_json，无需新增依赖）
        serde_json::from_str::<Vec<String>>(&json).unwrap_or_default()
    }

    // ================= 透视表筛选/排序接线（roe d18355a / 3b50a63） =================

    /// sheet_idx 是否为透视结果表(读 roe 内部 sheet.pivot,不依赖前台创建流程状态)。
    pub fn is_pivot_sheet(&self, sheet_idx: u32) -> bool {
        self.handle != 0 && roe_api::roe_sheet_is_pivot(self.handle, sheet_idx) == 1
    }

    /// 取透视表字段按钮的单元格绝对坐标,返回 JSON 数组原文:
    /// [{"kind":"FilterField|RowField|ColField","field_name":..,"row":..,"col":..,"filtered":..}]
    /// 页字段 row 与 base_row 无关,col=base_col+1;像素矩形由前台按单元格矩形计算。
    /// 失败(非透视 sheet/无效句柄)返回 None。
    pub fn pivot_buttons_json(&self, sheet_idx: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_get_pivot_buttons(self.handle, sheet_idx);
        unsafe { take_roe_string(ptr) }
    }

    /// 设置透视表某字段(页/行/列字段)的筛选值并就地刷新;values 为空=清除该字段筛选。
    /// 不包撤销事务,调用方自行 begin_edit/commit_edit。失败返回 false。
    pub fn pivot_set_filter(&mut self, sheet_idx: u32, field: &str, values: &[String]) -> bool {
        if self.handle == 0 { return false; }
        let Ok(c_field) = std::ffi::CString::new(field) else { return false; };
        let Ok(vj) = serde_json::to_string(values) else { return false; };
        let Ok(c_vals) = std::ffi::CString::new(vj) else { return false; };
        roe_api::roe_pivot_set_filter(self.handle, sheet_idx, c_field.as_ptr(), c_vals.as_ptr()) == 0
    }

    /// 设置透视表某行/列字段的排序并就地刷新;asc=true 升序(默认),false 降序。
    /// 不包撤销事务,调用方自行 begin_edit/commit_edit。失败返回 false。
    pub fn pivot_set_sort(&mut self, sheet_idx: u32, field: &str, asc: bool) -> bool {
        if self.handle == 0 { return false; }
        let Ok(c_field) = std::ffi::CString::new(field) else { return false; };
        roe_api::roe_pivot_set_sort(self.handle, sheet_idx, c_field.as_ptr(), if asc { 1 } else { 0 }) == 0
    }

    /// 取透视表当前配置 JSON:
    /// {"rows":[..],"cols":[..],"values":[..],"filters":[..],"source_idx":N,"fields":[数据源列名]}
    /// 非透视 sheet/无效句柄返回 None。
    pub fn pivot_config_json(&self, sheet_idx: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_get_pivot_config(self.handle, sheet_idx);
        unsafe { take_roe_string(ptr) }
    }

    /// 就地整体设置透视表的行/列/值/页字段并刷新。不包撤销事务,调用方自行 begin_edit/commit_edit。
    pub fn pivot_set_config(
        &mut self,
        sheet_idx: u32,
        rows: &[String],
        cols: &[String],
        values: &[String],
        filters: &[String],
    ) -> bool {
        if self.handle == 0 { return false; }
        let j = serde_json::json!({ "rows": rows, "cols": cols, "values": values, "filters": filters }).to_string();
        let Ok(c) = std::ffi::CString::new(j) else { return false; };
        roe_api::roe_pivot_set_config(self.handle, sheet_idx, c.as_ptr()) == 0
    }

    // ================= Writer 光标/选区（v95新增） =================
    // 三个方法均为对 roe-api 新增C-ABI的直接转发，不在doc-core侧持有任何光标/选区
    // 状态——光标位置、选区锚点、ideal_x 等状态由前端 AppState 持有（与现有
    // writer_cursor_para/offset 保持同一模式），这里只是"查询/移动一次就返回结果"
    // 的无状态薄封装，与 get_sheet_count 等现有方法风格一致。
    //
    // container_type 目前固定传 0（BODY），表格cell/文本框（1/2）本次不支持，
    // 这三个方法直接暴露裸参数，不引入 RoeLocation 类型跨越 FFI 边界的封装成本
    // （JSON 解析已经在 roe 侧完成了结构化，这里只需要把 Rust 原生类型转成
    // JSON 字符串结果，供上层 main.rs 按需取字段）。

    /// 单行内光标移动（字符左/右、词左/右，不跨行）。
    /// command: 0=CharLeft 1=CharRight 2=WordLeft 3=WordRight（与 roe_api::CursorInlineCommand 对应）
    /// 返回：Some(JSON字符串)，形如 {"container_type":0,"container_id":0,"local_para_index":N,"char_offset":N}
    /// 失败（无效句柄/非法command）返回 None，详情可用 roe_last_error_message 查询。
    pub fn cursor_move_inline(
        &self,
        local_para_index: u32,
        char_offset: u32,
        command: u32,
    ) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_cursor_move_inline(
            self.handle, 0, 0, local_para_index, char_offset, command,
        );
        unsafe { take_roe_string(ptr) }
    }

    /// 跨行光标移动（上/下），需要前端持有并传入 ideal_x（见 final 文档五.2，
    /// doc-core/roe 均不存储 ideal_x，每次调用都由调用方显式传入）。
    /// direction: 0=Up 1=Down（与 roe_api::CursorLineDirection 对应）
    /// 返回：Some(JSON字符串)，形如 {"new_loc":{...},"actual_x":F}
    /// actual_x 仅供前端画光标用，不应回写覆盖调用方自己保存的 ideal_x。
    pub fn cursor_move_line(
        &self,
        local_para_index: u32,
        char_offset: u32,
        direction: u32,
        ideal_x: f32,
    ) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_cursor_move_line(
            self.handle, 0, 0, local_para_index, char_offset, direction, ideal_x,
        );
        unsafe { take_roe_string(ptr) }
    }

    /// 查询选区几何（按行盒拼装的矩形数组），供前端画选区高亮。
    /// 起止点顺序任意，roe 侧会自动归一化（若 start 在 end 之后会自动交换）。
    /// 返回：Some(JSON字符串)，形如 [{"page_idx":N,"x":..,"y":..,"w":..,"h":..}, ...]
    /// 空选区（start==end）返回 Some("[]")，不是 None。
    pub fn query_selection_rects(
        &self,
        start_para: u32,
        start_offset: u32,
        end_para: u32,
        end_offset: u32,
    ) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_query_selection_rects(
            self.handle, 0, 0, start_para, start_offset, 0, 0, end_para, end_offset,
        );
        unsafe { take_roe_string(ptr) }
    }

    /// 查询指定段落的文字长度 + 文档正文总段落数（v99新增，供跨段落
    /// 光标跳转使用：段首按左键需要知道"上一段有多长"，段尾按右键需要
    /// 知道"是否已是最后一段"）。
    /// 返回：Some(JSON字符串)，形如 {"para_len":N,"total_para_count":N}
    /// 失败（无效句柄/越界/非法参数）返回 None。
    pub fn get_paragraph_info(&self, local_para_index: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_get_paragraph_info(
            self.handle, 0, 0, local_para_index, 0,
        );
        unsafe { take_roe_string(ptr) }
    }

    /// v231: 设置字符格式。kind = bold|italic|underline；value 0=取消 1=设置 2=清除覆盖
    pub fn writer_set_char_format(&self, kind: &str, para: u32, start: u32, end: u32, value: u32) -> Result<(), String> {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { let k = match kind { "bold" => 0, "italic" => 1, "underline" => 2, _ => return Err(format!("未知格式 {}", kind)) }; return self.cell_fmt_char(cid, cp, 0, u32::MAX, k, value, 0); }
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = match kind {
            "bold" => roe_api::roe_set_range_char_format_bold(self.handle, 0, 0, para, start, end, value),
            "italic" => roe_api::roe_set_range_char_format_italic(self.handle, 0, 0, para, start, end, value),
            "underline" => roe_api::roe_set_range_char_format_underline(self.handle, 0, 0, para, start, end, value),
            _ => return Err(format!("未知格式 {}", kind)),
        };
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v231: 查询字符格式。返回 0=全否 1=全是 2=混合 -1=出错
    pub fn writer_get_char_format(&self, kind: &str, para: u32, start: u32, end: u32) -> i32 {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { let k = match kind { "bold" => 0, "italic" => 1, "underline" => 2, _ => return -1 }; return self.cell_fmt_get(cid, cp, k); }
        if self.handle == 0 { return -1; }
        match kind {
            "bold" => roe_api::roe_get_range_char_format_bold(self.handle, 0, 0, para, start, end),
            "italic" => roe_api::roe_get_range_char_format_italic(self.handle, 0, 0, para, start, end),
            "underline" => roe_api::roe_get_range_char_format_underline(self.handle, 0, 0, para, start, end),
            _ => -1,
        }
    }

    /// v234: 删除线 value 0/1/2
    pub fn writer_set_strike(&self, para: u32, start: u32, end: u32, value: u32) -> Result<(), String> {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { return self.cell_fmt_char(cid, cp, 0, u32::MAX, 3, value, 0); }
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_set_range_char_format_strikethrough(self.handle, 0, 0, para, start, end, value);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    pub fn writer_get_strike(&self, para: u32, start: u32, end: u32) -> i32 {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { return self.cell_fmt_get(cid, cp, 3); }
        if self.handle == 0 { return -1; }
        roe_api::roe_get_range_char_format_strikethrough(self.handle, 0, 0, para, start, end)
    }
    /// v234: 上下标 value 0=正常 1=上标 2=下标 3=清除
    pub fn writer_set_script(&self, para: u32, start: u32, end: u32, value: u32) -> Result<(), String> {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { return self.cell_fmt_char(cid, cp, 0, u32::MAX, 4, value, 0); }
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_set_range_char_format_script(self.handle, 0, 0, para, start, end, value);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    /// which 1=上标 2=下标
    pub fn writer_get_script(&self, para: u32, start: u32, end: u32, which: u32) -> i32 {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { return self.cell_fmt_get(cid, cp, if which == 1 { 4 } else { 5 }); }
        if self.handle == 0 { return -1; }
        roe_api::roe_get_range_char_format_script(self.handle, 0, 0, para, start, end, which)
    }
    /// v234: 段落对齐 0左 1中 2右 3两端
    pub fn writer_set_alignment(&self, para: u32, align: u32) -> Result<(), String> {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { return self.cell_fmt_para(cid, cp, 0, align as f32); }
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_set_paragraph_alignment(self.handle, para, align);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v275: 读取页面设置 JSON {"width","height","top","bottom","left","right"}（pt）
    pub fn writer_get_page_setup(&self) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_get_page_setup(self.handle);
        unsafe { take_roe_string(ptr) }
    }

    /// v275: 设置页面（pt）。对所有节统一生效
    pub fn writer_set_page_setup(&self, w: f32, h: f32, top: f32, bottom: f32, left: f32, right: f32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_set_page_setup(self.handle, w, h, top, bottom, left, right);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v278: 读取页眉页脚 JSON {"header","footer","mode","align","title_page","even_odd"}
    pub fn writer_get_header_footer(&self) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_get_header_footer(self.handle);
        unsafe { take_roe_string(ptr) }
    }

    /// v278: 设置页眉页脚。mode 0无 1=N 2=第N页 3=N/总页数；align 0左 1中 2右
    pub fn writer_set_header_footer(&self, header: &str, footer: &str, header_align: i32, footer_align: i32, num_pos: i32, num_mode: i32, num_align: i32, title_page: bool, even_odd: bool) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let h = std::ffi::CString::new(header).map_err(|_| "页眉含非法字符".to_string())?;
        let f = std::ffi::CString::new(footer).map_err(|_| "页脚含非法字符".to_string())?;
        let code = roe_api::roe_set_header_footer(self.handle, h.as_ptr(), f.as_ptr(), header_align, footer_align, num_pos, num_mode, num_align, if title_page { 1 } else { 0 }, if even_odd { 1 } else { 0 });
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v284: 在选区上插入批注，返回新批注 id
    pub fn writer_insert_comment(&self, pa: u32, oa: u32, pb: u32, ob: u32, author: &str, text: &str) -> Result<i32, String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let a = std::ffi::CString::new(author).map_err(|_| "作者含非法字符".to_string())?;
        let t = std::ffi::CString::new(text).map_err(|_| "批注含非法字符".to_string())?;
        let d = std::ffi::CString::new("").unwrap();
        let id = roe_api::roe_insert_comment(self.handle, pa, oa, pb, ob, a.as_ptr(), t.as_ptr(), d.as_ptr());
        if id >= 0 { Ok(id) } else { Err(Self::formula_last_err()) }
    }

    /// v295: 插入脚注/尾注。kind 0=脚注 1=尾注
    pub fn writer_insert_note(&self, para: u32, offset: u32, kind: u32, text: &str) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let t = std::ffi::CString::new(text).map_err(|_| "注释含非法字符".to_string())?;
        let code = roe_api::roe_insert_footnote(self.handle, para, offset, kind, t.as_ptr());
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v292: 添加回复
    pub fn writer_reply_comment(&self, parent_id: u32, author: &str, text: &str) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let a = std::ffi::CString::new(author).map_err(|_| "作者含非法字符".to_string())?;
        let t = std::ffi::CString::new(text).map_err(|_| "回复含非法字符".to_string())?;
        let code = roe_api::roe_reply_comment(self.handle, parent_id, a.as_ptr(), t.as_ptr());
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v291: 设置批注完成状态
    pub fn writer_set_comment_done(&self, id: u32, done: bool) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_set_comment_done(self.handle, id, if done { 1 } else { 0 });
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v291: 编辑批注正文
    pub fn writer_edit_comment(&self, id: u32, text: &str) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let t = std::ffi::CString::new(text).map_err(|_| "批注含非法字符".to_string())?;
        let code = roe_api::roe_edit_comment(self.handle, id, t.as_ptr());
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v289: 删除批注（连同回复）
    pub fn writer_delete_comment(&self, id: u32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_delete_comment(self.handle, id);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v298: 删除光标处脚注/尾注。kind 0=脚注 1=尾注
    pub fn writer_delete_note(&self, para: u32, offset: u32, kind: u32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_delete_note(self.handle, para, offset, kind);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    /// v298: 插入分栏符
    pub fn writer_insert_column_break(&self, para: u32, offset: u32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_writer_insert_column_break(self.handle, para, offset);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    /// v298: 设置分栏（栏数 1-6，栏间距 pt）
    pub fn writer_set_columns(&self, num: u32, spacing_pt: f32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_writer_set_columns(self.handle, num, spacing_pt);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    /// v298: 插入图片（宽高 pt）
    pub fn writer_insert_image(&self, para: u32, offset: u32, bytes: &[u8], mime: &str, w: f32, h: f32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let m = std::ffi::CString::new(mime).map_err(|_| "mime 含非法字符".to_string())?;
        let code = roe_api::roe_writer_insert_image(self.handle, para, offset, bytes.as_ptr(), bytes.len(), m.as_ptr(), w, h);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v318f: 插入内联图表（chart_json 为 ChartModel 或 StatChartModel 的 JSON，宽高 pt）
    pub fn writer_insert_chart(&self, para: u32, offset: u32, chart_json: &str, w: f32, h: f32, is_stat: bool) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let c = std::ffi::CString::new(chart_json).map_err(|_| "图表 JSON 含非法字符".to_string())?;
        let code = roe_api::roe_writer_insert_chart(self.handle, para, offset, c.as_ptr(), w, h, if is_stat { 1 } else { 0 });
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v318m: 读取内联图表 ChartModel JSON
    pub fn writer_get_chart_json(&self, body_index: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        unsafe { take_roe_string(roe_api::roe_writer_get_chart_json(self.handle, body_index)) }
    }
    /// v318m: 替换内联图表内容（保留尺寸，入撤销栈）
    pub fn writer_set_chart_json(&self, body_index: u32, chart_json: &str) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let c = std::ffi::CString::new(chart_json).map_err(|_| "图表 JSON 含非法字符".to_string())?;
        let code = roe_api::roe_writer_set_chart_json(self.handle, body_index, c.as_ptr());
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v299: 命中内联图片，返回 (body索引, x, y, w, h)，文档坐标 pt（y 未加画布偏移）
    pub fn writer_hit_object(&mut self, x_pt: f32, y_pt: f32) -> Option<(u32, f32, f32, f32, f32)> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_writer_hit_object(self.handle, x_pt as f64, y_pt as f64)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        let g = |k: &str| v.get(k).and_then(|x| x.as_f64());
        Some((g("body_index")? as u32, g("x")? as f32, g("y")? as f32, g("w")? as f32, g("h")? as f32))
    }
    /// v299: 改图片尺寸（pt）
    pub fn writer_resize_object(&self, body_index: u32, w: f32, h: f32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_writer_resize_object(self.handle, body_index, w, h);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    /// v299: 删除图片
    pub fn writer_delete_object(&self, body_index: u32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_writer_delete_object(self.handle, body_index);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v302: 在文档坐标(x,y)处插入形状，返回形状的 body 下标
    pub fn writer_insert_shape(&self, geometry: &str, x: f32, y: f32, w: f32, h: f32) -> Result<u32, String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let g = std::ffi::CString::new(geometry).map_err(|_| "形状名含非法字符".to_string())?;
        let code = roe_api::roe_writer_insert_shape(self.handle, g.as_ptr(), x, y, w, h);
        if code >= 0 { Ok(code as u32) } else { Err(Self::formula_last_err()) }
    }

    /// v309: 在 body[after] 之后插入 rows x cols 表格，返回表格 body 下标
    pub fn writer_insert_table(&self, after: u32, rows: u32, cols: u32) -> Result<u32, String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_writer_insert_table(self.handle, after, rows, cols);
        if code >= 0 { Ok(code as u32) } else { Err(Self::formula_last_err()) }
    }
    /// v308: 设置对象旋转角（度）
    pub fn writer_set_object_rotation(&self, body_index: u32, degrees: f32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_writer_set_object_rotation(self.handle, body_index, degrees);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    /// v308: 读取对象旋转角（度）
    pub fn writer_object_rotation(&self, body_index: u32) -> f32 {
        if self.handle == 0 { return 0.0; }
        roe_api::roe_writer_object_rotation(self.handle, body_index)
    }
    /// v305: 对象类型 0=其他 1=图片 2=形状
    pub fn writer_object_kind(&self, body_index: u32) -> i32 {
        if self.handle == 0 { return 0; }
        roe_api::roe_writer_object_kind(self.handle, body_index)
    }
    /// v305: 改形状样式。fill/stroke: ""不改 "none"无 "#rrggbb"；width<0 不改
    pub fn writer_set_shape_style(&self, body_index: u32, fill: &str, stroke: &str, width: f32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let f = std::ffi::CString::new(fill).map_err(|_| "参数含非法字符".to_string())?;
        let s = std::ffi::CString::new(stroke).map_err(|_| "参数含非法字符".to_string())?;
        let code = roe_api::roe_writer_set_shape_style(self.handle, body_index, f.as_ptr(), s.as_ptr(), width);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v273: 行距规则。mode 0=倍数 1=固定值pt 2=最小值pt
    pub fn writer_set_para_line_rule(&self, para: u32, mode: u32, value: f32) -> Result<(), String> {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { if mode != 0 { return Err("单元格内暂只支持行距倍数".to_string()); } return self.cell_fmt_para(cid, cp, 3, value); }
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_set_paragraph_line_rule(self.handle, para, mode, value);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v274: 段落排序。start..=end 为 body 下标；kind 0=文字 1=数字
    pub fn writer_sort_paragraphs(&self, start: u32, end: u32, kind: u32, descending: bool) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_sort_paragraphs(self.handle, start, end, kind, if descending { 1 } else { 0 });
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v272: 段落底纹。rgb=None 清除
    pub fn writer_set_para_shading(&self, para: u32, rgb: Option<u32>) -> Result<(), String> {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { return self.cell_fmt_para(cid, cp, 4, match rgb { Some(c) => c as f32, None => -1.0 }); }
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let (v, clear) = match rgb { Some(c) => (c, 0u32), None => (0u32, 1u32) };
        let code = roe_api::roe_set_paragraph_shading(self.handle, para, v, clear);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v263: 段落间距。kind 0=行距倍数 1=段前pt 2=段后pt；value<0 清除
    pub fn writer_set_para_spacing(&self, para: u32, kind: u32, value: f32) -> Result<(), String> {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { let k = match kind { 0 => 3, 1 => 1, _ => 2 }; return self.cell_fmt_para(cid, cp, k, value); }
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_set_paragraph_spacing(self.handle, para, kind, value);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v265: 显示/隐藏段落标记（只影响渲染）
    pub fn writer_set_show_marks(&self, on: bool) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let json = std::ffi::CString::new(format!("{{\"show_marks\":{}}}", on)).map_err(|e| e.to_string())?;
        let code = roe_api::roe_set_writer_options(self.handle, json.as_ptr());
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v242: 设置段落列表。kind 0=取消 1=项目符号 2=编号
    pub fn writer_set_list(&self, para: u32, kind: u32, variant: u32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_set_paragraph_list(self.handle, para, kind, variant);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v253: 增减缩进。列表项升降级(ROE_列表)，普通段落左缩进±36pt。delta=+1/-1
    pub fn writer_change_indent(&self, para: u32, delta: i32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_change_list_level(self.handle, para, delta);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v245: 查找。返回 (段, 起, 止, 第几处, 共几处)；None=未找到
    pub fn writer_find(&self, pattern: &str, case_sensitive: bool, whole_word: bool, use_regex: bool,
                       from_para: u32, from_offset: u32, forward: bool) -> Result<Option<(u32, u32, u32, u32, u32)>, String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let cp = std::ffi::CString::new(pattern).map_err(|e| e.to_string())?;
        let ptr = roe_api::roe_find_text(self.handle, cp.as_ptr(), case_sensitive, whole_word, use_regex, from_para, from_offset, forward);
        let Some(json) = (unsafe { take_roe_string(ptr) }) else { return Err(Self::formula_last_err()); };
        let v: serde_json::Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        if !v["found"].as_bool().unwrap_or(false) { return Ok(None); }
        let g = |k: &str| v[k].as_u64().unwrap_or(0) as u32;
        Ok(Some((g("para"), g("start"), g("end"), g("index"), g("total"))))
    }

    /// v245: 全部替换，返回替换次数
    pub fn writer_replace_all(&self, pattern: &str, replacement: &str, case_sensitive: bool, whole_word: bool, use_regex: bool) -> Result<i32, String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let cp = std::ffi::CString::new(pattern).map_err(|e| e.to_string())?;
        let cr = std::ffi::CString::new(replacement).map_err(|e| e.to_string())?;
        let code = roe_api::roe_replace_text_ex(self.handle, cp.as_ptr(), cr.as_ptr(), case_sensitive, whole_word, use_regex);
        if code >= 0 { Ok(code) } else { Err(Self::formula_last_err()) }
    }

    /// v244: 设置段落样式。0=正文 1-4=标题1-4 5=标题 6=副标题
    pub fn writer_set_style(&self, para: u32, kind: u32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_set_paragraph_style(self.handle, para, kind);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }

    /// v240: 撤销分组。begin=true 开始，false 结束
    pub fn writer_undo_group(&self, begin: bool) {
        if begin { roe_api::roe_undo_group_begin(); } else { roe_api::roe_undo_group_end(); }
    }

    // ───────── v247: Writer 字符级剪贴板（应用内）─────────
    pub fn writer_clip_copy(&self, pa: u32, oa: u32, pb: u32, ob: u32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_writer_copy(self.handle, pa, oa, pb, ob);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    pub fn writer_clip_delete(&self, pa: u32, oa: u32, pb: u32, ob: u32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_writer_delete_range(self.handle, pa, oa, pb, ob);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    pub fn writer_clip_paste(&self, para: u32, offset: u32) -> Result<(u32, u32), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let ptr = roe_api::roe_writer_paste(self.handle, para, offset);
        let json = unsafe { take_roe_string(ptr) }.ok_or_else(Self::formula_last_err)?;
        let v: serde_json::Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        let p = v.get("local_para_index").and_then(|x| x.as_u64()).ok_or("缺 local_para_index")? as u32;
        let o = v.get("char_offset").and_then(|x| x.as_u64()).ok_or("缺 char_offset")? as u32;
        Ok((p, o))
    }
    pub fn writer_clip_has(&self) -> bool {
        roe_api::roe_writer_clip_has() != 0
    }
    // v249: 格式刷
    pub fn writer_painter_pick(&self, para: u32, offset: u32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_writer_painter_pick(self.handle, para, offset);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    pub fn writer_painter_apply(&self, para: u32, start: u32, end: u32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_writer_painter_apply(self.handle, para, start, end);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    /// v267: 插入分页符。返回 true=已拆段（光标应到下一段开头）
    pub fn writer_insert_page_break(&self, para: u32, offset: u32) -> Result<bool, String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_writer_insert_page_break(self.handle, para, offset);
        if code >= 0 { Ok(code == 1) } else { Err(Self::formula_last_err()) }
    }
    /// v266: 更改大小写。mode 0句首 1小写 2大写 3每词首字母 4切换
    pub fn writer_change_case(&self, para: u32, start: u32, end: u32, mode: u32) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_writer_change_case(self.handle, para, start, end, mode);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    /// v239: 一次清除区间内全部字符内联格式
    pub fn writer_clear_char_format(&self, para: u32, start: u32, end: u32) -> Result<(), String> {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) {
            for &(k, a, b) in &[(0u32, 2u32, 0u32), (1, 2, 0), (2, 2, 0), (3, 2, 0), (4, 3, 0), (5, 0, 0), (6, 0, 1), (7, 0, 1)] {
                self.cell_fmt_char(cid, cp, 0, u32::MAX, k, a, b)?;
            }
            return Ok(());
        }
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_clear_range_char_format_all(self.handle, 0, 0, para, start, end);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    /// v238: 文字颜色/背景色。kind="fg"|"bg"；rgb=None 清除
    pub fn writer_set_color(&self, kind: &str, para: u32, start: u32, end: u32, rgb: Option<u32>) -> Result<(), String> {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { return self.cell_fmt_char(cid, cp, 0, u32::MAX, if kind == "bg" { 7 } else { 6 }, rgb.unwrap_or(0), if rgb.is_none() { 1 } else { 0 }); }
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let (v, clear) = match rgb { Some(c) => (c, 0u32), None => (0u32, 1u32) };
        let code = if kind == "bg" {
            roe_api::roe_set_range_char_format_background(self.handle, 0, 0, para, start, end, v, clear)
        } else {
            roe_api::roe_set_range_char_format_color(self.handle, 0, 0, para, start, end, v, clear)
        };
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    /// v235: 字号（pt）。size=-1.0 清除覆盖
    pub fn writer_set_font_size(&self, para: u32, start: u32, end: u32, size: f32) -> Result<(), String> {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { return self.cell_fmt_char(cid, cp, 0, u32::MAX, 5, if size < 0.0 { 0 } else { (size * 10.0).round() as u32 }, 0); }
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let code = roe_api::roe_set_range_char_format_font_size(self.handle, 0, 0, para, start, end, size);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    /// v235: 查字号。>=0 一致；-1.0 混合或全未设置；-2.0 出错
    pub fn writer_get_font_size(&self, para: u32, start: u32, end: u32) -> f32 {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { let v = self.cell_fmt_get(cid, cp, 6); return if v > 0 { v as f32 / 10.0 } else { -1.0 }; }
        if self.handle == 0 { return -2.0; }
        roe_api::roe_get_range_char_format_font_size(self.handle, 0, 0, para, start, end)
    }
    /// v235: 字体名。None=清除覆盖
    pub fn writer_set_font_family(&self, para: u32, start: u32, end: u32, name: Option<&str>) -> Result<(), String> {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { return self.cell_fmt_font(cid, cp, name); }
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let c = match name {
            Some(n) => Some(std::ffi::CString::new(n).map_err(|_| "字体名含NUL".to_string())?),
            None => None,
        };
        let ptr = c.as_ref().map_or(std::ptr::null(), |x| x.as_ptr());
        let code = roe_api::roe_set_range_char_format_font_family(self.handle, 0, 0, para, start, end, ptr);
        if code == 0 { Ok(()) } else { Err(Self::formula_last_err()) }
    }
    /// v235: 查字体名。Some("__MIXED__")=混合；None=全未设置或出错
    pub fn writer_get_font_family(&self, para: u32, start: u32, end: u32) -> Option<String> {
        if let Some((cid, cp)) = Self::cell_fmt_target_rw(None) { let _ = (cid, cp); return None; }
        if self.handle == 0 { return None; }
        unsafe { take_roe_string(roe_api::roe_get_range_char_format_font_family(self.handle, 0, 0, para, start, end)) }
    }

    /// 在光标处插入一个字符（v100 最小切面）。对应 roe_api::roe_insert_char。
    /// 参数：local_para_index/char_offset 是插入前的光标位置，ch 是待插入
    /// 字符的 Unicode 码点（如 'a' as u32）。
    /// 返回：Some(JSON字符串)，形如
    /// {"container_type":0,"container_id":0,"local_para_index":N,"char_offset":N}
    /// 即插入完成后光标应移动到的新位置。失败（无效句柄/越界/非法码点）
    /// 返回 None，详情可用 roe_last_error_message 查询。
    ///
    /// 范围限制（本次有意收窄，见 ROE_Progress_v99.md/v100）：仅支持单个
    /// 字符插入，不支持批量字符串、不支持删除、不含跨段落/IME组字。
    pub fn insert_char(
        &self,
        local_para_index: u32,
        char_offset: u32,
        ch: u32,
    ) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_insert_char(
            self.handle, 0, 0, local_para_index, char_offset, ch,
        );
        unsafe { take_roe_string(ptr) }
    }

    /// 在光标处删除一个字符（v100 最小切面）。对应 roe_api::roe_delete_char。
    /// direction: 0=Backward退格（删除offset之前一个字符），
    ///            1=Forward删除键（删除offset处/之后一个字符）。
    /// 返回：Some(JSON字符串)，形如
    /// {"container_type":0,"container_id":0,"local_para_index":N,"char_offset":N}
    /// 即删除完成后光标应移动到的新位置。失败（无效句柄/越界/段首退格/
    /// 段尾Delete）返回 None，详情可用 roe_last_error_message 查询。
    pub fn delete_char(
        &self,
        local_para_index: u32,
        char_offset: u32,
        direction: u32,
    ) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_delete_char(
            self.handle, 0, 0, local_para_index, char_offset, direction,
        );
        unsafe { take_roe_string(ptr) }
    }

    /// 在光标处批量插入一段文本（v101新增）。对应 roe_api::roe_insert_text。
    /// 与 insert_char 的核心差异：整段文本一次性插入，只产生"一次"可撤销
    /// 操作（undo一次即可整体撤销整段文本），主要供 CJK 输入法组合完成后
    /// 一次性提交多字符使用（比如"你好"两个字），不是 insert_char 的替换。
    /// text 为待插入的完整字符串（内部会转成 UTF-8 字节传给 roe 侧）。
    /// 返回：Some(JSON字符串)，形如
    /// {"container_type":0,"container_id":0,"local_para_index":N,"char_offset":N}
    /// 即插入完成后光标应移动到的新位置（原offset + 插入文本的UTF-8字节
    /// 总长度）。失败（无效句柄/越界/空文本/非法UTF-8等）返回 None，
    /// 详情可用 roe_last_error_message 查询。
    pub fn insert_text(
        &self,
        local_para_index: u32,
        char_offset: u32,
        text: &str,
    ) -> Option<String> {
        if self.handle == 0 { return None; }
        if text.is_empty() { return None; }
        let bytes = text.as_bytes();
        let ptr = roe_api::roe_insert_text(
            self.handle, 0, 0, local_para_index, char_offset,
            bytes.as_ptr(), bytes.len() as u32,
        );
        unsafe { take_roe_string(ptr) }
    }

    /// v115：撤销上一步已提交的编辑。薄封装，直接调用roe_api::roe_undo。
    /// 语义完全由roe内核的undo_stack决定（每次insert_text/delete_char/
    /// replace_range等编辑操作调用成功后，roe内核都会独立push一条快照，
    /// 所以这里每调用一次undo，效果就是撤销"上一次成功的编辑操作"，
    /// 粒度由roe内核决定，前端不做任何合并或会话管理。
    ///
    /// 返回值变化（v113→v115）：从bool改为Option<String>，Some携带的
    /// JSON字符串是undo后光标应恢复到的精确位置（编辑发生前的锚点/
    /// 焦点），形如：
    /// {"container_type":0,"container_id":0,"local_para_index":N,
    ///  "anchor_offset":N,"focus_offset":N}
    /// 不再是"退化到段落开头"这种近似值（v113/v114遗留的简化处理，
    /// ROE_Progress_v115.md 三.4方案已定案，本次接入）。
    /// None=句柄无效或没有可撤销的历史（ROE_NO_UNDO_HISTORY，详情见
    /// roe_last_error_message）。
    pub fn undo(&self) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_undo(self.handle);
        unsafe { take_roe_string(ptr) }
    }

    /// v115：重做（撤销roe_undo的效果）。薄封装，直接调用roe_api::roe_redo。
    /// 返回值语义同undo（见该函数文档注释）：Some(JSON)=重做成功且携带
    /// 精确光标位置，None=句柄无效或没有可重做的历史。
    pub fn redo(&self) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_redo(self.handle);
        unsafe { take_roe_string(ptr) }
    }

    /// v112：区间替换（IME"编辑会话"模型的核心操作）。删除
    /// [start_char_offset, end_char_offset) 区间内容，插入 new_text，
    /// 整个操作是roe内核侧的一次原子提交（只产生一条undo记录），不是
    /// 调用方自己拼接的"先delete再insert"两步操作。
    ///
    /// 与 insert_text 的关键差异：new_text 允许为空字符串（对应"这一轮
    /// IME组合最终什么也没留下"或纯删除场景），不能像 insert_text 那样
    /// "文本为空就直接return None"——区间删除本身就是一种合法操作。
    /// start_char_offset == end_char_offset 时退化为纯插入。
    ///
    /// 返回替换后光标应处的新位置JSON（格式同 insert_text/delete_char），
    /// 失败（越界、跨段落区间等）返回 None。
    pub fn replace_range(
        &self,
        local_para_index: u32,
        start_char_offset: u32,
        end_char_offset: u32,
        new_text: &str,
    ) -> Option<String> {
        if self.handle == 0 { return None; }
        let bytes = new_text.as_bytes();
        let ptr = roe_api::roe_replace_range(
            self.handle, 0, 0, local_para_index,
            start_char_offset, end_char_offset,
            bytes.as_ptr(), bytes.len() as u32,
        );
        unsafe { take_roe_string(ptr) }
    }

    /// v228: 在 (para, char_offset) 插入公式，返回 (para_index, run_idx)
    pub fn formula_insert(&mut self, para: u32, char_offset: u32, latex: &str) -> Result<(u32, u32), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let b = latex.as_bytes();
        let ptr = roe_api::roe_formula_insert(self.handle, para, char_offset, b.as_ptr(), b.len() as u32);
        let json = unsafe { take_roe_string(ptr) }.ok_or_else(Self::formula_last_err)?;
        let v: serde_json::Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        let p = v.get("para_index").and_then(|x| x.as_u64()).ok_or("缺 para_index")? as u32;
        let r = v.get("run_idx").and_then(|x| x.as_u64()).ok_or("缺 run_idx")? as u32;
        Ok((p, r))
    }

    // ───────── v227: 公式封装（对应 roe-api/formula_edit.rs）─────────
    fn formula_last_err() -> String {
        unsafe { take_roe_string(roe_api::roe_last_error_message()) }
            .unwrap_or_else(|| "未知错误".to_string())
    }

    /// 公式命中检测：坐标口径同 writer_hit_test。命中返回 (para_index, run_idx)
    pub fn formula_hit_test(&mut self, x_pt: f64, y_pt: f64) -> Option<(u32, u32)> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_formula_hit_test(self.handle, x_pt, y_pt)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        if !v.get("hit").and_then(|h| h.as_bool()).unwrap_or(false) { return None; }
        let p = v.get("para_index")?.as_u64()? as u32;
        let r = v.get("run_idx")?.as_u64()? as u32;
        Some((p, r))
    }

    /// v229: 公式命中并返回框 (para, run, x, y, w, h)，全局坐标 pt
    pub fn formula_hit_box(&mut self, x_pt: f64, y_pt: f64) -> Option<(u32, u32, f64, f64, f64, f64)> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_formula_hit_test(self.handle, x_pt, y_pt)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        if !v.get("hit").and_then(|h| h.as_bool()).unwrap_or(false) { return None; }
        let p = v.get("para_index")?.as_u64()? as u32;
        let r = v.get("run_idx")?.as_u64()? as u32;
        let g = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
        Some((p, r, g("x"), g("y"), g("w"), g("h")))
    }

    /// 读取公式的 LaTeX
    pub fn formula_get(&mut self, para: u32, run_idx: u32) -> Option<String> {
        if self.handle == 0 { return None; }
        let json = unsafe { take_roe_string(roe_api::roe_formula_get(self.handle, para, run_idx)) }?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        v.get("latex").and_then(|x| x.as_str()).map(|s| s.to_string())
    }

    /// 写回公式。失败时模型不变，返回错误文字
    pub fn formula_set(&mut self, para: u32, run_idx: u32, latex: &str) -> Result<(), String> {
        if self.handle == 0 { return Err("没有打开的文档".to_string()); }
        let b = latex.as_bytes();
        let ptr = roe_api::roe_formula_set(self.handle, para, run_idx, b.as_ptr(), b.len() as u32);
        match unsafe { take_roe_string(ptr) } {
            Some(_) => Ok(()),
            None => Err(Self::formula_last_err()),
        }
    }

    /// 预览：返回 (RGBA 像素, 宽, 高)。空输入返回 Err("")
    pub fn formula_preview_rgba(latex: &str, font_pt: f64, scale: f64) -> Result<(Vec<u8>, u32, u32), String> {
        if latex.trim().is_empty() { return Err(String::new()); }
        let b = latex.as_bytes();
        let (mut w, mut h, mut len) = (0u32, 0u32, 0u32);
        let ptr = roe_api::roe_formula_preview_rgba(b.as_ptr(), b.len() as u32, font_pt, scale, &mut w, &mut h, &mut len);
        if ptr.is_null() { return Err(Self::formula_last_err()); }
        let data = unsafe { std::slice::from_raw_parts(ptr, len as usize).to_vec() };
        roe_api::roe_free_bytes(ptr, len);
        Ok((data, w, h))
    }

    /// 在光标处按回车切分段落（v105新增）。对应 roe_api::roe_split_paragraph。
    /// 把 (local_para_index, char_offset) 处的段落一分为二：前半段留在
    /// 原段落，后半段（继承原段落style等元数据）成为紧随其后的新段落。
    /// 参数 local_para_index/char_offset 是分段前的光标位置。
    /// 返回：Some(JSON字符串)，形如
    /// {"container_type":0,"container_id":0,"local_para_index":N,"char_offset":0}
    /// 即分段完成后光标应移动到的新位置（新段落开头，local_para_index为
    /// 原para_index+1）。失败（无效句柄/越界/非法offset）返回None，
    /// 详情可用 roe_last_error_message 查询。
    ///
    /// 范围限制（同 ROE_Progress_v104.md「3.4」）：仅支持光标处、无选区
    /// 的最简单情况，不处理"当前有选区，先删除选区内容再分段"的组合
    /// 场景；也不实现 Shift+回车的软换行语义。
    pub fn split_paragraph(
        &self,
        local_para_index: u32,
        char_offset: u32,
    ) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_split_paragraph(
            self.handle, 0, 0, local_para_index, char_offset,
        );
        unsafe { take_roe_string(ptr) }
    }

    /// 跨段落合并（v105新增，split_paragraph 的逆操作）。对应
    /// roe_api::roe_merge_paragraph。段首退格/段尾Delete时把光标所在段
    /// 与相邻段合并成一个段落，结果段落保留"保留方"的元数据（退格时是
    /// 前一段，Delete时是当前段），光标统一停在合并点。
    /// direction: 0=Backward退格 1=Forward Delete，与 delete_char 的编码一致。
    /// 返回：Some(JSON字符串)，形如
    /// {"container_type":0,"container_id":0,"local_para_index":N,"char_offset":N}
    /// 失败（无效句柄/越界/不满足合并前置条件，如已是首段/末段）返回
    /// None，详情可用 roe_last_error_message 查询。
    pub fn merge_paragraph(
        &self,
        local_para_index: u32,
        char_offset: u32,
        direction: u32,
    ) -> Option<String> {
        if self.handle == 0 { return None; }
        let ptr = roe_api::roe_merge_paragraph(
            self.handle, 0, 0, local_para_index, char_offset, direction,
        );
        unsafe { take_roe_string(ptr) }
    }

    /// 返回指定Sheet有数据的最大行数。失败返回-1。
    pub fn get_row_count(&self, sheet_idx: u32) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_get_row_count(self.handle, sheet_idx)
    }

    /// 返回指定Sheet有数据的最大列数。失败返回-1。
    pub fn get_col_count(&self, sheet_idx: u32) -> i32 {
        if self.handle == 0 { return -1; }
        roe_api::roe_get_col_count(self.handle, sheet_idx)
    }

    /// 在末尾新增一个Sheet，返回新Sheet的idx（0-based）。失败返回-1。
    pub fn add_sheet(&mut self, name: &str) -> i32 {
        if self.handle == 0 { return -1; }
        let c_name = match std::ffi::CString::new(name) {
            Ok(c) => c,
            Err(_) => return -1,
        };
        roe_api::roe_add_sheet(self.handle, c_name.as_ptr())
    }

    /// 删除指定Sheet（不能删最后一个）。成功返回true。
    pub fn remove_sheet(&mut self, sheet_idx: u32) -> bool {
        if self.handle == 0 { return false; }
        roe_api::roe_remove_sheet(self.handle, sheet_idx) == 0
    }

    /// 重命名指定Sheet。成功返回true。
    pub fn rename_sheet(&mut self, sheet_idx: u32, name: &str) -> bool {
        if self.handle == 0 { return false; }
        let c_name = match std::ffi::CString::new(name) {
            Ok(c) => c,
            Err(_) => return false,
        };
        roe_api::roe_rename_sheet(self.handle, sheet_idx, c_name.as_ptr()) == 0
    }

    /// 返回所有Sheet名称列表，供SheetTabBar初始化使用。
    pub fn get_all_sheet_names(&self) -> Vec<String> {
        let count = self.get_sheet_count();
        if count <= 0 { return vec!["Sheet1".to_string()]; }
        (0..count as u32).map(|i| self.get_sheet_name(i)).collect()
    }

    // ================= 批注（comment）查询 =================
    // Slint 批注气泡面板第一阶段（roe-api::comments 模块新增的两个
    // C-ABI 接口的薄封装，用法与文件顶部模块注释一致：
    // get_comment_threads_json 返回全文档批注文本，take_comment_hits_json
    // 返回"最近一次 render_writer_viewport 调用中可见"的批注坐标，
    // 两者用 comment_id 关联，调用方（Slint端）自行拼装展示。

    /// 全文档所有批注讨论串（root+replies）的纯文本内容，JSON字符串。
    /// 调用时机：文档打开后、批注数据发生变化后（本次暂无编辑批注的
    /// 接口，只有只读查询，所以实际调用时机目前仅为"文档打开后"）。
    /// 句柄无效时返回 roe-api 的错误占位 JSON。
    pub fn get_comment_threads_json(&mut self) -> String {
        if self.handle == 0 {
            return err_json("no document open");
        }
        match unsafe { take_roe_string(roe_get_comment_threads(self.handle)) } {
            Some(s) => s,
            None => err_json("roe_get_comment_threads failed"),
        }
    }

    /// "最近一次 render_writer_viewport 调用中，落在可见viewport范围内"
    /// 的批注命中坐标列表，JSON字符串。调用时机：每次调用
    /// render_writer_viewport 渲染完成后立即调用，用于更新气泡跟随
    /// 滚动的显示位置。句柄无效时返回 roe-api 的错误占位 JSON。
    pub fn take_comment_hits_json(&mut self) -> String {
        if self.handle == 0 {
            return err_json("no document open");
        }
        match unsafe { take_roe_string(roe_take_comment_hits(self.handle)) } {
            Some(s) => s,
            None => err_json("roe_take_comment_hits failed"),
        }
    }
}


/// 1pt = 96/72 逻辑像素(CSS px)。Slint 侧同值,见 app.slint 的 writer-scale。
pub const PX_PER_PT: f32 = 96.0 / 72.0;
/// Writer 画布顶部偏移(pt):文档全局坐标 y + 此值 = 画布坐标 y(点击命中则相反,减去它)。
/// 各处原先各自定义的 24.0 统一到这里。以后启用标尺时,改这一处即可。
pub const CANVAS_OFFSET_PT: f32 = 24.0;
/// 【技术债,待优化】设备像素比(HiDPI)目前【写死为 2.0】,对应开发机 14 寸 2000x3000 屏(系统缩放 200%)。
/// 正确做法:渲染时现取 Slint window().scale_factor()(窗口拖到别的屏会变),取不到退回 2.0。
/// 写死只影响位图清晰度,不影响布局与坐标(坐标走 writer-scale)。1x 屏会多耗内存,3x 屏会略糊。
/// 注意 app.slint 里 Writer 图片宽度和 ghost 的 pxs 里也有一个写死的 2,必须与此值一致。
static DEVICE_PPP_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x4000_0000); // 2.0f32
/// 当前设备像素比。默认 2.0;由前台在渲染前用 window().scale_factor() 更新(set_device_ppp)。
pub fn device_ppp() -> f32 {
    f32::from_bits(DEVICE_PPP_BITS.load(std::sync::atomic::Ordering::Relaxed))
}
/// 设置设备像素比。环境变量 ROE_PPP(如 1.5)优先于传入值;非法值(<=0 或>8)忽略,退回 2.0。
/// 返回值:是否发生了变化(调用方据此决定要不要重渲染)。
pub fn set_device_ppp(v: f32) -> bool {
    let mut x = v;
    if let Ok(s) = std::env::var("ROE_PPP") {
        if let Ok(e) = s.trim().parse::<f32>() { x = e; }
    }
    if !(x > 0.0 && x <= 8.0) { x = 2.0; }
    let old = DEVICE_PPP_BITS.swap(x.to_bits(), std::sync::atomic::Ordering::Relaxed);
    old != x.to_bits()
}
/// Writer 视口位图 像素/pt = 设备像素比 * PX_PER_PT。
/// 做缩放时再乘 zoom 并重渲染(位图必须重渲染,不能拉伸,否则放大会糊)。
static WRITER_ZOOM_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3F80_0000); // 1.0f32
/// Writer 当前缩放倍数(1.0=100%)。由前台 zoom-changed 回调更新。
pub fn writer_zoom() -> f32 {
    f32::from_bits(WRITER_ZOOM_BITS.load(std::sync::atomic::Ordering::Relaxed))
}
pub fn set_writer_zoom(z: f32) {
    let z = if z.is_finite() { z.clamp(0.1, 5.0) } else { 1.0 };
    WRITER_ZOOM_BITS.store(z.to_bits(), std::sync::atomic::Ordering::Relaxed);
}
/// Writer 视口位图 像素/pt = 设备像素比 * PX_PER_PT * 缩放。
pub fn doc_bitmap_scale() -> f32 { device_ppp() * PX_PER_PT * writer_zoom() }
/// Impress 视口位图倍数:Impress 还没统一 pt→px,仍是 1pt=1px、2 倍出图;统一时并入 Writer 方案。
static IMPRESS_SCALE_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3F80_0000); // 1.0f32
/// Impress 当前缩放(屏幕逻辑 px / pt)。默认 1.0;由前台缩放逻辑更新。
pub fn impress_scale() -> f32 {
    f32::from_bits(IMPRESS_SCALE_BITS.load(std::sync::atomic::Ordering::Relaxed))
}
pub fn set_impress_scale(v: f32) {
    let v = if v.is_finite() { v.clamp(0.1, 5.0) } else { 1.0 };
    IMPRESS_SCALE_BITS.store(v.to_bits(), std::sync::atomic::Ordering::Relaxed);
}
/// Impress 位图 像素/pt = 设备像素比 * 缩放。
pub fn impress_bitmap_scale() -> f32 { device_ppp() * impress_scale() }
/// Calc 当前缩放(屏幕逻辑px / Calc 100%坐标px)。默认 1.0;由前台设置,环境变量 ROE_CALC_SCALE 做测试开关。
static CALC_SCALE_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3F80_0000); // 1.0f32
pub fn set_calc_scale(v: f32) {
    let v = if v.is_finite() { v.clamp(0.25, 3.0) } else { 1.0 };
    CALC_SCALE_BITS.store(v.to_bits(), std::sync::atomic::Ordering::Relaxed);
}
fn calc_scale_raw() -> f32 { f32::from_bits(CALC_SCALE_BITS.load(std::sync::atomic::Ordering::Relaxed)) }
/// 传给内核的 zoom_pct = 设备像素比 x 缩放,取整 1%。内核不接受低于 100,所以夹到 >=100
/// (2倍屏缩放下限 50%,1倍屏下限 100%)。
pub fn calc_zoom_pct() -> u32 { ((device_ppp() * calc_scale_raw() * 100.0).round() as u32).max(100) }
/// Calc 位图 像素/逻辑px(与 zoom_pct 同源)。
pub fn calc_bitmap_scale() -> f32 { calc_zoom_pct() as f32 / 100.0 }
/// 实际生效的 Calc 缩放(已按 zoom_pct 取整并受下限约束),Slint 的 calc-scale 必须用这个值。
pub fn calc_scale() -> f32 { calc_bitmap_scale() / device_ppp() }
/// Calc 视口位图的物理像素尺寸。session 与前台必须都用它,不要各算各的。
pub fn calc_phys_size(vw: u32, vh: u32) -> (u32, u32) {
    let f = calc_zoom_pct() as f32 / 100.0;
    ((vw as f32 * f) as u32, (vh as f32 * f) as u32)
}


// ===== Calc 动态扩展滚动范围：每个 (文档句柄, sheet) 额外增加的 (高度pt, 宽度pt) =====
thread_local! {
    static CALC_EXTRA: std::cell::RefCell<std::collections::HashMap<(u64, u32), (f32, f32)>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}
fn calc_extra_get(handle: u64, sheet: u32) -> (f32, f32) {
    CALC_EXTRA.with(|m| m.borrow().get(&(handle, sheet)).copied().unwrap_or((0.0, 0.0)))
}
fn calc_extra_add(handle: u64, sheet: u32, is_height: bool) {
    CALC_EXTRA.with(|m| {
        let mut m = m.borrow_mut();
        let e = m.entry((handle, sheet)).or_insert((0.0, 0.0));
        if is_height { e.0 = (e.0 + 700.0).min(15_000_000.0); } else { e.1 = (e.1 + 480.0).min(786_432.0); }
    });
}
