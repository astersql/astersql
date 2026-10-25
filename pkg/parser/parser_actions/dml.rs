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
enum DmlRule {
    AssignmentAlt01,
    AssignmentListAlt01,
    AssignmentListAlt02,
    CancelImportStmtAlt01,
    CharsetOptAlt01,
    CharsetOptAlt02,
    ColumnNameOrUserVariableAlt01,
    ColumnNameOrUserVariableAlt02,
    ColumnNameOrUserVariableListAlt01,
    ColumnNameOrUserVariableListAlt02,
    ColumnNameOrUserVarListOptAlt01,
    ColumnNameOrUserVarListOptWithBracketsAlt01,
    ColumnNameOrUserVarListOptWithBracketsAlt02,
    ColumnSetValueListAlt01,
    ColumnSetValueListAlt02,
    DeleteFromStmtAlt01,
    DeleteFromStmtAlt02,
    DeleteWithoutUsingStmtAlt01,
    DeleteWithoutUsingStmtAlt02,
    DeleteWithUsingStmtAlt01,
    DryRunOptionsAlt01,
    DryRunOptionsAlt02,
    DryRunOptionsAlt03,
    FieldItemAlt01,
    FieldItemAlt02,
    FieldItemAlt03,
    FieldItemAlt04,
    FieldItemAlt05,
    FieldItemAlt06,
    FieldItemListAlt01,
    FieldItemListAlt02,
    FieldsAlt01,
    FieldsAlt02,
    FieldTerminatorAlt01,
    FieldTerminatorAlt02,
    FormatOptAlt01,
    FormatOptAlt02,
    IgnoreLinesAlt01,
    IgnoreLinesAlt02,
    ImportFromSelectStmtAlt01,
    ImportFromSelectStmtAlt02,
    ImportFromSelectStmtAlt03,
    ImportFromSelectStmtAlt04,
    ImportIntoStmtAlt01,
    ImportIntoStmtAlt02,
    ConflictTargetPredicateOptAlt01,
    ConflictTargetPredicateOptAlt02,
    ConflictUpdateOptAlt01,
    ConflictUpdateOptAlt02,
    ConflictUpdatePredicateOptAlt01,
    ConflictUpdatePredicateOptAlt02,
    InsertIntoStmtAlt01,
    InsertOnClauseAlt01,
    InsertOnClauseAlt02,
    InsertOnClauseAlt03,
    InsertOnClauseAlt04,
    InsertOnClauseAlt05,
    InsertOnClauseAlt06,
    InsertOnClauseAlt07,
    InsertOnClauseAlt08,
    InsertRowAliasOptAlt01,
    InsertRowAliasOptAlt02,
    InsertRowAliasOptAlt03,
    InsertValuesAlt01,
    InsertValuesAlt02,
    InsertValuesAlt03,
    InsertValuesAlt04,
    InsertValuesAlt05,
    InsertValuesAlt06,
    InsertValuesAlt07,
    InsertValuesAlt08,
    InsertValuesAlt09,
    InsertValuesAlt10,
    InsertValuesAlt11,
    LinesAlt01,
    LinesAlt02,
    LinesTerminatedAlt01,
    LinesTerminatedAlt02,
    LoadDataOptionAlt01,
    LoadDataOptionAlt02,
    LoadDataOptionListAlt01,
    LoadDataOptionListAlt02,
    LoadDataOptionListOptAlt01,
    LoadDataOptionListOptAlt02,
    LoadDataSetItemAlt01,
    LoadDataSetListAlt01,
    LoadDataSetListAlt02,
    LoadDataSetSpecOptAlt01,
    LoadDataSetSpecOptAlt02,
    LoadDataStmtAlt01,
    LocalOptAlt01,
    LocalOptAlt02,
    LowPriorityOptAlt01,
    LowPriorityOptAlt02,
    NonTransactionalDmlStmtAlt01,
    OptionalShardColumnAlt01,
    OptionalShardColumnAlt02,
    QuickOptionalAlt01,
    QuickOptionalAlt02,
    ReplaceIntoStmtAlt01,
    ReturningClauseAlt01,
    ReturningClauseAlt02,
    StartingAlt01,
    StartingAlt02,
    UpdateStmtAlt01,
    UpdateStmtNoWithAlt01,
    UpdateStmtNoWithAlt02,
    UpdateStmtNoWithAlt03,
}

