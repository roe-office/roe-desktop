//! 插入图表类型注册表（v221）。
//! 每种图表是 TYPES 里的一行：所属分类、名称、ChartKind、grouping、是否柱线组合。
//! 以后接通股价图/统计类图表，只需在 CATS 和 TYPES 里各加行。
//! 宿主无关：Impress/Calc/Writer 共用，数据由 chart_json 生成假数据。

use serde_json::{json, Value};

pub struct ChartTypeDef {
    pub cat: usize,
    pub name: &'static str,
    pub kind: &'static str,
    pub grouping: &'static str,
    pub combo: bool,
    pub id: &'static str,
}

pub const CATS: [&str; 14] = [
    "柱形图", "折线图", "饼图", "条形图", "面积图", "XY（散点图）", "雷达图", "组合图", "瀑布图", "漏斗图", "直方图", "箱形图", "旭日图", "树状图",
];

const fn d(cat: usize, name: &'static str, kind: &'static str, grouping: &'static str, combo: bool, id: &'static str) -> ChartTypeDef {
    ChartTypeDef { cat, name, kind, grouping, combo, id }
}

pub static TYPES: [ChartTypeDef; 23] = [
    d(0, "簇状柱形图", "Bar", "Standard", false, "bar"),
    d(0, "堆积柱形图", "Bar", "Stacked", false, "bar-stacked"),
    d(0, "百分比堆积柱形图", "Bar", "PercentStacked", false, "bar-pct"),
    d(1, "折线图", "Line", "Standard", false, "line"),
    d(1, "堆积折线图", "Line", "Stacked", false, "line-stacked"),
    d(1, "百分比堆积折线图", "Line", "PercentStacked", false, "line-pct"),
    d(2, "饼图", "Pie", "Standard", false, "pie"),
    d(2, "圆环图", "Doughnut", "Standard", false, "doughnut"),
    d(3, "簇状条形图", "HorizontalBar", "Standard", false, "hbar"),
    d(3, "堆积条形图", "HorizontalBar", "Stacked", false, "hbar-stacked"),
    d(3, "百分比堆积条形图", "HorizontalBar", "PercentStacked", false, "hbar-pct"),
    d(4, "面积图", "Area", "Standard", false, "area"),
    d(4, "堆积面积图", "Area", "Stacked", false, "area-stacked"),
    d(4, "百分比堆积面积图", "Area", "PercentStacked", false, "area-pct"),
    d(5, "散点图", "Scatter", "Standard", false, "scatter"),
    d(6, "雷达图", "Radar", "Standard", false, "radar"),
    d(7, "簇状柱形图-折线图", "Bar", "Standard", true, "combo"),
    d(8, "瀑布图", "Waterfall", "Standard", false, "waterfall"),
    d(9, "漏斗图", "Funnel", "Standard", false, "funnel"),
    d(10, "直方图", "Histogram", "Standard", false, "histogram"),
    d(11, "箱形图", "BoxWhisker", "Standard", false, "boxwhisker"),
    d(12, "旭日图", "Sunburst", "Standard", false, "sunburst"),
    d(13, "树状图", "Treemap", "Standard", false, "treemap"),
];

pub fn types_in_cat(cat: usize) -> Vec<&'static ChartTypeDef> {
    TYPES.iter().filter(|t| t.cat == cat).collect()
}

pub fn find(cat: usize, ty: usize) -> Option<&'static ChartTypeDef> {
    types_in_cat(cat).get(ty).copied()
}

