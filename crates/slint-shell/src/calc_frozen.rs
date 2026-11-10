//! 冻结窗格的坐标换算纯函数(逻辑 px，数据区坐标，不含标题栏)。
//! 语义A：scroll 为滚动区相对量；冻结区(坐标 < f)不随滚动，其余区域同原公式。
//! 本模块不做裁剪：滚动区内容被滚到冻结线后面时，doc_to_view 照常返回
//! doc - scroll(可能 < f)，调用方用 is_hidden_under_frozen 判断后裁剪。

#![allow(dead_code)]

/// 文档位置 -> 数据区视口位置。
pub fn doc_to_view(doc: f32, scroll: f32, f: f32) -> f32 {
    if f > 0.0 && doc < f { doc } else { doc - scroll }
}

/// 数据区视口位置 -> 文档位置。
pub fn view_to_doc(v: f32, scroll: f32, f: f32) -> f32 {
    if f > 0.0 && v < f { v } else { v + scroll }
}

/// 滚动区(doc >= f)的内容是否被滚到冻结线后面(被冻结区遮挡)。
pub fn is_hidden_under_frozen(doc: f32, scroll: f32, f: f32) -> bool {
    f > 0.0 && doc >= f && doc - scroll < f
}

/// pt 矩形 -> 屏幕框 (x, y, w, h)(逻辑px):x/y 按冻结分段换算后加标题栏、减 1 边框;
/// w/h 为 pt*K+2 余量。已知局限:选区跨冻结线时 w/h 不分段,右/下缘不准。
pub fn rect_to_screen(
    pt: (f32, f32, f32, f32),
    scroll: (f32, f32),
    frozen: (f32, f32),
    header: (f32, f32),
) -> (f32, f32, f32, f32) {
    const K: f32 = 96.0 / 72.0;
    (
        doc_to_view(pt.0 * K, scroll.0, frozen.0) + header.0 - 1.0,
        doc_to_view(pt.1 * K, scroll.1, frozen.1) + header.1 - 1.0,
        pt.2 * K + 2.0,
        pt.3 * K + 2.0,
    )
}

/// 鼠标视口坐标(含标题栏,逻辑px) -> 文档 pt。header 为标题栏宽/高,
/// 调用方须先排除标题栏内的点击(mouse < header)。
pub fn click_to_pt(mouse: f32, header: f32, scroll: f32, f: f32) -> f32 {
    const K: f32 = 96.0 / 72.0;
    view_to_doc(mouse - header, scroll, f) / K
}

/// 冻结区逻辑 px 宽高(不含标题栏)。fr/fc 为冻结行列数,
/// row_off_pt/col_off_pt 为第 fr 行/第 fc 列起点偏移(pt),失败时后台返回负数。
pub fn extent_px(fr: u32, fc: u32, row_off_pt: f32, col_off_pt: f32) -> (f32, f32) {
    const K: f32 = 96.0 / 72.0;
    let w = if fc > 0 { col_off_pt.max(0.0) * K } else { 0.0 };
    let h = if fr > 0 { row_off_pt.max(0.0) * K } else { 0.0 };
    (w, h)
}

