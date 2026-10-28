//! Calc「数据」页 数据分析:通用统计对话框。
//! 第一版:单列区域、同一 sheet、输出位置取左上角单元格。
//! 类型下标契约(Slint 按钮发 "calc.stat:<k>",只追加不改):
//! 0 描述性统计 1 相关性分析 2 协方差 3 卡方检验 4 F检验 5 配对t检验 6 Z检验 7 回归

use super::calc_home::{read_selection, refresh_view, sync_state_inner};
use super::calc_sparkline::{fmt_range, parse_range};
use super::RibbonCtx;

const TITLES: [&str; 12] = [
    "描述性统计", "相关性分析", "协方差", "卡方检验(独立性)",
    "F 检验", "配对 t 检验", "Z 检验(已知方差)", "回归分析",
    "单因素方差分析", "双因素方差分析(无重复)", "移动平均", "指数平滑",
];
/// 是否需要第二个输入区域
const NEED2: [bool; 12] = [false, true, true, false, true, true, true, true, false, false, false, false];

/// 分组方式文本 → Some(true)=按列, Some(false)=按行
fn grouping(s: &str) -> Option<bool> {
    match s.trim().to_lowercase().as_str() {
        "列" | "col" | "column" | "c" => Some(true),
        "行" | "row" | "r" => Some(false),
        _ => None,
    }
}

/// 字段 3..6 的标签(None = 不显示)
fn field_cfg(kind: usize) -> [Option<&'static str>; 4] {
    match kind {
        6 => [Some("变量 1 已知方差"), Some("变量 2 已知方差"), Some("假设均值差"), Some("Alpha")],
        8 => [None, None, Some("分组方式(列/行)"), Some("Alpha")],
        9 => [None, None, None, Some("Alpha")],
        10 => [Some("间隔"), None, None, None],
        11 => [None, None, None, Some("α (0~1)")],
        _ => [None, None, None, None],
    }
}

/// 字段 3..6 的默认值
fn defaults(kind: usize) -> [&'static str; 4] {
    match kind {
        8 => ["1", "1", "列", "0.05"],
        10 => ["3", "1", "0", "0.05"],
        11 => ["1", "1", "0", "0.3"],
        _ => ["1", "1", "0", "0.05"],
    }
}

/// 字段下标: 0 输入1 1 输入2 2 输出位置 3 方差1 4 方差2 5 假设差 6 Alpha
pub struct StatDialogState {
    pub sheet: u32,
    pub kind: usize,
    pub f: [String; 7],
}

impl StatDialogState {
    /// 返回 (能否确认, 提示)。空值只灰掉确定不报红;格式/尺寸不对才报红。
    fn validate(&self) -> (bool, String) {
        let k = self.kind;
        let f = &self.f;
        if f[0].trim().is_empty() || (NEED2[k] && f[1].trim().is_empty()) || f[2].trim().is_empty() {
            return (false, String::new());
        }
        let Some(a) = parse_range(&f[0]) else {
            return (false, "输入区域格式不对(如 A2:A11)".to_string());
        };
        let b = if NEED2[k] {
            match parse_range(&f[1]) {
                Some(b) => Some(b),
                None => return (false, "变量 2 范围格式不对".to_string()),
            }
        } else {
            None
        };
        if parse_range(&f[2]).is_none() {
            return (false, "输出位置格式不对(如 F1)".to_string());
        }
        match k {
            3 | 9 => {
                if a.2 - a.0 < 1 || a.3 - a.1 < 1 {
                    return (false, "至少需要 2 行 2 列".to_string());
                }
            }
            8 => match grouping(&f[5]) {
                None => return (false, "分组方式请填「列」或「行」".to_string()),
                Some(true) if a.3 - a.1 < 1 => return (false, "按列分组至少需要 2 列".to_string()),
                Some(false) if a.2 - a.0 < 1 => return (false, "按行分组至少需要 2 行".to_string()),
                _ => {}
            },
            _ => {
                if a.1 != a.3 {
                    return (false, "第一版仅支持单列区域".to_string());
                }
            }
        }
        if let Some(b) = b {
            if b.1 != b.3 {
                return (false, "第一版仅支持单列区域".to_string());
            }
            if matches!(k, 1 | 2 | 5 | 7) && (b.2 - b.0) != (a.2 - a.0) {
                return (false, "两个范围的行数必须相同".to_string());
            }
        }
        if k == 6 {
            let pos = |s: &str| s.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0);
            if pos(&f[3]).is_none() || pos(&f[4]).is_none() {
                return (false, "已知方差需为正数".to_string());
            }
            if f[5].trim().parse::<f64>().ok().filter(|v| v.is_finite()).is_none() {
                return (false, "假设均值差需为数字".to_string());
            }
            match f[6].trim().parse::<f64>() {
                Ok(v) if v > 0.0 && v < 1.0 => {}
                _ => return (false, "Alpha 需在 0 到 1 之间".to_string()),
            }
        }
        if matches!(k, 8 | 9 | 11) {
            match f[6].trim().parse::<f64>() {
                Ok(v) if v > 0.0 && v < 1.0 => {}
                _ => return (false, "Alpha 需在 0 到 1 之间".to_string()),
            }
        }
        if k == 10 {
            match f[3].trim().parse::<i64>() {
                Ok(v) if v >= 1 => {}
                _ => return (false, "间隔需为正整数".to_string()),
            }
        }
        (true, String::new())
    }
}