/// 生成该类型的假数据 JSON。thumb=true：无标题、无图例（缩略图用）。
/// 同一份 JSON 既能喂 roe_render_chart_to_png（预览），也能喂 roe_impress_insert_chart（插入）。
pub fn chart_json(def: &ChartTypeDef, w: u32, h: u32, thumb: bool) -> String {
    let (labels, series): (Vec<&str>, Vec<Value>) = match def.kind {
        "Pie" | "Doughnut" => (
            vec!["第一季度", "第二季度", "第三季度", "第四季度"],
            vec![json!({"name": "销售额", "data": [8.2, 3.2, 1.4, 1.2]})],
        ),
        "Scatter" => (
            vec!["1", "2", "3", "4", "5"],
            vec![
                json!({"name": "系列 1", "data": [3.2, 4.1, 2.8, 5.0, 3.9]}),
                json!({"name": "系列 2", "data": [1.5, 2.6, 3.1, 2.2, 4.0]}),
            ],
        ),
        "Radar" => (
            vec!["类别 1", "类别 2", "类别 3", "类别 4", "类别 5"],
            vec![
                json!({"name": "系列 1", "data": [4.0, 3.0, 4.5, 2.5, 3.5]}),
                json!({"name": "系列 2", "data": [2.0, 4.0, 2.5, 4.5, 3.0]}),
            ],
        ),
        _ => (
            vec!["类别 1", "类别 2", "类别 3", "类别 4"],
            vec![
                json!({"name": "系列 1", "data": [4.3, 2.5, 3.5, 4.5]}),
                json!({"name": "系列 2", "data": [2.4, 4.4, 1.8, 2.8]}),
                json!({"name": "系列 3", "data": [2.0, 2.0, 3.0, 5.0], "as_line": def.combo}),
            ],
        ),
    };
    let title = if thumb { Value::Null } else { json!("图表标题") };
    json!({
        "kind": def.kind,
        "title": title,
        "width": w,
        "height": h,
        "x_labels": labels,
        "series": series,
        "grouping": def.grouping,
        "show_legend": !thumb,
        "show_grid": !thumb,
    })
    .to_string()
}

/// 插入用 JSON（尺寸由插入接口的 x/y/w/h 决定）。
pub fn insert_json(cat: i32, ty: i32) -> Option<String> {
    if cat < 0 || ty < 0 { return None; }
    let def = find(cat as usize, ty as usize)?;
    if is_stat_def(def) { return Some(stat_json(def)); }
    Some(chart_json(def, 600, 400, false))
}

// v222: 统计类图表（目前仅漏斗、瀑布；其余四种后续补）
pub fn is_stat_def(def: &ChartTypeDef) -> bool {
    matches!(def.kind, "Waterfall" | "Funnel" | "Histogram" | "BoxWhisker" | "Sunburst" | "Treemap")
}

pub fn is_stat(cat: i32, ty: i32) -> bool {
    if cat < 0 || ty < 0 { return false; }
    find(cat as usize, ty as usize).map(is_stat_def).unwrap_or(false)
}