fn identify(rule_id: RuleId) -> Option<DmlRule> {
    Some(match rule_id.as_str() {
        "assignment_columnname_eqorassignmenteq_exprordef--abbd2bcf34d982b3" => {
            DmlRule::AssignmentAlt01
        }
        "assignmentlist_assignment--0a166b97bfe3c3b4" => DmlRule::AssignmentListAlt01,
        "assignmentlist_assignmentlist_assignment--aff66ca5b9afd7a1" => {
            DmlRule::AssignmentListAlt02
        }
        "cancelimportstmt_cancel_import_job_int64num--e13be4927c2b8490" => {
            DmlRule::CancelImportStmtAlt01
        }
        "charsetopt--bc76e3c2ad59d5a8" => DmlRule::CharsetOptAlt01,
        "charsetopt_character_set_charsetname--48b9dee462caa409" => DmlRule::CharsetOptAlt02,
        "columnnameoruservariable_columnname--b517a6ada04f2106" => {
            DmlRule::ColumnNameOrUserVariableAlt01
        }
        "columnnameoruservariable_uservariable--1ef8ac565a18d31c" => {
            DmlRule::ColumnNameOrUserVariableAlt02
        }
        "columnnameoruservariablelist_columnnameoruservar--56776b76ae23c5f4" => {
            DmlRule::ColumnNameOrUserVariableListAlt01
        }
        "columnnameoruservariablelist_columnnameoruservar--a42bd42cb51251df" => {
            DmlRule::ColumnNameOrUserVariableListAlt02
        }
        "columnnameoruservarlistopt--bab0a7359330ec88" => DmlRule::ColumnNameOrUserVarListOptAlt01,
        "columnnameoruservarlistoptwithbrackets--920b174c83513a45" => {
            DmlRule::ColumnNameOrUserVarListOptWithBracketsAlt01
        }
        "columnnameoruservarlistoptwithbrackets_columnnam--e72083604d819224" => {
            DmlRule::ColumnNameOrUserVarListOptWithBracketsAlt02
        }
        "columnsetvaluelist_columnname_eqorassignmenteq_e--81c33173993d205d" => {
            DmlRule::ColumnSetValueListAlt01
        }
        "columnsetvaluelist_columnsetvaluelist_columnname--3096b9c933658914" => {
            DmlRule::ColumnSetValueListAlt02
        }
        "deletefromstmt_withclause_deletewithoutusingstmt--d5efe8c2c6fd51b1" => {
            DmlRule::DeleteFromStmtAlt01
        }
        "deletefromstmt_withclause_deletewithusingstmt--cc9dfa96b925ab1b" => {
            DmlRule::DeleteFromStmtAlt02
        }
        "deletewithoutusingstmt_delete_tableoptimizerhint--c7ec4a539c53774a" => {
            DmlRule::DeleteWithoutUsingStmtAlt01
        }
        "deletewithoutusingstmt_delete_tableoptimizerhint--faae84a6dc3987c3" => {
            DmlRule::DeleteWithoutUsingStmtAlt02
        }
        "deletewithusingstmt_delete_tableoptimizerhintsop--94fda1000b3570a7" => {
            DmlRule::DeleteWithUsingStmtAlt01
        }
        "dryrunoptions--8950917b07763fb3" => DmlRule::DryRunOptionsAlt01,
        "dryrunoptions_dry_run--5869f3f812d81142" => DmlRule::DryRunOptionsAlt02,
        "dryrunoptions_dry_run_query--c4354d9ee37a9e5e" => DmlRule::DryRunOptionsAlt03,
        "fielditem_terminated_by_fieldterminator--d621bdcbef78bbb0" => DmlRule::FieldItemAlt01,
        "fielditem_optionallyenclosedby_fieldterminator--d2719f1467242445" => {
            DmlRule::FieldItemAlt02
        }
        "fielditem_enclosed_by_fieldterminator--216c183a80c90bba" => DmlRule::FieldItemAlt03,
        "fielditem_escaped_by_fieldterminator--39eeefb788ff94ba" => DmlRule::FieldItemAlt04,
        "fielditem_defined_null_by_textstring--8868e2ddac15acac" => DmlRule::FieldItemAlt05,
        "fielditem_defined_null_by_textstring_optionally--54ebeb431c8e832a" => {
            DmlRule::FieldItemAlt06
        }
        "fielditemlist_fielditemlist_fielditem--859945fcf40c6397" => DmlRule::FieldItemListAlt01,
        "fielditemlist_fielditem--2eb384198c23dcf4" => DmlRule::FieldItemListAlt02,
        "fields--c2f6be788fe65204" => DmlRule::FieldsAlt01,
        "fields_fieldsorcolumns_fielditemlist--55e04a20d32b56db" => DmlRule::FieldsAlt02,
        "fieldterminator_hexlit--39d24a9899193739" => DmlRule::FieldTerminatorAlt01,
        "fieldterminator_bitlit--40862a424ac27d67" => DmlRule::FieldTerminatorAlt02,
        "formatopt--6f6c3c30c5445663" => DmlRule::FormatOptAlt01,
        "formatopt_format_stringlit--0a890759997297b5" => DmlRule::FormatOptAlt02,
        "ignorelines--6bcd96d655631f16" => DmlRule::IgnoreLinesAlt01,
        "ignorelines_ignore_num_lines--5c60af60d042bb14" => DmlRule::IgnoreLinesAlt02,
        "importfromselectstmt_selectstmt--80d7dc191faacb89" => DmlRule::ImportFromSelectStmtAlt01,
        "importfromselectstmt_setoprstmt--59027c3832bd14a6" => DmlRule::ImportFromSelectStmtAlt02,
        "importfromselectstmt_selectstmtwithclause--1dbb53d4a71506d0" => {
            DmlRule::ImportFromSelectStmtAlt03
        }
        "importfromselectstmt_subselect--61f011315d2f7393" => DmlRule::ImportFromSelectStmtAlt04,
        "importintostmt_import_into_tablename_columnnameo--0db22da7c91872b5" => {
            DmlRule::ImportIntoStmtAlt01
        }
        "importintostmt_import_into_tablename_columnnameo--0d7a8d564098e762" => {
            DmlRule::ImportIntoStmtAlt02
        }
        "insertintostmt_insert_tableoptimizerhintsopt_pri--52aa8fe840ae2f74" => {
            DmlRule::InsertIntoStmtAlt01
        }
        "insertonclause_prec_empty--205eeb9aca732bea" => DmlRule::InsertOnClauseAlt01,
        "insertonclause_on_duplicate_key_update_assignmen--596d558711761c90" => {
            DmlRule::InsertOnClauseAlt02
        }
        "insertonclause_on_identifier_do_identifier--6263e521f96106b7" => {
            DmlRule::InsertOnClauseAlt03
        }
        "insertonclause_on_identifier_columnnamelist_conf--adf28edd3ec5e132" => {
            DmlRule::InsertOnClauseAlt04
        }
        "insertonclause_on_identifier_on_constraint_ident--e3e958280f69341a" => {
            DmlRule::InsertOnClauseAlt05
        }
        "insertonclause_on_identifier_do_update_conflictu--767abe990ec7f123" => {
            DmlRule::InsertOnClauseAlt06
        }
        "insertonclause_on_identifier_columnnamelist_conf--8fe8a61cf3400dbe" => {
            DmlRule::InsertOnClauseAlt07
        }
        "insertonclause_on_identifier_on_constraint_ident--048945f483e05836" => {
            DmlRule::InsertOnClauseAlt08
        }
        "insertrowaliasopt_prec_empty--ac3acd5bb9ed41e3" => DmlRule::InsertRowAliasOptAlt01,
        "insertrowaliasopt_as_identifier--8d529d6bb0d121cf" => DmlRule::InsertRowAliasOptAlt02,
        "insertrowaliasopt_as_identifier_identlist--6da6e30ff07a2b44" => {
            DmlRule::InsertRowAliasOptAlt03
        }
        "insertvalues_columnnamelistopt_valuesym_valuesli--a3fcf34718d062c2" => {
            DmlRule::InsertValuesAlt01
        }
        "insertvalues_columnnamelistopt_setoprstmt--69ab66940f1a7587" => DmlRule::InsertValuesAlt02,
        "insertvalues_columnnamelistopt_selectstmt--d5cf516618a84500" => DmlRule::InsertValuesAlt03,
        "insertvalues_columnnamelistopt_selectstmtwithcla--fce34cb2efa9a6b5" => {
            DmlRule::InsertValuesAlt04
        }
        "insertvalues_columnnamelistopt_subselect--88769be1ce0379c4" => DmlRule::InsertValuesAlt05,
        "insertvalues_valuesym_valueslist_insertrowaliaso--6eee0904ce4cd07f" => {
            DmlRule::InsertValuesAlt06
        }
        "insertvalues_setoprstmt--9dc7d64069d85e1a" => DmlRule::InsertValuesAlt07,
        "insertvalues_selectstmt--f75d64a4d69313f5" => DmlRule::InsertValuesAlt08,
        "insertvalues_selectstmtwithclause--cc3562ff04e04434" => DmlRule::InsertValuesAlt09,
        "insertvalues_subselect--bb980e8cc60951c7" => DmlRule::InsertValuesAlt10,
        "insertvalues_set_columnsetvaluelist_insertrowali--a29f4481d712f22e" => {
            DmlRule::InsertValuesAlt11
        }
        "lines--50f6e6ca85dd9644" => DmlRule::LinesAlt01,
        "lines_lines_starting_linesterminated--0fea5ea0591df020" => DmlRule::LinesAlt02,
        "linesterminated--47bd9a484e85a0f5" => DmlRule::LinesTerminatedAlt01,
        "linesterminated_terminated_by_fieldterminator--078f4b5f7df0d2a7" => {
            DmlRule::LinesTerminatedAlt02
        }
        "loaddataoption_identifier--650fcd02d8625da2" => DmlRule::LoadDataOptionAlt01,
        "loaddataoption_identifier_signedliteral--5da5f417014e0360" => DmlRule::LoadDataOptionAlt02,
        "loaddataoptionlist_loaddataoption--9ab72a79a075812c" => DmlRule::LoadDataOptionListAlt01,
        "loaddataoptionlist_loaddataoptionlist_loaddataop--23690621e7a608ad" => {
            DmlRule::LoadDataOptionListAlt02
        }
        "loaddataoptionlistopt--bf1c804718fa7449" => DmlRule::LoadDataOptionListOptAlt01,
        "loaddataoptionlistopt_with_loaddataoptionlist--872880def3e7e565" => {
            DmlRule::LoadDataOptionListOptAlt02
        }
        "loaddatasetitem_simpleident_exprordefault--05b51e440f38c705" => {
            DmlRule::LoadDataSetItemAlt01
        }
        "loaddatasetlist_loaddatasetlist_loaddatasetitem--be1fe52bff5eaac9" => {
            DmlRule::LoadDataSetListAlt01
        }
        "loaddatasetlist_loaddatasetitem--6c1c52e688ab443d" => DmlRule::LoadDataSetListAlt02,
        "loaddatasetspecopt--b6c562936102e081" => DmlRule::LoadDataSetSpecOptAlt01,
        "loaddatasetspecopt_set_loaddatasetlist--dae5059bf07c9d24" => {
            DmlRule::LoadDataSetSpecOptAlt02
        }
        "loaddatastmt_load_data_lowpriorityopt_localopt_i--2d68b143704ff639" => {
            DmlRule::LoadDataStmtAlt01
        }
        "localopt--9e9b360058ecbc71" => DmlRule::LocalOptAlt01,
        "localopt_local--3b74e63c1e517d91" => DmlRule::LocalOptAlt02,
        "lowpriorityopt--70e69bc6a2246f62" => DmlRule::LowPriorityOptAlt01,
        "lowpriorityopt_low_priority--8f1a90ae7e616ef6" => DmlRule::LowPriorityOptAlt02,
        "nontransactionaldmlstmt_batch_optionalshardcolum--20064cecd92e889e" => {
            DmlRule::NonTransactionalDmlStmtAlt01
        }
        "conflicttargetpredicateopt--e6d6a85b39cf6444" => {
            DmlRule::ConflictTargetPredicateOptAlt01
        }
        "conflicttargetpredicateopt_where_expression--933b8c07629f80d0" => {
            DmlRule::ConflictTargetPredicateOptAlt02
        }
        "conflictupdateopt--8b74dcaf583725e7" => DmlRule::ConflictUpdateOptAlt01,
        "conflictupdateopt_set_assignmentlist_conflictupd--f7d0684637a6035c" => {
            DmlRule::ConflictUpdateOptAlt02
        }
        "conflictupdatepredicateopt--98184a00535462f0" => {
            DmlRule::ConflictUpdatePredicateOptAlt01
        }
        "conflictupdatepredicateopt_where_expression--31266766ac1a10c4" => {
            DmlRule::ConflictUpdatePredicateOptAlt02
        }
        "optionalshardcolumn--92eca09a88777dcd" => DmlRule::OptionalShardColumnAlt01,
        "optionalshardcolumn_on_columnname--29b125a9b217c2e4" => DmlRule::OptionalShardColumnAlt02,
        "quickoptional_prec_empty--350d7f78c30b7916" => DmlRule::QuickOptionalAlt01,
        "quickoptional_quick--631e0b2cc0e80f86" => DmlRule::QuickOptionalAlt02,
        "replaceintostmt_replace_tableoptimizerhintsopt_p--376770e2df399622" => {
            DmlRule::ReplaceIntoStmtAlt01
        }
        "returningclause_prec_empty--2219f44067652212" => DmlRule::ReturningClauseAlt01,
        "returningclause_returning_fieldlist--c1444df4e6338fc9" => DmlRule::ReturningClauseAlt02,
        "starting--a45620a3c225ca99" => DmlRule::StartingAlt01,
        "starting_starting_by_fieldterminator--84bbddf92173127e" => DmlRule::StartingAlt02,
        "updatestmt_withclause_updatestmtnowith--5f221bdc6a6e73aa" => DmlRule::UpdateStmtAlt01,
        "updatestmtnowith_update_tableoptimizerhintsopt_p--98b9a28f1fc122fe" => {
            DmlRule::UpdateStmtNoWithAlt01
        }
        "updatestmtnowith_update_tableoptimizerhintsopt_p--43fd3758bb95c07c" => {
            DmlRule::UpdateStmtNoWithAlt02
        }
        "updatestmtnowith_update_tableoptimizerhintsopt_p--69a176d5eab67bc7" => {
            DmlRule::UpdateStmtNoWithAlt03
        }
        _ => return None,
    })
}

