//! 界面语言开关与 Rust 侧提示的英文化。中文(简/繁)界面原样显示；其余语言走英文。
use std::sync::atomic::{AtomicBool, Ordering};

static NON_ZH: AtomicBool = AtomicBool::new(false);

/// code 取值同 select_bundled_translation：""=简体中文原文，"zh_TW"=繁体，其余为非中文。
pub fn set_code(code: &str) {
    NON_ZH.store(!(code.is_empty() || code.starts_with("zh")), Ordering::Relaxed);
}

#[allow(dead_code)]
pub fn is_zh() -> bool {
    !NON_ZH.load(Ordering::Relaxed)
}

/// 二选一：中文界面返回 zh，其余返回 en。
pub fn t(zh: &'static str, en: &'static str) -> &'static str {
    if NON_ZH.load(Ordering::Relaxed) { en } else { zh }
}

const EXACT: &[(&str, &str)] = &[
    ("没有打开的文档", "No document is open"),
    ("请选择多个段落", "Please select multiple paragraphs"),
    ("请先选中文字", "Please select text first"),
    ("当前文档类型不支持保存", "Saving is not supported for this document type"),
    ("保存失败:序列化出错", "Save failed: serialization error"),
    ("新建文档失败", "Failed to create document"),
    ("请输入页码", "Please enter a page number"),
    ("新建幻灯片失败", "Failed to add slide"),
    ("重置版式失败", "Failed to reset layout"),
    ("复制幻灯片失败", "Failed to duplicate slide"),
    ("至少保留一张幻灯片", "At least one slide must remain"),
    ("删除幻灯片失败", "Failed to delete slide"),
    ("无法识别图片（仅支持 PNG/JPEG）", "Unrecognized image (PNG/JPEG only)"),
    ("插入图片失败", "Failed to insert image"),
    ("请先在正文中点击要插入图表的位置", "Click in the body text where the chart should go first"),
    ("已插入图表", "Chart inserted"),
    ("插入图表失败：所选区域没有可用于绘图的数据", "Failed to insert chart: the selected range has no plottable data"),
    ("插入图表失败", "Failed to insert chart"),
    ("请先选中文本框", "Please select a text box first"),
    ("设置段落样式失败", "Failed to set paragraph style"),
    ("该图表暂不支持编辑数据", "Editing data is not supported for this chart yet"),
    ("行列互换后超出 50 行 × 12 列上限", "Transposing would exceed the limit of 50 rows × 12 columns"),
    ("读取图表失败，未写回", "Failed to read chart; nothing was written back"),
    ("写回图表数据失败", "Failed to write chart data back"),
    ("设置图片背景失败", "Failed to set picture background"),
    ("Impress 暂不支持渐变填充", "Gradient fill is not supported in Impress yet"),
    ("母版视图暂不支持形状填充和线条样式", "Shape fill and line styles are not supported in master view"),
    ("母版背景暂不支持渐变", "Gradient is not supported for the master background"),
    ("设置背景失败", "Failed to set background"),
    ("设置样式失败", "Failed to set style"),
    ("插入表格失败", "Failed to insert table"),
    ("设置文字样式失败", "Failed to set text style"),
    ("已插入形状", "Shape inserted"),
    ("插入公式失败: 没有可插入的光标位置", "Failed to insert formula: no cursor position"),
    ("未找到", "Not found"),
    ("请输入要查找的内容", "Please enter text to find"),
    ("已删除图片", "Picture deleted"),
    ("已删除批注", "Comment deleted"),
    ("插入图表失败：所选起始单元格为空，请先选中含数据的区域左上角", "Failed to insert chart: the starting cell is empty; select the top-left cell of a data range first"),
    ("填充范围过大（上限 20 万格）", "Fill range too large (limit: 200,000 cells)"),
    ("填充失败", "Fill failed"),
    ("修改图例显示失败", "Failed to change legend visibility"),
    ("修改网格显示失败", "Failed to change gridlines"),
    ("修改X轴标题失败", "Failed to change X-axis title"),
    ("修改Y轴标题失败", "Failed to change Y-axis title"),
    ("修改图表类型失败", "Failed to change chart type"),
    ("修改图表标题失败", "Failed to change chart title"),
    ("修改边框颜色失败", "Failed to change border color"),
    ("修改边框宽度失败", "Failed to change border width"),
    ("插入图片失败(仅支持 PNG/JPEG,不超过 10MB)", "Failed to insert image (PNG/JPEG only, max 10 MB)"),
    ("插入文本框失败", "Failed to insert text box"),
    ("插入形状失败", "Failed to insert shape"),
    ("设置文字失败", "Failed to set text"),
    ("Calc 图形暂不支持渐变填充", "Gradient fill is not supported for Calc shapes yet"),
    ("设置图形样式失败", "Failed to set shape style"),
    ("删除图表失败", "Failed to delete chart"),
];

