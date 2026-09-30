// Copyright 2026 AsterSQL.

// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::super::*;
use super::{Context, Rhs};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum QueryRule {
    FieldAlt01,
    FieldAlt02,
    FieldAlt03,
    FieldAlt04,
    FieldAsNameOptAlt01,
    FieldAsNameAlt02,
    FieldAsNameAlt04,
    FieldListAlt01,
    FieldListAlt02,
    WithRollupClauseAlt01,
    WithRollupClauseAlt02,
    GroupByClauseAlt01,
    HavingClauseAlt01,
    HavingClauseAlt02,
    AsOfClauseOptAlt01,
    AsOfClauseAlt01,
    RowValueAlt01,
    ValuesOptAlt01,
    ValuesAlt01,
    ValuesAlt02,
    ExprOrDefaultAlt02,
    OrderByAlt01,
    ByListAlt01,
    ByListAlt02,
    ByItemAlt01,
    ByItemAlt02,
    OrderAlt01,
    OrderAlt02,
    OptOrderAlt01,
    OptOrderAlt02,
    OptOrderAlt03,
    OrderByOptionalAlt01,
    PriorityAlt01,
    PriorityAlt02,
    PriorityAlt03,
    PriorityOptAlt01,
    TableNameAlt01,
    TableNameAlt02,
    TableNameAlt03,
    TableNameListAlt01,
    TableNameListAlt02,
    TableNameOptWildAlt01,
    TableNameOptWildAlt02,
    TableAliasRefListAlt01,
    TableAliasRefListAlt02,
    SelectStmtBasicAlt01,
    SelectStmtFromDualTableAlt01,
    SelectStmtFromTableAlt01,
    TableSampleOptAlt01,
    TableSampleOptAlt02,
    TableSampleOptAlt03,
    TableSampleMethodOptAlt01,
    TableSampleMethodOptAlt02,
    TableSampleMethodOptAlt03,
    TableSampleMethodOptAlt04,
    TableSampleUnitOptAlt01,
    TableSampleUnitOptAlt02,
    TableSampleUnitOptAlt03,
    RepeatableOptAlt01,
    RepeatableOptAlt02,
    SelectStmtAlt01,
    SelectStmtAlt02,
    SelectStmtAlt03,
    SelectStmtAlt04,
    SelectStmtAlt05,
    SelectStmtWithClauseAlt01,
    SelectStmtWithClauseAlt02,
    WithClauseAlt01,
    WithClauseAlt02,
    WithListAlt01,
    WithListAlt02,
    CommonTableExprAlt01,
    WindowClauseOptionalAlt01,
    WindowClauseOptionalAlt02,
    WindowDefinitionListAlt01,
    WindowDefinitionListAlt02,
    WindowDefinitionAlt01,
    WindowNameAlt01,
    WindowSpecAlt01,
    WindowSpecDetailsAlt01,
    OptExistingWindowNameAlt01,
    OptPartitionClauseAlt01,
    OptPartitionClauseAlt02,
    OptWindowOrderByClauseAlt01,
    OptWindowOrderByClauseAlt02,
    OptWindowFrameClauseAlt01,
    OptWindowFrameClauseAlt02,
    WindowFrameUnitsAlt01,
    WindowFrameUnitsAlt02,
    WindowFrameUnitsAlt03,
    WindowFrameExtentAlt01,
    WindowFrameStartAlt01,
    WindowFrameStartAlt02,
    WindowFrameStartAlt03,
    WindowFrameStartAlt04,
    WindowFrameStartAlt05,
    WindowFrameBetweenAlt01,
    WindowFrameBoundAlt02,
    WindowFrameBoundAlt03,
    WindowFrameBoundAlt04,
    WindowFrameBoundAlt05,
    OptWindowingClauseAlt01,
    OptWindowingClauseAlt02,
    WindowingClauseAlt01,
    WindowNameOrSpecAlt01,
    WindowFuncCallAlt01,
    WindowFuncCallAlt02,
    WindowFuncCallAlt03,
    WindowFuncCallAlt04,
    WindowFuncCallAlt05,
    WindowFuncCallAlt06,
    WindowFuncCallAlt07,
    WindowFuncCallAlt08,
    WindowFuncCallAlt09,
    WindowFuncCallAlt10,
    WindowFuncCallAlt11,
    OptLeadLagInfoAlt01,
    OptLeadLagInfoAlt02,
    OptLeadLagInfoAlt03,
    OptLLDefaultAlt01,
    OptLLDefaultAlt02,
    OptNullTreatmentAlt01,
    OptNullTreatmentAlt02,
    OptNullTreatmentAlt03,
    OptFromFirstLastAlt01,
    OptFromFirstLastAlt02,
    OptFromFirstLastAlt03,
    TableRefsClauseAlt01,
    TableRefsAlt01,
    TableRefsAlt02,
    EscapedTableRefAlt02,
    TableFactorAlt01,
    TableFactorAlt02,
    TableFactorAlt03,
    TableFactorAlt04,
    PartitionNameListOptAlt01,
    PartitionNameListOptAlt02,
    TableAsNameOptAlt01,
    TableAsNameOptDeleteAlt01,
    TableAsNameAlt01,
    TableAsNameAlt02,
    IndexHintTypeAlt01,
    IndexHintTypeAlt02,
    IndexHintTypeAlt03,
    IndexHintScopeAlt01,
    IndexHintScopeAlt02,
    IndexHintScopeAlt03,
    IndexHintScopeAlt04,
    IndexHintAlt01,
    IndexNameListAlt01,
    IndexNameListAlt02,
    IndexNameListAlt03,
    IndexNameListAlt04,
    IndexNameListAlt05,
    IndexHintListAlt01,
    IndexHintListAlt02,
    IndexHintListOptAlt01,
    JoinTableAlt01,
    JoinTableAlt02,
    JoinTableAlt03,
    JoinTableAlt04,
    JoinTableAlt05,
    JoinTableAlt06,
    JoinTableAlt07,
    JoinTableAlt08,
    JoinTableAlt09,
    JoinTableAlt10,
    JoinTypeAlt01,
    JoinTypeAlt02,
    JoinTypeAlt03,
    LimitClauseAlt01,
    LimitClauseAlt02,
    LimitOptionAlt01,
    LimitOptionAlt02,
    FetchFirstOptAlt01,
    SelectStmtLimitAlt01,
    SelectStmtLimitAlt02,
    SelectStmtLimitAlt03,
    SelectStmtLimitAlt04,
    SelectStmtLimitOptAlt01,
    SelectStmtOptAlt01,
    SelectStmtOptAlt02,
    SelectStmtOptAlt03,
    SelectStmtOptAlt04,
    SelectStmtOptAlt05,
    SelectStmtOptAlt06,
    SelectStmtOptAlt07,
    SelectStmtOptAlt08,
    SelectStmtOptAlt09,
    SelectStmtOptsAlt01,
    SelectStmtOptsListAlt01,
    TableOptimizerHintsAlt01,
    TableOptimizerHintsOptAlt01,
    SelectStmtSQLCacheAlt01,
    SelectStmtSQLCacheAlt02,
    SelectStmtFieldListAlt01,
    SelectStmtGroupAlt01,
    SelectStmtIntoOptionAlt01,
    SelectStmtIntoOptionAlt02,
    SubSelectAlt01,
    SubSelectAlt02,
    SubSelectAlt03,
    SubSelectAlt04,
    SelectLockOptAlt01,
    SelectLockOptAlt02,
    SelectLockOptAlt03,
    SelectLockOptAlt04,
    SelectLockOptAlt05,
    SelectLockOptAlt06,
    SelectLockOptAlt07,
    SelectLockOptAlt08,
    SelectLockOptAlt09,
    OfTablesOptAlt01,
    OfTablesOptAlt02,
    SetOprStmtAlt03,
    SetOprStmtAlt04,
    SetOprStmtWoutLimitOrderByAlt01,
    SetOprStmtWoutLimitOrderByAlt02,
    SetOprStmtWithLimitOrderByAlt01,
    SetOprStmtWithLimitOrderByAlt02,
    SetOprStmtWithLimitOrderByAlt03,
    SetOprStmtWithLimitOrderByAlt04,
    SetOprStmtWithLimitOrderByAlt05,
    SetOprStmtWithLimitOrderByAlt06,
    SetOprClauseListAlt02,
    SetOprClauseAlt01,
    SetOprClauseAlt02,
    SetOprAlt01,
    SetOprAlt02,
    SetOprAlt03,
    WhereClauseAlt01,
    WhereClauseOptionalAlt01,
    ValuesStmtListAlt01,
    ValuesStmtListAlt02,
    RowStmtAlt01,
}