fn push(app: &crate::AppWindow, st: &StatDialogState) {
    let (ok, warn) = st.validate();
    let (l1, l2) = match st.kind {
        0 | 8 | 9 | 10 | 11 => ("输入区域", ""),
        3 => ("输入矩阵", ""),
        7 => ("因变量 Y 范围", "自变量 X 范围"),
        _ => ("变量 1 范围", "变量 2 范围"),
    };
    app.set_stat_title(TITLES[st.kind].into());
    app.set_stat_label1(l1.into());
    app.set_stat_label2(l2.into());
    app.set_stat_show_in2(NEED2[st.kind]);
    let cfg = field_cfg(st.kind);
    app.set_stat_show3(cfg[0].is_some());
    app.set_stat_show4(cfg[1].is_some());
    app.set_stat_show5(cfg[2].is_some());
    app.set_stat_show6(cfg[3].is_some());
    app.set_stat_lab3(cfg[0].unwrap_or("").into());
    app.set_stat_lab4(cfg[1].unwrap_or("").into());
    app.set_stat_lab5(cfg[2].unwrap_or("").into());
    app.set_stat_lab6(cfg[3].unwrap_or("").into());
    app.set_stat_can_confirm(ok);
    app.set_stat_warn(warn.into());
}

pub fn open(ctx: &RibbonCtx, kind: usize) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    if kind >= TITLES.len() { return; }
    let Some(sel) = read_selection(&app) else {
        eprintln!("[ribbon] calc.stat: 无选中单元格");
        return;
    };
    let (mut in1, mut in2) = (fmt_range(sel), String::new());
    // 两变量检验:多列选区自动取前两列分别填入
    if NEED2[kind] && sel.3 > sel.1 {
        in1 = fmt_range((sel.0, sel.1, sel.2, sel.1));
        in2 = fmt_range((sel.0, sel.1 + 1, sel.2, sel.1 + 1));
        if kind == 7 { std::mem::swap(&mut in1, &mut in2); } // 回归: 第一列 X、第二列 Y
    }
    let d = defaults(kind);
    let st = StatDialogState {
        sheet: ctx.current_sheet_idx.get(),
        kind,
        f: [in1, in2, String::new(), d[0].into(), d[1].into(), d[2].into(), d[3].into()],
    };
    eprintln!("[ribbon] calc.stat 打开 kind={} in1={} in2={}", TITLES[kind], st.f[0], st.f[1]);
    app.set_stat_t0(st.f[0].clone().into());
    app.set_stat_t1(st.f[1].clone().into());
    app.set_stat_t2("".into());
    app.set_stat_t3(st.f[3].clone().into());
    app.set_stat_t4(st.f[4].clone().into());
    app.set_stat_t5(st.f[5].clone().into());
    app.set_stat_t6(st.f[6].clone().into());
    push(&app, &st);
    ctx.state.borrow_mut().stat_dialog = Some(st);
    app.set_stat_visible(true);
}