fn dml_numeric_isize(value: &dyn Any) -> Option<isize> {
    value
        .downcast_ref::<i32>()
        .map(|value| *value as isize)
        .or_else(|| semantic_numeric_isize(value))
}

pub(super) fn owns(rule_id: RuleId) -> bool {
    identify(rule_id).is_some()
}

pub(super) fn apply(
    rule_id: RuleId,
    rhs: Rhs<'_>,
    context: Context<'_>,
) -> Option<Result<bool, isize>> {
    Some(apply_rule(identify(rule_id)?, rhs, context))
}

fn apply_rule(rule: DmlRule, rhs: Rhs<'_>, context: Context<'_>) -> Result<bool, isize> {
    if rule == DmlRule::UpdateStmtNoWithAlt03 {
        apply_update_from_rule(rhs, context)
    } else if matches!(
        rule,
        DmlRule::InsertIntoStmtAlt01
            | DmlRule::InsertRowAliasOptAlt01
            | DmlRule::InsertRowAliasOptAlt02
            | DmlRule::InsertRowAliasOptAlt03
            | DmlRule::InsertValuesAlt01
            | DmlRule::InsertValuesAlt02
            | DmlRule::InsertValuesAlt03
            | DmlRule::InsertValuesAlt04
            | DmlRule::InsertValuesAlt05
            | DmlRule::InsertValuesAlt06
            | DmlRule::InsertValuesAlt07
            | DmlRule::InsertValuesAlt08
            | DmlRule::InsertValuesAlt09
            | DmlRule::InsertValuesAlt10
            | DmlRule::InsertValuesAlt11
            | DmlRule::ColumnSetValueListAlt01
            | DmlRule::ColumnSetValueListAlt02
            | DmlRule::InsertOnClauseAlt01
            | DmlRule::InsertOnClauseAlt02
            | DmlRule::InsertOnClauseAlt03
            | DmlRule::InsertOnClauseAlt04
            | DmlRule::InsertOnClauseAlt05
            | DmlRule::InsertOnClauseAlt06
            | DmlRule::InsertOnClauseAlt07
            | DmlRule::InsertOnClauseAlt08
            | DmlRule::ConflictTargetPredicateOptAlt01
            | DmlRule::ConflictTargetPredicateOptAlt02
            | DmlRule::ConflictUpdateOptAlt01
            | DmlRule::ConflictUpdateOptAlt02
            | DmlRule::ConflictUpdatePredicateOptAlt01
            | DmlRule::ConflictUpdatePredicateOptAlt02
            | DmlRule::ReplaceIntoStmtAlt01
            | DmlRule::ReturningClauseAlt01
            | DmlRule::ReturningClauseAlt02
    ) {
        apply_insert_rule(rule, rhs, context)
    } else if matches!(
        rule,
        DmlRule::CharsetOptAlt01
            | DmlRule::CharsetOptAlt02
            | DmlRule::FieldItemAlt01
            | DmlRule::FieldItemAlt02
            | DmlRule::FieldItemAlt03
            | DmlRule::FieldItemAlt04
            | DmlRule::FieldItemAlt05
            | DmlRule::FieldItemAlt06
            | DmlRule::FieldItemListAlt01
            | DmlRule::FieldItemListAlt02
            | DmlRule::FieldsAlt01
            | DmlRule::FieldsAlt02
            | DmlRule::FieldTerminatorAlt01
            | DmlRule::FieldTerminatorAlt02
            | DmlRule::FormatOptAlt01
            | DmlRule::FormatOptAlt02
            | DmlRule::IgnoreLinesAlt01
            | DmlRule::IgnoreLinesAlt02
            | DmlRule::ImportFromSelectStmtAlt01
            | DmlRule::ImportFromSelectStmtAlt02
            | DmlRule::ImportFromSelectStmtAlt03
            | DmlRule::ImportFromSelectStmtAlt04
            | DmlRule::ImportIntoStmtAlt01
            | DmlRule::ImportIntoStmtAlt02
            | DmlRule::LinesAlt01
            | DmlRule::LinesAlt02
            | DmlRule::LinesTerminatedAlt01
            | DmlRule::LinesTerminatedAlt02
            | DmlRule::LoadDataOptionAlt01
            | DmlRule::LoadDataOptionAlt02
            | DmlRule::LoadDataOptionListAlt01
            | DmlRule::LoadDataOptionListAlt02
            | DmlRule::LoadDataOptionListOptAlt01
            | DmlRule::LoadDataOptionListOptAlt02
            | DmlRule::LoadDataSetItemAlt01
            | DmlRule::LoadDataSetListAlt01
            | DmlRule::LoadDataSetListAlt02
            | DmlRule::LoadDataSetSpecOptAlt01
            | DmlRule::LoadDataSetSpecOptAlt02
            | DmlRule::LoadDataStmtAlt01
            | DmlRule::LocalOptAlt01
            | DmlRule::LocalOptAlt02
            | DmlRule::LowPriorityOptAlt01
            | DmlRule::LowPriorityOptAlt02
            | DmlRule::StartingAlt01
            | DmlRule::StartingAlt02
    ) {
        apply_load_rule(rule, rhs, context)
    } else {
        apply_general_rule(rule, rhs, context)
    }
}