/// 统计图假数据：完整 StatChartModel JSON（该模型字段无默认值，需写全）
pub fn stat_json(def: &ChartTypeDef) -> String {
    let series_of = |name: &str, layout: &str, data_id: u32, subtotals: Vec<usize>, bin_width: Option<f64>, labels: Value| -> Value {
        json!({
            "name": name, "layout_id": layout, "format_idx": data_id, "data_id": data_id,
            "bin_width": bin_width, "bin_count": null, "interval_closed_right": true,
            "quartile_exclusive": true, "mean_line": false, "mean_marker": true,
            "show_outliers": true, "show_non_outliers": true,
            "subtotals": subtotals, "data_labels": labels
        })
    };
    // (gap_width, show_legend, show_grid, data, series)
    let (gap, legend, grid, data, series): (Value, bool, bool, Value, Value) = match def.kind {
        "Waterfall" => (
            Value::Null, false, true,
            json!([{ "id": 0, "categories": [["起始", "收入", "支出", "追加", "合计"]],
                     "values": [250.0, 80.0, -60.0, 40.0, 310.0], "value_kind": "val" }]),
            json!([series_of("系列 1", "waterfall", 0, vec![4], None, Value::Null)]),
        ),
        "Funnel" => (
            Value::Null, false, true,
            json!([{ "id": 0, "categories": [["访问", "咨询", "报价", "成交"]],
                     "values": [1000.0, 800.0, 500.0, 300.0], "value_kind": "val" }]),
            json!([series_of("系列 1", "funnel", 0, vec![], None, Value::Null)]),
        ),
        "Histogram" => (
            json!(0.0), false, true,
            json!([{ "id": 0, "categories": [],
                     "values": [10.0, 12.0, 14.0,
                                16.0, 16.0, 17.0, 18.0, 18.0, 19.0, 20.0,
                                21.0, 21.0, 22.0, 22.0, 23.0, 23.0, 24.0, 24.0, 25.0, 25.0,
                                26.0, 27.0, 27.0, 28.0, 29.0, 30.0,
                                32.0, 35.0],
                     "value_kind": "val" }]),
            json!([series_of("系列 1", "clusteredColumn", 0, vec![], Some(5.0), Value::Null)]),
        ),
        "BoxWhisker" => {
            let mk = |id: u32, outlier: bool| -> Value {
                let mut cats: Vec<&str> = vec!["A"; 9];
                cats.extend(vec!["B"; 9]);
                let off = id as f64 * 3.0;
                let mut vals: Vec<f64> = [14.0, 18.0, 20.0, 22.0, 24.0, 26.0, 28.0, 31.0].iter().map(|v| v + off).collect();
                vals.push(if outlier { 58.0 } else { 35.0 });
                vals.extend([8.0, 11.0, 14.0, 16.0, 18.0, 21.0, 23.0, 26.0, 29.0].iter().map(|v| v + off));
                json!({ "id": id, "categories": [cats], "values": vals, "value_kind": "val" })
            };
            (
                json!(1.0), false, true,
                json!([mk(0, true), mk(1, false)]),
                json!([
                    series_of("系列 1", "boxWhisker", 0, vec![], None, Value::Null),
                    series_of("系列 2", "boxWhisker", 1, vec![], None, Value::Null)
                ]),
            )
        }
        _ => {
            // 旭日 / 树状：分类从叶子层到最顶层；叶子名为空的行，叶子名写在上一层
            let layout = if def.kind == "Sunburst" { "sunburst" } else { "treemap" };
            let labels = json!({ "pos": "ctr", "show_value": false, "show_category": true, "show_series": false });
            (
                json!(0.5), true, false,
                json!([{ "id": 0,
                         "categories": [
                             ["类别A", "类别B", "类别C", "类别D", "类别E"],
                             ["子类1", "子类1", "子类2", "子类3", "子类3"],
                             ["组1", "组1", "组1", "组2", "组2"]
                         ],
                         "values": [3.0, 2.0, 4.0, 3.0, 2.0], "value_kind": "size" }]),
                json!([series_of("系列 1", layout, 0, vec![], None, labels)]),
            )
        }
    };
    json!({
        "kind": def.kind,
        "title_present": true,
        "title": "图表标题",
        "show_legend": legend,
        "legend_pos": null,
        "show_grid": grid,
        "gap_width": gap,
        "data": data,
        "series": series
    }).to_string()
}

// ───────────── v261: Calc 真实数据（纯函数，不依赖界面/会话） ─────────────

/// 从表格区域提取出的图表数据。
#[derive(Debug, Clone, PartialEq)]
pub struct RealData {
    pub x_labels: Vec<String>,
    pub series: Vec<(String, Vec<f64>)>,
    /// 提取过程中的提示（非数值按 0、超限截断等），由调用方显示在状态栏。
    pub warnings: Vec<String>,
}

pub const MAX_POINTS: usize = 500;
pub const MAX_SERIES: usize = 12;

/// 宽松数值解析：去首尾空白、千分位逗号；尾部 % 除以 100；拒绝 inf/nan。
pub fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() { return None; }
    let (body, pct) = match t.strip_suffix('%') { Some(b) => (b.trim(), true), None => (t, false) };
    let cleaned: String = body.chars().filter(|c| *c != ',').collect();
    let v: f64 = cleaned.parse().ok()?;
    if !v.is_finite() { return None; }
    Some(if pct { v / 100.0 } else { v })
}

fn cell_at(cells: &[Vec<String>], r: usize, c: usize) -> &str {
    cells.get(r).and_then(|row| row.get(c)).map(|s| s.as_str()).unwrap_or("")
}

