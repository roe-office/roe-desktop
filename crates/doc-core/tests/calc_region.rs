use doc_core::{new_tile_cache, parse_region_json, parse_row_texts_json};

#[test]
fn parse_region_variants() {
    // 字符串化 JSON(query_script 常见形态)
    let raw = r#"{"ok":true,"value":"{\"start\":[0,0],\"end\":[3,2]}"}"#;
    assert_eq!(parse_region_json(raw), Some((0, 0, 3, 2)));
    // 已展开对象
    let raw2 = r#"{"ok":true,"value":{"start":[1,2],"end":[5,6]}}"#;
    assert_eq!(parse_region_json(raw2), Some((1, 2, 5, 6)));
    // 无数据
    let none = r#"{"ok":true,"value":"{\"direction\":\"none\"}"}"#;
    assert_eq!(parse_region_json(none), None);
    // 失败
    assert_eq!(parse_region_json(r#"{"ok":false,"error":"x"}"#), None);
    assert_eq!(parse_region_json("not json"), None);
}

#[test]
fn parse_row_texts_variants() {
    let raw = r#"{"ok":true,"value":"[\"a\\\"b\",\"\",\"3\"]"}"#;
    assert_eq!(parse_row_texts_json(raw), Some(vec!["a\"b".to_string(), "".to_string(), "3".to_string()]));
    let raw2 = r#"{"ok":true,"value":["x","y"]}"#;
    assert_eq!(parse_row_texts_json(raw2), Some(vec!["x".to_string(), "y".to_string()]));
    assert_eq!(parse_row_texts_json(r#"{"ok":false}"#), None);
}

#[test]
fn region_and_rows_end_to_end() {
    let mut s = new_tile_cache();
    assert!(s.new_document(2) != 0, "新建 Calc 文档失败");
    // 表头 + 2 行数据,A1 留空
    assert!(s.calc_set_cell(0, 0, 1, "销售额"));
    assert!(s.calc_set_cell(0, 0, 2, "利润"));
    assert!(s.calc_set_cell(0, 1, 0, "一月"));
    assert!(s.calc_set_cell(0, 1, 1, "100"));
    assert!(s.calc_set_cell(0, 1, 2, "20"));
    assert!(s.calc_set_cell(0, 2, 0, "二月"));
    assert!(s.calc_set_cell(0, 2, 1, "200"));
    assert!(s.calc_set_cell(0, 2, 2, "30"));

    // 从区域内任一格出发,都应探测到 (0,0)-(2,2)
    assert_eq!(s.calc_current_region(0, 1, 1), Some((0, 0, 2, 2)));
    assert_eq!(s.calc_current_region(0, 2, 2), Some((0, 0, 2, 2)));
    // 远处空白格 → None
    assert_eq!(s.calc_current_region(0, 50, 50), None);

    let rows = s.calc_get_rows_text(0, 0, 0, 2, 2, 500);
    println!("rows => {rows:?}");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], vec!["", "销售额", "利润"]);
    assert_eq!(rows[1], vec!["一月", "100", "20"]);

    // max_rows 截断
    assert_eq!(s.calc_get_rows_text(0, 0, 0, 2, 2, 2).len(), 2);
    // 反向区间
    assert!(s.calc_get_rows_text(0, 2, 0, 1, 2, 500).is_empty());
}