fn apply_update_from_rule(mut rhs: Rhs<'_>, context: Context<'_>) -> Result<bool, isize> {
    let rhs_len = rhs.len();
    let Context {
        output: out,
        parser_state,
        lexer: _,
    } = context;
    if parser_state.sql_dialect() != parser_ast::SqlDialect::PostgreSQL {
        return Err(1);
    }
    let Some(target) = rhs[rhs_len - 6].item.as_deref().and_then(|item| {
        if let Some(join) = item.downcast_ref::<parser_ast::Join>() {
            Some(join.clone())
        } else {
            item.downcast_ref::<parser_ast::TableSource>()
                .map(|source| parser_ast::Join {
                    Left: Some(Box::new(parser_ast::ResultSetNode::TableSource(
                        source.clone(),
                    ))),
                    ..Default::default()
                })
        }
    }) else {
        return Ok(false);
    };
    let Some(from) = rhs[rhs_len - 2]
        .item
        .as_deref()
        .and_then(|item| item.downcast_ref::<parser_ast::Join>())
        .cloned()
    else {
        return Ok(false);
    };
    out.statement = Some(Box::new(parser_ast::UpdateStmt {
        Priority: rhs[rhs_len - 8]
            .item
            .as_deref()
            .and_then(dml_numeric_isize)
            .unwrap_or_default() as i32,
        TableRefs: Some(parser_ast::TableRefsClause { TableRefs: target }),
        From: Some(parser_ast::TableRefsClause { TableRefs: from }),
        List: rhs[rhs_len - 4]
            .item
            .as_deref()
            .and_then(|item| item.downcast_ref::<Vec<parser_ast::Assignment>>())
            .cloned()
            .unwrap_or_default(),
        IgnoreErr: rhs[rhs_len - 7]
            .item
            .as_deref()
            .and_then(|item| item.downcast_ref::<bool>())
            .copied()
            .unwrap_or(false),
        Where: rhs[rhs_len - 1]
            .item
            .as_deref()
            .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
            .cloned(),
        TableHints: rhs[rhs_len - 9]
            .item
            .as_deref()
            .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOptimizerHint>>())
            .cloned()
            .unwrap_or_default(),
        Returning: rhs[rhs_len]
            .item
            .as_deref()
            .and_then(|item| item.downcast_ref::<Vec<parser_ast::SelectField>>())
            .cloned()
            .unwrap_or_default(),
        ..Default::default()
    }));
    Ok(true)
}

fn apply_general_rule(
    rule: DmlRule,
    mut rhs: Rhs<'_>,
    context: Context<'_>,
) -> Result<bool, isize> {
    let rhs_len = rhs.len();
    let Context {
        output: out,
        parser_state,
        lexer: _,
    } = context;
    match rule {
        DmlRule::AssignmentAlt01 => {
            let column = rhs[rhs_len - 2]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                .cloned()
                .unwrap_or_default();
            let Some(expr) = rhs[rhs_len].expr.clone() else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::Assignment {
                Column: column,
                Expr: expr,
            }));
        }
        DmlRule::AssignmentListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::Assignment>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ));
        }
        DmlRule::AssignmentListAlt02 => {
            let mut values = rhs[rhs_len - 2]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::Assignment>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Assignment>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DmlRule::ColumnNameOrUserVarListOptAlt01
        | DmlRule::ColumnNameOrUserVarListOptWithBracketsAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::ColumnNameOrUserVar>::new()))
        }
        DmlRule::ColumnNameOrUserVariableListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ColumnNameOrUserVar>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ));
        }
        DmlRule::ColumnNameOrUserVariableListAlt02 => {
            let mut values = rhs[rhs_len - 2]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnNameOrUserVar>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnNameOrUserVar>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DmlRule::ColumnNameOrUserVariableAlt01 => {
            out.item = Some(Box::new(parser_ast::ColumnNameOrUserVar {
                ColumnName: rhs[rhs_len]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                    .cloned(),
                ..Default::default()
            }));
        }
        DmlRule::ColumnNameOrUserVariableAlt02 => {
            out.item = Some(Box::new(parser_ast::ColumnNameOrUserVar {
                UserVar: rhs[rhs_len].expr.clone(),
                ..Default::default()
            }));
        }
        DmlRule::ColumnNameOrUserVarListOptWithBracketsAlt02 => {
            out.item = rhs[rhs_len - 1].item.take()
        }
        DmlRule::CancelImportStmtAlt01 => {
            let job_id = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<i64>())
                .copied()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::ImportIntoActionStmt {
                node_text: Default::default(),
                Tp: parser_ast::ImportIntoActionTp::Cancel,
                JobID: job_id,
            }));
        }
        DmlRule::DeleteWithoutUsingStmtAlt02 | DmlRule::DeleteWithUsingStmtAlt01 => {
            let using_clause = rule == DmlRule::DeleteWithUsingStmtAlt01;
            let (
                priority_back,
                quick_back,
                ignore_back,
                tables_back,
                join_back,
                hints_back,
                before_from,
            ) = if !using_clause {
                (6, 5, 4, 3, 1, 7, true)
            } else {
                (8, 7, 6, 4, 2, 9, false)
            };
            let Some(join) = rhs[rhs_len - join_back]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Join>())
                .cloned()
            else {
                return Ok(false);
            };
            let targets = rhs[rhs_len - tables_back]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableSource>>())
                .cloned()
                .unwrap_or_default();
            let returning = if using_clause {
                rhs[rhs_len]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::SelectField>>())
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if using_clause
                && parser_state.sql_dialect() != parser_ast::SqlDialect::PostgreSQL
                && !returning.is_empty()
            {
                return Err(1);
            }
            let is_postgresql_using =
                using_clause && parser_state.sql_dialect() == parser_ast::SqlDialect::PostgreSQL;
            if is_postgresql_using && targets.len() != 1 {
                return Err(1);
            }
            let target_refs = if is_postgresql_using {
                parser_ast::TableRefsClause {
                    TableRefs: parser_ast::Join {
                        Left: Some(Box::new(parser_ast::ResultSetNode::TableSource(
                            targets[0].clone(),
                        ))),
                        ..Default::default()
                    },
                }
            } else {
                parser_ast::TableRefsClause { TableRefs: join.clone() }
            };
            out.statement = Some(Box::new(parser_ast::DeleteStmt {
                Priority: rhs[rhs_len - priority_back]
                    .item
                    .as_deref()
                    .and_then(dml_numeric_isize)
                    .unwrap_or_default() as i32,
                Quick: rhs[rhs_len - quick_back]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                IgnoreErr: rhs[rhs_len - ignore_back]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                IsMultiTable: using_clause && !is_postgresql_using,
                BeforeFrom: before_from,
                Tables: if is_postgresql_using {
                    Vec::new()
                } else {
                    targets
                        .iter()
                        .map(|target| target.Source.clone())
                        .collect()
                },
                TableRefs: Some(target_refs),
                Using: is_postgresql_using
                    .then_some(parser_ast::TableRefsClause { TableRefs: join }),
                UsingOffset: is_postgresql_using
                    .then_some(rhs[rhs_len - 3].offset.max(0) as usize),
                TableHints: rhs[rhs_len - hints_back]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOptimizerHint>>())
                    .cloned()
                    .unwrap_or_default(),
                Where: rhs[rhs_len - usize::from(using_clause)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                    .cloned(),
                Returning: returning,
                ..Default::default()
            }));
        }
        DmlRule::DeleteFromStmtAlt01 | DmlRule::DeleteFromStmtAlt02 => {
            let Some(mut statement) = rhs[rhs_len]
                .statement
                .take()
                .and_then(|node| node.into_any().downcast::<parser_ast::DeleteStmt>().ok())
                .map(|node| *node)
            else {
                return Ok(false);
            };
            statement.With = rhs[rhs_len - 1]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::WithClause>().ok())
                .map(|item| item.into_shared());
            out.statement = Some(Box::new(statement));
        }
        DmlRule::DeleteWithoutUsingStmtAlt01 => {
            let Some(mut table) = rhs[rhs_len - 7]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            table.PartitionNames = rhs[rhs_len - 6]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            table.IndexHints = rhs[rhs_len - 4]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::IndexHint>>())
                .cloned()
                .unwrap_or_default();
            let alias = rhs[rhs_len - 5]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::CIStr>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::DeleteStmt {
                TableRefs: Some(parser_ast::TableRefsClause {
                    TableRefs: parser_ast::Join {
                        Left: Some(Box::new(parser_ast::ResultSetNode::TableSource(
                            parser_ast::TableSource {
                                Source: table,
                                AsName: alias,
                                ..Default::default()
                            },
                        ))),
                        ..Default::default()
                    },
                }),
                Priority: rhs[rhs_len - 11]
                    .item
                    .as_deref()
                    .and_then(dml_numeric_isize)
                    .unwrap_or_default() as i32,
                Quick: rhs[rhs_len - 10]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                IgnoreErr: rhs[rhs_len - 9]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                TableHints: rhs[rhs_len - 12]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOptimizerHint>>())
                    .cloned()
                    .unwrap_or_default(),
                Where: rhs[rhs_len - 3]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                    .cloned(),
                Order: rhs[rhs_len - 2]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                    .cloned()
                    .unwrap_or_default(),
                Limit: rhs[rhs_len - 1]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::Limit>())
                    .cloned(),
                Returning: rhs[rhs_len]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::SelectField>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }));
        }
        DmlRule::QuickOptionalAlt01 => out.item = Some(Box::new(false)),
        DmlRule::QuickOptionalAlt02 => out.item = Some(Box::new(true)),
        DmlRule::NonTransactionalDmlStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::NonTransactionalDMLStmt {
                node_text: Default::default(),
                DryRun: rhs[rhs_len - 1]
                    .item
                    .as_deref()
                    .and_then(dml_numeric_isize)
                    .unwrap_or_default() as i32,
                ShardColumn: rhs[rhs_len - 4]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                    .cloned()
                    .unwrap_or_default(),
                Limit: rhs[rhs_len - 2]
                    .item
                    .as_deref()
                    .map(getUint64FromNUM)
                    .unwrap_or_default(),
                DMLStmt: rhs[rhs_len].statement.take(),
            }));
        }
        DmlRule::DryRunOptionsAlt01 | DmlRule::DryRunOptionsAlt02 | DmlRule::DryRunOptionsAlt03 => {
            out.item = Some(Box::new(match rule {
                DmlRule::DryRunOptionsAlt01 => 0i32,
                DmlRule::DryRunOptionsAlt02 => 1i32,
                _ => 2i32,
            }));
        }
        DmlRule::OptionalShardColumnAlt01 => out.item = None,
        DmlRule::OptionalShardColumnAlt02 => out.item = rhs[rhs_len].item.take(),
        DmlRule::UpdateStmtAlt01 => {
            let Some(mut statement) = rhs[rhs_len]
                .statement
                .take()
                .and_then(|node| node.into_any().downcast::<parser_ast::UpdateStmt>().ok())
                .map(|node| *node)
            else {
                return Ok(false);
            };
            statement.With = rhs[rhs_len - 1]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::WithClause>().ok())
                .map(|item| item.into_shared());
            out.statement = Some(Box::new(statement));
        }
        DmlRule::UpdateStmtNoWithAlt01 => {
            let Some(join) = rhs[rhs_len - 6].item.as_deref().and_then(|item| {
                if let Some(join) = item.downcast_ref::<parser_ast::Join>() {
                    Some(join.clone())
                } else {
                    item.downcast_ref::<parser_ast::TableSource>()
                        .map(|source| parser_ast::Join {
                            Left: Some(Box::new(parser_ast::ResultSetNode::TableSource(
                                source.clone(),
                            ))),
                            ..Default::default()
                        })
                }
            }) else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::UpdateStmt {
                Priority: rhs[rhs_len - 8]
                    .item
                    .as_deref()
                    .and_then(dml_numeric_isize)
                    .unwrap_or_default() as i32,
                TableRefs: Some(parser_ast::TableRefsClause { TableRefs: join }),
                List: rhs[rhs_len - 4]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::Assignment>>())
                    .cloned()
                    .unwrap_or_default(),
                IgnoreErr: rhs[rhs_len - 7]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Where: rhs[rhs_len - 3]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                    .cloned(),
                Order: rhs[rhs_len - 2]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                    .cloned()
                    .unwrap_or_default(),
                Limit: rhs[rhs_len - 1]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::Limit>())
                    .cloned(),
                TableHints: rhs[rhs_len - 9]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOptimizerHint>>())
                    .cloned()
                    .unwrap_or_default(),
                Returning: rhs[rhs_len]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::SelectField>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }));
        }
        DmlRule::UpdateStmtNoWithAlt02 => {
            let Some(join) = rhs[rhs_len - 4]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Join>())
                .cloned()
            else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::UpdateStmt {
                Priority: rhs[rhs_len - 6]
                    .item
                    .as_deref()
                    .and_then(dml_numeric_isize)
                    .unwrap_or_default() as i32,
                TableRefs: Some(parser_ast::TableRefsClause { TableRefs: join }),
                List: rhs[rhs_len - 2]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::Assignment>>())
                    .cloned()
                    .unwrap_or_default(),
                IgnoreErr: rhs[rhs_len - 5]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                Where: rhs[rhs_len - 1]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                    .cloned(),
                TableHints: rhs[rhs_len - 7]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOptimizerHint>>())
                    .cloned()
                    .unwrap_or_default(),
                Returning: rhs[rhs_len]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::SelectField>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }));
        }
        _ => unreachable!("non-general DML rule routed to general actions"),
    }
    Ok(true)
}

