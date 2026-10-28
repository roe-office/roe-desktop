use doc_core::new_tile_cache;

#[test]
fn open_parquet_then_page() {
    let home = match std::env::var("HOME") { Ok(h) => h, Err(_) => return };
    let dir = format!("{home}/smalldata-lab/hive-warehouse/polars_test.db/t_pq");
    if !std::path::Path::new(&dir).exists() { return; }

    let mut s = new_tile_cache();
    assert!(s.new_document(2) != 0, "新建 Calc 文档失败");

    let (rows, cols) = s.df_open_parquet(&format!("file:{dir}"), "t").unwrap();
    println!("open => rows={rows} cols={cols:?}");
    assert_eq!(rows, 3);
    assert_eq!(cols, vec!["id", "name", "amount"]);

    let p = s.df_page("t", 0, 2).unwrap();
    println!("page0 => {p:?}");
    assert_eq!(p.total_rows, 3);
    assert_eq!(p.rows.len(), 2);
    assert_eq!(p.rows[0][1], "alpha");

    let p2 = s.df_page("t", 2, 2).unwrap();
    println!("page1 => {p2:?}");
    assert_eq!(p2.rows.len(), 1);
    assert_eq!(p2.offset, 2);

    assert!(s.df_page("nope", 0, 2).is_err(), "不存在的数据集应报错");
    assert!(s.df_page("a\"b", 0, 2).is_err(), "含引号的名称应被拒绝");
    assert!(s.df_open_parquet("/nonexistent_dir_xyz", "x").is_err(), "坏路径应报错");
}