pub fn field_edited(ctx: &RibbonCtx, idx: usize, text: &str) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    let Some(st) = s.stat_dialog.as_mut() else { return };
    if idx < st.f.len() { st.f[idx] = text.to_string(); }
    push(&app, st);
}

pub fn cancel(ctx: &RibbonCtx) {
    ctx.state.borrow_mut().stat_dialog = None;
    eprintln!("[ribbon] calc.stat 对话框取消");
}

/// 解析 Rhai 返回 {"rows_written":N} / {"rows_written":0,"error":"..."}
fn parse_result(raw: &str) -> Result<i64, String> {
    // execute_script 返回外壳 {"ok":..,"return_value":"<函数返回的JSON字符串>"},先剥一层
    let inner: String = match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(o) if o.get("return_value").is_some() => {
            if o.get("ok").and_then(|x| x.as_bool()) == Some(false) {
                return Err(format!("脚本执行失败: {}", raw));
            }
            o.get("return_value").and_then(|x| x.as_str()).unwrap_or("").to_string()
        }
        _ => raw.to_string(),
    };
    let raw = inner.as_str();
    // execute_script 返回外壳 {"ok":..,"return_value":"<函数返回的JSON字符串>"},先剥一层
    let inner: String = match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(o) if o.get("return_value").is_some() => {
            if o.get("ok").and_then(|x| x.as_bool()) == Some(false) {
                return Err(format!("脚本执行失败: {}", raw));
            }
            o.get("return_value").and_then(|x| x.as_str()).unwrap_or("").to_string()
        }
        _ => raw.to_string(),
    };
    let raw = inner.as_str();
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => {
            if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
                return Err(e.to_string());
            }
            match v.get("rows_written").and_then(|x| x.as_i64()) {
                Some(n) if n > 0 => Ok(n),
                _ => Err(format!("未写入任何行: {}", raw)),
            }
        }
        Err(_) => Err(format!("无法解析返回: {}", raw)),
    }
}

/// 从 execute_script 外壳取所有 dirty_region 的并集 → (r1,c1,r2,c2)
fn dirty_region(raw: &str) -> Option<(u32, u32, u32, u32)> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let arr = v.get("dirty_regions")?.as_array()?;
    let mut acc: Option<(u32, u32, u32, u32)> = None;
    for d in arr {
        let g = |k: &str| d.get(k).and_then(|x| x.as_u64()).map(|n| n as u32);
        let (Some(r1), Some(c1), Some(r2), Some(c2)) = (g("r1"), g("c1"), g("r2"), g("c2")) else { continue };
        acc = Some(match acc {
            None => (r1, c1, r2, c2),
            Some((a, b, c, d2)) => (a.min(r1), b.min(c1), c.max(r2), d2.max(c2)),
        });
    }
    acc
}