fn conflict_keyword_is_unquoted(
    parser_state: &Parser,
    symbol: &yySymType,
    expected: &str,
) -> bool {
    super::identifier(parser_state, symbol).1.is_some_and(|identifier| {
        !identifier.Quoted && identifier.Decoded.eq_ignore_ascii_case(expected)
    })
}

#[derive(Clone)]
struct ConflictPredicate {
    expression: parser_ast::ExprNode,
    span: (usize, usize),
}

#[derive(Clone, Default)]
struct ConflictUpdateClause {
    assignments: Vec<parser_ast::Assignment>,
    predicate: Option<ConflictPredicate>,
}

fn make_postgresql_conflict_clause(
    parser_state: &Parser,
    rhs: &Rhs<'_>,
    target: parser_ast::PostgreSQLConflictTarget,
    columns: Vec<parser_ast::ColumnName>,
    constraint: Option<parser_ast::PostgreSQLIdentifier>,
    predicate: Option<ConflictPredicate>,
    action: parser_ast::PostgreSQLConflictAction,
    update: Option<ConflictUpdateClause>,
) -> Result<parser_ast::PostgreSQLConflictClause, isize> {
    let start = usize::try_from(rhs[1].offset).map_err(|_| 1isize)?;
    let end = rhs[rhs.len()].end_offset;
    let original = parser_state.src.get(start..end).ok_or(1isize)?.to_owned();
    let target_predicate_span = predicate.as_ref().map(|predicate| predicate.span);
    let predicate = predicate.map(|predicate| predicate.expression);
    let (update_assignments, update_predicate, update_predicate_span) = update
        .map(|update| {
            let span = update.predicate.as_ref().map(|predicate| predicate.span);
            let predicate = update.predicate.map(|predicate| predicate.expression);
            (update.assignments, predicate, span)
        })
        .unwrap_or_default();
    Ok(parser_ast::PostgreSQLConflictClause {
        Target: target,
        Columns: columns,
        Constraint: constraint,
        Predicate: predicate,
        TargetPredicateSpan: target_predicate_span,
        Action: action,
        UpdateAssignments: update_assignments,
        UpdatePredicate: update_predicate,
        UpdatePredicateSpan: update_predicate_span,
        SourceSpan: (start, end),
        OriginalText: original,
    })
}

#[derive(Clone)]
enum InsertOnClauseAction {
    OnDuplicate(Vec<parser_ast::Assignment>),
    PostgreSQLConflict(parser_ast::PostgreSQLConflictClause),
}

