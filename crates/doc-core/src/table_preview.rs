//! 表格样式缩略图取色。下标与 calc_home.rs 的 "tstyle:<idx>" 完全一致:
//! 0~20 Light1~21, 21~48 Medium1~28, 49~59 Dark1~11。
pub use roe_model::excel_table_styles::StylePreview;

pub const TABLE_STYLE_COUNT: usize = 60;

pub fn table_style_name(idx: usize) -> Option<String> {
    match idx {
        0..=20 => Some(format!("TableStyleLight{}", idx + 1)),
        21..=48 => Some(format!("TableStyleMedium{}", idx - 20)),
        49..=59 => Some(format!("TableStyleDark{}", idx - 48)),
        _ => None,
    }
}

/// 全部 60 个样式的预览色,用默认 Office 色板。取不到的项为默认值(全 None)。
pub fn table_style_previews() -> Vec<StylePreview> {
    (0..TABLE_STYLE_COUNT)
        .map(|i| {
            table_style_name(i)
                .and_then(|n| roe_model::excel_table_styles::style_preview(&n, &[]))
                .unwrap_or_default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_cover_all_and_match_names() {
        let v = table_style_previews();
        assert_eq!(v.len(), 60);
        assert_eq!(table_style_name(0).as_deref(), Some("TableStyleLight1"));
        assert_eq!(table_style_name(21).as_deref(), Some("TableStyleMedium1"));
        assert_eq!(table_style_name(49).as_deref(), Some("TableStyleDark1"));
        assert!(table_style_name(60).is_none());
        for i in 0..60 {
            let n = table_style_name(i).unwrap();
            assert!(roe_model::excel_table_styles_data::find_table_style(&n).is_some(), "{}", n);
        }
    }
}