/// 把选区矩阵（行优先的显示文本）转成图表数据。无法成图返回 None。
pub fn extract_chart_data(cells: &[Vec<String>]) -> Option<RealData> {
    let ncols = cells.iter().map(|r| r.len()).max()?;
    if ncols == 0 { return None; }
    let mut warnings: Vec<String> = Vec::new();

    // 单列：必须全是数值，分类取 1..n
    if ncols == 1 {
        let mut vals: Vec<f64> = Vec::new();
        for r in 0..cells.len() {
            let t = cell_at(cells, r, 0).trim();
            if t.is_empty() { continue; }
            vals.push(parse_num(t)?);
        }
        if vals.is_empty() { return None; }
        if vals.len() > MAX_POINTS {
            vals.truncate(MAX_POINTS);
            warnings.push(format!("数据点超过 {}，已截断", MAX_POINTS));
        }
        let x_labels = (1..=vals.len()).map(|i| i.to_string()).collect();
        return Some(RealData { x_labels, series: vec![("系列 1".to_string(), vals)], warnings });
    }

    // 表头：首行第 2 列起全是非数字文本，且至少有一个非空
    let has_header = (1..ncols).all(|c| {
        let t = cell_at(cells, 0, c).trim();
        t.is_empty() || parse_num(t).is_none()
    }) && (1..ncols).any(|c| !cell_at(cells, 0, c).trim().is_empty());
    let start = if has_header { 1 } else { 0 };

    let mut rows: Vec<usize> = (start..cells.len())
        .filter(|&r| (0..ncols).any(|c| !cell_at(cells, r, c).trim().is_empty()))
        .collect();
    if rows.is_empty() { return None; }
    if rows.len() > MAX_POINTS {
        rows.truncate(MAX_POINTS);
        warnings.push(format!("数据行超过 {}，已截断", MAX_POINTS));
    }

    let x_labels: Vec<String> = rows.iter().enumerate().map(|(i, &r)| {
        let t = cell_at(cells, r, 0).trim();
        if t.is_empty() { (i + 1).to_string() } else { t.to_string() }
    }).collect();

    let mut series: Vec<(String, Vec<f64>)> = Vec::new();
    let mut bad = 0usize;
    for c in 1..ncols {
        if !rows.iter().any(|&r| parse_num(cell_at(cells, r, c)).is_some()) { continue; }
        if series.len() >= MAX_SERIES {
            warnings.push(format!("系列超过 {}，多余的列已忽略", MAX_SERIES));
            break;
        }
        let head = if has_header { cell_at(cells, 0, c).trim() } else { "" };
        let name = if head.is_empty() { format!("系列 {}", series.len() + 1) } else { head.to_string() };
        let vals: Vec<f64> = rows.iter().map(|&r| {
            let raw = cell_at(cells, r, c);
            match parse_num(raw) {
                Some(v) => v,
                None => { if !raw.trim().is_empty() { bad += 1; } 0.0 }
            }
        }).collect();
        series.push((name, vals));
    }
    if series.is_empty() { return None; }
    if bad > 0 { warnings.push(format!("{} 个非数值单元格按 0 处理", bad)); }
    Some(RealData { x_labels, series, warnings })
}

/// 用真实数据生成图表 JSON。统计类图表（Calc 不支持）或无数据返回 None。
/// 同时写 width/height（roe_render_chart_to_png）与 width_pt/height_pt（insert_chart）。
pub fn chart_json_real(def: &ChartTypeDef, data: &RealData, w_pt: f32, h_pt: f32, title: Option<&str>, thumb: bool) -> Option<String> {
    if is_stat_def(def) || data.series.is_empty() { return None; }
    let n = if matches!(def.kind, "Pie" | "Doughnut") { 1 } else { data.series.len() };
    let last = n - 1;
    let series: Vec<Value> = data.series.iter().take(n).enumerate().map(|(i, (name, vals))| {
        json!({ "name": name, "data": vals, "as_line": def.combo && n >= 2 && i == last })
    }).collect();
    let title_v = if thumb {
        Value::Null
    } else {
        match title { Some(t) if !t.trim().is_empty() => json!(t), _ => json!("图表标题") }
    };
    Some(json!({
        "kind": def.kind,
        "title": title_v,
        "width": w_pt, "height": h_pt,
        "width_pt": w_pt, "height_pt": h_pt,
        "x_labels": data.x_labels,
        "series": series,
        "grouping": def.grouping,
        "show_legend": !thumb,
        "show_grid": !thumb,
    }).to_string())
}

#[cfg(test)]
mod real_data_tests {
    use super::*;

