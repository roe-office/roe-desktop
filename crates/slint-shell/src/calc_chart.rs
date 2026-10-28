//! Calc 浮动图表的纯逻辑：坐标换算 + 按 (sheet, chart_id) 管理的矩形仓库。
//! 不依赖界面/会话，便于单测。换算只有这一处。
//!
//! 约定：roe 的 pt 与单元格同一坐标系；视口逻辑 px = pt * PT_TO_PX - scroll + 表头。
//! scroll 的单位是逻辑 px（与 calc-scroll-x/y 一致）。

use std::collections::HashMap;

pub const PT_TO_PX: f32 = 96.0 / 72.0;
pub const HEADER_W: f32 = 30.0; // 列标题宽(逻辑px)
pub const HEADER_H: f32 = 18.0; // 行标题高(逻辑px)

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PtRect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoordMap { pub scroll_x: f32, pub scroll_y: f32 }

impl CoordMap {
    pub fn new(scroll_x: f32, scroll_y: f32) -> Self { Self { scroll_x, scroll_y } }

    /// pt 矩形 → 视口逻辑 px 矩形（含表头偏移）。
    pub fn rect_to_view(&self, r: PtRect) -> PtRect {
        PtRect {
            x: r.x * PT_TO_PX - self.scroll_x + HEADER_W,
            y: r.y * PT_TO_PX - self.scroll_y + HEADER_H,
            w: r.w * PT_TO_PX,
            h: r.h * PT_TO_PX,
        }
    }

    /// 视口逻辑 px 点 → sheet pt 点。
    pub fn view_to_pt(&self, vx: f32, vy: f32) -> (f32, f32) {
        ((vx - HEADER_W + self.scroll_x) / PT_TO_PX, (vy - HEADER_H + self.scroll_y) / PT_TO_PX)
    }

    /// 视口 px 矩形 → pt 矩形（拖拽提交用）。
    pub fn rect_to_pt(&self, r: PtRect) -> PtRect {
        let (x, y) = self.view_to_pt(r.x, r.y);
        PtRect { x, y, w: r.w / PT_TO_PX, h: r.h / PT_TO_PX }
    }
}

#[derive(Debug, Default)]
pub struct ChartStore { map: HashMap<(u32, u64), (PtRect, bool)>, plain: std::collections::HashSet<(u32, u64)>, shapes: std::collections::HashSet<(u32, u64)> }

impl ChartStore {
    pub fn new() -> Self { Self::default() }
    pub fn set(&mut self, sheet: u32, id: u64, r: PtRect) { self.map.insert((sheet, id), (r, false)); }
    /// stat=true 为统计图：渲染固定压在普通图之上，命中保持同序。
    pub fn set_ex(&mut self, sheet: u32, id: u64, r: PtRect, stat: bool) { self.map.insert((sheet, id), (r, stat)); }
    pub fn get(&self, sheet: u32, id: u64) -> Option<PtRect> { self.map.get(&(sheet, id)).map(|v| v.0) }
    pub fn remove(&mut self, sheet: u32, id: u64) { self.map.remove(&(sheet, id)); self.plain.remove(&(sheet, id)); self.shapes.remove(&(sheet, id)); }
    pub fn is_stat(&self, sheet: u32, id: u64) -> bool { self.map.get(&(sheet, id)).map(|v| v.1).unwrap_or(false) }
    pub fn clear_sheet(&mut self, sheet: u32) { self.map.retain(|k, _| k.0 != sheet); self.plain.retain(|k| k.0 != sheet); self.shapes.retain(|k| k.0 != sheet); }
    pub fn clear_all(&mut self) { self.map.clear(); self.plain.clear(); self.shapes.clear(); }
    /// 图片/图形(非图表):可选中拖动,但不进图表覆盖层。
    pub fn is_plain(&self, sheet: u32, id: u64) -> bool { self.plain.contains(&(sheet, id)) }
    /// 图形/文本框(可改填充和边框)。
    pub fn is_shape(&self, sheet: u32, id: u64) -> bool { self.shapes.contains(&(sheet, id)) }
    pub fn len(&self) -> usize { self.map.len() }

