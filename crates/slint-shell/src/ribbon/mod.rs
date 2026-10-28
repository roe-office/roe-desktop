//! Ribbon 按钮接线。
//!
//! 套路：每个 Ribbon 页的按钮统一触发 `action("软件.命令")`，
//! AppWindow 只有一个 `ribbon-action(string)` 回调，这里按前缀分发到
//! 各软件/各页的独立文件。新增按钮 = slint 一行 + 对应文件里一个 match 分支，
//! 不再往 main.rs 里堆代码。

pub mod calc_cf;
pub mod cf_icon_styles;
pub mod calc_data;
pub mod calc_df;
mod chain_line;
pub mod calc_names;
pub mod calc_home;
pub mod calc_sparkline;
pub mod calc_stat;
pub mod writer_home;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::{AppState, AppWindow};

/// Ribbon handler 共用的上下文：各 handler 不再各自捕获一堆变量。
#[derive(Clone)]
pub struct RibbonCtx {
    pub state: Rc<RefCell<AppState>>,
    pub app_weak: slint::Weak<AppWindow>,
    pub current_sheet_idx: Rc<Cell<u32>>,
}

/// 注册 ribbon-action 回调。main.rs 只需调用这一个函数。
pub fn install(app: &AppWindow, ctx: RibbonCtx) {
    // 通用列勾选对话框的回调(目前只服务于"删除重复值")
    {
        let c = ctx.clone();
        app.on_column_pick_toggle_column(move |i| calc_data::dedup_toggle_column(&c, i as usize));
        let c = ctx.clone();
        app.on_column_pick_toggle_all(move || calc_data::dedup_toggle_all(&c));
        let c = ctx.clone();
        app.on_column_pick_toggle_header(move || calc_data::dedup_toggle_header(&c));
        let c = ctx.clone();
        app.on_column_pick_confirmed(move || calc_data::dedup_confirm(&c));
        let c = ctx.clone();
        app.on_column_pick_cancelled(move || calc_data::dedup_cancel(&c));
        // 名称管理器对话框
        let c = ctx.clone();
        app.on_name_mgr_select(move |i| calc_names::select(&c, i));
        let c = ctx.clone();
        app.on_name_mgr_add(move |n, r| calc_names::add(&c, n.as_str(), r.as_str()));
        let c = ctx.clone();
        app.on_name_mgr_remove(move |i| calc_names::remove(&c, i));
        let c = ctx.clone();
        app.on_name_mgr_closed(move || calc_names::close(&c));
        // 分列对话框
        let c = ctx.clone();
        app.on_text_split_toggle(move |i| calc_data::text_split_toggle(&c, i as usize));
        let c = ctx.clone();
        app.on_text_split_toggle_merge(move || calc_data::text_split_toggle_merge(&c));
        let c = ctx.clone();
        app.on_text_split_custom_edited(move |t| calc_data::text_split_custom_edited(&c, t.as_str()));
        let c = ctx.clone();
        app.on_text_split_confirmed(move || calc_data::text_split_confirm(&c));
        let c = ctx.clone();
        app.on_text_split_cancelled(move || calc_data::text_split_cancel(&c));
        // 数据验证设置对话框
        let c = ctx.clone();
        app.on_dv_kind_selected(move |i| calc_data::dv_kind_selected(&c, i as usize));
        let c = ctx.clone();
        app.on_dv_operator_selected(move |i| calc_data::dv_operator_selected(&c, i as usize));
        let c = ctx.clone();
        app.on_dv_toggle_allow_blank(move || calc_data::dv_toggle_allow_blank(&c));
        let c = ctx.clone();
        app.on_dv_toggle_show_error(move || calc_data::dv_toggle_show_error(&c));
        let c = ctx.clone();
        app.on_dv_formula1_edited(move |t| calc_data::dv_formula1_edited(&c, t.as_str()));
        let c = ctx.clone();
        app.on_dv_formula2_edited(move |t| calc_data::dv_formula2_edited(&c, t.as_str()));
        let c = ctx.clone();
        app.on_dv_error_msg_edited(move |t| calc_data::dv_error_msg_edited(&c, t.as_str()));
        let c = ctx.clone();
        app.on_dv_confirmed(move || calc_data::dv_confirm(&c));
        let c = ctx.clone();
        app.on_dv_cancelled(move || calc_data::dv_cancel(&c));
        // 条件格式"突出显示单元格规则"对话框
        let c = ctx.clone();
        app.on_cfhl_op_selected(move |i| calc_cf::hl_op_selected(&c, i as usize));
        let c = ctx.clone();
        app.on_cfhl_preset_selected(move |i| calc_cf::hl_preset_selected(&c, i as usize));
        let c = ctx.clone();
        app.on_cfhl_v1_edited(move |t| calc_cf::hl_v1_edited(&c, t.as_str()));
        let c = ctx.clone();
        app.on_cfhl_v2_edited(move |t| calc_cf::hl_v2_edited(&c, t.as_str()));
        let c = ctx.clone();
        app.on_cfhl_confirmed(move || calc_cf::hl_confirm(&c));
        let c = ctx.clone();
        app.on_cfhl_cancelled(move || calc_cf::hl_cancel(&c));
        // 迷你图对话框 / 插入入口
        let c = ctx.clone();
        app.on_spk_open(move |k| {
            if k == 3 { calc_sparkline::clear_selected(&c) } else { calc_sparkline::open(&c, k as usize) }
        });
        let c = ctx.clone();
        app.on_spk_kind_selected(move |i| calc_sparkline::kind_selected(&c, i as usize));
        let c = ctx.clone();
        app.on_spk_data_edited(move |t| calc_sparkline::data_edited(&c, t.as_str()));
        let c = ctx.clone();
        app.on_spk_loc_edited(move |t| calc_sparkline::loc_edited(&c, t.as_str()));
        let c = ctx.clone();
        app.on_spk_confirmed(move || calc_sparkline::confirm(&c));
        let c = ctx.clone();
        app.on_spk_cancelled(move || calc_sparkline::cancel(&c));
        // 数据分析对话框
        let c = ctx.clone();
        app.on_stat_field_edited(move |i, t| calc_stat::field_edited(&c, i as usize, t.as_str()));
        let c = ctx.clone();
        app.on_stat_confirmed(move || calc_stat::confirm(&c));
        let c = ctx.clone();
        app.on_stat_cancelled(move || calc_stat::cancel(&c));
        // 条件格式"图标集"对话框
        let c = ctx.clone();
        app.on_cfic_picked(move |i| calc_cf::icon_picked(&c, i as usize));
        let c = ctx.clone();
        app.on_cfic_toggle_reverse(move || calc_cf::icon_toggle_reverse(&c));
        let c = ctx.clone();
        app.on_cfic_confirmed(move || calc_cf::icon_confirm(&c));
        let c = ctx.clone();
        app.on_cfic_cancelled(move || calc_cf::icon_cancel(&c));
        // 分类汇总对话框
        let c = ctx.clone();
        app.on_subtotal_group_selected_changed(move |i| calc_data::subtotal_group_selected_changed(&c, i as usize));
        let c = ctx.clone();
        app.on_subtotal_func_selected_changed(move |i| calc_data::subtotal_func_selected_changed(&c, i as usize));
        let c = ctx.clone();
        app.on_subtotal_toggle_agg(move |i| calc_data::subtotal_toggle_agg(&c, i as usize));
        let c = ctx.clone();
        app.on_subtotal_toggle_clear(move || calc_data::subtotal_toggle_clear(&c));
        let c = ctx.clone();
        app.on_subtotal_confirmed(move || calc_data::subtotal_confirm(&c));
        let c = ctx.clone();
        app.on_subtotal_cancelled(move || calc_data::subtotal_cancel(&c));
        // 数据验证:单元格提交前校验(带返回值回调,同步守卫)
        let c = ctx.clone();
        app.on_calc_validate_input(move |t| calc_data::dv_validate_input(&c, t.as_str()));
        // 筛选对话框
        let c = ctx.clone();
        app.on_filter_toggle_item(move |i| calc_data::filter_toggle(&c, i as usize));
        let c = ctx.clone();
        app.on_filter_toggle_all(move || calc_data::filter_toggle_all(&c));
        let c = ctx.clone();
        app.on_filter_sort_asc(move || calc_data::filter_sort(&c, true));
        let c = ctx.clone();
        app.on_filter_sort_desc(move || calc_data::filter_sort(&c, false));
        let c = ctx.clone();
        app.on_filter_confirmed(move || calc_data::filter_confirm(&c));
        let c = ctx.clone();
        app.on_filter_cancelled(move || calc_data::filter_cancel(&c));
    }
    // 数据集分页对话框(parquet 只读预览)
    {
        let c = ctx.clone();
        app.on_df_prev(move || calc_df::prev(&c));
        let c = ctx.clone();
        app.on_df_next(move || calc_df::next(&c));
        let c = ctx.clone();
        app.on_df_closed(move || calc_df::close(&c));
        let c = ctx.clone();
        app.on_dfo_confirmed(move || calc_df::confirm(&c));
        let c = ctx.clone();
        app.on_dfo_cancelled(move || calc_df::cancel(&c));
        let c = ctx.clone();
        app.on_sql_run(move |s| calc_df::run_sql(&c, s.as_str()));
        let c = ctx.clone();
        app.on_sql_save(move |s| calc_df::save_sql(&c, s.as_str()));
        let c = ctx.clone();
        app.on_chain_run(move |s| calc_df::run_chain(&c, s.as_str()));
        let c = ctx.clone();
        app.on_chain_save(move |s| calc_df::save_chain(&c, s.as_str()));
        let c = ctx.clone();
        app.on_chain_ds_picked(move |n| calc_df::pick_chain_ds(&c, n.as_str()));
        let c = ctx.clone();
        app.on_chain_col_toggled(move |i| calc_df::toggle_chain_col(&c, i));
        let c = ctx.clone();
        app.on_chain_build_line(move |k, n, d, a| calc_df::build_line(&c, k.as_str(), n.as_str(), d, a.as_str()).into());
    }
    app.on_ribbon_action(move |cmd| {
        dispatch(cmd.as_str(), &ctx);
    });
}

fn dispatch(cmd: &str, ctx: &RibbonCtx) {
    if cmd != "writer.sync" {
        eprintln!("[ribbon] action={}", cmd);
    }
    match cmd.split_once('.') {
        Some(("calc", sub)) => {
            // 先试「数据」页命令,未处理再交「开始」页
            if !calc_data::handle(sub, ctx) {
                calc_home::handle(sub, ctx);
            }
        }
        Some(("writer", sub)) => writer_home::handle(sub, ctx),
        _ => eprintln!("[ribbon] 未接线的命令: {}", cmd),
    }
}