fn build_script(st: &StatDialogState) -> Option<String> {
    let a = parse_range(&st.f[0])?;
    let o = parse_range(&st.f[2])?;
    let s = st.sheet;
    let out = format!("{}, {}, {}", s, o.0, o.1);
    let two = |a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)| {
        format!("{}, {}, {}, {}, {}, {}, {}, {}", s, a.0, a.2, a.1, s, b.0, b.2, b.1)
    };
    let b = if NEED2[st.kind] { Some(parse_range(&st.f[1])?) } else { None };
    Some(match st.kind {
        0 => format!("describe_stats({}, {}, {}, {}, {})", s, a.0, a.2, a.1, out),
        1 => format!("correl({}, {})", two(a, b?), out),
        2 => format!("covariance({}, {})", two(a, b?), out),
        3 => format!("chi_square_test({}, {}, {}, {}, {}, {})", s, a.0, a.2, a.1, a.3, out),
        4 => format!("f_test({}, {})", two(a, b?), out),
        // tails=2(双尾), test_type=1(配对)
        5 => format!("t_test({}, 2, 1, {})", two(a, b?), out),
        6 => {
            let n = |i: usize| st.f[i].trim().parse::<f64>().ok();
            format!("z_test({}, {:?}, {:?}, {:?}, {:?}, {})", two(a, b?), n(3)?, n(4)?, n(5)?, n(6)?, out)
        }
        // regression(sheet_y, ..., cy, sheet_x, ..., cx, out):Y 在前、X 在后
        7 => format!("regression({}, {})", two(a, b?), out),
        8 => {
            let g = grouping(&st.f[5])?;
            let al = st.f[6].trim().parse::<f64>().ok()?;
            format!("anova_single_factor({}, {}, {}, {}, {}, {}, {:?}, {})", s, a.0, a.2, a.1, a.3, g, al, out)
        }
        9 => {
            let al = st.f[6].trim().parse::<f64>().ok()?;
            format!("anova_two_factor({}, {}, {}, {}, {}, {:?}, {})", s, a.0, a.2, a.1, a.3, al, out)
        }
        10 => {
            let n = st.f[3].trim().parse::<i64>().ok()?;
            format!("moving_average({}, {}, {}, {}, {}, {})", s, a.0, a.2, a.1, n, out)
        }
        11 => {
            let al = st.f[6].trim().parse::<f64>().ok()?;
            format!("exponential_smoothing({}, {}, {}, {}, {:?}, {})", s, a.0, a.2, a.1, al, out)
        }
        _ => return None,
    })
}