fn apply_insert_rule(rule: DmlRule, mut rhs: Rhs<'_>, context: Context<'_>) -> Result<bool, isize> {
    let rhs_len = rhs.len();
    let Context {
        output: out,
        parser_state,
        lexer: yylex,
    } = context;
    match rule {
        DmlRule::InsertIntoStmtAlt01 => {
            let Some(mut statement) = rhs[rhs_len - 2]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::InsertStmt>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let Some(table) = rhs[rhs_len - 4]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            statement.Table = Some(parser_ast::TableRefsClause {
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
            statement.IgnoreErr = rhs[rhs_len - 6]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            statement.Priority = rhs[rhs_len - 7]
                .item
                .as_deref()
                .and_then(dml_numeric_isize)
                .unwrap_or_default() as i32;
            statement.TableHints = rhs[rhs_len - 8]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOptimizerHint>>())
                .cloned()
                .unwrap_or_default();
            statement.PartitionNames = rhs[rhs_len - 3]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            let on_clause = rhs[rhs_len - 1]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Option<InsertOnClauseAction>>())
                .cloned()
                .unwrap_or_default();
            match on_clause {
                Some(InsertOnClauseAction::OnDuplicate(assignments)) => {
                    statement.OnDuplicate = assignments;
                }
                Some(InsertOnClauseAction::PostgreSQLConflict(conflict)) => {
                    statement.PostgreSQLConflict = Some(conflict);
                }
                None => {}
            }
            statement.Returning = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::SelectField>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(statement));
        }
        DmlRule::InsertValuesAlt01 => {
            let columns = rhs[rhs_len - 4]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<parser_ast::ColumnName>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            let lists = rhs[rhs_len - 1]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<Vec<parser_ast::ExprNode>>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            let mut statement = parser_ast::InsertStmt {
                Columns: columns,
                Lists: lists,
                ..Default::default()
            };
            if let Some(alias) = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<insertRowAlias>())
            {
                statement.RowAlias = alias.rowAlias.clone();
                statement.ColumnAliases = alias.columnAliases.clone();
            }
            out.item = Some(Box::new(statement));
        }
        DmlRule::InsertValuesAlt02
        | DmlRule::InsertValuesAlt03
        | DmlRule::InsertValuesAlt04
        | DmlRule::InsertValuesAlt05 => {
            let columns = rhs[rhs_len - 2]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnName>>())
                .cloned()
                .unwrap_or_default();
            let select = if rule == DmlRule::InsertValuesAlt05 {
                rhs[rhs_len]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<SubquerySemantic>().ok())
                    .and_then(|mut semantic| semantic.query.take())
            } else {
                rhs[rhs_len].statement.take()
            };
            let Some(select) = select else {
                return Ok(false);
            };
            let select = if rule == DmlRule::InsertValuesAlt05 {
                mark_select_in_braces(select)?
            } else {
                select
            };
            out.item = Some(Box::new(parser_ast::InsertStmt {
                Columns: columns,
                Select: Some(select),
                ..Default::default()
            }));
        }
        DmlRule::InsertValuesAlt07
        | DmlRule::InsertValuesAlt08
        | DmlRule::InsertValuesAlt09
        | DmlRule::InsertValuesAlt10 => {
            let select = if rule == DmlRule::InsertValuesAlt10 {
                rhs[rhs_len]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<SubquerySemantic>().ok())
                    .and_then(|mut semantic| semantic.query.take())
            } else {
                rhs[rhs_len].statement.take()
            };
            let Some(select) = select else {
                return Ok(false);
            };
            let select = if rule == DmlRule::InsertValuesAlt10 {
                mark_select_in_braces(select)?
            } else {
                select
            };
            out.item = Some(Box::new(parser_ast::InsertStmt {
                Select: Some(select),
                ..Default::default()
            }));
        }
        DmlRule::InsertValuesAlt06 => {
            let lists = rhs[rhs_len - 1]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<Vec<parser_ast::ExprNode>>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            let mut statement = parser_ast::InsertStmt {
                Lists: lists,
                ..Default::default()
            };
            if let Some(alias) = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<insertRowAlias>())
            {
                statement.RowAlias = alias.rowAlias.clone();
                statement.ColumnAliases = alias.columnAliases.clone();
            }
            out.item = Some(Box::new(statement));
        }
        DmlRule::InsertValuesAlt11 => {
            let Some(mut statement) = rhs[rhs_len - 1]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::InsertStmt>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            if let Some(alias) = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<insertRowAlias>())
            {
                statement.RowAlias = alias.rowAlias.clone();
                statement.ColumnAliases = alias.columnAliases.clone();
            }
            out.item = Some(Box::new(statement));
        }
        DmlRule::ColumnSetValueListAlt01 => {
            let column = rhs[rhs_len - 2]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                .cloned()
                .unwrap_or_default();
            let values = rhs[rhs_len].expr.clone().into_iter().collect::<Vec<_>>();
            out.item = Some(Box::new(parser_ast::InsertStmt {
                Columns: vec![column],
                Lists: vec![values],
                Setlist: true,
                ..Default::default()
            }));
        }
        DmlRule::ColumnSetValueListAlt02 => {
            let Some(mut statement) = rhs[rhs_len - 4]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::InsertStmt>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            if let Some(column) = rhs[rhs_len - 2]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
            {
                statement.Columns.push(column.clone());
            }
            if let Some(value) = rhs[rhs_len].expr.clone() {
                if statement.Lists.is_empty() {
                    statement.Lists.push(Vec::new());
                }
                statement.Lists[0].push(value);
            }
            out.item = Some(Box::new(statement));
        }
        DmlRule::InsertRowAliasOptAlt01
        | DmlRule::ReturningClauseAlt01 => out.item = None,
        DmlRule::InsertOnClauseAlt01 => {
            out.item = Some(Box::new(None::<InsertOnClauseAction>));
        }
        DmlRule::InsertOnClauseAlt02 => {
            let assignments = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::Assignment>>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(Some(InsertOnClauseAction::OnDuplicate(
                assignments,
            ))));
        }
        DmlRule::ConflictTargetPredicateOptAlt01 => {
            out.expr = None;
            out.item = Some(Box::new(None::<ConflictPredicate>));
        }
        DmlRule::ConflictTargetPredicateOptAlt02
        | DmlRule::ConflictUpdatePredicateOptAlt02 => {
            let expression = rhs[rhs_len].expr.clone().ok_or(1isize)?;
            let start = usize::try_from(rhs[1].offset).map_err(|_| 1isize)?;
            let predicate = ConflictPredicate {
                expression: expression.clone(),
                span: (start, rhs[rhs_len].end_offset),
            };
            out.expr = Some(expression);
            out.item = Some(Box::new(Some(predicate)));
        }
        DmlRule::ConflictUpdatePredicateOptAlt01 => {
            out.expr = None;
            out.item = Some(Box::new(None::<ConflictPredicate>));
        }
        DmlRule::ConflictUpdateOptAlt01 => {
            out.item = Some(Box::new(None::<ConflictUpdateClause>));
        }
        DmlRule::ConflictUpdateOptAlt02 => {
            let assignments = rhs[2]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::Assignment>>())
                .cloned()
                .unwrap_or_default();
            let predicate = rhs[3]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Option<ConflictPredicate>>())
                .cloned()
                .flatten();
            out.item = Some(Box::new(Some(ConflictUpdateClause {
                assignments,
                predicate,
            })));
        }
        DmlRule::InsertOnClauseAlt03
        | DmlRule::InsertOnClauseAlt04
        | DmlRule::InsertOnClauseAlt05
        | DmlRule::InsertOnClauseAlt06
        | DmlRule::InsertOnClauseAlt07
        | DmlRule::InsertOnClauseAlt08 => {
            let conflict_word = &rhs[2];
            let (action_word_index, update_clause_index) = match rule {
                DmlRule::InsertOnClauseAlt03 => (4, None),
                DmlRule::InsertOnClauseAlt04 => (8, None),
                DmlRule::InsertOnClauseAlt05 => (7, None),
                DmlRule::InsertOnClauseAlt06 => (4, Some(5)),
                DmlRule::InsertOnClauseAlt07 => (8, Some(9)),
                DmlRule::InsertOnClauseAlt08 => (7, Some(8)),
                _ => unreachable!(),
            };
            let action_word = &rhs[action_word_index];
            let conflict_is_unquoted =
                conflict_keyword_is_unquoted(parser_state, conflict_word, "CONFLICT");
            let update_action = matches!(
                rule,
                DmlRule::InsertOnClauseAlt06
                    | DmlRule::InsertOnClauseAlt07
                    | DmlRule::InsertOnClauseAlt08
            );
            let action = if update_action {
                Some(parser_ast::PostgreSQLConflictAction::DoUpdate)
            } else if conflict_keyword_is_unquoted(parser_state, action_word, "NOTHING") {
                Some(parser_ast::PostgreSQLConflictAction::DoNothing)
            } else {
                None
            };
            if parser_state.sql_dialect() != parser_ast::SqlDialect::PostgreSQL
                || !conflict_is_unquoted
                || action.is_none()
            {
                yylex.AppendError(yylex.Errorf(
                    "expected unquoted PostgreSQL ON CONFLICT DO NOTHING or DO UPDATE keywords",
                    &[],
                ));
                if parser_state.sql_dialect() == parser_ast::SqlDialect::PostgreSQL {
                    let offset = if conflict_is_unquoted {
                        action_word.offset
                    } else {
                        conflict_word.offset
                    };
                    if let Ok(offset) = usize::try_from(offset) {
                        yylex.set_postgresql_syntax_error_offset(offset);
                    }
                }
                return Err(1);
            }
            let action = action.expect("validated PostgreSQL conflict action");
            let update = update_clause_index
                .and_then(|index| rhs[index].item.as_deref())
                .and_then(|item| item.downcast_ref::<Option<ConflictUpdateClause>>())
                .cloned()
                .flatten();
            let (target, columns, constraint, predicate) = match rule {
                DmlRule::InsertOnClauseAlt03 | DmlRule::InsertOnClauseAlt06 => (
                    parser_ast::PostgreSQLConflictTarget::Any,
                    Vec::new(),
                    None,
                    None,
                ),
                DmlRule::InsertOnClauseAlt04 | DmlRule::InsertOnClauseAlt07 => {
                    let columns = rhs[4]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnName>>())
                        .cloned()
                        .unwrap_or_default();
                    (
                        parser_ast::PostgreSQLConflictTarget::Inference,
                        columns,
                        None,
                        rhs[6]
                            .item
                            .as_deref()
                            .and_then(|item| item.downcast_ref::<Option<ConflictPredicate>>())
                            .cloned()
                            .flatten(),
                    )
                }
                DmlRule::InsertOnClauseAlt05 | DmlRule::InsertOnClauseAlt08 => {
                    let constraint = super::identifier(parser_state, &rhs[5]).1;
                    let Some(constraint) = constraint else {
                        return Err(1);
                    };
                    (
                        parser_ast::PostgreSQLConflictTarget::Constraint,
                        Vec::new(),
                        Some(constraint),
                        None,
                    )
                }
                _ => unreachable!(),
            };
            let valid_action = match action {
                parser_ast::PostgreSQLConflictAction::DoNothing => update.is_none(),
                parser_ast::PostgreSQLConflictAction::DoUpdate => {
                    target != parser_ast::PostgreSQLConflictTarget::Any
                        && update
                            .as_ref()
                            .is_some_and(|update| !update.assignments.is_empty())
                }
            };
            if !valid_action {
                yylex.AppendError(yylex.Errorf(
                    "PostgreSQL ON CONFLICT DO UPDATE requires a conflict target and SET assignments",
                    &[],
                ));
                if let Ok(offset) = usize::try_from(action_word.offset) {
                    yylex.set_postgresql_syntax_error_offset(offset);
                }
                return Err(1);
            }
            let clause = make_postgresql_conflict_clause(
                parser_state,
                &rhs,
                target,
                columns,
                constraint,
                predicate,
                action,
                update,
            )?;
            out.item = Some(Box::new(Some(
                InsertOnClauseAction::PostgreSQLConflict(clause),
            )));
        }
        DmlRule::InsertRowAliasOptAlt02 | DmlRule::InsertRowAliasOptAlt03 => {
            out.item = Some(Box::new(insertRowAlias {
                rowAlias: parser_ast::NewCIStr(
                    &rhs[rhs_len
                        - if rule == DmlRule::InsertRowAliasOptAlt02 {
                            0
                        } else {
                            3
                        }]
                    .ident,
                ),
                columnAliases: if rule == DmlRule::InsertRowAliasOptAlt03 {
                    rhs[rhs_len - 1]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                        .cloned()
                        .unwrap_or_default()
                } else {
                    Vec::new()
                },
            }));
        }
        DmlRule::ReturningClauseAlt02 => out.item = rhs[rhs_len].item.take(),
        DmlRule::ReplaceIntoStmtAlt01 => {
            let Some(mut statement) = rhs[rhs_len]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::InsertStmt>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            if !statement.RowAlias.O.is_empty() || !statement.ColumnAliases.is_empty() {
                yylex.AppendError(ErrSyntax.GenWithStackByArgs(&[]));
                return Err(1);
            }
            let Some(table) = rhs[rhs_len - 2]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            statement.IsReplace = true;
            statement.Priority = rhs[rhs_len - 4]
                .item
                .as_deref()
                .and_then(dml_numeric_isize)
                .unwrap_or_default() as i32;
            statement.TableHints = rhs[rhs_len - 5]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::TableOptimizerHint>>())
                .cloned()
                .unwrap_or_default();
            statement.Table = Some(parser_ast::TableRefsClause {
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
            statement.PartitionNames = rhs[rhs_len - 1]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::CIStr>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(statement));
        }
        _ => unreachable!("non-insert DML rule routed to insert actions"),
    }
    Ok(true)
}