fn identify(rule_id: RuleId) -> Option<QueryRule> {
    match rule_id.as_str() {
        "field_prec--adfc6ad80526339b" => Some(QueryRule::FieldAlt01),
        "field_identifier_prec--bd90d8902cf5ebba" => Some(QueryRule::FieldAlt02),
        "field_identifier_identifier_prec--87242d3ad01f8af5" => Some(QueryRule::FieldAlt03),
        "field_expression_fieldasnameopt--345551df2e60fe38" => Some(QueryRule::FieldAlt04),
        "fieldasnameopt_prec_higherthanreturning--d66a8b909d7a7f92" => {
            Some(QueryRule::FieldAsNameOptAlt01)
        }
        "fieldasname_as_identifier--f6112dde9e6232c8" => Some(QueryRule::FieldAsNameAlt02),
        "fieldasname_as_stringlit--ba90ffa2205932c3" => Some(QueryRule::FieldAsNameAlt04),
        "fieldlist_field--55ef4bdfa4d5818a" => Some(QueryRule::FieldListAlt01),
        "fieldlist_fieldlist_field--cf5bf28586706ce2" => Some(QueryRule::FieldListAlt02),
        "withrollupclause_prec_lowerthanwith--df78463046b55a95" => {
            Some(QueryRule::WithRollupClauseAlt01)
        }
        "withrollupclause_with_rollup--e0ee89e13b89b58b" => Some(QueryRule::WithRollupClauseAlt02),
        "groupbyclause_group_by_bylist_withrollupclause--4e537f380de76225" => {
            Some(QueryRule::GroupByClauseAlt01)
        }
        "havingclause--d483459f3b06ba51" => Some(QueryRule::HavingClauseAlt01),
        "havingclause_having_expression--e5ef08b4df4409fb" => Some(QueryRule::HavingClauseAlt02),
        "asofclauseopt_prec_empty--198ca96a0a3846c8" => Some(QueryRule::AsOfClauseOptAlt01),
        "asofclause_asof_timestamp_expression--2d2d0974616b013d" => {
            Some(QueryRule::AsOfClauseAlt01)
        }
        "rowvalue_valuesopt--255e628369cf1781" => Some(QueryRule::RowValueAlt01),
        "valuesopt--befd8687c2a001d8" => Some(QueryRule::ValuesOptAlt01),
        "values_values_exprordefault--ee853389c4e15d37" => Some(QueryRule::ValuesAlt01),
        "values_exprordefault--eac802d91f003701" => Some(QueryRule::ValuesAlt02),
        "exprordefault_default--95128c402163c3e4" => Some(QueryRule::ExprOrDefaultAlt02),
        "orderby_order_by_bylist--e28ffaa53722e00d" => Some(QueryRule::OrderByAlt01),
        "bylist_byitem--e3d44ccbbf5e5b83" => Some(QueryRule::ByListAlt01),
        "bylist_bylist_byitem--6f9751166328cfae" => Some(QueryRule::ByListAlt02),
        "byitem_expression--29ec47482a998488" => Some(QueryRule::ByItemAlt01),
        "byitem_expression_order--0b80edc8dc7551f0" => Some(QueryRule::ByItemAlt02),
        "order_asc--976af59cef0d078d" => Some(QueryRule::OrderAlt01),
        "order_desc--6eeb6d9e070f763d" => Some(QueryRule::OrderAlt02),
        "optorder--ee678c8f94d9cf6a" => Some(QueryRule::OptOrderAlt01),
        "optorder_asc--315b30e186b0f25e" => Some(QueryRule::OptOrderAlt02),
        "optorder_desc--92e41d542b821548" => Some(QueryRule::OptOrderAlt03),
        "orderbyoptional_prec_empty--fef7645500cd5ee0" => Some(QueryRule::OrderByOptionalAlt01),
        "priority_low_priority--c6329281ff02a6d9" => Some(QueryRule::PriorityAlt01),
        "priority_high_priority--6d20e17cc31d7da7" => Some(QueryRule::PriorityAlt02),
        "priority_delayed--3fef608e911a1302" => Some(QueryRule::PriorityAlt03),
        "priorityopt--df67fe216fa574ae" => Some(QueryRule::PriorityOptAlt01),
        "tablename_identifier--7d846223712c4126" => Some(QueryRule::TableNameAlt01),
        "tablename_identifier_identifier--01188383505f340b" => Some(QueryRule::TableNameAlt02),
        "tablename_identifier--cd0532a691a4662a" => Some(QueryRule::TableNameAlt03),
        "tablenamelist_tablename--20400d6a3ec9fcec" => Some(QueryRule::TableNameListAlt01),
        "tablenamelist_tablenamelist_tablename--325702bb5eceb0ff" => {
            Some(QueryRule::TableNameListAlt02)
        }
        "tablenameoptwild_identifier_optwild--1950462aaa42bbd6" => {
            Some(QueryRule::TableNameOptWildAlt01)
        }
        "tablenameoptwild_identifier_identifier_optwild--0b3290b74f89a5e5" => {
            Some(QueryRule::TableNameOptWildAlt02)
        }
        "tablealiasreflist_tablenameoptwild--30fd1d27945a5e8f" => {
            Some(QueryRule::TableAliasRefListAlt01)
        }
        "tablealiasreflist_tablealiasreflist_tablenameopt--871549a2ee9ebc78" => {
            Some(QueryRule::TableAliasRefListAlt02)
        }
        "selectstmtbasic_select_selectstmtopts_selectstmt--27a6c1194444bd40" => {
            Some(QueryRule::SelectStmtBasicAlt01)
        }
        "selectstmtfromdualtable_selectstmtbasic_fromdual--c5d0aebde66bfc9e" => {
            Some(QueryRule::SelectStmtFromDualTableAlt01)
        }
        "selectstmtfromtable_selectstmtbasic_from_tablere--e29284d9e0a03db9" => {
            Some(QueryRule::SelectStmtFromTableAlt01)
        }
        "tablesampleopt_prec_empty--a5d88fe5873e5bc8" => Some(QueryRule::TableSampleOptAlt01),
        "tablesampleopt_tablesample_tablesamplemethodopt--595fa1aa2397f417" => {
            Some(QueryRule::TableSampleOptAlt02)
        }
        "tablesampleopt_tablesample_tablesamplemethodopt--6591cccf76347ad0" => {
            Some(QueryRule::TableSampleOptAlt03)
        }
        "tablesamplemethodopt_prec_empty--a8a1687f35cea373" => {
            Some(QueryRule::TableSampleMethodOptAlt01)
        }
        "tablesamplemethodopt_system--e46cccc975c72ddb" => {
            Some(QueryRule::TableSampleMethodOptAlt02)
        }
        "tablesamplemethodopt_bernoulli--34bb4a80671c4bbc" => {
            Some(QueryRule::TableSampleMethodOptAlt03)
        }
        "tablesamplemethodopt_regions--55f2f653b758161f" => {
            Some(QueryRule::TableSampleMethodOptAlt04)
        }
        "tablesampleunitopt_prec_empty--9b259db4a8b6ecda" => {
            Some(QueryRule::TableSampleUnitOptAlt01)
        }
        "tablesampleunitopt_rows--425e6b166987fc36" => Some(QueryRule::TableSampleUnitOptAlt02),
        "tablesampleunitopt_percent--6aaf2d61a2803f80" => Some(QueryRule::TableSampleUnitOptAlt03),
        "repeatableopt_prec_empty--a0945dd80b69e669" => Some(QueryRule::RepeatableOptAlt01),
        "repeatableopt_repeatable_expression--ccf6d36469a0f80e" => {
            Some(QueryRule::RepeatableOptAlt02)
        }
        "selectstmt_selectstmtbasic_whereclauseoptional_s--a21ecaa95690c18f" => {
            Some(QueryRule::SelectStmtAlt01)
        }
        "selectstmt_selectstmtfromdualtable_selectstmtgro--52418a09915b6485" => {
            Some(QueryRule::SelectStmtAlt02)
        }
        "selectstmt_selectstmtfromtable_orderbyoptional_s--0c8c1b144a6c68ee" => {
            Some(QueryRule::SelectStmtAlt03)
        }
        "selectstmt_table_tablename_orderbyoptional_selec--9d3922cb560b15f3" => {
            Some(QueryRule::SelectStmtAlt04)
        }
        "selectstmt_values_valuesstmtlist_orderbyoptional--5eb8774b5e3e346a" => {
            Some(QueryRule::SelectStmtAlt05)
        }
        "selectstmtwithclause_withclause_selectstmt--6c2830e955cc8854" => {
            Some(QueryRule::SelectStmtWithClauseAlt01)
        }
        "selectstmtwithclause_withclause_subselect--5b09170d41530fc0" => {
            Some(QueryRule::SelectStmtWithClauseAlt02)
        }
        "withclause_with_withlist--48a7361852398683" => Some(QueryRule::WithClauseAlt01),
        "withclause_with_recursive_withlist--783be93a7c647ccf" => Some(QueryRule::WithClauseAlt02),
        "withlist_withlist_commontableexpr--db9bea65f8dc066c" => Some(QueryRule::WithListAlt01),
        "withlist_commontableexpr--79685d6b2934b390" => Some(QueryRule::WithListAlt02),
        "commontableexpr_identifier_identlistwithparenopt--c951be5209073e48" => {
            Some(QueryRule::CommonTableExprAlt01)
        }
        "windowclauseoptional--d9f70e493da76b84" => Some(QueryRule::WindowClauseOptionalAlt01),
        "windowclauseoptional_window_windowdefinitionlist--b2726ce31f3a546e" => {
            Some(QueryRule::WindowClauseOptionalAlt02)
        }
        "windowdefinitionlist_windowdefinition--f81a475371e304f4" => {
            Some(QueryRule::WindowDefinitionListAlt01)
        }
        "windowdefinitionlist_windowdefinitionlist_window--539603d64dba587d" => {
            Some(QueryRule::WindowDefinitionListAlt02)
        }
        "windowdefinition_windowname_as_windowspec--7b12a934f60f5035" => {
            Some(QueryRule::WindowDefinitionAlt01)
        }
        "windowname_identifier--377a3fe0779d1bb8" => Some(QueryRule::WindowNameAlt01),
        "windowspec_windowspecdetails--a6a4e6093db4af1f" => Some(QueryRule::WindowSpecAlt01),
        "windowspecdetails_optexistingwindowname_optparti--93c5351d8fae66ee" => {
            Some(QueryRule::WindowSpecDetailsAlt01)
        }
        "optexistingwindowname--90055d9b2e11a9c0" => Some(QueryRule::OptExistingWindowNameAlt01),
        "optpartitionclause--ad7ef00d1e411b65" => Some(QueryRule::OptPartitionClauseAlt01),
        "optpartitionclause_partition_by_bylist--7604bf893de57aba" => {
            Some(QueryRule::OptPartitionClauseAlt02)
        }
        "optwindoworderbyclause--e42428033649509a" => Some(QueryRule::OptWindowOrderByClauseAlt01),
        "optwindoworderbyclause_order_by_bylist--3e91e427461f0b0d" => {
            Some(QueryRule::OptWindowOrderByClauseAlt02)
        }
        "optwindowframeclause--deb1a9327ffdf8da" => Some(QueryRule::OptWindowFrameClauseAlt01),
        "optwindowframeclause_windowframeunits_windowfram--b69bc2eaf7933d16" => {
            Some(QueryRule::OptWindowFrameClauseAlt02)
        }
        "windowframeunits_rows--003e9d2c85bb972f" => Some(QueryRule::WindowFrameUnitsAlt01),
        "windowframeunits_range--99ba8418cadbf047" => Some(QueryRule::WindowFrameUnitsAlt02),
        "windowframeunits_groups--2c888d21058d9220" => Some(QueryRule::WindowFrameUnitsAlt03),
        "windowframeextent_windowframestart--a13a84c728fef200" => {
            Some(QueryRule::WindowFrameExtentAlt01)
        }
        "windowframestart_unbounded_preceding--6bdda3bac1d2c83e" => {
            Some(QueryRule::WindowFrameStartAlt01)
        }
        "windowframestart_numliteral_preceding--c843a9ce1cd3e873" => {
            Some(QueryRule::WindowFrameStartAlt02)
        }
        "windowframestart_parammarker_preceding--91ea6bc5b95e9e0d" => {
            Some(QueryRule::WindowFrameStartAlt03)
        }
        "windowframestart_interval_expression_timeunit_pr--6d8a3cce367206de" => {
            Some(QueryRule::WindowFrameStartAlt04)
        }
        "windowframestart_current_row--c4744ad7fcd1a4f6" => Some(QueryRule::WindowFrameStartAlt05),
        "windowframebetween_between_windowframebound_and--4dc4985d339471aa" => {
            Some(QueryRule::WindowFrameBetweenAlt01)
        }
        "windowframebound_unbounded_following--282b39965b104794" => {
            Some(QueryRule::WindowFrameBoundAlt02)
        }
        "windowframebound_numliteral_following--ff600c8e967dd565" => {
            Some(QueryRule::WindowFrameBoundAlt03)
        }
        "windowframebound_parammarker_following--c4f3b0568581acbf" => {
            Some(QueryRule::WindowFrameBoundAlt04)
        }
        "windowframebound_interval_expression_timeunit_fo--3b6ce28c24796ac4" => {
            Some(QueryRule::WindowFrameBoundAlt05)
        }
        "optwindowingclause--8dcc48e1519412d7" => Some(QueryRule::OptWindowingClauseAlt01),
        "optwindowingclause_windowingclause--1e32e2bd223ae94b" => {
            Some(QueryRule::OptWindowingClauseAlt02)
        }
        "windowingclause_over_windownameorspec--c79766679f7e8356" => {
            Some(QueryRule::WindowingClauseAlt01)
        }
        "windownameorspec_windowname--bdcc369258f8071e" => Some(QueryRule::WindowNameOrSpecAlt01),
        "windowfunccall_row_number_windowingclause--1c87cd16941775ca" => {
            Some(QueryRule::WindowFuncCallAlt01)
        }
        "windowfunccall_rank_windowingclause--14be040d263b484c" => {
            Some(QueryRule::WindowFuncCallAlt02)
        }
        "windowfunccall_dense_rank_windowingclause--50099d631bf47638" => {
            Some(QueryRule::WindowFuncCallAlt03)
        }
        "windowfunccall_cume_dist_windowingclause--2a1d980579b981b7" => {
            Some(QueryRule::WindowFuncCallAlt04)
        }
        "windowfunccall_percent_rank_windowingclause--20d704a36eee9c02" => {
            Some(QueryRule::WindowFuncCallAlt05)
        }
        "windowfunccall_ntile_simpleexpr_windowingclause--7ab841bbf6b0a811" => {
            Some(QueryRule::WindowFuncCallAlt06)
        }
        "windowfunccall_lead_expression_optleadlaginfo_op--d446a9ae8a5eccff" => {
            Some(QueryRule::WindowFuncCallAlt07)
        }
        "windowfunccall_lag_expression_optleadlaginfo_opt--ae7afdabe8eb4d6d" => {
            Some(QueryRule::WindowFuncCallAlt08)
        }
        "windowfunccall_first_value_expression_optnulltre--e9e157db89cf9f32" => {
            Some(QueryRule::WindowFuncCallAlt09)
        }
        "windowfunccall_last_value_expression_optnulltrea--2592580f761eab68" => {
            Some(QueryRule::WindowFuncCallAlt10)
        }
        "windowfunccall_nth_value_expression_simpleexpr_o--25c5d3a0a9080cc2" => {
            Some(QueryRule::WindowFuncCallAlt11)
        }
        "optleadlaginfo--8514b4b5e2aabf84" => Some(QueryRule::OptLeadLagInfoAlt01),
        "optleadlaginfo_numliteral_optlldefault--cb509b0866b176ac" => {
            Some(QueryRule::OptLeadLagInfoAlt02)
        }
        "optleadlaginfo_parammarker_optlldefault--6a89f3503e20857e" => {
            Some(QueryRule::OptLeadLagInfoAlt03)
        }
        "optlldefault--515d84fa322b2557" => Some(QueryRule::OptLLDefaultAlt01),
        "optlldefault_expression--4865c01e92871540" => Some(QueryRule::OptLLDefaultAlt02),
        "optnulltreatment--86bbf706f98f8705" => Some(QueryRule::OptNullTreatmentAlt01),
        "optnulltreatment_respect_nulls--7565f5723ec9ac08" => {
            Some(QueryRule::OptNullTreatmentAlt02)
        }
        "optnulltreatment_ignore_nulls--72f5029adb840148" => Some(QueryRule::OptNullTreatmentAlt03),
        "optfromfirstlast--575b1497c0f005f4" => Some(QueryRule::OptFromFirstLastAlt01),
        "optfromfirstlast_from_first--d899350be2a3c14b" => Some(QueryRule::OptFromFirstLastAlt02),
        "optfromfirstlast_from_last--5818d30ba2492237" => Some(QueryRule::OptFromFirstLastAlt03),
        "tablerefsclause_tablerefs--dad0a204d804e539" => Some(QueryRule::TableRefsClauseAlt01),
        "tablerefs_escapedtableref--cca7677766123fd4" => Some(QueryRule::TableRefsAlt01),
        "tablerefs_tablerefs_escapedtableref--2d84fcfd7b4d500a" => Some(QueryRule::TableRefsAlt02),
        "escapedtableref_identifier_tableref--3b257b88c3a9ce20" => {
            Some(QueryRule::EscapedTableRefAlt02)
        }
        "tablefactor_tablename_partitionnamelistopt_table--576f7b92df05ff5e" => {
            Some(QueryRule::TableFactorAlt01)
        }
        "tablefactor_subselect_tableasnameopt--d10ccb0196334c6f" => {
            Some(QueryRule::TableFactorAlt02)
        }
        "tablefactor_lateral_subselect_tableasname_identl--661840cd0f69db58" => {
            Some(QueryRule::TableFactorAlt03)
        }
        "tablefactor_tablerefs--76dd9f75764a4480" => Some(QueryRule::TableFactorAlt04),
        "partitionnamelistopt--63bd7f0ca404da07" => Some(QueryRule::PartitionNameListOptAlt01),
        "partitionnamelistopt_partition_partitionnamelist--472022a361659a84" => {
            Some(QueryRule::PartitionNameListOptAlt02)
        }
        "tableasnameopt_prec_empty--ba925be0fd99e42b" => Some(QueryRule::TableAsNameOptAlt01),
        "tableasnameoptdelete_prec_higherthanreturning--8c4e5f049bc03c65" => {
            Some(QueryRule::TableAsNameOptDeleteAlt01)
        }
        "tableasname_identifier--155f74b93a366bea" => Some(QueryRule::TableAsNameAlt01),
        "tableasname_as_identifier--4284dac8298fed70" => Some(QueryRule::TableAsNameAlt02),
        "indexhinttype_use_keyorindex--f98d89651db57a0e" => Some(QueryRule::IndexHintTypeAlt01),
        "indexhinttype_ignore_keyorindex--cd8a57e228895739" => Some(QueryRule::IndexHintTypeAlt02),
        "indexhinttype_force_keyorindex--8ff6c792f9ed772a" => Some(QueryRule::IndexHintTypeAlt03),
        "indexhintscope--e7ab9d6a56bc6a66" => Some(QueryRule::IndexHintScopeAlt01),
        "indexhintscope_for_join--1e5edce4098fd704" => Some(QueryRule::IndexHintScopeAlt02),
        "indexhintscope_for_order_by--fc6d229a068a6d7f" => Some(QueryRule::IndexHintScopeAlt03),
        "indexhintscope_for_group_by--bd5e84801309ace0" => Some(QueryRule::IndexHintScopeAlt04),
        "indexhint_indexhinttype_indexhintscope_indexname--bb6e4fb3bfd1ea31" => {
            Some(QueryRule::IndexHintAlt01)
        }
        "indexnamelist--26895f0b1aefbf10" => Some(QueryRule::IndexNameListAlt01),
        "indexnamelist_identifier--65e393437e7da1f2" => Some(QueryRule::IndexNameListAlt02),
        "indexnamelist_indexnamelist_identifier--a1bfbda83bf3fcc3" => {
            Some(QueryRule::IndexNameListAlt03)
        }
        "indexnamelist_primary--f914e685bfafcdd5" => Some(QueryRule::IndexNameListAlt04),
        "indexnamelist_indexnamelist_primary--6ce7fd97295f4c5e" => {
            Some(QueryRule::IndexNameListAlt05)
        }
        "indexhintlist_indexhint--e5a6e4467b943e84" => Some(QueryRule::IndexHintListAlt01),
        "indexhintlist_indexhintlist_indexhint--a0140851f47bc4f9" => {
            Some(QueryRule::IndexHintListAlt02)
        }
        "indexhintlistopt--e327d2fa3c041bdf" => Some(QueryRule::IndexHintListOptAlt01),
        "jointable_tableref_crossopt_tableref_prec_tabler--8ca24cabe063a64d" => {
            Some(QueryRule::JoinTableAlt01)
        }
        "jointable_tableref_crossopt_tableref_on_expressi--0561dc2ee2193c66" => {
            Some(QueryRule::JoinTableAlt02)
        }
        "jointable_tableref_crossopt_tableref_using_colum--a04862b4ea79622f" => {
            Some(QueryRule::JoinTableAlt03)
        }
        "jointable_tableref_jointype_outeropt_join_tabler--c75ae74349fb8509" => {
            Some(QueryRule::JoinTableAlt04)
        }
        "jointable_tableref_jointype_outeropt_join_tabler--f09ae4872211d856" => {
            Some(QueryRule::JoinTableAlt05)
        }
        "jointable_tableref_natural_join_tableref--59dac23b826e27ad" => {
            Some(QueryRule::JoinTableAlt06)
        }
        "jointable_tableref_natural_jointype_outeropt_joi--5e5902f024cba49d" => {
            Some(QueryRule::JoinTableAlt07)
        }
        "jointable_tableref_straight_join_tableref--3b6f0706f2e94adf" => {
            Some(QueryRule::JoinTableAlt08)
        }
        "jointable_tableref_straight_join_tableref_on_exp--5b580a5685decc48" => {
            Some(QueryRule::JoinTableAlt09)
        }
        "jointable_tableref_straight_join_tableref_using--3c107b74be39efd1" => {
            Some(QueryRule::JoinTableAlt10)
        }
        "jointype_left--23e4b6170f2f2db7" => Some(QueryRule::JoinTypeAlt01),
        "jointype_right--90e52b57663562e6" => Some(QueryRule::JoinTypeAlt02),
        "jointype_full_outer_join--3d2a3bf71e63fb6e" => Some(QueryRule::JoinTypeAlt03),
        "limitclause_prec_empty--03e5a7000a2cb76b" => Some(QueryRule::LimitClauseAlt01),
        "limitclause_limit_limitoption--3f45fdc54e5378d3" => Some(QueryRule::LimitClauseAlt02),
        "limitoption_lengthnum--84b5031e6ebe10fc" => Some(QueryRule::LimitOptionAlt01),
        "limitoption_parammarker--48838ddbd9c79b93" => Some(QueryRule::LimitOptionAlt02),
        "fetchfirstopt--b9ab490a15ad004c" => Some(QueryRule::FetchFirstOptAlt01),
        "selectstmtlimit_limit_limitoption--0c6871ce706c62da" => {
            Some(QueryRule::SelectStmtLimitAlt01)
        }
        "selectstmtlimit_limit_limitoption_limitoption--7e37eea6ddf981a8" => {
            Some(QueryRule::SelectStmtLimitAlt02)
        }
        "selectstmtlimit_limit_limitoption_offset_limitop--a1155b969b4ada1d" => {
            Some(QueryRule::SelectStmtLimitAlt03)
        }
        "selectstmtlimit_fetch_firstornext_fetchfirstopt--ec1a1b68011c0476" => {
            Some(QueryRule::SelectStmtLimitAlt04)
        }
        "selectstmtlimitopt--df02f2ed52d28dc7" => Some(QueryRule::SelectStmtLimitOptAlt01),
        "selectstmtopt_tableoptimizerhints--a21aad8794614064" => {
            Some(QueryRule::SelectStmtOptAlt01)
        }
        "selectstmtopt_distinctopt--5e047c166185205e" => Some(QueryRule::SelectStmtOptAlt02),
        "selectstmtopt_priority--a99b80c2ddaa442d" => Some(QueryRule::SelectStmtOptAlt03),
        "selectstmtopt_sql_small_result--88f1b2f99e82b76d" => Some(QueryRule::SelectStmtOptAlt04),
        "selectstmtopt_sql_big_result--4e4af3549a40a86c" => Some(QueryRule::SelectStmtOptAlt05),
        "selectstmtopt_sql_buffer_result--b398bb4eb91667f0" => Some(QueryRule::SelectStmtOptAlt06),
        "selectstmtopt_selectstmtsqlcache--798a0d66cfbd7339" => Some(QueryRule::SelectStmtOptAlt07),
        "selectstmtopt_sql_calc_found_rows--3ee2e8dae324f03c" => {
            Some(QueryRule::SelectStmtOptAlt08)
        }
        "selectstmtopt_straight_join--e09773c761b8725a" => Some(QueryRule::SelectStmtOptAlt09),
        "selectstmtopts_prec_empty--344ee12e53dcad03" => Some(QueryRule::SelectStmtOptsAlt01),
        "selectstmtoptslist_selectstmtoptslist_selectstmt--e65b3b45c3cd6b93" => {
            Some(QueryRule::SelectStmtOptsListAlt01)
        }
        "tableoptimizerhints_hintcomment--6963ea6c3c4b12b1" => {
            Some(QueryRule::TableOptimizerHintsAlt01)
        }
        "tableoptimizerhintsopt--b6f7c871fb846e69" => Some(QueryRule::TableOptimizerHintsOptAlt01),
        "selectstmtsqlcache_sql_cache--48b2cdaa7232edeb" => {
            Some(QueryRule::SelectStmtSQLCacheAlt01)
        }
        "selectstmtsqlcache_sql_no_cache--1999985588438ad1" => {
            Some(QueryRule::SelectStmtSQLCacheAlt02)
        }
        "selectstmtfieldlist_fieldlist--072aabc2e0e51688" => {
            Some(QueryRule::SelectStmtFieldListAlt01)
        }
        "selectstmtgroup--5151c65bb74cac22" => Some(QueryRule::SelectStmtGroupAlt01),
        "selectstmtintooption--2546f50013e5f020" => Some(QueryRule::SelectStmtIntoOptionAlt01),
        "selectstmtintooption_into_outfile_stringlit_fiel--119fa21262dfaaab" => {
            Some(QueryRule::SelectStmtIntoOptionAlt02)
        }
        "subselect_selectstmt--37aed99ca1ab33e3" => Some(QueryRule::SubSelectAlt01),
        "subselect_setoprstmt--5f0bcd7f8e4ed554" => Some(QueryRule::SubSelectAlt02),
        "subselect_selectstmtwithclause--5daf9a493f8e0922" => Some(QueryRule::SubSelectAlt03),
        "subselect_subselect--de01f65bcc36b927" => Some(QueryRule::SubSelectAlt04),
        "selectlockopt--ea6be10d089508db" => Some(QueryRule::SelectLockOptAlt01),
        "selectlockopt_for_update_oftablesopt--a8ebc851a0bb3b6f" => {
            Some(QueryRule::SelectLockOptAlt02)
        }
        "selectlockopt_for_share_oftablesopt--157ac2f3809b24e3" => {
            Some(QueryRule::SelectLockOptAlt03)
        }
        "selectlockopt_for_update_oftablesopt_nowait--42c4920bb50192af" => {
            Some(QueryRule::SelectLockOptAlt04)
        }
        "selectlockopt_for_update_oftablesopt_wait_num--942937e2d4242fe2" => {
            Some(QueryRule::SelectLockOptAlt05)
        }
        "selectlockopt_for_share_oftablesopt_nowait--eb8762fdadc8e89b" => {
            Some(QueryRule::SelectLockOptAlt06)
        }
        "selectlockopt_for_update_oftablesopt_skip_locked--763e561193cea976" => {
            Some(QueryRule::SelectLockOptAlt07)
        }
        "selectlockopt_for_share_oftablesopt_skip_locked--7ef55c872080b9ca" => {
            Some(QueryRule::SelectLockOptAlt08)
        }
        "selectlockopt_lock_in_share_mode--19ee2b48793f86d2" => Some(QueryRule::SelectLockOptAlt09),
        "oftablesopt--40058fce305cbdfa" => Some(QueryRule::OfTablesOptAlt01),
        "oftablesopt_of_tablenamelist--4b978e4668974127" => Some(QueryRule::OfTablesOptAlt02),
        "setoprstmt_withclause_setoprstmtwithlimitorderby--fd50d6d9bd5b3045" => {
            Some(QueryRule::SetOprStmtAlt03)
        }
        "setoprstmt_withclause_setoprstmtwoutlimitorderby--9339da25c08017fe" => {
            Some(QueryRule::SetOprStmtAlt04)
        }
        "setoprstmtwoutlimitorderby_setoprclauselist_seto--8a49b3f62182353f" => {
            Some(QueryRule::SetOprStmtWoutLimitOrderByAlt01)
        }
        "setoprstmtwoutlimitorderby_setoprclauselist_seto--208afd7efb4ff1d1" => {
            Some(QueryRule::SetOprStmtWoutLimitOrderByAlt02)
        }
        "setoprstmtwithlimitorderby_setoprclauselist_seto--b4b54307bfa85e6d" => {
            Some(QueryRule::SetOprStmtWithLimitOrderByAlt01)
        }
        "setoprstmtwithlimitorderby_setoprclauselist_seto--58d8aa458c3d5aad" => {
            Some(QueryRule::SetOprStmtWithLimitOrderByAlt02)
        }
        "setoprstmtwithlimitorderby_setoprclauselist_seto--b591cdf16a593e90" => {
            Some(QueryRule::SetOprStmtWithLimitOrderByAlt03)
        }
        "setoprstmtwithlimitorderby_subselect_orderby--e9758643e0785e04" => {
            Some(QueryRule::SetOprStmtWithLimitOrderByAlt04)
        }
        "setoprstmtwithlimitorderby_subselect_selectstmtl--118d7750c3ec2a14" => {
            Some(QueryRule::SetOprStmtWithLimitOrderByAlt05)
        }
        "setoprstmtwithlimitorderby_subselect_orderby_sel--672ed47d450a8729" => {
            Some(QueryRule::SetOprStmtWithLimitOrderByAlt06)
        }
        "setoprclauselist_setoprclauselist_setopr_setoprc--346dc987a4161ef3" => {
            Some(QueryRule::SetOprClauseListAlt02)
        }
        "setoprclause_selectstmt--42a930695ee2ef08" => Some(QueryRule::SetOprClauseAlt01),
        "setoprclause_subselect--da5dd144c0f2ae4c" => Some(QueryRule::SetOprClauseAlt02),
        "setopr_union_setopropt--661d5006f264ae58" => Some(QueryRule::SetOprAlt01),
        "setopr_except_setopropt--8671b549409f6ae8" => Some(QueryRule::SetOprAlt02),
        "setopr_intersect_setopropt--7432f665920308e6" => Some(QueryRule::SetOprAlt03),
        "whereclause_where_expression--b88e9b599898c861" => Some(QueryRule::WhereClauseAlt01),
        "whereclauseoptional_prec_empty--955d363cd91cac8b" => {
            Some(QueryRule::WhereClauseOptionalAlt01)
        }
        "valuesstmtlist_rowstmt--e1662922c96b2f96" => Some(QueryRule::ValuesStmtListAlt01),
        "valuesstmtlist_valuesstmtlist_rowstmt--ff31353e73923462" => {
            Some(QueryRule::ValuesStmtListAlt02)
        }
        "rowstmt_row_rowvalue--8b332fa25db826e5" => Some(QueryRule::RowStmtAlt01),
        _ => None,
    }
}