pub fn confirm(ctx: &RibbonCtx) {
    let Some(app) = ctx.app_weak.upgrade() else { return };
    let mut s = ctx.state.borrow_mut();
    match s.stat_dialog.as_ref() {
        Some(st) if st.validate().0 => {}
        Some(_) => { app.set_stat_visible(true); return; }
        None => return,
    }
    if s.session.is_none() { return; }
    let Some(st) = s.stat_dialog.take() else { return };
    let Some(script) = build_script(&st) else { return };
    eprintln!("[ribbon] calc.stat script: {}", script);
    let res = {
        let Some(sess) = s.session.as_mut() else { return };
        let raw = sess.execute_script(&script);
        eprintln!("[ribbon] calc.stat 返回: {}", raw);
        let r = parse_result(&raw);
        if r.is_ok() {
            // 输出区值列(标签列右侧)统一 4 位小数,避免 p 值显示成 0
            if let Some((r1, c1, r2, c2)) = dirty_region(&raw) {
                if st.kind == 10 {
                    // 移动平均:只有数值列,没有标签列
                    sess.calc_apply_format_str(st.sheet, r1, c1, r2, c2, "num_format", Some("0.0000"));
                } else if c2 > c1 {
                    let ok = sess.calc_apply_format_str(st.sheet, r1, c1 + 1, r2, c2, "num_format", Some("0.0000"));
                    eprintln!("[ribbon] calc.stat 设数字格式 ({},{})-({},{}) ok={}", r1, c1 + 1, r2, c2, ok);
                    // 计数类行(观测数/自由度/Count)不适合小数格式,恢复常规
                    for r in r1..=r2 {
                        let lab = sess.calc_get_cell(st.sheet, r, c1).to_lowercase();
                        let lab = lab.trim();
                        if lab.contains("observations") || lab.contains("degrees of freedom")
                            || lab == "count" || lab == "df" {
                            sess.calc_apply_format_str(st.sheet, r, c1 + 1, r, c2, "num_format", None);
                        }
                    }
                    // 标签列自适应列宽,避免 "Known Variance 1" 等被截断
                    let fit = sess.calc_auto_fit_columns(st.sheet, c1, c1 + 1);
                    eprintln!("[ribbon] calc.stat 标签列自适应 col={} ok={}", c1, fit);
                    // 方差分析:Count 列(组摘要行)与 DF 列(ANOVA 表行)恢复常规
                    if matches!(st.kind, 8 | 9) {
                        for r in r1..=r2 {
                            let lab = sess.calc_get_cell(st.sheet, r, c1);
                            let lab = lab.trim();
                            let off = if lab.starts_with("Column ") || lab.starts_with("Row ") {
                                1
                            } else if matches!(lab, "Between Groups" | "Within Groups" | "Total" | "Rows" | "Columns" | "Error") {
                                2
                            } else {
                                continue;
                            };
                            if c1 + off <= c2 {
                                sess.calc_apply_format_str(st.sheet, r, c1 + off, r, c1 + off, "num_format", None);
                            }
                        }
                    }
                }
            }
            refresh_view(&app, sess);
            sync_state_inner(&app, sess, st.sheet);
        }
        r
    };
    if let Err(e) = res {
        eprintln!("[ribbon] calc.stat 失败: {}", e);
        push(&app, &st);
        app.set_stat_warn(e.into());
        s.stat_dialog = Some(st);
        app.set_stat_visible(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn st(kind: usize, a: &str, b: &str, o: &str) -> StatDialogState {
        StatDialogState {
            sheet: 0,
            kind,
            f: [a.into(), b.into(), o.into(), "1".into(), "1".into(), "0".into(), "0.05".into()],
        }
    }
    #[test]
    fn script_shapes_batch2() {
        let mut s8 = st(8, "A2:C9", "", "F1");
        s8.f[5] = "列".into();
        assert!(s8.validate().0);
        assert_eq!(build_script(&s8).unwrap(),
            "anova_single_factor(0, 1, 8, 0, 2, true, 0.05, 0, 0, 5)");
        s8.f[5] = "行".into();
        assert_eq!(build_script(&s8).unwrap(),
            "anova_single_factor(0, 1, 8, 0, 2, false, 0.05, 0, 0, 5)");
        s8.f[5] = "x".into();
        assert!(!s8.validate().0);
        let mut s8b = st(8, "A2:A9", "", "F1");
        s8b.f[5] = "列".into();
        assert!(!s8b.validate().0);
        assert_eq!(build_script(&st(9, "A2:C9", "", "F1")).unwrap(),
            "anova_two_factor(0, 1, 8, 0, 2, 0.05, 0, 0, 5)");
        let mut s10 = st(10, "A2:A9", "", "F1");
        s10.f[3] = "3".into();
        assert!(s10.validate().0);
        assert_eq!(build_script(&s10).unwrap(), "moving_average(0, 1, 8, 0, 3, 0, 0, 5)");
        s10.f[3] = "0".into();
        assert!(!s10.validate().0);
        let mut s11 = st(11, "A2:A9", "", "F1");
        s11.f[6] = "0.3".into();
        assert!(s11.validate().0);
        assert_eq!(build_script(&s11).unwrap(), "exponential_smoothing(0, 1, 8, 0, 0.3, 0, 0, 5)");
    }
    #[test]
    fn validate_basic() {
        assert!(st(0, "A2:A11", "", "F1").validate().0);
        assert!(!st(0, "A2:B11", "", "F1").validate().0);
        assert!(!st(0, "A2:A11", "", "").validate().0);
        assert!(st(5, "A2:A9", "B2:B9", "F1").validate().0);
        assert!(!st(5, "A2:A9", "B2:B8", "F1").validate().0);
        assert!(st(4, "A2:A9", "B2:B8", "F1").validate().0);
        assert!(st(3, "B2:C3", "", "F1").validate().0);
        assert!(!st(3, "B2:B3", "", "F1").validate().0);
    }
    #[test]
    fn script_shapes() {
        assert_eq!(build_script(&st(0, "A2:A11", "", "F1")).unwrap(),
            "describe_stats(0, 1, 10, 0, 0, 0, 5)");
        assert_eq!(build_script(&st(5, "A2:A9", "B2:B9", "F1")).unwrap(),
            "t_test(0, 1, 8, 0, 0, 1, 8, 1, 2, 1, 0, 0, 5)");
        assert_eq!(build_script(&st(3, "B2:C3", "", "F1")).unwrap(),
            "chi_square_test(0, 1, 2, 1, 2, 0, 0, 5)");
        assert_eq!(build_script(&st(6, "A2:A9", "B2:B9", "F1")).unwrap(),
            "z_test(0, 1, 8, 0, 0, 1, 8, 1, 1.0, 1.0, 0.0, 0.05, 0, 0, 5)");
    }
}
