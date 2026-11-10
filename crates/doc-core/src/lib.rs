mod session;
pub mod table_preview;
pub use session::*;

/// 占位符提示文案语言开关（true=英文，false=中文）。
pub fn set_placeholder_lang(en: bool) {
    roe_api::set_placeholder_lang_en(en);
}