    fn m(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|r| r.iter().map(|s| s.to_string()).collect()).collect()
    }

    #[test]
    fn parse_num_variants() {
        assert_eq!(parse_num(" 1,234 "), Some(1234.0));
        assert_eq!(parse_num("12%"), Some(0.12));
        assert_eq!(parse_num("-3.5"), Some(-3.5));
        assert_eq!(parse_num(""), None);
        assert_eq!(parse_num("abc"), None);
        assert_eq!(parse_num("inf"), None);
    }

    #[test]
    fn header_and_two_series() {
        let d = extract_chart_data(&m(&[&["", "销售额", "利润"], &["一月", "100", "20"], &["二月", "200", "30"]])).unwrap();
        assert_eq!(d.x_labels, vec!["一月", "二月"]);
        assert_eq!(d.series, vec![("销售额".to_string(), vec![100.0, 200.0]), ("利润".to_string(), vec![20.0, 30.0])]);
        assert!(d.warnings.is_empty());
    }

    #[test]
    fn no_header_uses_default_names() {
        let d = extract_chart_data(&m(&[&["A", "1", "2"], &["B", "3", "4"]])).unwrap();
        assert_eq!(d.x_labels, vec!["A", "B"]);
        assert_eq!(d.series[0].0, "系列 1");
        assert_eq!(d.series[1].0, "系列 2");
        assert_eq!(d.series[1].1, vec![2.0, 4.0]);
    }

    #[test]
    fn text_column_is_dropped() {
        let d = extract_chart_data(&m(&[&["", "数", "备注"], &["a", "1", "x"], &["b", "2", "y"]])).unwrap();
        assert_eq!(d.series.len(), 1);
        assert_eq!(d.series[0].0, "数");
    }

    #[test]
    fn non_numeric_cell_becomes_zero_with_warning() {
        let d = extract_chart_data(&m(&[&["", "v"], &["a", "1"], &["b", "abc"]])).unwrap();
        assert_eq!(d.series[0].1, vec![1.0, 0.0]);
        assert_eq!(d.warnings.len(), 1);
    }

    #[test]
    fn blank_rows_skipped_and_header_only_is_none() {
        let d = extract_chart_data(&m(&[&["", "v"], &["a", "1"], &["", ""]])).unwrap();
        assert_eq!(d.x_labels, vec!["a"]);
        assert!(extract_chart_data(&m(&[&["", "v"]])).is_none());
    }

    #[test]
    fn single_column() {
        let d = extract_chart_data(&m(&[&["3"], &["5"]])).unwrap();
        assert_eq!(d.x_labels, vec!["1", "2"]);
        assert_eq!(d.series, vec![("系列 1".to_string(), vec![3.0, 5.0])]);
        assert!(extract_chart_data(&m(&[&["a"], &["b"]])).is_none());
    }

    fn sample3() -> RealData {
        extract_chart_data(&m(&[&["", "x", "y", "z"], &["a", "1", "2", "3"], &["b", "4", "5", "6"]])).unwrap()
    }

    #[test]
    fn json_pie_keeps_first_series_only() {
        let j = chart_json_real(find(2, 0).unwrap(), &sample3(), 560.0, 340.0, Some("T"), false).unwrap();
        let v: Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["kind"], "Pie");
        assert_eq!(v["series"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn json_combo_marks_last_series_as_line() {
        let j = chart_json_real(find(7, 0).unwrap(), &sample3(), 560.0, 340.0, None, false).unwrap();
        let v: Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["series"][0]["as_line"], false);
        assert_eq!(v["series"][2]["as_line"], true);
    }

    #[test]
    fn json_size_fields_and_thumb() {
        let j = chart_json_real(find(0, 0).unwrap(), &sample3(), 560.0, 340.0, Some("T"), true).unwrap();
        let v: Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["width_pt"].as_f64().unwrap(), 560.0);
        assert_eq!(v["height"].as_f64().unwrap(), 340.0);
        assert!(v["title"].is_null());
        assert_eq!(v["show_legend"], false);
    }

    #[test]
    fn json_stat_is_none() {
        assert!(chart_json_real(find(8, 0).unwrap(), &sample3(), 560.0, 340.0, None, false).is_none());
    }
}

