use doc_core::new_tile_cache;

#[test]
fn preview_register_and_sheet_head_csv() {
    let p = std::env::temp_dir().join(format!("office_rs2_df_file_{}.csv", std::process::id()));
    std::fs::write(&p, "a,b\n1,x\n2,y\n3,z\n").unwrap();
    let path = p.to_string_lossy().to_string();

    let mut s = new_tile_cache();
    assert!(s.new_document(2) != 0, "新建 Calc 文档失败");

    let pv = s.df_preview_file(&path, "csv", r#"{"head":2}"#).unwrap();
    println!("preview => {pv}");
    assert_eq!(pv["rows"], 3);
    assert_eq!(pv["head"]["rows"].as_array().unwrap().len(), 2);

    let (name, rows, cols) = s.df_register_file(&path, "csv", "").unwrap();
    println!("register => {name} rows={rows} cols={cols:?}");
    assert_eq!(name, "df_1");
    assert_eq!(rows, 3);
    assert_eq!(cols, vec!["a", "b"]);

    let before = s.get_all_sheet_names().len();
    let idx = s.df_sheet_head(&name, 2).unwrap();
    let names = s.get_all_sheet_names();
    println!("sheet idx={idx} names={names:?}");
    assert_eq!(names.len(), before + 1);
    assert!(names.iter().any(|n| n == "df_1"));

    // 再来一次:重名不覆盖
    s.df_sheet_head(&name, 2).unwrap();
    assert_eq!(s.get_all_sheet_names().len(), before + 2);

    assert!(s.df_sheet_head("nope", 2).is_err(), "不存在的数据集应报错");
    assert!(s.df_sheet_head("a\"b", 2).is_err(), "含引号的名称应被拒绝");
    assert!(s.df_preview_file("/nonexistent_df_file.csv", "csv", "").is_err());
    assert!(s.df_preview_file(&path, "bogus", "").is_err());
    let _ = std::fs::remove_file(&p);
}
