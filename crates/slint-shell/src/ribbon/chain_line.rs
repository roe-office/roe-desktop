//! DF 链对话框:由「已选列 + 参数」生成一行 Rhai 语句。纯函数,不依赖界面,可单测。
//! 约定:JSON 参数一律放在反引号字符串里;列名含反引号或 `${` 时拒绝生成。

pub const AGG_FNS: [&str; 9] = ["sum", "mean", "min", "max", "count", "first", "last", "std", "var"];

fn check_name(n: &str) -> Result<(), String> {
    if n.contains('`') || n.contains("${") {
        Err(format!("列名「{n}」含反引号或 ${{,无法自动生成,请手写"))
    } else {
        Ok(())
    }
}

/// 生成带双引号的 JSON 字符串(同样适用于 Rhai 普通双引号串)。
fn jstr(s: &str) -> String {
    let mut o = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn jarr(cols: &[String]) -> Result<String, String> {
    for c in cols {
        check_name(c)?;
    }
    Ok(format!("[{}]", cols.iter().map(|c| jstr(c)).collect::<Vec<_>>().join(",")))
}

/// kind: head/select/drop/sort/dedup/dropnulls/rename/groupby/withcol
/// sel: 已选列(按点击顺序);n: 行数文本;desc: 排序是否降序;agg: 聚合函数名。
pub fn build_chain_line(kind: &str, sel: &[String], n: &str, desc: bool, agg: &str) -> Result<String, String> {
    let need = |k: usize, what: &str| -> Result<(), String> {
        if sel.len() < k {
            Err(format!("请先在「列名」里选中{what}"))
        } else {
            Ok(())
        }
    };
    match kind {
        "head" => {
            if n.trim().is_empty() {
                return Err("请先在「输出行数」里选前10/20/50,再点「取前N行」".to_string());
            }
            let v: u32 = n.trim().parse().map_err(|_| format!("行数 N「{}」不是有效的正整数", n.trim()))?;
            if v == 0 {
                return Err("行数 N 必须大于 0".to_string());
            }
            Ok(format!("df_head({v});"))
        }
        "select" => {
            need(1, "至少 1 列")?;
            Ok(format!("df_select(`{}`);", jarr(sel)?))
        }
        "drop" => {
            need(1, "至少 1 列")?;
            Ok(format!("df_drop(`{}`);", jarr(sel)?))
        }
        "sort" => {
            need(1, "至少 1 列")?;
            let d = vec![if desc { "true" } else { "false" }; sel.len()].join(",");
            Ok(format!("df_sort(`{}`, `[{}]`);", jarr(sel)?, d))
        }
        "dedup" => Ok(format!("df_dedup(`{}`);", jarr(sel)?)),
        "dropnulls" => Ok(format!("df_drop_nulls(`{}`);", jarr(sel)?)),
        "rename" => {
            need(1, "要改名的列")?;
            check_name(&sel[0])?;
            Ok(format!("df_rename({}, \"新名\");", jstr(&sel[0])))
        }
        "groupby" => {
            need(2, "至少 2 列(最后选中的是被聚合列,其余是分组列)")?;
            if !AGG_FNS.contains(&agg) {
                return Err(format!("不支持的聚合函数: {agg}"));
            }
            let (keys, last) = sel.split_at(sel.len() - 1);
            let c = &last[0];
            check_name(c)?;
            Ok(format!(
                "df_groupby_agg(`{}`, `[{{\"col\":{},\"fn\":\"{}\",\"as\":{}}}]`);",
                jarr(keys)?,
                jstr(c),
                agg,
                jstr(&format!("{agg}_{c}"))
            ))
        }
        "withcol" => {
            need(1, "1~2 列")?;
            check_name(&sel[0])?;
            let a = jstr(&sel[0]);
            let body = if sel.len() >= 2 {
                check_name(&sel[1])?;
                format!("{{\"a\":{a},\"op\":\"mul\",\"b\":{}}}", jstr(&sel[1]))
            } else {
                format!("{{\"a\":{a},\"op\":\"mul\",\"val\":100}}")
            };
            Ok(format!("df_with_column(\"新列\", `{body}`);"))
        }
        other => Err(format!("未知模板: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn head_and_errors() {
        assert_eq!(build_chain_line("head", &[], " 100 ", false, "sum").unwrap(), "df_head(100);");
        assert!(build_chain_line("head", &[], "abc", false, "sum").is_err());
        assert!(build_chain_line("head", &[], "0", false, "sum").is_err());
        assert!(build_chain_line("head", &[], "", false, "sum").is_err());
    }

    #[test]
    fn select_sort_dedup() {
        let s = v(&["Region", "Sales"]);
        assert_eq!(build_chain_line("select", &s, "", false, "").unwrap(), "df_select(`[\"Region\",\"Sales\"]`);");
        assert_eq!(
            build_chain_line("sort", &v(&["Sales"]), "", true, "").unwrap(),
            "df_sort(`[\"Sales\"]`, `[true]`);"
        );
        assert_eq!(
            build_chain_line("sort", &s, "", false, "").unwrap(),
            "df_sort(`[\"Region\",\"Sales\"]`, `[false,false]`);"
        );
        assert_eq!(build_chain_line("dedup", &[], "", false, "").unwrap(), "df_dedup(`[]`);");
        assert!(build_chain_line("select", &[], "", false, "").is_err());
    }

    #[test]
    fn groupby_last_is_agg_col() {
        let s = v(&["Region", "Quarter", "Sales"]);
        assert_eq!(
            build_chain_line("groupby", &s, "", false, "sum").unwrap(),
            "df_groupby_agg(`[\"Region\",\"Quarter\"]`, `[{\"col\":\"Sales\",\"fn\":\"sum\",\"as\":\"sum_Sales\"}]`);"
        );
        assert!(build_chain_line("groupby", &v(&["Sales"]), "", false, "sum").is_err());
        assert!(build_chain_line("groupby", &s, "", false, "median").is_err());
    }

    #[test]
    fn rename_withcol_and_bad_names() {
        assert_eq!(
            build_chain_line("rename", &v(&["Sales"]), "", false, "").unwrap(),
            "df_rename(\"Sales\", \"新名\");"
        );
        assert_eq!(
            build_chain_line("withcol", &v(&["Sales"]), "", false, "").unwrap(),
            "df_with_column(\"新列\", `{\"a\":\"Sales\",\"op\":\"mul\",\"val\":100}`);"
        );
        assert_eq!(
            build_chain_line("withcol", &v(&["Sales", "Quantity"]), "", false, "").unwrap(),
            "df_with_column(\"新列\", `{\"a\":\"Sales\",\"op\":\"mul\",\"b\":\"Quantity\"}`);"
        );
        assert!(build_chain_line("select", &v(&["a`b"]), "", false, "").is_err());
        assert!(build_chain_line("select", &v(&["a${b"]), "", false, "").is_err());
        assert!(build_chain_line("nope", &[], "", false, "").is_err());
    }
}