// ───────────── Calc 选区 JSON → 文本矩阵（纯函数，供 extract_chart_data 使用） ─────────────

fn range_val_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            Some(f) => format!("{}", f),
            None => n.to_string(),
        },
        _ => String::new(),
    }
}

/// roe_get_range_json 的返回 → 行优先文本矩阵。无法解析返回 None。
/// 取值优先级：value → cached_value（公式格）→ 空串（空格/错误格/合并区非左上角）。
pub fn range_json_to_matrix(json: &str) -> Option<Vec<Vec<String>>> {
    let v: Value = serde_json::from_str(json).ok()?;
    let rows = v.get("rows")?.as_array()?;
    Some(rows.iter().map(|row| {
        row.as_array().map(|cells| cells.iter().map(|c| {
            if let Some(x) = c.get("value") { return range_val_text(x); }
            if let Some(x) = c.get("cached_value") { return range_val_text(x); }
            String::new()
        }).collect()).unwrap_or_default()
    }).collect())
}

#[cfg(test)]
mod range_matrix_tests {
    use super::range_json_to_matrix;

    #[test]
    fn converts_values_formulas_and_blanks() {
        let j = r#"{"rows":[
          [{"row":0,"col":0,"value":"品类"},{"row":0,"col":1,"value":"销量"}],
          [{"row":1,"col":0,"value":2020.0},{"row":1,"col":1,"formula_source":"=1+2","cached_value":3.5}],
          [{"row":2,"col":0},{"row":2,"col":1,"merged_into":[1,1]}],
          [{"row":3,"col":0,"error":"Div0"},{"row":3,"col":1,"value":true}]
        ]}"#;
        let m = range_json_to_matrix(j).unwrap();
        assert_eq!(m[0], vec!["品类", "销量"]);
        assert_eq!(m[1], vec!["2020", "3.5"]);
        assert_eq!(m[2], vec!["", ""]);
        assert_eq!(m[3], vec!["", "true"]);
    }

    #[test]
    fn bad_json_is_none() {
        assert!(range_json_to_matrix("{oops").is_none());
        assert!(range_json_to_matrix("{\"x\":1}").is_none());
    }

    #[test]
    fn feeds_extract_chart_data() {
        let j = r#"{"rows":[
          [{"row":0,"col":0,"value":"Label"},{"row":0,"col":1,"value":"A"}],
          [{"row":1,"col":0,"value":"Q1"},{"row":1,"col":1,"value":10.0}],
          [{"row":2,"col":0,"value":"Q2"},{"row":2,"col":1,"value":20.5}]
        ]}"#;
        let d = crate::chart_registry::extract_chart_data(&range_json_to_matrix(j).unwrap()).unwrap();
        assert_eq!(d.x_labels, vec!["Q1", "Q2"]);
        assert_eq!(d.series, vec![("A".to_string(), vec![10.0, 20.5])]);
    }
}

// ───────────── 统计图真实数据（在 stat_json 假数据骨架上替换 data/subtotals） ─────────────

/// 用选区数据生成统计图 JSON。非统计类或数据不足返回 None。
/// 瀑布/漏斗：分类+数值（取第 1 个系列）；直方图：全部数值展平；
/// 箱线图：每个系列一组，系列名作分类；旭日/树状：退化为单层分类+数值（size）。
pub fn stat_json_real(def: &ChartTypeDef, data: &RealData, title: Option<&str>) -> Option<String> {
    if !is_stat_def(def) || data.series.is_empty() { return None; }
    let mut v: Value = serde_json::from_str(&stat_json(def)).ok()?;
    let first = &data.series[0].1;
    let new_data: Value = match def.kind {
        "Waterfall" | "Funnel" => {
            if first.is_empty() { return None; }
            json!([{ "id": 0, "categories": [data.x_labels.clone()], "values": first, "value_kind": "val" }])
        }
        "Histogram" => {
            let all: Vec<f64> = data.series.iter().flat_map(|(_, v)| v.iter().copied()).collect();
            if all.len() < 2 { return None; }
            json!([{ "id": 0, "categories": [], "values": all, "value_kind": "val" }])
        }
        "BoxWhisker" => {
            let mut cats: Vec<String> = Vec::new();
            let mut vals: Vec<f64> = Vec::new();
            for (name, vs) in &data.series {
                for x in vs { cats.push(name.clone()); vals.push(*x); }
            }
            if vals.len() < 2 { return None; }
            json!([{ "id": 0, "categories": [cats], "values": vals, "value_kind": "val" }])
        }
        _ => {
            if first.is_empty() { return None; }
            json!([{ "id": 0, "categories": [data.x_labels.clone()], "values": first, "value_kind": "size" }])
        }
    };
    v["data"] = new_data;
    // 箱线图真实数据只有 1 组 data，只保留第 1 个 series；瀑布汇总柱不预设
    if def.kind == "BoxWhisker" {
        if let Some(arr) = v["series"].as_array_mut() { arr.truncate(1); }
    }
    if def.kind == "Waterfall" {
        v["series"][0]["subtotals"] = json!([]);
    }
    match title {
        Some(t) if !t.trim().is_empty() => { v["title_present"] = json!(true); v["title"] = json!(t); }
        _ => {}
    }
    Some(v.to_string())
}

