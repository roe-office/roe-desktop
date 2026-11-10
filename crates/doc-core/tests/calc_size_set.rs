use doc_core::new_tile_cache;

fn close(a: f32, b: f32) -> bool { (a - b).abs() < 0.5 }

#[test]
fn no_document_returns_false() {
    let mut s = new_tile_cache();
    assert!(!s.set_column_width_pt(0, 0, 50.0));
    assert!(!s.set_row_height_pt(0, 0, 30.0));
}

#[test]
fn column_width_roundtrip() {
    let mut s = new_tile_cache();
    assert!(s.new_document(2) != 0, "新建 Calc 文档失败");
    assert!(s.set_column_width_pt(0, 2, 100.0));
    assert!(close(s.get_column_width_pt(0, 2), 100.0));
    // 其他列不受影响,且后续列的起点随之后移
    let before = s.get_col_offset_pt(0, 3);
    assert!(s.set_column_width_pt(0, 2, 120.0));
    assert!(s.get_col_offset_pt(0, 3) > before);
}

#[test]
fn row_height_roundtrip() {
    let mut s = new_tile_cache();
    assert!(s.new_document(2) != 0);
    assert!(s.set_row_height_pt(0, 4, 40.0));
    assert!(close(s.get_row_height_pt(0, 4), 40.0));
}