pub(super) fn owns(rule_id: RuleId) -> bool {
    identify(rule_id).is_some()
}

pub(super) fn apply(
    rule_id: RuleId,
    rhs: Rhs<'_>,
    context: Context<'_>,
) -> Option<Result<bool, isize>> {
    let rule = identify(rule_id)?;
    Some(apply_rule(rule, rhs, context))
}

fn apply_rule(rule: QueryRule, mut rhs: Rhs<'_>, context: Context<'_>) -> Result<bool, isize> {
    let rhs_len = rhs.len();
    let Context {
        output: out,
        parser_state,
        lexer: yylex,
    } = context;
    match rule {
        QueryRule::AsOfClauseAlt01 => {
            let Some(expr) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::AsOfClause { TsExpr: expr }));
        }
        QueryRule::PriorityAlt01 => out.item = Some(Box::new(1i32)),
        QueryRule::PriorityAlt02 => out.item = Some(Box::new(2i32)),
        QueryRule::PriorityAlt03 => out.item = Some(Box::new(3i32)),
        QueryRule::PriorityOptAlt01 => out.item = Some(Box::new(0i32)),
        QueryRule::TableSampleOptAlt01
        | QueryRule::WindowClauseOptionalAlt01
        | QueryRule::OptWindowingClauseAlt01
        | QueryRule::TableOptimizerHintsOptAlt01
        | QueryRule::SelectStmtGroupAlt01
        | QueryRule::SelectStmtIntoOptionAlt01
        | QueryRule::SelectLockOptAlt01 => out.item = None,
        QueryRule::TableSampleOptAlt02 | QueryRule::TableSampleOptAlt03 => {
            let method_back = if rule == QueryRule::TableSampleOptAlt02 {
                5
            } else {
                3
            };
            let repeat_back = 0;
            out.item = Some(Box::new(parser_ast::TableSample {
                SampleMethod: rhs[rhs_len - (method_back)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::SampleMethodType>())
                    .copied()
                    .unwrap_or_default(),
                Expr: if rule == QueryRule::TableSampleOptAlt02 {
                    rhs[rhs_len - (3)].expr.clone()
                } else {
                    None
                },
                SampleClauseUnit: if rule == QueryRule::TableSampleOptAlt02 {
                    rhs[rhs_len - (2)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::SampleClauseUnitType>())
                        .copied()
                        .unwrap_or_default()
                } else {
                    Default::default()
                },
                RepeatableSeed: rhs[rhs_len - (repeat_back)].expr.clone(),
            }));
        }
        QueryRule::TableSampleMethodOptAlt01
        | QueryRule::TableSampleMethodOptAlt02
        | QueryRule::TableSampleMethodOptAlt03
        | QueryRule::TableSampleMethodOptAlt04 => {
            out.item = Some(Box::new(match rule {
                QueryRule::TableSampleMethodOptAlt01 => parser_ast::SampleMethodType::None,
                QueryRule::TableSampleMethodOptAlt02 => parser_ast::SampleMethodType::System,
                QueryRule::TableSampleMethodOptAlt03 => parser_ast::SampleMethodType::Bernoulli,
                _ => parser_ast::SampleMethodType::TiDBRegion,
            }))
        }
        QueryRule::TableSampleUnitOptAlt01
        | QueryRule::TableSampleUnitOptAlt02
        | QueryRule::TableSampleUnitOptAlt03 => {
            out.item = Some(Box::new(match rule {
                QueryRule::TableSampleUnitOptAlt01 => parser_ast::SampleClauseUnitType::Default,
                QueryRule::TableSampleUnitOptAlt02 => parser_ast::SampleClauseUnitType::Row,
                _ => parser_ast::SampleClauseUnitType::Percent,
            }))
        }
        QueryRule::RepeatableOptAlt01 => out.expr = None,
        QueryRule::RepeatableOptAlt02 => out.expr = rhs[rhs_len - (1)].expr.clone(),
        QueryRule::SelectStmtWithClauseAlt01 => {
            let Some(with_clause) = rhs[rhs_len - (1)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::WithClause>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let Some(statement) = rhs[rhs_len - (0)].statement.take() else {
                return Ok(false);
            };
            let Ok(mut select) = statement.into_any().downcast::<parser_ast::SelectStmt>() else {
                return Ok(false);
            };
            select.With = Some(with_clause.into_shared());
            out.statement = Some(select);
        }
        QueryRule::SelectStmtWithClauseAlt02 => {
            let Some(with_clause) = rhs[rhs_len - (1)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::WithClause>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let Some(subquery) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<SubquerySemantic>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let Some(statement) = subquery.query.take() else {
                return Ok(false);
            };
            let statement = match statement.into_any().downcast::<parser_ast::SelectStmt>() {
                Ok(mut select) => {
                    select.IsInBraces = true;
                    select.WithBeforeBraces = true;
                    select.With = Some(with_clause.into_shared());
                    select as Box<dyn parser_ast::Node>
                }
                Err(statement) => match statement.downcast::<parser_ast::SetOprStmt>() {
                    Ok(mut set_op) => {
                        set_op.IsInBraces = true;
                        set_op.With = Some(with_clause.into_shared());
                        set_op as Box<dyn parser_ast::Node>
                    }
                    Err(_) => return Ok(false),
                },
            };
            out.statement = Some(statement);
        }
        QueryRule::WithClauseAlt01 => out.item = rhs[rhs_len - (0)].item.take(),
        QueryRule::WithClauseAlt02 => {
            let Some(mut with_clause) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::WithClause>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            with_clause.IsRecursive = true;
            for cte in &mut with_clause.CTEs {
                cte.IsRecursive = true;
            }
            out.item = Some(Box::new(with_clause));
        }
        QueryRule::WithListAlt01 => {
            let Some(mut with_clause) = rhs[rhs_len - (2)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::WithClause>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let Some(cte) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::CommonTableExpression>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            with_clause.CTEs.push(cte);
            out.item = Some(Box::new(with_clause));
        }
        QueryRule::WithListAlt02 => {
            let Some(cte) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::CommonTableExpression>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::WithClause {
                IsRecursive: false,
                CTEs: vec![cte],
            }));
        }
        QueryRule::CommonTableExprAlt01 => {
            let Some(subquery) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<SubquerySemantic>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let Some(query) = subquery.query.take() else {
                return Ok(false);
            };
            let columns = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::CommonTableExpression {
                Name: parser_ast::NewCIStr(&rhs[rhs_len - (3)].ident),
                ColNameList: columns,
                Query: query,
                IsRecursive: false,
            }));
        }
        QueryRule::WindowClauseOptionalAlt02 => out.item = rhs[rhs_len - (0)].item.take(),
        QueryRule::WindowDefinitionListAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::WindowSpec>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        QueryRule::WindowDefinitionListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::WindowSpec>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::WindowSpec>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        QueryRule::WindowDefinitionAlt01 => {
            let Some(mut spec) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::WindowSpec>())
                .cloned()
            else {
                return Ok(false);
            };
            spec.Name = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::CIStr>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(spec));
        }
        QueryRule::WindowNameAlt01 => {
            out.item = Some(Box::new(parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident)))
        }
        QueryRule::WindowSpecAlt01 => out.item = rhs[rhs_len - (1)].item.take(),
        QueryRule::WindowSpecDetailsAlt01 => {
            out.item = Some(Box::new(parser_ast::WindowSpec {
                Ref: rhs[rhs_len - (3)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::CIStr>())
                    .cloned()
                    .unwrap_or_default(),
                PartitionBy: rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                    .cloned()
                    .unwrap_or_default(),
                OrderBy: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                    .cloned()
                    .unwrap_or_default(),
                Frame: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::FrameClause>())
                    .cloned()
                    .map(Box::new),
                ..Default::default()
            }))
        }
        QueryRule::OptExistingWindowNameAlt01 => {
            out.item = Some(Box::new(parser_ast::CIStr::default()))
        }
        QueryRule::OptPartitionClauseAlt01
        | QueryRule::OptWindowOrderByClauseAlt01
        | QueryRule::OptWindowFrameClauseAlt01 => out.item = None,
        QueryRule::OptPartitionClauseAlt02 | QueryRule::OptWindowOrderByClauseAlt02 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                    .cloned()
                    .unwrap_or_default(),
            ))
        }
        QueryRule::OptWindowFrameClauseAlt02 => {
            let frame_type = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::FrameType>())
                .copied()
                .unwrap_or_default();
            let Some(extent) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::FrameExtent>())
                .cloned()
            else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::FrameClause {
                Type: frame_type,
                Extent: extent,
            }));
        }
        QueryRule::WindowFrameUnitsAlt01
        | QueryRule::WindowFrameUnitsAlt02
        | QueryRule::WindowFrameUnitsAlt03 => {
            out.item = Some(Box::new(match rule {
                QueryRule::WindowFrameUnitsAlt01 => parser_ast::FrameType::Rows,
                QueryRule::WindowFrameUnitsAlt02 => parser_ast::FrameType::Ranges,
                _ => parser_ast::FrameType::Groups,
            }))
        }
        QueryRule::WindowFrameExtentAlt01 => {
            let Some(start) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::FrameBound>())
                .cloned()
            else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::FrameExtent {
                Start: start,
                End: parser_ast::FrameBound {
                    Type: parser_ast::BoundType::CurrentRow,
                    ..Default::default()
                },
            }));
        }
        QueryRule::WindowFrameStartAlt01
        | QueryRule::WindowFrameStartAlt02
        | QueryRule::WindowFrameStartAlt03
        | QueryRule::WindowFrameStartAlt04
        | QueryRule::WindowFrameStartAlt05
        | QueryRule::WindowFrameBoundAlt02
        | QueryRule::WindowFrameBoundAlt03
        | QueryRule::WindowFrameBoundAlt04
        | QueryRule::WindowFrameBoundAlt05 => {
            let preceding = rule <= QueryRule::WindowFrameStartAlt05;
            let bound_type = if rule == QueryRule::WindowFrameStartAlt05 {
                parser_ast::BoundType::CurrentRow
            } else if preceding {
                parser_ast::BoundType::Preceding
            } else {
                parser_ast::BoundType::Following
            };
            let unbounded = matches!(
                rule,
                QueryRule::WindowFrameStartAlt01 | QueryRule::WindowFrameBoundAlt02
            );
            let expr = match rule {
                QueryRule::WindowFrameStartAlt02 | QueryRule::WindowFrameBoundAlt03 => {
                    Some(parser_ast::ExprNode::Value(
                        rhs[rhs_len - (1)]
                            .item
                            .as_deref()
                            .map(semantic_value_text)
                            .unwrap_or_default(),
                    ))
                }
                QueryRule::WindowFrameStartAlt03 | QueryRule::WindowFrameBoundAlt04 => Some(
                    parser_ast::ExprNode::ParamMarker(rhs[rhs_len - (0)].offset.max(0) as usize),
                ),
                QueryRule::WindowFrameStartAlt04 | QueryRule::WindowFrameBoundAlt05 => {
                    rhs[rhs_len - (2)].expr.clone()
                }
                _ => None,
            };
            let unit = if matches!(
                rule,
                QueryRule::WindowFrameStartAlt04 | QueryRule::WindowFrameBoundAlt05
            ) {
                rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TimeUnitType>())
                    .copied()
                    .unwrap_or_default()
            } else {
                parser_ast::TimeUnitType::Invalid
            };
            out.item = Some(Box::new(parser_ast::FrameBound {
                Type: bound_type,
                UnBounded: unbounded,
                Expr: expr,
                Unit: unit,
            }));
        }
        QueryRule::WindowFrameBetweenAlt01 => {
            let Some(start) = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::FrameBound>())
                .cloned()
            else {
                return Ok(false);
            };
            let Some(end) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::FrameBound>())
                .cloned()
            else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::FrameExtent {
                Start: start,
                End: end,
            }));
        }
        QueryRule::OptWindowingClauseAlt02 | QueryRule::WindowingClauseAlt01 => {
            out.item = rhs[rhs_len - (0)].item.take()
        }
        QueryRule::WindowNameOrSpecAlt01 => {
            out.item = Some(Box::new(parser_ast::WindowSpec {
                Name: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::CIStr>())
                    .cloned()
                    .unwrap_or_default(),
                OnlyAlias: true,
                ..Default::default()
            }))
        }
        QueryRule::WindowFuncCallAlt01
        | QueryRule::WindowFuncCallAlt02
        | QueryRule::WindowFuncCallAlt03
        | QueryRule::WindowFuncCallAlt04
        | QueryRule::WindowFuncCallAlt05
        | QueryRule::WindowFuncCallAlt06
        | QueryRule::WindowFuncCallAlt09
        | QueryRule::WindowFuncCallAlt10
        | QueryRule::WindowFuncCallAlt11 => {
            let (name_back, args, ignore_back, from_last_back) = match rule {
                QueryRule::WindowFuncCallAlt01
                | QueryRule::WindowFuncCallAlt02
                | QueryRule::WindowFuncCallAlt03
                | QueryRule::WindowFuncCallAlt04
                | QueryRule::WindowFuncCallAlt05 => (3, Vec::new(), None, None),
                QueryRule::WindowFuncCallAlt06 => (
                    4,
                    rhs[rhs_len - (2)].expr.clone().into_iter().collect(),
                    None,
                    None,
                ),
                QueryRule::WindowFuncCallAlt09 | QueryRule::WindowFuncCallAlt10 => (
                    5,
                    rhs[rhs_len - (3)].expr.clone().into_iter().collect(),
                    Some(1),
                    None,
                ),
                _ => (
                    8,
                    [
                        rhs[rhs_len - (6)].expr.clone(),
                        rhs[rhs_len - (4)].expr.clone(),
                    ]
                    .into_iter()
                    .flatten()
                    .collect(),
                    Some(1),
                    Some(2),
                ),
            };
            let Some(spec) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::WindowSpec>())
                .cloned()
            else {
                return Ok(false);
            };
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::WindowFunction {
                    Name: rhs[rhs_len - (name_back)].ident.clone(),
                    Args: args,
                    Distinct: false,
                    IgnoreNull: ignore_back
                        .and_then(|back| rhs[rhs_len - (back)].item.as_deref())
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false),
                    FromLast: from_last_back
                        .and_then(|back| rhs[rhs_len - (back)].item.as_deref())
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false),
                    Spec: Box::new(spec),
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        QueryRule::WindowFuncCallAlt07 | QueryRule::WindowFuncCallAlt08 => {
            let mut args = rhs[rhs_len - (4)]
                .expr
                .clone()
                .into_iter()
                .collect::<Vec<_>>();
            if let Some(extra) = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
            {
                args.extend(extra.clone());
            }
            let Some(spec) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::WindowSpec>())
                .cloned()
            else {
                return Ok(false);
            };
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::WindowFunction {
                    Name: rhs[rhs_len - (6)].ident.clone(),
                    Args: args,
                    Distinct: false,
                    IgnoreNull: rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false),
                    FromLast: false,
                    Spec: Box::new(spec),
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        QueryRule::OptLeadLagInfoAlt01 | QueryRule::OptLLDefaultAlt01 => out.item = None,
        QueryRule::OptLeadLagInfoAlt02 => {
            let mut args = vec![parser_ast::ExprNode::Value(
                rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .map(semantic_value_text)
                    .unwrap_or_default(),
            )];
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
            {
                args.push(value.clone());
            }
            out.item = Some(Box::new(args));
        }
        QueryRule::OptLeadLagInfoAlt03 => {
            let mut args = vec![parser_ast::ExprNode::ParamMarker(
                rhs[rhs_len - (1)].offset.max(0) as usize,
            )];
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
            {
                args.push(value.clone());
            }
            out.item = Some(Box::new(args));
        }
        QueryRule::OptLLDefaultAlt02 => {
            out.item = rhs[rhs_len - (0)]
                .expr
                .clone()
                .map(|expr| Box::new(expr) as Box<dyn Any>)
        }
        QueryRule::OptNullTreatmentAlt01
        | QueryRule::OptNullTreatmentAlt02
        | QueryRule::OptFromFirstLastAlt01
        | QueryRule::OptFromFirstLastAlt02 => out.item = Some(Box::new(false)),
        QueryRule::OptNullTreatmentAlt03 | QueryRule::OptFromFirstLastAlt03 => {
            out.item = Some(Box::new(true))
        }
        QueryRule::SelectStmtIntoOptionAlt02 => {
            out.item = Some(Box::new(parser_ast::SelectIntoOption {
                Tp: parser_ast::SelectIntoType::Outfile,
                FileName: rhs[rhs_len - (2)].ident.clone(),
                FieldsInfo: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::FieldsClause>())
                    .cloned(),
                LinesInfo: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::LinesClause>())
                    .cloned(),
            }))
        }
        QueryRule::SubSelectAlt01 | QueryRule::SubSelectAlt02 | QueryRule::SubSelectAlt03 => {
            let Some(statement) = rhs[rhs_len - (1)].statement.take() else {
                return Ok(false);
            };
            let query = parser_ast::NodeRef::new(statement);
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Subquery {
                    Query: query.clone(),
                    MultiRows: false,
                    Exists: false,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
            out.item = Some(Box::new(SubquerySemantic { query }));
        }
        QueryRule::SubSelectAlt04 => {
            let Some(subquery) = rhs[rhs_len - (1)]
                .item
                .take()
                .and_then(|item| item.downcast::<SubquerySemantic>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Subquery {
                    Query: subquery.query.clone(),
                    MultiRows: false,
                    Exists: false,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
            out.item = Some(Box::new(subquery));
        }
        QueryRule::SelectLockOptAlt02
        | QueryRule::SelectLockOptAlt03
        | QueryRule::SelectLockOptAlt04
        | QueryRule::SelectLockOptAlt05
        | QueryRule::SelectLockOptAlt06
        | QueryRule::SelectLockOptAlt07
        | QueryRule::SelectLockOptAlt08
        | QueryRule::SelectLockOptAlt09 => {
            let (lock_type, tables_back, wait_back) = match rule {
                QueryRule::SelectLockOptAlt02 => {
                    (parser_ast::SelectLockType::ForUpdate, Some(0), None)
                }
                QueryRule::SelectLockOptAlt03 => {
                    (parser_ast::SelectLockType::ForShare, Some(0), None)
                }
                QueryRule::SelectLockOptAlt04 => {
                    (parser_ast::SelectLockType::ForUpdateNoWait, Some(1), None)
                }
                QueryRule::SelectLockOptAlt05 => {
                    (parser_ast::SelectLockType::ForUpdateWaitN, Some(2), Some(0))
                }
                QueryRule::SelectLockOptAlt06 => {
                    (parser_ast::SelectLockType::ForShareNoWait, Some(1), None)
                }
                QueryRule::SelectLockOptAlt07 => (
                    parser_ast::SelectLockType::ForUpdateSkipLocked,
                    Some(2),
                    None,
                ),
                QueryRule::SelectLockOptAlt08 => (
                    parser_ast::SelectLockType::ForShareSkipLocked,
                    Some(2),
                    None,
                ),
                _ => (parser_ast::SelectLockType::ForShare, None, None),
            };
            let tables = tables_back
                .and_then(|back| rhs[rhs_len - (back)].item.as_deref())
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                .cloned()
                .unwrap_or_default();
            let wait_sec = wait_back
                .and_then(|back| rhs[rhs_len - (back)].item.as_deref())
                .map(getUint64FromNUM)
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::SelectLockInfo {
                lock_type,
                LockType: lock_type,
                WaitSec: wait_sec,
                Tables: tables,
            }));
        }
        QueryRule::OfTablesOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::TableName>::new()))
        }
        QueryRule::OfTablesOptAlt02 => out.item = rhs[rhs_len - (0)].item.take(),
        QueryRule::SetOprStmtWoutLimitOrderByAlt01 => {
            let Some(mut list) = rhs[rhs_len - (2)]
                .item
                .take()
                .and_then(|item| item.downcast::<SetOprSemantic>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let operator = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SetOprType>())
                .copied()
                .unwrap_or_default();
            let Some(statement) = rhs[rhs_len - (0)].statement.take() else {
                return Ok(false);
            };
            let Ok(mut select) = statement.into_any().downcast::<parser_ast::SelectStmt>() else {
                return Ok(false);
            };
            let order_by = std::mem::take(&mut select.OrderBy);
            let limit = select.Limit.take();
            list.nodes.push(select);
            list.operators.push(Some(operator));
            let mut statement = parser_ast::SetOprStmt::new(parser_ast::SetOprSelectList {
                node_text: Default::default(),
                selects: list.nodes,
                operators: list.operators,
                With: None,
                OrderBy: Vec::new(),
                Limit: None,
                AfterSetOperator: None,
            });
            statement.OrderBy = order_by;
            statement.Limit = limit;
            out.statement = Some(Box::new(statement));
        }
        QueryRule::SetOprStmtAlt03 | QueryRule::SetOprStmtAlt04 => {
            let Some(with_clause) = rhs[rhs_len - (1)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::WithClause>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let Some(statement) = rhs[rhs_len - (0)].statement.take() else {
                return Ok(false);
            };
            let Ok(mut set_op) = statement.into_any().downcast::<parser_ast::SetOprStmt>() else {
                return Ok(false);
            };
            set_op.With = Some(with_clause.into_shared());
            out.statement = Some(set_op);
        }
        QueryRule::SetOprStmtWoutLimitOrderByAlt02
        | QueryRule::SetOprStmtWithLimitOrderByAlt01
        | QueryRule::SetOprStmtWithLimitOrderByAlt02
        | QueryRule::SetOprStmtWithLimitOrderByAlt03 => {
            let (left_back, operator_back, subquery_back, order_back, limit_back) = match rule {
                QueryRule::SetOprStmtWoutLimitOrderByAlt02 => (2, 1, 0, None, None),
                QueryRule::SetOprStmtWithLimitOrderByAlt01 => (3, 2, 1, Some(0), None),
                QueryRule::SetOprStmtWithLimitOrderByAlt02 => (3, 2, 1, None, Some(0)),
                _ => (4, 3, 2, Some(1), Some(0)),
            };
            let Some(mut left) = rhs[rhs_len - (left_back)]
                .item
                .take()
                .and_then(|item| item.downcast::<SetOprSemantic>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let operator = rhs[rhs_len - (operator_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SetOprType>())
                .copied()
                .unwrap_or_default();
            let Some(subquery) = take_subquery_statement(&mut rhs[rhs_len - (subquery_back)])
            else {
                return Ok(false);
            };
            let Some(mut nested) = nested_set_op_list(subquery, true) else {
                return Ok(false);
            };
            nested.AfterSetOperator = Some(operator);
            left.nodes.push(Box::new(nested));
            left.operators.push(Some(operator));
            let mut statement = parser_ast::SetOprStmt::new(parser_ast::SetOprSelectList {
                node_text: Default::default(),
                selects: left.nodes,
                operators: left.operators,
                With: None,
                OrderBy: Vec::new(),
                Limit: None,
                AfterSetOperator: None,
            });
            if let Some(back) = order_back {
                statement.OrderBy = rhs[rhs_len - (back)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                    .cloned()
                    .unwrap_or_default();
            }
            if let Some(back) = limit_back {
                statement.Limit = rhs[rhs_len - (back)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::Limit>())
                    .cloned();
            }
            out.statement = Some(Box::new(statement));
        }
        QueryRule::SetOprStmtWithLimitOrderByAlt04
        | QueryRule::SetOprStmtWithLimitOrderByAlt05
        | QueryRule::SetOprStmtWithLimitOrderByAlt06 => {
            let (subquery_back, order_back, limit_back) = match rule {
                QueryRule::SetOprStmtWithLimitOrderByAlt04 => (1, Some(0), None),
                QueryRule::SetOprStmtWithLimitOrderByAlt05 => (1, None, Some(0)),
                _ => (2, Some(1), Some(0)),
            };
            let Some(subquery) = take_subquery_statement(&mut rhs[rhs_len - (subquery_back)])
            else {
                return Ok(false);
            };
            let Some(nested) = nested_set_op_list(subquery, true) else {
                return Ok(false);
            };
            let mut statement = parser_ast::SetOprStmt::new(parser_ast::SetOprSelectList::new(
                vec![Box::new(nested)],
            ));
            if let Some(back) = order_back {
                statement.OrderBy = rhs[rhs_len - (back)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                    .cloned()
                    .unwrap_or_default();
            }
            if let Some(back) = limit_back {
                statement.Limit = rhs[rhs_len - (back)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::Limit>())
                    .cloned();
            }
            out.statement = Some(Box::new(statement));
        }
        QueryRule::SetOprClauseListAlt02 => {
            let Some(mut left) = rhs[rhs_len - (2)]
                .item
                .take()
                .and_then(|item| item.downcast::<SetOprSemantic>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let Some(mut right) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<SetOprSemantic>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let operator = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SetOprType>())
                .copied()
                .unwrap_or_default();
            if let Some(first) = right.operators.first_mut() {
                *first = Some(operator);
            }
            left.nodes.append(&mut right.nodes);
            left.operators.append(&mut right.operators);
            out.item = Some(Box::new(left));
        }
        QueryRule::SetOprClauseAlt01 => {
            let Some(statement) = rhs[rhs_len - (0)].statement.take() else {
                return Ok(false);
            };
            out.item = Some(Box::new(SetOprSemantic {
                nodes: vec![statement],
                operators: vec![None],
            }));
        }
        QueryRule::SetOprClauseAlt02 => {
            let Some(subquery) = take_subquery_statement(&mut rhs[rhs_len - (0)]) else {
                return Ok(false);
            };
            let Some(nested) = nested_set_op_list(subquery, false) else {
                return Ok(false);
            };
            out.item = Some(Box::new(SetOprSemantic {
                nodes: vec![Box::new(nested)],
                operators: vec![None],
            }));
        }
        QueryRule::SetOprAlt01 | QueryRule::SetOprAlt02 | QueryRule::SetOprAlt03 => {
            let distinct = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(true);
            let operator = match (rule, distinct) {
                (QueryRule::SetOprAlt01, true) => parser_ast::SetOprType::Union,
                (QueryRule::SetOprAlt01, false) => parser_ast::SetOprType::UnionAll,
                (QueryRule::SetOprAlt02, true) => parser_ast::SetOprType::Except,
                (QueryRule::SetOprAlt02, false) => parser_ast::SetOprType::ExceptAll,
                (QueryRule::SetOprAlt03, true) => parser_ast::SetOprType::Intersect,
                _ => parser_ast::SetOprType::IntersectAll,
            };
            out.item = Some(Box::new(operator));
        }
        QueryRule::SelectStmtBasicAlt01 => {
            let options = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SelectStmtOpts>())
                .cloned()
                .unwrap_or_default();
            let fields = rhs[rhs_len - (1)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::FieldList>().ok())
                .map(|fields| *fields)
                .unwrap_or_default();
            let table_hints = options.TableHints.clone();
            out.item = Some(Box::new(parser_ast::SelectStmt {
                Distinct: options.Distinct,
                SelectStmtOpts: options,
                Fields: fields,
                TableHints: table_hints,
                ..Default::default()
            }));
        }
        QueryRule::SelectStmtFromDualTableAlt01 => {
            let Some(mut statement) = rhs[rhs_len - (2)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::SelectStmt>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            statement.Where = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                .cloned();
            out.item = Some(Box::new(statement));
        }
        QueryRule::SelectStmtFromTableAlt01 => {
            let Some(mut statement) = rhs[rhs_len - (6)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::SelectStmt>().ok())
                .map(|statement| *statement)
            else {
                return Ok(false);
            };
            statement.From = rhs[rhs_len - (4)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableRefsClause>())
                .cloned();
            statement.Where = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                .cloned();
            let group_by = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<GroupBySemantic>())
                .cloned()
                .unwrap_or_default();
            statement.GroupBy = group_by.items;
            statement.GroupByRollup = group_by.rollup;
            statement.Having = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                .cloned();
            statement.WindowSpecs = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::WindowSpec>>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(statement));
        }
        QueryRule::SelectStmtAlt01 => {
            let Some(mut statement) = rhs[rhs_len - (6)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::SelectStmt>().ok())
                .map(|statement| *statement)
            else {
                return Ok(false);
            };
            if let Some(where_expr) = rhs[rhs_len - (5)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
            {
                statement.Where = Some(where_expr.clone());
            }
            let group_by = rhs[rhs_len - (4)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<GroupBySemantic>())
                .cloned()
                .unwrap_or_default();
            statement.GroupBy = group_by.items;
            statement.GroupByRollup = group_by.rollup;
            statement.OrderBy = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                .cloned()
                .unwrap_or_default();
            statement.Limit = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Limit>())
                .cloned();
            statement.lock_info = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SelectLockInfo>())
                .map(|lock| parser_ast::SelectLockInfo {
                    lock_type: lock.lock_type,
                    LockType: lock.LockType,
                    WaitSec: lock.WaitSec,
                    Tables: lock.Tables.clone(),
                });
            statement.SelectIntoOpt = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SelectIntoOption>())
                .cloned();
            out.statement = Some(Box::new(statement));
        }
        QueryRule::SelectStmtAlt02 => {
            let Some(mut statement) = rhs[rhs_len - (5)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::SelectStmt>().ok())
                .map(|statement| *statement)
            else {
                return Ok(false);
            };
            let group_by = rhs[rhs_len - (4)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<GroupBySemantic>())
                .cloned()
                .unwrap_or_default();
            statement.GroupBy = group_by.items;
            statement.GroupByRollup = group_by.rollup;
            statement.OrderBy = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                .cloned()
                .unwrap_or_default();
            statement.Limit = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Limit>())
                .cloned();
            statement.lock_info = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SelectLockInfo>())
                .map(|lock| parser_ast::SelectLockInfo {
                    lock_type: lock.lock_type,
                    LockType: lock.LockType,
                    WaitSec: lock.WaitSec,
                    Tables: lock.Tables.clone(),
                });
            statement.SelectIntoOpt = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SelectIntoOption>())
                .cloned();
            out.statement = Some(Box::new(statement));
        }
        QueryRule::SelectStmtAlt03 => {
            let Some(mut statement) = rhs[rhs_len - (4)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::SelectStmt>().ok())
                .map(|statement| *statement)
            else {
                return Ok(false);
            };
            statement.OrderBy = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                .cloned()
                .unwrap_or_default();
            statement.Limit = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Limit>())
                .cloned();
            statement.lock_info = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SelectLockInfo>())
                .map(|lock| parser_ast::SelectLockInfo {
                    lock_type: lock.lock_type,
                    LockType: lock.LockType,
                    WaitSec: lock.WaitSec,
                    Tables: lock.Tables.clone(),
                });
            statement.SelectIntoOpt = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SelectIntoOption>())
                .cloned();
            out.statement = Some(Box::new(statement));
        }
        QueryRule::SelectStmtAlt04 | QueryRule::SelectStmtAlt05 => {
            let mut statement = parser_ast::SelectStmt {
                Kind: if rule == QueryRule::SelectStmtAlt04 {
                    parser_ast::SelectStmtKind::Table
                } else {
                    parser_ast::SelectStmtKind::Values
                },
                Fields: parser_ast::FieldList {
                    Fields: vec![parser_ast::SelectField {
                        WildCard: Some(parser_ast::WildCardField::default()),
                        ..Default::default()
                    }],
                },
                ..Default::default()
            };
            if rule == QueryRule::SelectStmtAlt04 {
                let Some(table) = rhs[rhs_len - (4)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned()
                else {
                    return Ok(false);
                };
                statement.From = Some(parser_ast::TableRefsClause {
                    TableRefs: parser_ast::Join {
                        Left: Some(Box::new(parser_ast::ResultSetNode::TableSource(
                            parser_ast::TableSource {
                                Source: table,
                                ..Default::default()
                            },
                        ))),
                        ..Default::default()
                    },
                });
            } else {
                statement.Lists = rhs[rhs_len - (4)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::RowExpr>>())
                    .cloned()
                    .unwrap_or_default();
            }
            statement.OrderBy = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                .cloned()
                .unwrap_or_default();
            statement.Limit = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Limit>())
                .cloned();
            statement.lock_info = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SelectLockInfo>())
                .cloned();
            statement.SelectIntoOpt = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SelectIntoOption>())
                .cloned();
            out.statement = Some(Box::new(statement));
        }
        QueryRule::RowValueAlt01 => out.item = rhs[rhs_len - (1)].item.take(),
        QueryRule::ValuesOptAlt01 => out.item = Some(Box::new(Vec::<parser_ast::ExprNode>::new())),
        QueryRule::ValuesAlt01 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<parser_ast::ExprNode>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)].expr.take() {
                values.push(value);
            }
            out.item = Some(Box::new(values));
        }
        QueryRule::ValuesAlt02 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .expr
                    .clone()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        QueryRule::ExprOrDefaultAlt02 => {
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::DefaultValue,
                OriginTextPosition: 0,
                Flag: Default::default(),
            })
        }
        QueryRule::OrderByAlt01 => {
            let items = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(items));
        }
        QueryRule::ByListAlt01 => {
            let items = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ByItem>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(items));
        }
        QueryRule::ByListAlt02 => {
            let mut items = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                .cloned()
                .unwrap_or_default();
            if let Some(item) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ByItem>())
            {
                items.push(item.clone());
            }
            out.item = Some(Box::new(items));
        }
        QueryRule::ByItemAlt01 | QueryRule::ByItemAlt02 => {
            let expr_back = if rule == QueryRule::ByItemAlt01 { 0 } else { 1 };
            let Some(expr) = rhs[rhs_len - (expr_back)].expr.clone() else {
                return Ok(false);
            };
            // Go rewrites signed integer value expressions to PositionExpr.
            // Retain the integer representation while exposing its reference flag.
            if matches!(&expr.Kind, parser_ast::ExprKind::Value(value)
                if matches!(value.Datum, parser_ast::ValueDatum::Int64(_)))
            {
                expr.SetFlag(parser_ast::flag::FLAG_HAS_REFERENCE);
            }
            let desc = rule == QueryRule::ByItemAlt02
                && rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false);
            out.item = Some(Box::new(parser_ast::ByItem {
                Expr: expr,
                Desc: desc,
            }));
        }
        QueryRule::OrderAlt01 | QueryRule::OptOrderAlt01 | QueryRule::OptOrderAlt02 => {
            out.item = Some(Box::new(false))
        }
        QueryRule::OrderAlt02 | QueryRule::OptOrderAlt03 => out.item = Some(Box::new(true)),
        QueryRule::OrderByOptionalAlt01 => out.item = None,
        QueryRule::FieldAlt01 => {
            out.item = Some(Box::new(parser_ast::SelectField {
                WildCard: Some(parser_ast::WildCardField::default()),
                ..Default::default()
            }));
        }
        QueryRule::FieldAlt02 => {
            out.item = Some(Box::new(parser_ast::SelectField {
                WildCard: Some(parser_ast::WildCardField {
                    Table: parser_ast::NewCIStr(&rhs[rhs_len - (2)].ident),
                    ..Default::default()
                }),
                ..Default::default()
            }));
        }
        QueryRule::FieldAlt03 => {
            out.item = Some(Box::new(parser_ast::SelectField {
                WildCard: Some(parser_ast::WildCardField {
                    Schema: parser_ast::NewCIStr(&rhs[rhs_len - (4)].ident),
                    Table: parser_ast::NewCIStr(&rhs[rhs_len - (2)].ident),
                }),
                ..Default::default()
            }));
        }
        QueryRule::FieldAlt04 => {
            out.item = Some(Box::new(parser_ast::SelectField {
                Expr: rhs[rhs_len - (1)].expr.take(),
                AsName: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
                ..Default::default()
            }));
        }
        QueryRule::FieldAsNameOptAlt01 => out.ident.clear(),
        QueryRule::FieldAsNameAlt02 | QueryRule::FieldAsNameAlt04 => {
            out.ident = rhs[rhs_len - (0)].ident.clone()
        }
        QueryRule::FieldListAlt01 => {
            let mut field = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::SelectField>().ok())
                .map(|field| *field)
                .unwrap_or_default();
            field.Offset = rhs[rhs_len - (0)].offset.max(0) as usize;
            if field.Expr.is_some() {
                let end = parser_state.yylval.offset.max(field.Offset as i32) as usize;
                field.OriginalText = parser_state
                    .src
                    .get(field.Offset..end)
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
            }
            out.item = Some(Box::new(vec![field]));
        }
        QueryRule::FieldListAlt02 => {
            let mut fields = rhs[rhs_len - (2)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<parser_ast::SelectField>>().ok())
                .map(|fields| *fields)
                .unwrap_or_default();
            if let Some(mut field) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::SelectField>().ok())
                .map(|field| *field)
            {
                field.Offset = rhs[rhs_len - (0)].offset.max(0) as usize;
                if field.Expr.is_some() {
                    let end = parser_state.yylval.offset.max(field.Offset as i32) as usize;
                    field.OriginalText = parser_state
                        .src
                        .get(field.Offset..end)
                        .unwrap_or_default()
                        .trim()
                        .to_owned();
                }
                fields.push(field);
            }
            out.item = Some(Box::new(fields));
        }
        QueryRule::WithRollupClauseAlt01 => out.item = Some(Box::new(false)),
        QueryRule::WithRollupClauseAlt02 => out.item = Some(Box::new(true)),
        QueryRule::GroupByClauseAlt01 => {
            let items = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                .cloned()
                .unwrap_or_default();
            let rollup = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            out.item = Some(Box::new(GroupBySemantic { items, rollup }));
        }
        QueryRule::HavingClauseAlt01 | QueryRule::AsOfClauseOptAlt01 => out.item = None,
        QueryRule::HavingClauseAlt02 => {
            out.item = rhs[rhs_len - (0)]
                .expr
                .clone()
                .map(|expr| Box::new(expr) as Box<dyn Any>);
        }
        QueryRule::SelectStmtOptAlt01 => {
            out.item = Some(Box::new(parser_ast::SelectStmtOpts {
                SQLCache: true,
                TableHints: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOptimizerHint>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        QueryRule::SelectStmtOptsAlt01 => {
            out.item = Some(Box::new(parser_ast::SelectStmtOpts {
                SQLCache: true,
                ..Default::default()
            }))
        }
        QueryRule::SelectStmtOptAlt02 => {
            let distinct = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            out.item = Some(Box::new(parser_ast::SelectStmtOpts {
                Distinct: distinct,
                ExplicitAll: !distinct,
                SQLCache: true,
                ..Default::default()
            }));
        }
        QueryRule::SelectStmtOptAlt03 => {
            out.item = Some(Box::new(parser_ast::SelectStmtOpts {
                SQLCache: true,
                Priority: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<i32>())
                    .copied()
                    .unwrap_or_default(),
                ..Default::default()
            }))
        }
        QueryRule::SelectStmtOptAlt04
        | QueryRule::SelectStmtOptAlt05
        | QueryRule::SelectStmtOptAlt06
        | QueryRule::SelectStmtOptAlt07
        | QueryRule::SelectStmtOptAlt08
        | QueryRule::SelectStmtOptAlt09 => {
            let mut options = parser_ast::SelectStmtOpts {
                SQLCache: true,
                ..Default::default()
            };
            match rule {
                QueryRule::SelectStmtOptAlt04 => options.SQLSmallResult = true,
                QueryRule::SelectStmtOptAlt05 => options.SQLBigResult = true,
                QueryRule::SelectStmtOptAlt06 => options.SQLBufferResult = true,
                QueryRule::SelectStmtOptAlt07 => {
                    options.SQLCache = rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false)
                }
                QueryRule::SelectStmtOptAlt08 => options.CalcFoundRows = true,
                _ => options.StraightJoin = true,
            }
            out.item = Some(Box::new(options));
        }
        QueryRule::SelectStmtOptsListAlt01 => {
            let mut options = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SelectStmtOpts>())
                .cloned()
                .unwrap_or_default();
            if let Some(next) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::SelectStmtOpts>())
            {
                options.Distinct |= next.Distinct;
                options.ExplicitAll |= next.ExplicitAll;
                options.SQLCache &= next.SQLCache;
                if options.TableHints.is_empty() {
                    options.TableHints = next.TableHints.clone();
                }
                if next.Priority != 0 {
                    options.Priority = next.Priority;
                }
                options.SQLSmallResult |= next.SQLSmallResult;
                options.SQLBigResult |= next.SQLBigResult;
                options.SQLBufferResult |= next.SQLBufferResult;
                options.CalcFoundRows |= next.CalcFoundRows;
                options.StraightJoin |= next.StraightJoin;
            }
            if options.Distinct && options.ExplicitAll {
                yylex.AppendError(
                    ErrWrongUsage.GenWithStackByArgs(&["ALL".into(), "DISTINCT".into()]),
                );
                return Err(1);
            }
            out.item = Some(Box::new(options));
        }
        QueryRule::TableOptimizerHintsAlt01 => {
            let (hints, warnings) = parser_state.parseHint(
                &rhs[rhs_len - (0)].ident,
                mysql::SQLMode(yylex.sql_mode_bits()),
                yylex.hint_position(),
            );
            for warning in warnings {
                yylex.AppendError(warning);
                yylex.LastErrorAsWarn();
            }
            out.item = Some(Box::new(
                hints.into_iter().map(|hint| *hint).collect::<Vec<_>>(),
            ));
        }
        QueryRule::SelectStmtSQLCacheAlt01 => out.item = Some(Box::new(true)),
        QueryRule::SelectStmtSQLCacheAlt02 => out.item = Some(Box::new(false)),
        QueryRule::SelectStmtFieldListAlt01 => {
            let fields = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<parser_ast::SelectField>>().ok())
                .map(|fields| *fields)
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::FieldList { Fields: fields }));
        }
        QueryRule::TableNameAlt01 | QueryRule::TableNameOptWildAlt01 => {
            let back = if rule == QueryRule::TableNameOptWildAlt01 {
                1
            } else {
                0
            };
            out.item = Some(Box::new(parser_ast::TableName {
                Name: parser_ast::NewCIStr(&rhs[rhs_len - (back)].ident),
                ..Default::default()
            }));
        }
        QueryRule::TableNameAlt02 | QueryRule::TableNameOptWildAlt02 => {
            let (schema_back, name_back) = if rule == QueryRule::TableNameOptWildAlt02 {
                (3, 1)
            } else {
                (2, 0)
            };
            let schema = rhs[rhs_len - (schema_back)].ident.clone();
            if isInCorrectIdentifierName(&schema) {
                yylex.AppendError(ErrWrongDBName.GenWithStackByArgs(&[schema.into()]));
                return Err(1);
            }
            out.item = Some(Box::new(parser_ast::TableName {
                Schema: parser_ast::NewCIStr(&schema),
                Name: parser_ast::NewCIStr(&rhs[rhs_len - (name_back)].ident),
                ..Default::default()
            }));
        }
        QueryRule::TableNameAlt03 => {
            out.item = Some(Box::new(parser_ast::TableName {
                Schema: parser_ast::NewCIStr("*"),
                Name: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
                ..Default::default()
            }));
        }
        QueryRule::TableNameListAlt01 | QueryRule::TableAliasRefListAlt01 => {
            let table = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(vec![table]));
        }
        QueryRule::TableNameListAlt02 | QueryRule::TableAliasRefListAlt02 => {
            let mut tables = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableName>>())
                .cloned()
                .unwrap_or_default();
            if let Some(table) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
            {
                tables.push(table.clone());
            }
            out.item = Some(Box::new(tables));
        }
        QueryRule::TableRefsClauseAlt01 => {
            let join = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Join>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::TableRefsClause { TableRefs: join }));
        }
        QueryRule::TableRefsAlt01 => {
            if let Some(join) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Join>())
            {
                out.item = Some(Box::new(join.clone()));
            } else if let Some(source) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableSource>())
            {
                out.item = Some(Box::new(parser_ast::Join {
                    Left: Some(Box::new(parser_ast::ResultSetNode::TableSource(
                        source.clone(),
                    ))),
                    ..Default::default()
                }));
            } else {
                return Ok(false);
            }
        }
        QueryRule::TableRefsAlt02 => {
            let Some(left) = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(result_set_from_item)
            else {
                return Ok(false);
            };
            let Some(right) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(result_set_from_item)
            else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::Join {
                Left: Some(Box::new(left)),
                Right: Some(Box::new(right)),
                Tp: parser_ast::JoinType::CrossJoin,
                ..Default::default()
            }));
        }
        QueryRule::EscapedTableRefAlt02 => {
            out.item = rhs[rhs_len - (1)].item.take();
        }
        QueryRule::TableFactorAlt01 => {
            let Some(mut table) = rhs[rhs_len - (5)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            table.PartitionNames = rhs[rhs_len - (4)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            table.IndexHints = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::IndexHint>>())
                .cloned()
                .unwrap_or_default();
            let alias = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::CIStr>())
                .cloned()
                .unwrap_or_default();
            let table_sample = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableSample>())
                .cloned();
            let as_of = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::AsOfClause>())
                .cloned();
            out.item = Some(Box::new(parser_ast::TableSource {
                Source: table,
                AsName: alias,
                TableSample: table_sample,
                AsOf: as_of,
                ..Default::default()
            }));
        }
        QueryRule::TableFactorAlt02 | QueryRule::TableFactorAlt03 => {
            let subquery_back = if rule == QueryRule::TableFactorAlt02 {
                1
            } else {
                2
            };
            let Some(subquery) = rhs[rhs_len - (subquery_back)]
                .item
                .take()
                .and_then(|item| item.downcast::<SubquerySemantic>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::TableSource {
                QuerySource: Some(subquery.query),
                AsName: rhs[rhs_len
                    - (if rule == QueryRule::TableFactorAlt02 {
                        0
                    } else {
                        1
                    })]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::CIStr>())
                .cloned()
                .unwrap_or_default(),
                Lateral: rule == QueryRule::TableFactorAlt03,
                ColumnNames: if rule == QueryRule::TableFactorAlt03 {
                    rhs[rhs_len - (0)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                        .cloned()
                        .unwrap_or_default()
                } else {
                    Vec::new()
                },
                ..Default::default()
            }));
        }
        QueryRule::TableFactorAlt04 => {
            let Some(mut join) = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Join>())
                .cloned()
            else {
                return Ok(false);
            };
            join.ExplicitParens = true;
            out.item = Some(Box::new(join));
        }
        QueryRule::PartitionNameListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::CIStr>::new()))
        }
        QueryRule::PartitionNameListOptAlt02 => {
            out.item = rhs[rhs_len - (1)].item.take();
        }
        QueryRule::TableAsNameOptAlt01 | QueryRule::TableAsNameOptDeleteAlt01 => {
            out.item = Some(Box::new(parser_ast::CIStr::default()))
        }
        QueryRule::TableAsNameAlt01 | QueryRule::TableAsNameAlt02 => {
            out.item = Some(Box::new(parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident)));
        }
        QueryRule::IndexHintTypeAlt01 => out.item = Some(Box::new(parser_ast::IndexHintType::Use)),
        QueryRule::IndexHintTypeAlt02 => {
            out.item = Some(Box::new(parser_ast::IndexHintType::Ignore))
        }
        QueryRule::IndexHintTypeAlt03 => {
            out.item = Some(Box::new(parser_ast::IndexHintType::Force))
        }
        QueryRule::IndexHintScopeAlt01 => {
            out.item = Some(Box::new(parser_ast::IndexHintScope::Scan))
        }
        QueryRule::IndexHintScopeAlt02 => {
            out.item = Some(Box::new(parser_ast::IndexHintScope::Join))
        }
        QueryRule::IndexHintScopeAlt03 => {
            out.item = Some(Box::new(parser_ast::IndexHintScope::OrderBy))
        }
        QueryRule::IndexHintScopeAlt04 => {
            out.item = Some(Box::new(parser_ast::IndexHintScope::GroupBy))
        }
        QueryRule::IndexHintAlt01 => {
            let names = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            let hint_type = rhs[rhs_len - (4)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::IndexHintType>())
                .copied()
                .unwrap_or_default();
            let hint_scope = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::IndexHintScope>())
                .copied()
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::IndexHint {
                IndexNames: names,
                HintType: hint_type,
                HintScope: hint_scope,
            }));
        }
        QueryRule::IndexNameListAlt01 => out.item = Some(Box::new(Vec::<parser_ast::CIStr>::new())),
        QueryRule::IndexNameListAlt02 | QueryRule::IndexNameListAlt04 => {
            out.item = Some(Box::new(vec![parser_ast::NewCIStr(
                &rhs[rhs_len - (0)].ident,
            )]));
        }
        QueryRule::IndexNameListAlt03 | QueryRule::IndexNameListAlt05 => {
            let mut names = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            names.push(parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident));
            out.item = Some(Box::new(names));
        }
        QueryRule::IndexHintListAlt01 => {
            let hints = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::IndexHint>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(hints));
        }
        QueryRule::IndexHintListAlt02 => {
            let mut hints = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::IndexHint>>())
                .cloned()
                .unwrap_or_default();
            if let Some(hint) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::IndexHint>())
            {
                hints.push(hint.clone());
            }
            out.item = Some(Box::new(hints));
        }
        QueryRule::IndexHintListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::IndexHint>::new()))
        }
        QueryRule::JoinTableAlt01
        | QueryRule::JoinTableAlt02
        | QueryRule::JoinTableAlt03
        | QueryRule::JoinTableAlt04
        | QueryRule::JoinTableAlt05
        | QueryRule::JoinTableAlt06
        | QueryRule::JoinTableAlt07
        | QueryRule::JoinTableAlt08
        | QueryRule::JoinTableAlt09
        | QueryRule::JoinTableAlt10 => {
            let (left_back, right_back, join_type_back, on_back, using_back, natural, straight) =
                match rule {
                    QueryRule::JoinTableAlt01 => (2, 0, None, None, None, false, false),
                    QueryRule::JoinTableAlt02 => (4, 2, None, Some(0), None, false, false),
                    QueryRule::JoinTableAlt03 => (6, 4, None, None, Some(1), false, false),
                    QueryRule::JoinTableAlt04 => (6, 2, Some(5), Some(0), None, false, false),
                    QueryRule::JoinTableAlt05 => (8, 4, Some(7), None, Some(1), false, false),
                    QueryRule::JoinTableAlt06 => (3, 0, None, None, None, true, false),
                    QueryRule::JoinTableAlt07 => (5, 0, Some(3), None, None, true, false),
                    QueryRule::JoinTableAlt08 => (2, 0, None, None, None, false, true),
                    QueryRule::JoinTableAlt09 => (4, 2, None, Some(0), None, false, true),
                    _ => (6, 4, None, None, Some(1), false, true),
                };
            let Some(left) = rhs[rhs_len - (left_back)]
                .item
                .as_deref()
                .and_then(result_set_from_item)
            else {
                return Ok(false);
            };
            let Some(right) = rhs[rhs_len - (right_back)]
                .item
                .as_deref()
                .and_then(result_set_from_item)
            else {
                return Ok(false);
            };
            let join_type = join_type_back
                .and_then(|back| rhs[rhs_len - (back)].item.as_deref())
                .and_then(|item| item.downcast_ref::<parser_ast::JoinType>())
                .copied()
                .unwrap_or_default();
            let on = on_back.and_then(|back| rhs[rhs_len - (back)].expr.clone());
            let using = using_back
                .and_then(|back| rhs[rhs_len - (back)].item.as_deref())
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnName>>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::Join {
                Left: Some(Box::new(left)),
                Right: Some(Box::new(right)),
                Tp: join_type,
                On: on,
                Using: using,
                NaturalJoin: natural,
                StraightJoin: straight,
                ..Default::default()
            }));
        }
        QueryRule::JoinTypeAlt01 => out.item = Some(Box::new(parser_ast::JoinType::LeftJoin)),
        QueryRule::JoinTypeAlt02 => out.item = Some(Box::new(parser_ast::JoinType::RightJoin)),
        QueryRule::JoinTypeAlt03 => out.item = Some(Box::new(parser_ast::JoinType::FullJoin)),
        QueryRule::LimitClauseAlt01 | QueryRule::SelectStmtLimitOptAlt01 => out.item = None,
        QueryRule::LimitClauseAlt02 | QueryRule::SelectStmtLimitAlt01 => {
            let count = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                .cloned();
            out.item = Some(Box::new(parser_ast::Limit {
                Count: count,
                Offset: None,
            }));
        }
        QueryRule::LimitOptionAlt01 => {
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .map(semantic_value_text)
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::ExprNode::Value(value)));
        }
        QueryRule::LimitOptionAlt02 => {
            out.item = Some(Box::new(parser_ast::ExprNode::ParamMarker(
                rhs[rhs_len - (0)].offset.max(0) as usize,
            )))
        }
        QueryRule::FetchFirstOptAlt01 => {
            out.item = Some(Box::new(parser_ast::ExprNode::Value("1".to_owned())))
        }
        QueryRule::SelectStmtLimitAlt02 | QueryRule::SelectStmtLimitAlt03 => {
            let (offset_back, count_back) = if rule == QueryRule::SelectStmtLimitAlt02 {
                (2, 0)
            } else {
                (0, 2)
            };
            let offset = rhs[rhs_len - (offset_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                .cloned();
            let count = rhs[rhs_len - (count_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                .cloned();
            out.item = Some(Box::new(parser_ast::Limit {
                Count: count,
                Offset: offset,
            }));
        }
        QueryRule::SelectStmtLimitAlt04 => {
            let count = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                .cloned();
            out.item = Some(Box::new(parser_ast::Limit {
                Count: count,
                Offset: None,
            }));
        }
        QueryRule::WhereClauseAlt01 => {
            out.item = rhs[rhs_len - (0)]
                .expr
                .clone()
                .map(|expr| Box::new(expr) as Box<dyn Any>);
        }
        QueryRule::WhereClauseOptionalAlt01 => out.item = None,
        QueryRule::ValuesStmtListAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::RowExpr>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        QueryRule::ValuesStmtListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::RowExpr>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::RowExpr>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        QueryRule::RowStmtAlt01 => {
            out.item = Some(Box::new(parser_ast::RowExpr {
                Values: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                    .cloned()
                    .unwrap_or_default(),
            }))
        }
    }
    Ok(true)
}
