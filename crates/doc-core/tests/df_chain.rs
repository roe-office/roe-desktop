use doc_core::new_tile_cache;

#[test]
fn chain_columns_preview_save_and_errors() {
    let p = std::env::temp_dir().join(format!("office_rs2_df_chain_{}.csv", std::process::id()));
    std::fs::write(&p, "a,b\n1,x\n2,x\n3,y\n").unwrap();
    let path = p.to_string_lossy().to_string();

    let mut s = new_tile_cache();
    assert!(s.new_document(2) != 0);
    let (name, _, _) = s.df_register_file(&path, "csv", "").unwrap();
    assert_eq!(name, "df_1");

    assert_eq!(s.df_columns("df_1").unwrap(), vec!["a", "b"]);
    assert!(s.df_columns("bad name").is_err());
    assert!(s.df_columns("nope").is_err());

    let chain = r#"df_use("df_1");
df_groupby_agg(`["b"]`, `[{"col":"a","fn":"sum","as":"s"}]`);
"#;
    let pv = s.df_chain_preview(chain).unwrap();
    println!("preview => {pv}");
    assert_eq!(pv["total_rows"], 2);
    assert_eq!(s.df_list_datasets().unwrap().len(), 1, "预览不应注册");

    let (n, rows, cols) = s.df_chain_save(chain).unwrap();
    assert_eq!(n, "chain_1");
    assert_eq!(rows, 2);
    assert_eq!(cols, vec!["b", "s"]);
    assert_eq!(s.df_list_datasets().unwrap().len(), 2);

    // 出错:报行号,且不新增数据集
    let bad = "df_use(\"df_1\");\ndf_use(\"missing\");\ndf_head(1);\n";
    let e1 = s.df_chain_preview(bad).unwrap_err();
    println!("err => {e1}");
    assert!(e1.contains("第2行"), "{e1}");
    let e2 = s.df_chain_save(bad).unwrap_err();
    assert!(e2.contains("第2行"), "{e2}");
    assert!(s.df_chain_save("  ").is_err());
    assert_eq!(s.df_list_datasets().unwrap().len(), 2);

    let _ = std::fs::remove_file(&p);
}
