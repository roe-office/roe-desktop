use doc_core::new_tile_cache;

#[test]
fn no_document_defaults() {
    let mut s = new_tile_cache();
    assert_eq!(s.get_frozen_panes(0), (0, 0));
    assert!(!s.set_frozen_panes(0, 1, 1));
}

#[test]
fn frozen_roundtrip() {
    let mut s = new_tile_cache();
    assert!(s.new_document(2) != 0, "新建 Calc 文档失败");
    assert_eq!(s.get_frozen_panes(0), (0, 0), "新文档默认不冻结");
    assert!(s.set_frozen_panes(0, 1, 2));
    assert_eq!(s.get_frozen_panes(0), (1, 2));
}

#[test]
fn frozen_only_rows_or_cols_and_clear() {
    let mut s = new_tile_cache();
    assert!(s.new_document(2) != 0);
    assert!(s.set_frozen_panes(0, 3, 0));
    assert_eq!(s.get_frozen_panes(0), (3, 0));
    assert!(s.set_frozen_panes(0, 0, 2));
    assert_eq!(s.get_frozen_panes(0), (0, 2));
    assert!(s.set_frozen_panes(0, 0, 0), "取消冻结应成功");
    assert_eq!(s.get_frozen_panes(0), (0, 0));
}

#[test]
fn frozen_out_of_range_rejected() {
    let mut s = new_tile_cache();
    assert!(s.new_document(2) != 0);
    assert!(!s.set_frozen_panes(0, 5000, 0), "行数越界应被拒绝");
    assert_eq!(s.get_frozen_panes(0), (0, 0), "被拒绝后状态不变");
    assert!(!s.set_frozen_panes(99, 1, 1), "sheet 越界应失败");
}