    /// 用 roe_list_charts 的 JSON 整体替换指定 sheet 的缓存，返回对象数。
    /// JSON 解析失败返回 None，且不动原缓存。
    pub fn load_sheet_json(&mut self, sheet: u32, json: &str) -> Option<usize> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        let arr = v.as_array()?;
        self.clear_sheet(sheet);
        let mut n = 0;
        for it in arr {
            let id = match it.get("id").and_then(|x| x.as_u64()) { Some(i) => i, None => continue };
            let g = |k: &str| it.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            let kind = it.get("kind").and_then(|x| x.as_str()).unwrap_or("");
            if it.get("hidden").and_then(|x| x.as_bool()).unwrap_or(false) { continue; }
            let stat = it.get("stat").and_then(|x| x.as_bool()).unwrap_or(false) || kind == "stat_chart";
            let is_plain = kind == "image" || kind == "shape";
            self.set_ex(sheet, id, PtRect { x: g("x_pt"), y: g("y_pt"), w: g("width_pt"), h: g("height_pt") }, stat);
            if is_plain { self.plain.insert((sheet, id)); }
            if kind == "shape" { self.shapes.insert((sheet, id)); }
            n += 1;
        }
        Some(n)
    }

    /// 命中 pt 点：只看指定 sheet。叠放序 = 统计图在上，同类内 id 大者在上（与渲染一致）。
    pub fn hit_pt(&self, sheet: u32, x_pt: f32, y_pt: f32) -> Option<u64> {
        self.map.iter()
            .filter(|((s, _), (r, _))| *s == sheet && x_pt >= r.x && x_pt <= r.x + r.w && y_pt >= r.y && y_pt <= r.y + r.h)
            .map(|((_, id), (_, stat))| (*stat, *id))
            .max()
            .map(|(_, id)| id)
    }

    /// 命中视口点：落在表头区域不算命中（图表只画在数据区内）。
    pub fn hit_view(&self, sheet: u32, map: &CoordMap, vx: f32, vy: f32) -> Option<u64> {
        if vx < HEADER_W || vy < HEADER_H { return None; }
        let (x, y) = map.view_to_pt(vx, vy);
        self.hit_pt(sheet, x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> PtRect { PtRect { x, y, w, h } }

    #[test]
    fn view_matches_roe_composition() {
        // 与 roe 测试同口径：B2(48pt,15pt) 无滚动 → 视口 (48*4/3+30, 15*4/3+18)=(94,38)
        let v = CoordMap::new(0.0, 0.0).rect_to_view(r(48.0, 15.0, 100.0, 60.0));
        assert!((v.x - 94.0).abs() < 0.01 && (v.y - 38.0).abs() < 0.01);
        assert!((v.w - 133.333).abs() < 0.01 && (v.h - 80.0).abs() < 0.01);
    }

    #[test]
    fn scroll_shifts_view() {
        let v = CoordMap::new(10.0, 20.0).rect_to_view(r(48.0, 15.0, 10.0, 10.0));
        assert!((v.x - 84.0).abs() < 0.01 && (v.y - 18.0).abs() < 0.01);
    }

    #[test]
    fn roundtrip_rect() {
        let m = CoordMap::new(37.0, 11.0);
        let src = r(123.4, 56.7, 200.0, 150.0);
        let back = m.rect_to_pt(m.rect_to_view(src));
        assert!((back.x - src.x).abs() < 0.01 && (back.y - src.y).abs() < 0.01);
        assert!((back.w - src.w).abs() < 0.01 && (back.h - src.h).abs() < 0.01);
    }

    #[test]
    fn hit_ignores_other_sheets() {
        let mut s = ChartStore::new();
        s.set(0, 1, r(0.0, 0.0, 100.0, 100.0));
        s.set(1, 1, r(0.0, 0.0, 100.0, 100.0));
        assert_eq!(s.hit_pt(0, 50.0, 50.0), Some(1));
        s.clear_sheet(1);
        assert_eq!(s.hit_pt(1, 50.0, 50.0), None);
        assert_eq!(s.hit_pt(0, 50.0, 50.0), Some(1));
    }

    #[test]
    fn hit_prefers_higher_id() {
        let mut s = ChartStore::new();
        s.set(0, 1, r(0.0, 0.0, 100.0, 100.0));
        s.set(0, 2, r(50.0, 50.0, 100.0, 100.0));
        assert_eq!(s.hit_pt(0, 60.0, 60.0), Some(2));
        assert_eq!(s.hit_pt(0, 10.0, 10.0), Some(1));
        assert_eq!(s.hit_pt(0, 500.0, 500.0), None);
    }

    #[test]
    fn hit_view_skips_header_and_uses_scroll() {
        let mut s = ChartStore::new();
        s.set(0, 7, r(48.0, 15.0, 100.0, 60.0));
        let m = CoordMap::new(0.0, 0.0);
        assert_eq!(s.hit_view(0, &m, 100.0, 40.0), Some(7));
        assert_eq!(s.hit_view(0, &m, 20.0, 40.0), None); // 列标题区
        assert_eq!(s.hit_view(0, &m, 100.0, 10.0), None); // 行标题区
        // 横向滚动 50px 后，图表视口范围由 [94,227] 左移为 [44,177]
        assert_eq!(s.hit_view(0, &m, 200.0, 40.0), Some(7)); // 无滚动时 200 在范围内
        let m2 = CoordMap::new(50.0, 0.0);
        assert_eq!(s.hit_view(0, &m2, 200.0, 40.0), None);   // 滚动后 200 已在范围外
        assert_eq!(s.hit_view(0, &m2, 50.0, 40.0), Some(7)); // 左移后的新位置命中
    }

    #[test]
    fn clear_all_and_remove() {
        let mut s = ChartStore::new();
        s.set(0, 1, r(0.0, 0.0, 1.0, 1.0));
        s.set(1, 2, r(0.0, 0.0, 1.0, 1.0));
        s.remove(0, 1);
        assert_eq!(s.len(), 1);
        s.clear_all();
        assert_eq!(s.len(), 0);
    }
}

#[cfg(test)]
mod tests_load {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> PtRect { PtRect { x, y, w, h } }

    #[test]
    fn stat_on_top_of_normal() {
        let mut s = ChartStore::new();
        s.set_ex(0, 5, r(0.0, 0.0, 100.0, 100.0), true);
        s.set_ex(0, 9, r(0.0, 0.0, 100.0, 100.0), false);
        assert_eq!(s.hit_pt(0, 50.0, 50.0), Some(5));
    }

    #[test]
    fn load_json_replaces_sheet() {
        let mut s = ChartStore::new();
        s.set(0, 9, r(0.0, 0.0, 10.0, 10.0));
        let j = r#"[{"id":1,"kind":"Bar","x_pt":0.0,"y_pt":0.0,"width_pt":100.0,"height_pt":100.0},{"id":2,"kind":"Funnel","stat":true,"x_pt":50.0,"y_pt":50.0,"width_pt":100.0,"height_pt":100.0}]"#;
        assert_eq!(s.load_sheet_json(0, j), Some(2));
        assert_eq!(s.get(0, 9), None);
        assert_eq!(s.hit_pt(0, 60.0, 60.0), Some(2));
        assert_eq!(s.hit_pt(0, 10.0, 10.0), Some(1));
        assert_eq!(s.load_sheet_json(0, "bad"), None);
        assert_eq!(s.len(), 2);
    }
}


// ===== 图表覆盖层(把视口位图里的图表像素裁出来盖回高亮框之上) =====

/// 测试用的默认位图倍数(2 倍屏、缩放 1.0)。运行时由调用处传 doc_core::calc_bitmap_scale()。
pub const BITMAP_SCALE: f32 = 2.0;

/// 一块覆盖层:x/y/w/h 为数据区容器内的逻辑 px,cx/cy/cw/ch 为位图源像素裁剪。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PicClip { pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub cx: i32, pub cy: i32, pub cw: i32, pub ch: i32 }

/// 指定 sheet 的对象矩形,按渲染叠放序从下到上(普通图在下,统计图在上,同类 id 小者在下)。
pub fn ordered_rects(store: &ChartStore, sheet: u32) -> Vec<PtRect> {
    let mut v: Vec<(bool, u64, PtRect)> = store.map.iter()
        .filter(|((s, _), _)| *s == sheet)
        .map(|((_, id), (r, stat))| (*stat, *id, *r))
        .collect();
    v.sort_by_key(|t| (t.0, t.1));
    v.into_iter().map(|t| t.2).collect()
}

/// render = 位图渲染时的 scroll(逻辑px);cur = 当前 scroll;vw/vh = 视口逻辑尺寸(含表头)。
/// 裁剪取位图像素(按 render),显示位置按 cur;与数据区求交,表头与视口外不出像素。
pub fn float_pic_clips(objs: &[PtRect], render: (f32, f32), cur: (f32, f32), vw: f32, vh: f32, s: f32) -> Vec<PicClip> {
    let mut out = Vec::new();
    for r in objs {
        let bx0 = r.x * PT_TO_PX - render.0 + HEADER_W;
        let by0 = r.y * PT_TO_PX - render.1 + HEADER_H;
        let bx1 = bx0 + r.w * PT_TO_PX;
        let by1 = by0 + r.h * PT_TO_PX;
        let cx0 = bx0.max(HEADER_W);
        let cy0 = by0.max(HEADER_H);
        let cx1 = bx1.min(vw);
        let cy1 = by1.min(vh);
        if cx1 <= cx0 || cy1 <= cy0 { continue; }
        let sx0 = (cx0 * s).round();
        let sy0 = (cy0 * s).round();
        let cw = (cx1 * s).round() - sx0;
        let ch = (cy1 * s).round() - sy0;
        if cw < 1.0 || ch < 1.0 { continue; }
        out.push(PicClip {
            x: r.x * PT_TO_PX - cur.0 + (sx0 / s - bx0),
            y: r.y * PT_TO_PX - cur.1 + (sy0 / s - by0),
            w: cw / s,
            h: ch / s,
            cx: sx0 as i32, cy: sy0 as i32, cw: cw as i32, ch: ch as i32,
        });
    }
    out
}

#[cfg(test)]
mod float_pic_tests {
    use super::*;
    fn r(x: f32, y: f32, w: f32, h: f32) -> PtRect { PtRect { x, y, w, h } }
    fn near(a: f32, b: f32) -> bool { (a - b).abs() < 0.02 }

    #[test]
    fn inside_no_scroll() {
        let v = float_pic_clips(&[r(48.0, 15.0, 100.0, 60.0)], (0.0, 0.0), (0.0, 0.0), 800.0, 600.0, BITMAP_SCALE);
        assert_eq!(v.len(), 1);
        let p = v[0];
        assert_eq!((p.cx, p.cy, p.cw), (188, 76, 267));
        assert!(near(p.x, 64.0) && near(p.y, 20.0));
    }

    #[test]
    fn clipped_by_header() {
        // 渲染 scroll_x=100 → bx0=-6,左侧 36px 被表头截掉
        let v = float_pic_clips(&[r(48.0, 15.0, 100.0, 60.0)], (100.0, 0.0), (100.0, 0.0), 800.0, 600.0, BITMAP_SCALE);
        assert_eq!(v.len(), 1);
        let p = v[0];
        assert_eq!(p.cx, 60);
        assert!(near(p.x, 0.0), "x={}", p.x);
        assert!(near(p.w, 97.5), "w={}", p.w);
    }

    #[test]
    fn outside_skipped() {
        assert!(float_pic_clips(&[r(48.0, 15.0, 100.0, 60.0)], (1000.0, 0.0), (1000.0, 0.0), 800.0, 600.0, BITMAP_SCALE).is_empty());
        assert!(float_pic_clips(&[r(5000.0, 15.0, 100.0, 60.0)], (0.0, 0.0), (0.0, 0.0), 800.0, 600.0, BITMAP_SCALE).is_empty());
    }

    #[test]
    fn cur_differs_from_render() {
        let v = float_pic_clips(&[r(48.0, 15.0, 100.0, 60.0)], (0.0, 0.0), (10.0, 0.0), 800.0, 600.0, BITMAP_SCALE);
        let p = v[0];
        assert_eq!(p.cx, 188); // 裁剪仍按渲染时 scroll
        assert!(near(p.x, 54.0)); // 显示按当前 scroll
    }

    #[test]
    fn order_stat_on_top() {
        let mut st = ChartStore::new();
        st.set_ex(0, 5, r(1.0, 0.0, 1.0, 1.0), true);
        st.set_ex(0, 9, r(2.0, 0.0, 1.0, 1.0), false);
        st.set_ex(0, 3, r(3.0, 0.0, 1.0, 1.0), false);
        st.set_ex(1, 1, r(4.0, 0.0, 1.0, 1.0), false);
        let xs: Vec<f32> = ordered_rects(&st, 0).iter().map(|q| q.x).collect();
        assert_eq!(xs, vec![3.0, 2.0, 1.0]);
    }
}