/// 读取冻结行列与起点偏移,写入 calc-frozen-w/h(逻辑px,不含标题栏)。
/// 在切表、改列宽行高、冻结/取消冻结后调用。
pub fn refresh_extent(app: &super::AppWindow, sess: &doc_core::TileCacheRust, sheet: u32) {
    let (fr, fc) = sess.get_frozen_panes(sheet);
    let row_off = if fr > 0 { sess.get_row_offset_pt(sheet, fr) } else { 0.0 };
    let col_off = if fc > 0 { sess.get_col_offset_pt(sheet, fc) } else { 0.0 };
    let (w, h) = extent_px(fr, fc, row_off, col_off);
    app.set_calc_frozen_w(w);
    app.set_calc_frozen_h(h);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extent_none_is_zero() {
        assert_eq!(extent_px(0, 0, 123.0, 456.0), (0.0, 0.0));
    }

    #[test]
    fn extent_converts_pt_to_px() {
        // 96/72 换算:72pt -> 96px;fc>0 取列偏移作宽,fr>0 取行偏移作高
        let (w, h) = extent_px(2, 1, 36.0, 72.0);
        assert!((w - 96.0).abs() < 1e-3, "w={w}");
        assert!((h - 48.0).abs() < 1e-3, "h={h}");
    }

    #[test]
    fn extent_only_rows_or_cols() {
        assert_eq!(extent_px(0, 2, 99.0, 72.0).1, 0.0, "无冻结行高应为0");
        assert_eq!(extent_px(3, 0, 36.0, 99.0).0, 0.0, "无冻结列宽应为0");
    }

    #[test]
    fn extent_negative_clamped() {
        // 后台失败返回 -1.0,不能产生负宽高
        assert_eq!(extent_px(1, 1, -1.0, -1.0), (0.0, 0.0));
    }


    #[test]
    fn no_freeze_equals_plain_formula() {
        // f=0 必须与现有公式 doc-scroll / v+scroll 完全一致(零回归)
        for (d, s) in [(0.0, 0.0), (50.0, 20.0), (300.0, 120.0)] {
            assert_eq!(doc_to_view(d, s, 0.0), d - s);
            assert_eq!(view_to_doc(d, s, 0.0), d + s);
        }
    }

    #[test]
    fn frozen_area_ignores_scroll() {
        // f=100：doc/v < 100 属冻结区，不随滚动
        assert_eq!(doc_to_view(40.0, 500.0, 100.0), 40.0);
        assert_eq!(view_to_doc(40.0, 500.0, 100.0), 40.0);
    }

    #[test]
    fn scroll_area_follows_scroll() {
        assert_eq!(doc_to_view(300.0, 120.0, 100.0), 180.0);
        assert_eq!(view_to_doc(180.0, 120.0, 100.0), 300.0);
    }

    #[test]
    fn scroll_zero_is_identity() {
        for x in [0.0, 50.0, 100.0, 250.0] {
            assert_eq!(doc_to_view(x, 0.0, 100.0), x);
            assert_eq!(view_to_doc(x, 0.0, 100.0), x);
        }
    }

    #[test]
    fn roundtrip_in_scroll_area() {
        let (f, s) = (100.0, 77.0);
        for v in [100.0, 150.0, 400.0] {
            assert_eq!(doc_to_view(view_to_doc(v, s, f), s, f), v);
        }
    }

    #[test]
    fn hidden_under_frozen_detection() {
        // f=100, scroll=120：文档 150 滚到视口 30(<100)，被冻结区遮挡
        assert!(is_hidden_under_frozen(150.0, 120.0, 100.0));
        // 文档 300 -> 视口 180，可见
        assert!(!is_hidden_under_frozen(300.0, 120.0, 100.0));
        // 冻结区内的位置自身不算被遮挡
        assert!(!is_hidden_under_frozen(40.0, 120.0, 100.0));
        // 无冻结永不遮挡
        assert!(!is_hidden_under_frozen(10.0, 50.0, 0.0));
    }

    // ── click_to_pt:鼠标视口坐标(含标题栏) -> 文档 pt ──
    fn near(a: f32, b: f32) -> bool { (a - b).abs() < 0.01 }

    #[test]
    fn click_to_pt_no_freeze_equals_original_formula() {
        // 原公式:((mouse - header) + scroll) / (96/72)
        let k = 96.0_f32 / 72.0;
        for (m, h, sc) in [(130.0_f32, 30.0_f32, 100.0_f32), (45.0, 30.0, 0.0), (500.0, 18.0, 37.5)] {
            assert!(near(click_to_pt(m, h, sc, 0.0), ((m - h) + sc) / k));
        }
    }

    #[test]
    fn click_to_pt_in_frozen_area_ignores_scroll() {
        // v=70-30=40 < 80 -> 文档 40px -> 30pt
        assert!(near(click_to_pt(70.0, 30.0, 100.0, 80.0), 30.0));
    }

    #[test]
    fn click_to_pt_in_scroll_area_adds_scroll() {
        // v=130-30=100 >= 80 -> 200px -> 150pt
        assert!(near(click_to_pt(130.0, 30.0, 100.0, 80.0), 150.0));
    }

    #[test]
    fn click_to_pt_on_frozen_line_belongs_to_scroll_area() {
        // v=110-30=80 == f -> 归滚动区 -> 180px -> 135pt
        assert!(near(click_to_pt(110.0, 30.0, 100.0, 80.0), 135.0));
    }

    // ── rect_to_screen:pt 矩形 -> 屏幕框(含标题栏偏移、-1 边框、+2 余量) ──
    const SC: (f32, f32) = (100.0, 50.0);
    const FZ: (f32, f32) = (80.0, 40.0);
    const HD: (f32, f32) = (30.0, 18.0);

    fn near4(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
        near(a.0, b.0) && near(a.1, b.1) && near(a.2, b.2) && near(a.3, b.3)
    }

    #[test]
    fn rect_to_screen_no_freeze_equals_original_formula() {
        let k = 96.0_f32 / 72.0;
        let pt = (150.0_f32, 90.0_f32, 20.0_f32, 20.0_f32);
        let got = rect_to_screen(pt, SC, (0.0, 0.0), HD);
        let want = (pt.0 * k - SC.0 + HD.0 - 1.0, pt.1 * k - SC.1 + HD.1 - 1.0, pt.2 * k + 2.0, pt.3 * k + 2.0);
        assert!(near4(got, want), "{:?} vs {:?}", got, want);
    }

    #[test]
    fn rect_to_screen_in_frozen_area_ignores_scroll() {
        let got = rect_to_screen((30.0, 15.0, 20.0, 20.0), SC, FZ, HD);
        assert!(near4(got, (69.0, 37.0, 28.6667, 28.6667)), "{:?}", got);
    }

    #[test]
    fn rect_to_screen_in_scroll_area_uses_scroll() {
        let got = rect_to_screen((150.0, 90.0, 20.0, 20.0), SC, FZ, HD);
        assert!(near4(got, (129.0, 87.0, 28.6667, 28.6667)), "{:?}", got);
    }

    #[test]
    fn rect_to_screen_known_limit_cross_freeze_line_width() {
        // 已知局限:起点在冻结区、终点跨入滚动区时,宽度仍按 w*K+2 算,右缘不准。
        // 真机验证时处理;此处只固定当前行为,避免被误当成正确。
        let got = rect_to_screen((45.0, 15.0, 60.0, 20.0), SC, FZ, HD);
        assert!(near(got.0, 89.0) && near(got.2, 82.0), "{:?}", got);
    }
}