fn mark_select_in_braces(
    statement: Box<dyn parser_ast::Node>,
) -> Result<Box<dyn parser_ast::Node>, isize> {
    match statement.into_any().downcast::<parser_ast::SelectStmt>() {
        Ok(mut select) => {
            select.IsInBraces = true;
            Ok(select)
        }
        Err(statement) => match statement.downcast::<parser_ast::SetOprStmt>() {
            Ok(mut set_op) => {
                set_op.IsInBraces = true;
                Ok(set_op)
            }
            Err(_) => Err(0),
        },
    }
}

fn apply_load_rule(rule: DmlRule, mut rhs: Rhs<'_>, context: Context<'_>) -> Result<bool, isize> {
    let rhs_len = rhs.len();
    let Context {
        output: out,
        parser_state: _,
        lexer: yylex,
    } = context;
    match rule {
        DmlRule::LowPriorityOptAlt01 | DmlRule::LowPriorityOptAlt02 => {
            out.item = Some(Box::new(rule == DmlRule::LowPriorityOptAlt02))
        }
        DmlRule::FormatOptAlt01 => out.item = Some(Box::new(None::<String>)),
        DmlRule::FormatOptAlt02 => out.item = Some(Box::new(Some(rhs[rhs_len].ident.clone()))),
        DmlRule::IgnoreLinesAlt01 => out.item = Some(Box::new(None::<u64>)),
        DmlRule::IgnoreLinesAlt02 => {
            out.item = Some(Box::new(Some(
                rhs[rhs_len - 1]
                    .item
                    .as_deref()
                    .and_then(dml_numeric_isize)
                    .unwrap_or_default()
                    .max(0) as u64,
            )))
        }
        DmlRule::CharsetOptAlt01 => out.item = Some(Box::new(None::<String>)),
        DmlRule::CharsetOptAlt02 => out.item = Some(Box::new(Some(rhs[rhs_len].ident.clone()))),
        DmlRule::LocalOptAlt01 => out.item = None,
        DmlRule::LocalOptAlt02 => out.item = Some(Box::new(rhs[rhs_len].ident.clone())),
        DmlRule::FieldsAlt01 => out.item = None,
        DmlRule::FieldsAlt02 => {
            let items = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::FieldItem>>())
                .cloned()
                .unwrap_or_default();
            let mut clause = parser_ast::FieldsClause::default();
            for item in items {
                match item.Type {
                    parser_ast::FieldItemType::Terminated => clause.Terminated = Some(item.Value),
                    parser_ast::FieldItemType::Enclosed => {
                        clause.Enclosed = Some(item.Value);
                        clause.OptEnclosed = item.OptEnclosed;
                    }
                    parser_ast::FieldItemType::Escaped => clause.Escaped = Some(item.Value),
                    parser_ast::FieldItemType::DefinedNullBy => {
                        clause.DefinedNullBy = Some(item.Value);
                        clause.NullValueOptEnclosed = item.OptEnclosed;
                    }
                }
            }
            out.item = Some(Box::new(clause));
        }
        DmlRule::FieldItemListAlt01 | DmlRule::FieldItemListAlt02 => {
            let mut values = if rule == DmlRule::FieldItemListAlt01 {
                rhs[rhs_len - 1]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::FieldItem>>())
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if let Some(value) = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::FieldItem>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DmlRule::FieldItemAlt01
        | DmlRule::FieldItemAlt02
        | DmlRule::FieldItemAlt03
        | DmlRule::FieldItemAlt04
        | DmlRule::FieldItemAlt05
        | DmlRule::FieldItemAlt06 => {
            let (tp, value_back, optional) = match rule {
                DmlRule::FieldItemAlt01 => (parser_ast::FieldItemType::Terminated, 0, false),
                DmlRule::FieldItemAlt02 => (parser_ast::FieldItemType::Enclosed, 0, true),
                DmlRule::FieldItemAlt03 => (parser_ast::FieldItemType::Enclosed, 0, false),
                DmlRule::FieldItemAlt04 => (parser_ast::FieldItemType::Escaped, 0, false),
                DmlRule::FieldItemAlt05 => (parser_ast::FieldItemType::DefinedNullBy, 0, false),
                _ => (parser_ast::FieldItemType::DefinedNullBy, 2, true),
            };
            let value = if matches!(rule, DmlRule::FieldItemAlt05 | DmlRule::FieldItemAlt06) {
                rhs[rhs_len - value_back]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<TextStringSemantic>())
                    .map(|item| item.value.clone())
                    .unwrap_or_default()
            } else {
                rhs[rhs_len - value_back].ident.clone()
            };
            if matches!(
                rule,
                DmlRule::FieldItemAlt02 | DmlRule::FieldItemAlt03 | DmlRule::FieldItemAlt04
            ) && value != "\\"
                && value.len() > 1
            {
                yylex.AppendError(yylex.Errorf("Wrong field terminators", &[]));
                return Err(1);
            }
            out.item = Some(Box::new(parser_ast::FieldItem {
                Type: tp,
                Value: value,
                OptEnclosed: optional,
            }));
        }
        DmlRule::FieldTerminatorAlt01 => {
            out.ident = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_test_driver::HexLiteral>())
                .map(|value| value.ToString())
                .unwrap_or_default()
        }
        DmlRule::FieldTerminatorAlt02 => {
            out.ident = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_test_driver::BitLiteral>())
                .map(|value| value.ToString())
                .unwrap_or_default()
        }
        DmlRule::LinesAlt01 => out.item = None,
        DmlRule::LinesAlt02 => {
            out.item = Some(Box::new(parser_ast::LinesClause {
                Starting: rhs[rhs_len - 1]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Option<String>>())
                    .cloned()
                    .flatten(),
                Terminated: rhs[rhs_len]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Option<String>>())
                    .cloned()
                    .flatten(),
            }));
        }
        DmlRule::StartingAlt01 | DmlRule::LinesTerminatedAlt01 => {
            out.item = Some(Box::new(None::<String>))
        }
        DmlRule::StartingAlt02 | DmlRule::LinesTerminatedAlt02 => {
            out.item = Some(Box::new(Some(rhs[rhs_len].ident.clone())))
        }
        DmlRule::LoadDataSetSpecOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::Assignment>::new()))
        }
        DmlRule::LoadDataSetSpecOptAlt02 => out.item = rhs[rhs_len].item.take(),
        DmlRule::LoadDataSetListAlt01 | DmlRule::LoadDataSetListAlt02 => {
            let mut values = if rule == DmlRule::LoadDataSetListAlt01 {
                rhs[rhs_len - 2]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::Assignment>>())
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if let Some(value) = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::Assignment>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DmlRule::LoadDataSetItemAlt01 => {
            let Some(column_expr) = rhs[rhs_len - 2].expr.as_ref() else {
                return Ok(false);
            };
            let parser_ast::ExprKind::Column(column) = &column_expr.Kind else {
                return Ok(false);
            };
            let Some(expr) = rhs[rhs_len].expr.clone() else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::Assignment {
                Column: column.clone(),
                Expr: expr,
            }));
        }
        DmlRule::LoadDataOptionListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::LoadDataOpt>::new()))
        }
        DmlRule::LoadDataOptionListOptAlt02 => out.item = rhs[rhs_len].item.take(),
        DmlRule::LoadDataOptionListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::LoadDataOpt>())
                    .cloned()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ));
        }
        DmlRule::LoadDataOptionListAlt02 => {
            let mut values = rhs[rhs_len - 2]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::LoadDataOpt>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::LoadDataOpt>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        DmlRule::LoadDataOptionAlt01 | DmlRule::LoadDataOptionAlt02 => {
            let name_back = if rule == DmlRule::LoadDataOptionAlt01 {
                0
            } else {
                2
            };
            out.item = Some(Box::new(parser_ast::LoadDataOpt {
                Name: rhs[rhs_len - name_back].ident.to_ascii_lowercase(),
                Value: if rule == DmlRule::LoadDataOptionAlt02 {
                    rhs[rhs_len].expr.clone()
                } else {
                    None
                },
            }));
        }
        DmlRule::LoadDataStmtAlt01 => {
            let local = rhs[rhs_len - 14].item.is_some();
            let mut on_duplicate = rhs[rhs_len - 10]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::OnDuplicateKeyHandlingType>())
                .copied()
                .unwrap_or_default();
            if local && on_duplicate == parser_ast::OnDuplicateKeyHandlingType::Error {
                on_duplicate = parser_ast::OnDuplicateKeyHandlingType::Ignore;
            }
            let columns_and_vars = rhs[rhs_len - 2]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnNameOrUserVar>>())
                .cloned()
                .unwrap_or_default();
            let columns = columns_and_vars
                .iter()
                .filter_map(|value| value.ColumnName.clone())
                .collect();
            out.statement = Some(Box::new(parser_ast::LoadDataStmt {
                node_text: Default::default(),
                LowPriority: rhs[rhs_len - 15]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                FileLocRef: if local {
                    parser_ast::FileLocRef::Client
                } else {
                    parser_ast::FileLocRef::ServerOrRemote
                },
                Path: rhs[rhs_len - 12].ident.clone(),
                Format: rhs[rhs_len - 11]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Option<String>>())
                    .cloned()
                    .flatten(),
                OnDuplicate: on_duplicate,
                Table: rhs[rhs_len - 7]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned()
                    .unwrap_or_default(),
                Charset: rhs[rhs_len - 6]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Option<String>>())
                    .cloned()
                    .flatten(),
                FieldsInfo: rhs[rhs_len - 5]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::FieldsClause>())
                    .cloned(),
                LinesInfo: rhs[rhs_len - 4]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::LinesClause>())
                    .cloned(),
                IgnoreLines: rhs[rhs_len - 3]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Option<u64>>())
                    .copied()
                    .flatten(),
                ColumnsAndUserVars: columns_and_vars,
                Columns: columns,
                ColumnAssignments: rhs[rhs_len - 1]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::Assignment>>())
                    .cloned()
                    .unwrap_or_default(),
                Options: rhs[rhs_len]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::LoadDataOpt>>())
                    .cloned()
                    .unwrap_or_default(),
            }));
        }
        DmlRule::ImportIntoStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::ImportIntoStmt {
                Table: rhs[rhs_len - 6]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned()
                    .unwrap_or_default(),
                ColumnsAndUserVars: rhs[rhs_len - 5]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnNameOrUserVar>>())
                    .cloned()
                    .unwrap_or_default(),
                ColumnAssignments: rhs[rhs_len - 4]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::Assignment>>())
                    .cloned()
                    .unwrap_or_default(),
                Path: rhs[rhs_len - 2].ident.clone(),
                Format: rhs[rhs_len - 1]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Option<String>>())
                    .cloned()
                    .flatten(),
                Options: rhs[rhs_len]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::LoadDataOpt>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }));
        }
        DmlRule::ImportIntoStmtAlt02 => {
            let columns = rhs[rhs_len - 4]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnNameOrUserVar>>())
                .cloned()
                .unwrap_or_default();
            if columns.iter().any(|value| value.ColumnName.is_none()) {
                yylex.AppendError(yylex.Errorf(
                    "Cannot use user variable in IMPORT INTO FROM SELECT statement.",
                    &[],
                ));
                return Err(1);
            }
            if rhs[rhs_len - 3]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::Assignment>>())
                .is_some_and(|assignments| !assignments.is_empty())
            {
                yylex.AppendError(yylex.Errorf(
                    "Cannot use SET clause in IMPORT INTO FROM SELECT statement.",
                    &[],
                ));
                return Err(1);
            }
            out.statement = Some(Box::new(parser_ast::ImportIntoStmt {
                Table: rhs[rhs_len - 5]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned()
                    .unwrap_or_default(),
                ColumnsAndUserVars: columns,
                Select: rhs[rhs_len - 1].statement.take(),
                Options: rhs[rhs_len]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::LoadDataOpt>>())
                    .cloned()
                    .unwrap_or_default(),
                ..Default::default()
            }));
        }
        DmlRule::ImportFromSelectStmtAlt01
        | DmlRule::ImportFromSelectStmtAlt02
        | DmlRule::ImportFromSelectStmtAlt03 => out.statement = rhs[rhs_len].statement.take(),
        DmlRule::ImportFromSelectStmtAlt04 => {
            let Some(statement) = take_subquery_statement(&mut rhs[rhs_len]) else {
                return Ok(false);
            };
            let statement = match statement.into_any().downcast::<parser_ast::SelectStmt>() {
                Ok(mut select) => {
                    select.IsInBraces = true;
                    select as Box<dyn parser_ast::Node>
                }
                Err(statement) => match statement.downcast::<parser_ast::SetOprStmt>() {
                    Ok(mut set_op) => {
                        set_op.IsInBraces = true;
                        set_op as Box<dyn parser_ast::Node>
                    }
                    Err(_) => return Ok(false),
                },
            };
            out.statement = Some(statement);
        }
        _ => unreachable!("non-load DML rule routed to load actions"),
    }
    Ok(true)
}