/// 形如 “中文前缀: 详情” 的提示：只翻前缀，详情原样保留。
const PREFIX: &[(&str, &str)] = &[
    ("读取文件失败", "Failed to read file"),
    ("打开文档失败", "Failed to open document"),
    ("保存失败", "Save failed"),
    ("已保存", "Saved"),
    ("读取图片失败", "Failed to read image"),
    ("插入图表失败", "Failed to insert chart"),
    ("调整图片失败", "Failed to adjust image"),
    ("图表数据解析失败", "Failed to parse chart data"),
    ("序列化失败", "Serialization failed"),
    ("写回图表数据失败", "Failed to write chart data back"),
    ("插入表格失败", "Failed to insert table"),
    ("旋转失败", "Rotation failed"),
    ("设置形状样式失败", "Failed to set shape style"),
    ("插入形状失败", "Failed to insert shape"),
    ("查找失败", "Find failed"),
    ("替换失败", "Replace failed"),
    ("删除图片失败", "Failed to delete picture"),
    ("标记完成失败", "Failed to mark as done"),
    ("删除批注失败", "Failed to delete comment"),
];

pub fn tr_status(s: &str) -> String {
    if s.is_empty() || s.is_ascii() { return s.to_string(); }
    for (zh, en) in EXACT {
        if s == *zh { return (*en).to_string(); }
    }
    for (zh, en) in PREFIX {
        if let Some(rest) = s.strip_prefix(zh) {
            if let Some(tail) = rest.strip_prefix(": ") { return format!("{}: {}", en, tail); }
        }
    }
    if let Some(r) = s.strip_prefix("已转到第 ") {
        if let Some((a, b)) = r.split_once(" 页，共 ") {
            if let Some(b) = b.strip_suffix(" 页") { return format!("Went to page {} of {}", a, b); }
        }
    }
    if let Some(r) = s.strip_prefix("第 ") {
        if let Some((a, b)) = r.split_once(" 处，共 ") {
            if let Some(b) = b.strip_suffix(" 处") { return format!("Match {} of {}", a, b); }
        }
    }
    if let Some(r) = s.strip_prefix("已替换 ") {
        if let Some(a) = r.strip_suffix(" 处") { return format!("Replaced {} occurrence(s)", a); }
    }
    eprintln!("[I18N-MISS] {}", s);
    s.to_string()
}

pub trait StatusExt {
    fn set_status_msg(&self, s: slint::SharedString);
}

impl StatusExt for crate::AppWindow {
    fn set_status_msg(&self, s: slint::SharedString) {
        if NON_ZH.load(Ordering::Relaxed) {
            self.set_status_message(tr_status(s.as_str()).into());
        } else {
            self.set_status_message(s);
        }
    }
}

const CHART_NAMES: &[(&str, &str)] = &[
    ("柱形图", "Column"), ("折线图", "Line"), ("饼图", "Pie"), ("条形图", "Bar"), ("面积图", "Area"),
    ("XY（散点图）", "XY (Scatter)"), ("雷达图", "Radar"), ("组合图", "Combo"), ("瀑布图", "Waterfall"),
    ("漏斗图", "Funnel"), ("直方图", "Histogram"), ("箱形图", "Box & Whisker"), ("旭日图", "Sunburst"), ("树状图", "Treemap"),
    ("簇状柱形图", "Clustered Column"), ("堆积柱形图", "Stacked Column"), ("百分比堆积柱形图", "100% Stacked Column"),
    ("堆积折线图", "Stacked Line"), ("百分比堆积折线图", "100% Stacked Line"), ("圆环图", "Doughnut"),
    ("簇状条形图", "Clustered Bar"), ("堆积条形图", "Stacked Bar"), ("百分比堆积条形图", "100% Stacked Bar"),
    ("堆积面积图", "Stacked Area"), ("百分比堆积面积图", "100% Stacked Area"), ("散点图", "Scatter"),
    ("簇状柱形图-折线图", "Clustered Column - Line"),
];

pub fn tr_chart(s: &str) -> String {
    if is_zh() { return s.to_string(); }
    for (zh, en) in CHART_NAMES { if s == *zh { return (*en).to_string(); } }
    s.to_string()
}

const CHART_WORDS: &[(&str, &str)] = &[
    ("第一季度", "Q1"), ("第二季度", "Q2"), ("第三季度", "Q3"), ("第四季度", "Q4"), ("销售额", "Sales"),
    ("系列 1", "Series 1"), ("系列 2", "Series 2"), ("系列 3", "Series 3"),
    ("类别 1", "Category 1"), ("类别 2", "Category 2"), ("类别 3", "Category 3"), ("类别 4", "Category 4"), ("类别 5", "Category 5"),
    ("类别A", "Category A"), ("类别B", "Category B"), ("类别C", "Category C"), ("类别D", "Category D"), ("类别E", "Category E"),
    ("子类1", "Sub 1"), ("子类2", "Sub 2"), ("子类3", "Sub 3"), ("组1", "Group 1"), ("组2", "Group 2"),
    ("起始", "Start"), ("收入", "Income"), ("支出", "Expense"), ("追加", "Added"), ("合计", "Total"),
    ("访问", "Visits"), ("咨询", "Inquiries"), ("报价", "Quotes"), ("成交", "Deals"), ("图表标题", "Chart Title"),
];

/// 图表预览/假数据 JSON：非中文界面时把示例文字换成英文（中文界面原样）。
pub fn localize_chart(mut s: String) -> String {
    if is_zh() { return s; }
    for (zh, en) in CHART_WORDS {
        s = s.replace(&format!("\"{}\"", zh), &format!("\"{}\"", en));
    }
    s
}
