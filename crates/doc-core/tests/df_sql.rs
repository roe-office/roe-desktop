use doc_core::new_tile_cache;

#[test]
fn list_preview_save_sql() {
    let p = std::env::temp_dir().join(format!("office_rs2_df_sql_{}.csv", std::process::id()));
    std::fs::write(&p, "a,b\n1,x\n2,y\n3,z\n").unwrap();
    let path = p.to_string_lossy().to_string();

    let mut s = new_tile_cache();
    assert!(s.new_document(2) != 0, "新建 Calc 文档失败");

    // 空容器:列表为空,预览/保存报错
    assert!(s.df_list_datasets().unwrap().is_empty());
    assert!(s.df_sql_preview("SELECT 1").is_err());
    assert!(s.df_sql_save("SELECT 1").is_err());

    let (name, _, _) = s.df_register_file(&path, "csv", "").unwrap();
    assert_eq!(name, "df_1");
    assert_eq!(s.df_list_datasets().unwrap(), vec![("df_1".to_string(), 2u64)]);

    // 预览:SQL 含双引号与换行,验证转义
    let sql = "SELECT \"a\", b FROM df_1\nWHERE a >= 2\nORDER BY a";
    let pv = s.df_sql_preview(sql).unwrap();
    println!("preview => {pv}");
    assert_eq!(pv["total_rows"], 2);
    assert_eq!(pv["columns"].as_array().unwrap().len(), 2);
    // 预览不注册
    assert_eq!(s.df_list_datasets().unwrap().len(), 1);

    // 单引号字符串
    let pv = s.df_sql_preview("SELECT a FROM df_1 WHERE b = 'x'").unwrap();
    assert_eq!(pv["total_rows"], 1);

    // 保存:新数据集 df_2
    let (n2, rows, cols) = s.df_sql_save(sql).unwrap();
    println!("save => {n2} rows={rows} cols={cols:?}");
    assert_eq!(n2, "sql_1");
    assert_eq!(rows, 2);
    assert_eq!(cols, vec!["a", "b"]);
    assert_eq!(s.df_list_datasets().unwrap().len(), 2);

    // 新数据集可写预览 sheet
    let before = s.get_all_sheet_names().len();
    s.df_sheet_head(&n2, 50).unwrap();
    assert_eq!(s.get_all_sheet_names().len(), before + 1);

    // 错误路径:不新增数据集
    assert!(s.df_sql_preview("SELEKT oops").is_err());
    assert!(s.df_sql_save("SELEKT oops").is_err());
    assert!(s.df_sql_save("   ").is_err());
    assert_eq!(s.df_list_datasets().unwrap().len(), 2);

    let _ = std::fs::remove_file(&p);
}