#[cfg(test)]
mod stat_real_tests {
    use super::*;

    fn def_of(kind: &str) -> &'static ChartTypeDef {
        for cat in 0..CATS.len() {
            for d in types_in_cat(cat) { if d.kind == kind { return d; } }
        }
        panic!("未找到 {kind}");
    }
    fn data() -> RealData {
        RealData {
            x_labels: vec!["Q1".into(), "Q2".into(), "Q3".into()],
            series: vec![("A".into(), vec![10.0, 20.0, 30.0]), ("B".into(), vec![5.0, 6.0, 7.0])],
            warnings: vec![],
        }
    }

    #[test]
    fn funnel_uses_first_series() {
        let j = stat_json_real(def_of("Funnel"), &data(), None).unwrap();
        let v: Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["data"][0]["values"], json!([10.0, 20.0, 30.0]));
        assert_eq!(v["data"][0]["categories"], json!([["Q1", "Q2", "Q3"]]));
    }

    #[test]
    fn histogram_flattens_all() {
        let j = stat_json_real(def_of("Histogram"), &data(), None).unwrap();
        let v: Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["data"][0]["values"].as_array().unwrap().len(), 6);
    }

    #[test]
    fn box_groups_by_series_name() {
        let j = stat_json_real(def_of("BoxWhisker"), &data(), Some("T")).unwrap();
        let v: Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["data"][0]["categories"][0], json!(["A","A","A","B","B","B"]));
        assert_eq!(v["series"].as_array().unwrap().len(), 1);
        assert_eq!(v["title"], json!("T"));
    }

    #[test]
    fn waterfall_clears_subtotals_and_non_stat_is_none() {
        let j = stat_json_real(def_of("Waterfall"), &data(), None).unwrap();
        let v: Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["series"][0]["subtotals"], json!([]));
        assert!(stat_json_real(def_of("Bar"), &data(), None).is_none());
    }

    #[test]
    fn result_has_all_model_fields() {
        for k in ["Waterfall", "Funnel", "Histogram", "BoxWhisker", "Sunburst", "Treemap"] {
            let j = stat_json_real(def_of(k), &data(), None).unwrap();
            let v: Value = serde_json::from_str(&j).unwrap();
            for f in ["kind", "title_present", "title", "show_legend", "legend_pos", "show_grid", "gap_width", "data", "series"] {
                assert!(v.get(f).is_some(), "{k}: 缺顶层字段 {f}");
            }
            for d in v["data"].as_array().unwrap() {
                for f in ["id", "categories", "values", "value_kind"] {
                    assert!(d.get(f).is_some(), "{k}: data 缺字段 {f}");
                }
            }
            assert!(!v["series"].as_array().unwrap().is_empty(), "{k}: series 为空");
            for sr in v["series"].as_array().unwrap() {
                for f in ["name", "layout_id", "format_idx", "data_id", "bin_width", "bin_count",
                          "interval_closed_right", "quartile_exclusive", "mean_line", "mean_marker",
                          "show_outliers", "show_non_outliers", "subtotals", "data_labels"] {
                    assert!(sr.get(f).is_some(), "{k}: series 缺字段 {f}");
                }
            }
        }
    }
}
