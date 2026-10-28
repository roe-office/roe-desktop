//! Calc 拖拽填充的纯逻辑：公式相对引用平移、等差序列判断。不依赖界面和内核。
#![allow(dead_code)]

const MAX_COL: i64 = 16383; // XFD，0 起
const MAX_ROW: i64 = 1_048_576; // 1 起

fn letters_to_col(s: &[char]) -> i64 {
    let mut v: i64 = 0;
    for c in s {
        v = v * 26 + (c.to_ascii_uppercase() as i64 - 'A' as i64 + 1);
    }
    v - 1
}

fn col_to_letters(col: i64) -> String {
    let mut n = col + 1;
    let mut out = Vec::new();
    while n > 0 {
        let r = ((n - 1) % 26) as u8;
        out.push((b'A' + r) as char);
        n = (n - 1) / 26;
    }
    out.iter().rev().collect()
}

/// 在 ch[i..] 尝试解析一个 A1 样式单元格引用，成功返回 (结束位置, 替换文本)。
fn parse_ref(ch: &[char], i: usize, dr: i64, dc: i64) -> Option<(usize, String)> {
    let n = ch.len();
    let mut j = i;
    let abs_c = ch[j] == '$';
    if abs_c { j += 1; }
    let ls = j;
    while j < n && ch[j].is_ascii_alphabetic() { j += 1; }
    let ll = j - ls;
    if ll == 0 || ll > 3 { return None; }
    let col = letters_to_col(&ch[ls..j]);
    if col > MAX_COL { return None; }
    let abs_r = j < n && ch[j] == '$';
    if abs_r { j += 1; }
    let ds = j;
    while j < n && ch[j].is_ascii_digit() { j += 1; }
    let dl = j - ds;
    if dl == 0 || dl > 7 { return None; }
    let row: i64 = ch[ds..j].iter().collect::<String>().parse().ok()?;
    if row < 1 || row > MAX_ROW { return None; }
    if j < n {
        let c = ch[j];
        if c.is_alphanumeric() || c == '_' || c == '(' || c == '!' || c == '.' { return None; }
    }
    let nc = if abs_c { col } else { col + dc };
    let nr = if abs_r { row } else { row + dr };
    if nc < 0 || nc > MAX_COL || nr < 1 || nr > MAX_ROW {
        return Some((j, "#REF!".to_string()));
    }
    let repl = format!(
        "{}{}{}{}",
        if abs_c { "$" } else { "" },
        col_to_letters(nc),
        if abs_r { "$" } else { "" },
        nr
    );
    Some((j, repl))
}

/// 把公式里的相对引用按 (dr 行, dc 列) 平移。src 含开头的 '='。
pub fn shift_formula(src: &str, dr: i32, dc: i32) -> String {
    let (dr, dc) = (dr as i64, dc as i64);
    let ch: Vec<char> = src.chars().collect();
    let n = ch.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        let c = ch[i];
        if c == '"' || c == '\'' {
            let q = c;
            out.push(c);
            i += 1;
            while i < n {
                out.push(ch[i]);
                if ch[i] == q {
                    if i + 1 < n && ch[i + 1] == q {
                        out.push(q);
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        let prev_ok = i == 0 || !(ch[i - 1].is_alphanumeric() || ch[i - 1] == '_' || ch[i - 1] == '.');
        if prev_ok && (c == '$' || c.is_ascii_alphabetic()) {
            if let Some((end, repl)) = parse_ref(&ch, i, dr, dc) {
                out.push_str(&repl);
                i = end;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// 源区域恰好 2 个数值且等差（差不为 0）时返回步长，否则 None（走复制填充）。
pub fn series_step(vals: &[f64]) -> Option<f64> {
    if vals.len() != 2 || !vals.iter().all(|v| v.is_finite()) { return None; }
    let d = vals[1] - vals[0];
    if d.abs() > 1e-12 { Some(d) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_shift() {
        assert_eq!(shift_formula("=A1+B2", 1, 0), "=A2+B3");
        assert_eq!(shift_formula("=A1+B2", 0, 1), "=B1+C2");
    }
    #[test]
    fn absolute_refs() {
        assert_eq!(shift_formula("=$A$1+A1", 1, 1), "=$A$1+B2");
        assert_eq!(shift_formula("=$A1+A$1", 1, 1), "=$A2+B$1");
    }
    #[test]
    fn ranges_and_functions() {
        assert_eq!(shift_formula("=SUM(A1:A3)", 1, 0), "=SUM(A2:A4)");
        assert_eq!(shift_formula("=LOG10(A1)", 1, 0), "=LOG10(A2)");
        assert_eq!(shift_formula("=sum(a1:b2)", 0, 1), "=sum(B1:C2)");
    }
    #[test]
    fn sheet_prefix() {
        assert_eq!(shift_formula("=Sheet1!A1", 1, 0), "=Sheet1!A2");
        assert_eq!(shift_formula("='My Sheet'!A1", 1, 0), "='My Sheet'!A2");
        assert_eq!(shift_formula("=Q1!A1", 1, 0), "=Q1!A2");
    }
    #[test]
    fn strings_untouched() {
        assert_eq!(shift_formula("=\"A1\"&A1", 1, 0), "=\"A1\"&A2");
        assert_eq!(shift_formula("=\"a\"\"A1\"&B1", 1, 0), "=\"a\"\"A1\"&B2");
    }
    #[test]
    fn out_of_range() {
        assert_eq!(shift_formula("=A1", -1, 0), "=#REF!");
        assert_eq!(shift_formula("=A1+B1", 0, -1), "=#REF!+A1");
    }
    #[test]
    fn whole_col_row_untouched() {
        assert_eq!(shift_formula("=SUM(A:A)", 1, 1), "=SUM(A:A)");
    }
    #[test]
    fn series() {
        assert_eq!(series_step(&[1.0, 3.0]), Some(2.0));
        assert_eq!(series_step(&[5.0, 5.0]), None);
        assert_eq!(series_step(&[1.0]), None);
        assert_eq!(series_step(&[1.0, 2.0, 3.0]), None);
    }
}
