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
enum MiscRule {
    BeginTransactionStmtAlt01,
    BeginTransactionStmtAlt02,
    BeginTransactionStmtAlt03,
    BeginTransactionStmtAlt04,
    BeginTransactionStmtAlt05,
    BeginTransactionStmtAlt06,
    BeginTransactionStmtAlt07,
    BeginTransactionStmtAlt08,
    BeginTransactionStmtAlt09,
    ColumnNameListOptAlt01,
    CommitStmtAlt01,
    CommitStmtAlt02,
    DoStmtAlt01,
    EmptyStmtAlt01,
    TraceStmtAlt01,
    TraceStmtAlt02,
    TraceStmtAlt03,
    TraceStmtAlt04,
    ExplainStmtAlt01,
    ExplainStmtAlt02,
    ExplainStmtAlt03,
    ExplainStmtAlt04,
    ExplainStmtAlt05,
    ExplainStmtAlt06,
    ExplainStmtAlt07,
    ExplainStmtAlt08,
    ExplainStmtAlt09,
    ExplainStmtAlt10,
    ExplainStmtAlt11,
    ExplainStmtAlt12,
    ExplainStmtAlt13,
    ExplainStmtAlt14,
    ExplainStmtAlt15,
    ExplainStmtAlt16,
    ExplainStmtAlt17,
    ExplainStmtAlt18,
    ExplainStmtAlt19,
    ExplainStmtAlt20,
    ExplainStmtAlt21,
    ExplainStmtAlt22,
    SavepointStmtAlt01,
    ReleaseSavepointStmtAlt01,
    CallStmtAlt01,
    ProcedureCallAlt01,
    ProcedureCallAlt02,
    ProcedureCallAlt03,
    ProcedureCallAlt04,
    ValuesListAlt01,
    ValuesListAlt02,
    PreparedStmtAlt01,
    PrepareSQLAlt01,
    PrepareSQLAlt02,
    ExecuteStmtAlt01,
    ExecuteStmtAlt02,
    DeallocateStmtAlt01,
    RollbackStmtAlt01,
    RollbackStmtAlt02,
    RollbackStmtAlt03,
    RollbackStmtAlt04,
    CompletionTypeWithinTransactionAlt01,
    CompletionTypeWithinTransactionAlt02,
    CompletionTypeWithinTransactionAlt03,
    CompletionTypeWithinTransactionAlt04,
    CompletionTypeWithinTransactionAlt05,
    CompletionTypeWithinTransactionAlt06,
    CompletionTypeWithinTransactionAlt07,
    HelpStmtAlt01,
    TransactionCharsAlt01,
    TransactionCharsAlt02,
    TransactionCharAlt01,
    TransactionCharAlt02,
    TransactionCharAlt03,
    TransactionCharAlt04,
    IsolationLevelAlt01,
    IsolationLevelAlt02,
    IsolationLevelAlt03,
    IsolationLevelAlt04,
    StatementAlt82,
    TraceableStmtAlt08,
    ExplainableStmtAlt08,
    StatementListAlt01,
    StatementListAlt02,
    UseStmtAlt01,
    BindableStmtAlt04,
    SpPdparamsAlt01,
    SpPdparamsAlt02,
    SpPdparamAlt01,
    SpOptInoutAlt01,
    SpOptInoutAlt02,
    SpOptInoutAlt03,
    SpOptInoutAlt04,
    ProcedureStatementStmtAlt03,
    ProcedureCursorSelectStmtAlt03,
    ProcedureUnlabeledBlockAlt01,
    ProcedureDeclIdentsAlt01,
    ProcedureDeclIdentsAlt02,
    ProcedureOptDefaultAlt01,
    ProcedureOptDefaultAlt02,
    ProcedureDeclAlt01,
    ProcedureDeclAlt02,
    ProcedureDeclAlt03,
    ProcedureHandlerTypeAlt01,
    ProcedureHandlerTypeAlt02,
    ProcedureHcondListAlt01,
    ProcedureHcondListAlt02,
    ProcedureHcondAlt01,
    ProcedureHcondAlt02,
    ProcedureHcondAlt03,
    ProcedureHcondAlt04,
    ProcedurceCondAlt01,
    ProcedurceCondAlt02,
    ProcedureOpenCurAlt01,
    ProcedureFetchIntoAlt01,
    ProcedureCloseCurAlt01,
    ProcedureFetchListAlt01,
    ProcedureFetchListAlt02,
    ProcedureDeclsOptAlt01,
    ProcedureDeclsOptAlt02,
    ProcedureDeclsAlt01,
    ProcedureDeclsAlt02,
    ProcedureProcStmtsAlt01,
    ProcedureProcStmtsAlt02,
    ProcedureProcStmt1sAlt01,
    ProcedureProcStmt1sAlt02,
    ProcedureBlockContentAlt01,
    ProcedureIfstmtAlt01,
    ProcedureIfAlt01,
    procedurceElseIfsAlt01,
    procedurceElseIfsAlt02,
    procedurceElseIfsAlt03,
    ProcedureCaseStmtAlt01,
    ProcedureCaseStmtAlt02,
    SimpleWhenThenListAlt01,
    SimpleWhenThenListAlt02,
    SearchedWhenThenListAlt01,
    SearchedWhenThenListAlt02,
    SimpleWhenThenAlt01,
    SearchWhenThenAlt01,
    ElseCaseOptAlt01,
    ElseCaseOptAlt02,
    ProcedureSimpleCaseAlt01,
    ProcedureSearchedCaseAlt01,
    ProcedureUnlabelLoopBlockAlt01,
    ProcedureUnlabelLoopStmtAlt01,
    ProcedureUnlabelLoopStmtAlt02,
    ProcedureLabeledBlockAlt01,
    ProcedurceLabelOptAlt01,
    ProcedurceLabelOptAlt02,
    ProcedurelabeledLoopStmtAlt01,
    ProcedureIterateAlt01,
    ProcedureLeaveAlt01,
    CreateProcedureStmtAlt01,
    DropProcedureStmtAlt01,
}

fn identify(rule_id: RuleId) -> Option<MiscRule> {
    Some(match rule_id.as_str() {
        "begintransactionstmt_begin--36ad34a4ed39ddfc" => MiscRule::BeginTransactionStmtAlt01,
        "begintransactionstmt_begin_optimistic--4ac067c64cf27acb" => {
            MiscRule::BeginTransactionStmtAlt03
        }
        "begintransactionstmt_begin_pessimistic--2f49c6297b9e112f" => {
            MiscRule::BeginTransactionStmtAlt02
        }
        "begintransactionstmt_start_transaction--42dac693f619dabb" => {
            MiscRule::BeginTransactionStmtAlt04
        }
        "begintransactionstmt_start_transaction_read_only--1d28ab8883df52ad" => {
            MiscRule::BeginTransactionStmtAlt08
        }
        "begintransactionstmt_start_transaction_read_only--b5c36e19764bf90b" => {
            MiscRule::BeginTransactionStmtAlt09
        }
        "begintransactionstmt_start_transaction_read_writ--4591d5926c371b7c" => {
            MiscRule::BeginTransactionStmtAlt05
        }
        "begintransactionstmt_start_transaction_with_caus--7de6e2bb1c040a0e" => {
            MiscRule::BeginTransactionStmtAlt07
        }
        "begintransactionstmt_start_transaction_with_cons--09c9a255bb83a301" => {
            MiscRule::BeginTransactionStmtAlt06
        }
        "bindablestmt_subselect--e324bc87f878601d" => MiscRule::BindableStmtAlt04,
        "callstmt_call_procedurecall--789db3f3450311df" => MiscRule::CallStmtAlt01,
        "columnnamelistopt--349efe0fc413719d" => MiscRule::ColumnNameListOptAlt01,
        "commitstmt_commit--94b01d492b914e0c" => MiscRule::CommitStmtAlt01,
        "commitstmt_commit_completiontypewithintransactio--bf15b8bdb8b20e97" => {
            MiscRule::CommitStmtAlt02
        }
        "completiontypewithintransaction_and_chain--ac065eef24db33c7" => {
            MiscRule::CompletionTypeWithinTransactionAlt04
        }
        "completiontypewithintransaction_and_chain_no_rel--81f802d6944cfb0f" => {
            MiscRule::CompletionTypeWithinTransactionAlt01
        }
        "completiontypewithintransaction_and_no_chain--896e80362a224616" => {
            MiscRule::CompletionTypeWithinTransactionAlt05
        }
        "completiontypewithintransaction_and_no_chain_no--a597c6af65cd2474" => {
            MiscRule::CompletionTypeWithinTransactionAlt03
        }
        "completiontypewithintransaction_and_no_chain_rel--5ca431466874572d" => {
            MiscRule::CompletionTypeWithinTransactionAlt02
        }
        "completiontypewithintransaction_no_release--7d5953ea7f7581ff" => {
            MiscRule::CompletionTypeWithinTransactionAlt07
        }
        "completiontypewithintransaction_release--d68b87e4f2002764" => {
            MiscRule::CompletionTypeWithinTransactionAlt06
        }
        "createprocedurestmt_create_procedure_ifnotexists--f88d1f04f7772c97" => {
            MiscRule::CreateProcedureStmtAlt01
        }
        "deallocatestmt_deallocatesym_prepare_identifier--255d0933436a6613" => {
            MiscRule::DeallocateStmtAlt01
        }
        "dostmt_do_expressionlist--b4fb81ab94c53a5e" => MiscRule::DoStmtAlt01,
        "dropprocedurestmt_drop_procedure_ifexists_tablen--aeaf824a0667d212" => {
            MiscRule::DropProcedureStmtAlt01
        }
        "elsecaseopt--77946694592cb04d" => MiscRule::ElseCaseOptAlt01,
        "elsecaseopt_else_procedureprocstmt1s--315057052ba215e8" => MiscRule::ElseCaseOptAlt02,
        "emptystmt--bede6c684b818500" => MiscRule::EmptyStmtAlt01,
        "executestmt_execute_identifier--fe78c6917741561f" => MiscRule::ExecuteStmtAlt01,
        "executestmt_execute_identifier_using_uservariabl--11b7c28846e07b12" => {
            MiscRule::ExecuteStmtAlt02
        }
        "explainablestmt_subselect--ba5cc5a0787b3ed3" => MiscRule::ExplainableStmtAlt08,
        "explainstmt_explainsym_analyze_explainablestmt--ba11be5c840f55c0" => {
            MiscRule::ExplainStmtAlt17
        }
        "explainstmt_explainsym_analyze_format_explainfor--81827c57f797cbde" => {
            MiscRule::ExplainStmtAlt19
        }
        "explainstmt_explainsym_analyze_format_explainfor--9cfd10cf92eb937d" => {
            MiscRule::ExplainStmtAlt20
        }
        "explainstmt_explainsym_analyze_format_stringlit--6003a83fbbb76a8c" => {
            MiscRule::ExplainStmtAlt22
        }
        "explainstmt_explainsym_analyze_format_stringlit--df8cee52ae7f8abf" => {
            MiscRule::ExplainStmtAlt21
        }
        "explainstmt_explainsym_analyze_stringlit--18dc22be40741d0b" => MiscRule::ExplainStmtAlt18,
        "explainstmt_explainsym_explainablestmt--8618cb7b29eca07c" => MiscRule::ExplainStmtAlt08,
        "explainstmt_explainsym_explore_analyze_selectstm--b850925757405cea" => {
            MiscRule::ExplainStmtAlt04
        }
        "explainstmt_explainsym_explore_analyze_stringlit--212f21e71e30dc7e" => {
            MiscRule::ExplainStmtAlt05
        }
        "explainstmt_explainsym_explore_replayer_stringli--b91cd6f3ea9b32c2" => {
            MiscRule::ExplainStmtAlt03
        }
        "explainstmt_explainsym_explore_selectstmt--0b87ec8fb92aecde" => MiscRule::ExplainStmtAlt01,
        "explainstmt_explainsym_explore_stringlit--fc909f4190a6899a" => MiscRule::ExplainStmtAlt02,
        "explainstmt_explainsym_for_connection_num--e22d78e724d05130" => MiscRule::ExplainStmtAlt10,
        "explainstmt_explainsym_format_explainformattype--ac1cdb7748576aa2" => {
            MiscRule::ExplainStmtAlt14
        }
        "explainstmt_explainsym_format_explainformattype--c0dbc38da38bf03a" => {
            MiscRule::ExplainStmtAlt13
        }
        "explainstmt_explainsym_format_explainformattype--cd8e999c8c8b9399" => {
            MiscRule::ExplainStmtAlt15
        }
        "explainstmt_explainsym_format_stringlit_explaina--36a6b92133d05bb0" => {
            MiscRule::ExplainStmtAlt12
        }
        "explainstmt_explainsym_format_stringlit_for_conn--1ff28cd4085d127c" => {
            MiscRule::ExplainStmtAlt11
        }
        "explainstmt_explainsym_format_stringlit_stringli--1446643f82bfbc9b" => {
            MiscRule::ExplainStmtAlt16
        }
        "explainstmt_explainsym_stringlit--59eb9ec03305b08f" => MiscRule::ExplainStmtAlt09,
        "explainstmt_explainsym_tablename--029e49cab7f93f8a" => MiscRule::ExplainStmtAlt06,
        "explainstmt_explainsym_tablename_columnname--eb850f8ba14f3f4f" => {
            MiscRule::ExplainStmtAlt07
        }
        "helpstmt_help_stringlit--65f71a7359761a32" => MiscRule::HelpStmtAlt01,
        "isolationlevel_read_committed--7c662264ac401c78" => MiscRule::IsolationLevelAlt02,
        "isolationlevel_read_uncommitted--c8652a69ed03722b" => MiscRule::IsolationLevelAlt03,
        "isolationlevel_repeatable_read--10a38b486f03f355" => MiscRule::IsolationLevelAlt01,
        "isolationlevel_serializable--7cb317b22ea6a887" => MiscRule::IsolationLevelAlt04,
        "preparedstmt_prepare_identifier_from_preparesql--3fea6ed74caf637e" => {
            MiscRule::PreparedStmtAlt01
        }
        "preparesql_stringlit--ae97d7be99687a55" => MiscRule::PrepareSQLAlt01,
        "preparesql_uservariable--4aaa68a1735226c2" => MiscRule::PrepareSQLAlt02,
        "procedurcecond_num--11cb8018937f992e" => MiscRule::ProcedurceCondAlt01,
        "procedurcecond_sqlstate_optvalue_stringlit--67759bdcc378e8e7" => {
            MiscRule::ProcedurceCondAlt02
        }
        "procedurceelseifs--fa3dce9c85870482" => MiscRule::procedurceElseIfsAlt01,
        "procedurceelseifs_else_procedureprocstmt1s--a6bc302ef7f4a6af" => {
            MiscRule::procedurceElseIfsAlt03
        }
        "procedurceelseifs_elseif_procedureif--0b8a1ac033ded9b7" => {
            MiscRule::procedurceElseIfsAlt02
        }
        "procedurcelabelopt--890e83eca4783408" => MiscRule::ProcedurceLabelOptAlt01,
        "procedurcelabelopt_identifier--97689a82e351fe3a" => MiscRule::ProcedurceLabelOptAlt02,
        "procedureblockcontent_begin_proceduredeclsopt_pr--d9724949790f81c2" => {
            MiscRule::ProcedureBlockContentAlt01
        }
        "procedurecall_identifier--c88bc9cf88d2c1dc" => MiscRule::ProcedureCallAlt01,
        "procedurecall_identifier_expressionlistopt--f68d0d2d2dbb76f6" => {
            MiscRule::ProcedureCallAlt03
        }
        "procedurecall_identifier_identifier--a8a62e95fa8b4725" => MiscRule::ProcedureCallAlt02,
        "procedurecall_identifier_identifier_expressionli--c2c711eeab8d2c2f" => {
            MiscRule::ProcedureCallAlt04
        }
        "procedurecasestmt_proceduresearchedcase--9a6a9e67f4d45953" => {
            MiscRule::ProcedureCaseStmtAlt02
        }
        "procedurecasestmt_proceduresimplecase--e9afffc073fdee98" => {
            MiscRule::ProcedureCaseStmtAlt01
        }
        "procedureclosecur_close_identifier--be33b520a70b5cce" => MiscRule::ProcedureCloseCurAlt01,
        "procedurecursorselectstmt_subselect--9f59fd2b94a6bb43" => {
            MiscRule::ProcedureCursorSelectStmtAlt03
        }
        "proceduredecl_declare_identifier_cursor_for_proc--7c0121dc36de44e0" => {
            MiscRule::ProcedureDeclAlt02
        }
        "proceduredecl_declare_proceduredeclidents_type_p--9f56223add6f8fcc" => {
            MiscRule::ProcedureDeclAlt01
        }
        "proceduredecl_declare_procedurehandlertype_handl--a120c9f00c56be7d" => {
            MiscRule::ProcedureDeclAlt03
        }
        "proceduredeclidents_identifier--50d727190095449f" => MiscRule::ProcedureDeclIdentsAlt01,
        "proceduredeclidents_proceduredeclidents_identifi--93123888fee2f82d" => {
            MiscRule::ProcedureDeclIdentsAlt02
        }
        "proceduredecls_proceduredecl--68e9790050398a7c" => MiscRule::ProcedureDeclsAlt01,
        "proceduredecls_proceduredecls_proceduredecl--c468606cc9005992" => {
            MiscRule::ProcedureDeclsAlt02
        }
        "proceduredeclsopt--0f7cf6ed453ce14c" => MiscRule::ProcedureDeclsOptAlt01,
        "proceduredeclsopt_proceduredecls--16de2979dfd5ce57" => MiscRule::ProcedureDeclsOptAlt02,
        "procedurefetchinto_fetch_procedureoptfetchno_ide--3b78173c3948d95c" => {
            MiscRule::ProcedureFetchIntoAlt01
        }
        "procedurefetchlist_identifier--1f4873f6287d8d78" => MiscRule::ProcedureFetchListAlt01,
        "procedurefetchlist_procedurefetchlist_identifier--0d7456747ee94af7" => {
            MiscRule::ProcedureFetchListAlt02
        }
        "procedurehandlertype_continue--a950326abdb8bdf8" => MiscRule::ProcedureHandlerTypeAlt01,
        "procedurehandlertype_exit--ba84b89b2eca9e53" => MiscRule::ProcedureHandlerTypeAlt02,
        "procedurehcond_not_found--cba41c19510a68c6" => MiscRule::ProcedureHcondAlt03,
        "procedurehcond_procedurcecond--7ead1188ebd7fee5" => MiscRule::ProcedureHcondAlt01,
        "procedurehcond_sqlexception--1404755fcdd88744" => MiscRule::ProcedureHcondAlt04,
        "procedurehcond_sqlwarning--9d42013b93e749ef" => MiscRule::ProcedureHcondAlt02,
        "procedurehcondlist_procedurehcond--07faaad4f335d53c" => MiscRule::ProcedureHcondListAlt01,
        "procedurehcondlist_procedurehcondlist_procedureh--73d8ade971dd2603" => {
            MiscRule::ProcedureHcondListAlt02
        }
        "procedureif_expression_then_procedureprocstmt1s--7d26f49f41ba5dd9" => {
            MiscRule::ProcedureIfAlt01
        }
        "procedureifstmt_if_procedureif_end_if--f89ef370c2036143" => MiscRule::ProcedureIfstmtAlt01,
        "procedureiterate_iterate_identifier--c49dd3b2179c94d0" => MiscRule::ProcedureIterateAlt01,
        "procedurelabeledblock_identifier_procedureblockc--d1e189806c8c344a" => {
            MiscRule::ProcedureLabeledBlockAlt01
        }
        "procedurelabeledloopstmt_identifier_procedureunl--a5d4c0de85c40190" => {
            MiscRule::ProcedurelabeledLoopStmtAlt01
        }
        "procedureleave_leave_identifier--d830ae9c8afaedf4" => MiscRule::ProcedureLeaveAlt01,
        "procedureopencur_open_identifier--fc38fc5107f36c50" => MiscRule::ProcedureOpenCurAlt01,
        "procedureoptdefault--45ded256f5fa21a8" => MiscRule::ProcedureOptDefaultAlt01,
        "procedureoptdefault_default_expression--cb656f7f9a21923a" => {
            MiscRule::ProcedureOptDefaultAlt02
        }
        "procedureprocstmt1s_procedureprocstmt--51f5c3a044990d33" => {
            MiscRule::ProcedureProcStmt1sAlt01
        }
        "procedureprocstmt1s_procedureprocstmt1s_procedur--0285d70d582db16c" => {
            MiscRule::ProcedureProcStmt1sAlt02
        }
        "procedureprocstmts--5d6788c67c169085" => MiscRule::ProcedureProcStmtsAlt01,
        "procedureprocstmts_procedureprocstmts_procedurep--a9f6e46cfcf70794" => {
            MiscRule::ProcedureProcStmtsAlt02
        }
        "proceduresearchedcase_case_searchedwhenthenlist--55e8908b38c1a1c7" => {
            MiscRule::ProcedureSearchedCaseAlt01
        }
        "proceduresimplecase_case_expression_simplewhenth--0eca15498b161991" => {
            MiscRule::ProcedureSimpleCaseAlt01
        }
        "procedurestatementstmt_subselect--0e2aeb1e4668ff0e" => {
            MiscRule::ProcedureStatementStmtAlt03
        }
        "procedureunlabeledblock_procedureblockcontent--20e1aa0430e9f863" => {
            MiscRule::ProcedureUnlabeledBlockAlt01
        }
        "procedureunlabelloopblock_procedureunlabelloopst--1e78600b86a807cd" => {
            MiscRule::ProcedureUnlabelLoopBlockAlt01
        }
        "procedureunlabelloopstmt_repeat_procedureprocstm--13f2fb6d13e60b80" => {
            MiscRule::ProcedureUnlabelLoopStmtAlt02
        }
        "procedureunlabelloopstmt_while_expression_do_pro--5cd523ea2c98d71d" => {
            MiscRule::ProcedureUnlabelLoopStmtAlt01
        }
        "releasesavepointstmt_release_savepoint_identifie--e0c5d64cfc69ee87" => {
            MiscRule::ReleaseSavepointStmtAlt01
        }
        "rollbackstmt_rollback--1692510c7238cfa2" => MiscRule::RollbackStmtAlt01,
        "rollbackstmt_rollback_completiontypewithintransa--77ade4b1015187bd" => {
            MiscRule::RollbackStmtAlt02
        }
        "rollbackstmt_rollback_to_identifier--98f50dd756521e24" => MiscRule::RollbackStmtAlt03,
        "rollbackstmt_rollback_to_savepoint_identifier--53cd4209d1ab2925" => {
            MiscRule::RollbackStmtAlt04
        }
        "savepointstmt_savepoint_identifier--040000eca08ec7ab" => MiscRule::SavepointStmtAlt01,
        "searchedwhenthenlist_searchedwhenthenlist_search--a9424451afda95b3" => {
            MiscRule::SearchedWhenThenListAlt02
        }
        "searchedwhenthenlist_searchwhenthen--c33420b3824dde31" => {
            MiscRule::SearchedWhenThenListAlt01
        }
        "searchwhenthen_when_expression_then_procedurepro--eb8bd5c182684e13" => {
            MiscRule::SearchWhenThenAlt01
        }
        "simplewhenthen_when_expression_then_procedurepro--b6f23ef533f26655" => {
            MiscRule::SimpleWhenThenAlt01
        }
        "simplewhenthenlist_simplewhenthen--863152bce500812c" => MiscRule::SimpleWhenThenListAlt01,
        "simplewhenthenlist_simplewhenthenlist_simplewhen--a7100f933e1a87f1" => {
            MiscRule::SimpleWhenThenListAlt02
        }
        "spoptinout--dd5a71f6e0d82540" => MiscRule::SpOptInoutAlt01,
        "spoptinout_in--73d286f11da5c060" => MiscRule::SpOptInoutAlt02,
        "spoptinout_inout--a8736280df0af8bc" => MiscRule::SpOptInoutAlt04,
        "spoptinout_out--a77472e5634571fb" => MiscRule::SpOptInoutAlt03,
        "sppdparam_spoptinout_identifier_type--fa08e9142074cfe6" => MiscRule::SpPdparamAlt01,
        "sppdparams_sppdparam--76f0ee542530fdd7" => MiscRule::SpPdparamsAlt02,
        "sppdparams_sppdparams_sppdparam--b2500379fbbeb414" => MiscRule::SpPdparamsAlt01,
        "statement_subselect--1a3c2c4087fa6d5f" => MiscRule::StatementAlt82,
        "statementlist_statement--2a3a09f420fb2dfc" => MiscRule::StatementListAlt01,
        "statementlist_statementlist_statement--288a40a8eb444b3a" => MiscRule::StatementListAlt02,
        "traceablestmt_subselect--ca7265fc7301a39d" => MiscRule::TraceableStmtAlt08,
        "tracestmt_trace_format_stringlit_traceablestmt--fbc91fbcca170375" => {
            MiscRule::TraceStmtAlt02
        }
        "tracestmt_trace_plan_target_stringlit_traceables--ce282f0aa1964240" => {
            MiscRule::TraceStmtAlt04
        }
        "tracestmt_trace_plan_traceablestmt--0c86974cd6cc0732" => MiscRule::TraceStmtAlt03,
        "tracestmt_trace_traceablestmt--04374f2bc9b9da57" => MiscRule::TraceStmtAlt01,
        "transactionchar_isolation_level_isolationlevel--cc96b9e9e0c26112" => {
            MiscRule::TransactionCharAlt01
        }
        "transactionchar_read_only--e41a31a013b39df4" => MiscRule::TransactionCharAlt03,
        "transactionchar_read_only_asofclause--072fde030b957190" => MiscRule::TransactionCharAlt04,
        "transactionchar_read_write--7bb46ba019797063" => MiscRule::TransactionCharAlt02,
        "transactionchars_transactionchar--beca3b1de20736df" => MiscRule::TransactionCharsAlt01,
        "transactionchars_transactionchars_transactioncha--a866265f1b7c8514" => {
            MiscRule::TransactionCharsAlt02
        }
        "usestmt_use_dbname--ea7a96ce4bdefecd" => MiscRule::UseStmtAlt01,
        "valueslist_rowvalue--01a9e56e0210fd5f" => MiscRule::ValuesListAlt01,
        "valueslist_valueslist_rowvalue--e6eacd8ff680dee9" => MiscRule::ValuesListAlt02,
        _ => return None,
    })
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

fn apply_rule(rule: MiscRule, mut rhs: Rhs<'_>, context: Context<'_>) -> Result<bool, isize> {
    let rhs_len = rhs.len();
    let Context {
        output: out,
        parser_state,
        lexer: yylex,
    } = context;
    match rule {
        MiscRule::BeginTransactionStmtAlt01
        | MiscRule::BeginTransactionStmtAlt04
        | MiscRule::BeginTransactionStmtAlt05
        | MiscRule::BeginTransactionStmtAlt06 => {
            out.statement = Some(Box::new(parser_ast::BeginStmt::default()));
        }
        MiscRule::BeginTransactionStmtAlt02 => {
            out.statement = Some(Box::new(parser_ast::BeginStmt {
                Mode: parser_ast::Pessimistic.to_owned(),
                ..Default::default()
            }));
        }
        MiscRule::BeginTransactionStmtAlt03 => {
            out.statement = Some(Box::new(parser_ast::BeginStmt {
                Mode: parser_ast::Optimistic.to_owned(),
                ..Default::default()
            }));
        }
        MiscRule::BeginTransactionStmtAlt07 => {
            out.statement = Some(Box::new(parser_ast::BeginStmt {
                CausalConsistencyOnly: true,
                ..Default::default()
            }));
        }
        MiscRule::BeginTransactionStmtAlt08 => {
            out.statement = Some(Box::new(parser_ast::BeginStmt {
                ReadOnly: true,
                ..Default::default()
            }));
        }
        MiscRule::BeginTransactionStmtAlt09 => {
            out.statement = Some(Box::new(parser_ast::BeginStmt {
                ReadOnly: true,
                AsOf: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::AsOfClause>())
                    .cloned(),
                ..Default::default()
            }));
        }
        MiscRule::ColumnNameListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::ColumnName>::new()))
        }
        MiscRule::CommitStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::CommitStmt::default()))
        }
        MiscRule::CommitStmtAlt02 => {
            let completion = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::CompletionType>())
                .copied()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::CommitStmt {
                node_text: Default::default(),
                CompletionType: completion,
            }));
        }
        MiscRule::DoStmtAlt01 => {
            let expressions = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::DoStmt {
                node_text: Default::default(),
                Exprs: expressions,
            }));
        }
        MiscRule::ExplainStmtAlt01
        | MiscRule::ExplainStmtAlt04
        | MiscRule::ExplainStmtAlt08
        | MiscRule::ExplainStmtAlt12
        | MiscRule::ExplainStmtAlt14
        | MiscRule::ExplainStmtAlt17
        | MiscRule::ExplainStmtAlt19
        | MiscRule::ExplainStmtAlt22 => {
            let child_back = 0;
            let Some(child) = rhs[rhs_len - (child_back)].statement.take() else {
                return Ok(false);
            };
            let (analyze, explore, format) = match rule {
                MiscRule::ExplainStmtAlt01 => (false, true, String::new()),
                MiscRule::ExplainStmtAlt04 => (true, true, String::new()),
                MiscRule::ExplainStmtAlt08 => (false, false, "row".to_owned()),
                MiscRule::ExplainStmtAlt12 | MiscRule::ExplainStmtAlt14 => {
                    (false, false, rhs[rhs_len - (1)].ident.clone())
                }
                MiscRule::ExplainStmtAlt17 => (true, false, "row".to_owned()),
                _ => (true, false, rhs[rhs_len - (1)].ident.clone()),
            };
            out.statement = Some(Box::new(parser_ast::ExplainStmt {
                node_text: Default::default(),
                analyze,
                stmt: Some(child),
                Format: format,
                Explore: explore,
                SQLDigest: String::new(),
                ReplayerFile: String::new(),
                PlanDigest: String::new(),
            }));
        }
        MiscRule::ExplainStmtAlt02
        | MiscRule::ExplainStmtAlt03
        | MiscRule::ExplainStmtAlt05
        | MiscRule::ExplainStmtAlt09
        | MiscRule::ExplainStmtAlt15
        | MiscRule::ExplainStmtAlt16
        | MiscRule::ExplainStmtAlt18
        | MiscRule::ExplainStmtAlt20
        | MiscRule::ExplainStmtAlt21 => {
            let analyze = matches!(
                rule,
                MiscRule::ExplainStmtAlt05
                    | MiscRule::ExplainStmtAlt18
                    | MiscRule::ExplainStmtAlt20
                    | MiscRule::ExplainStmtAlt21
            );
            let explore = matches!(
                rule,
                MiscRule::ExplainStmtAlt02
                    | MiscRule::ExplainStmtAlt03
                    | MiscRule::ExplainStmtAlt05
            );
            let format = match rule {
                MiscRule::ExplainStmtAlt09 | MiscRule::ExplainStmtAlt18 => "row".to_owned(),
                MiscRule::ExplainStmtAlt15
                | MiscRule::ExplainStmtAlt16
                | MiscRule::ExplainStmtAlt20
                | MiscRule::ExplainStmtAlt21 => rhs[rhs_len - (1)].ident.clone(),
                _ => String::new(),
            };
            let value = rhs[rhs_len - (0)].ident.clone();
            out.statement = Some(Box::new(parser_ast::ExplainStmt {
                node_text: Default::default(),
                analyze,
                stmt: None,
                Format: format,
                Explore: explore,
                SQLDigest: if matches!(
                    rule,
                    MiscRule::ExplainStmtAlt02 | MiscRule::ExplainStmtAlt05
                ) {
                    value.clone()
                } else {
                    String::new()
                },
                ReplayerFile: if rule == MiscRule::ExplainStmtAlt03 {
                    value.clone()
                } else {
                    String::new()
                },
                PlanDigest: if !matches!(
                    rule,
                    MiscRule::ExplainStmtAlt02
                        | MiscRule::ExplainStmtAlt03
                        | MiscRule::ExplainStmtAlt05
                ) {
                    value
                } else {
                    String::new()
                },
            }));
        }
        MiscRule::ExplainStmtAlt10 | MiscRule::ExplainStmtAlt11 | MiscRule::ExplainStmtAlt13 => {
            out.statement = Some(Box::new(parser_ast::ExplainForStmt {
                node_text: Default::default(),
                Format: if rule == MiscRule::ExplainStmtAlt10 {
                    "row".to_owned()
                } else {
                    rhs[rhs_len - (3)].ident.clone()
                },
                ConnectionID: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default()
                    .max(0) as u64,
            }))
        }
        MiscRule::ExplainStmtAlt06 | MiscRule::ExplainStmtAlt07 => {
            let table_back = if rule == MiscRule::ExplainStmtAlt06 {
                0
            } else {
                1
            };
            let table = rhs[rhs_len - (table_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned();
            let column = if rule == MiscRule::ExplainStmtAlt07 {
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                    .cloned()
            } else {
                None
            };
            let show = parser_ast::ShowStmt {
                Tp: parser_ast::ShowStmtType::Columns,
                Table: table,
                Column: column,
                ..Default::default()
            };
            out.statement = Some(Box::new(parser_ast::ExplainStmt::new(
                false,
                Box::new(show),
            )));
        }
        MiscRule::EmptyStmtAlt01 => out.statement = None,
        MiscRule::TraceStmtAlt01
        | MiscRule::TraceStmtAlt02
        | MiscRule::TraceStmtAlt03
        | MiscRule::TraceStmtAlt04 => {
            let Some(statement) = rhs[rhs_len - (0)].statement.take() else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::TraceStmt {
                node_text: Default::default(),
                Stmt: statement,
                Format: if rule == MiscRule::TraceStmtAlt01 {
                    "row".to_owned()
                } else if rule == MiscRule::TraceStmtAlt02 {
                    rhs[rhs_len - (1)].ident.clone()
                } else {
                    String::new()
                },
                TracePlan: matches!(rule, MiscRule::TraceStmtAlt03 | MiscRule::TraceStmtAlt04),
                TracePlanTarget: if rule == MiscRule::TraceStmtAlt04 {
                    rhs[rhs_len - (1)].ident.clone()
                } else {
                    String::new()
                },
            }));
        }
        MiscRule::SavepointStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::SavepointStmt {
                node_text: Default::default(),
                Name: rhs[rhs_len - (0)].ident.clone(),
            }));
        }
        MiscRule::ReleaseSavepointStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::ReleaseSavepointStmt {
                node_text: Default::default(),
                Name: rhs[rhs_len - (0)].ident.clone(),
            }));
        }
        MiscRule::PreparedStmtAlt01 => {
            let prepare_sql = rhs[rhs_len - (0)].item.as_deref();
            let sql_text = prepare_sql
                .and_then(|item| item.downcast_ref::<String>())
                .cloned()
                .unwrap_or_default();
            let sql_var = prepare_sql
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                .and_then(|expression| match &expression.Kind {
                    parser_ast::ExprKind::Variable {
                        Name,
                        IsSystem: false,
                        ..
                    } => Some(Name.clone()),
                    _ => None,
                });
            out.statement = Some(Box::new(parser_ast::PrepareStmt {
                node_text: Default::default(),
                Name: rhs[rhs_len - (2)].ident.clone(),
                SQLText: sql_text,
                SQLVar: sql_var,
            }));
        }
        MiscRule::ProcedureUnlabeledBlockAlt01 => {
            out.statement = rhs[rhs_len - (0)].statement.take()
        }
        MiscRule::ProcedureDeclIdentsAlt01 | MiscRule::ProcedureDeclIdentsAlt02 => {
            let mut values = if rule == MiscRule::ProcedureDeclIdentsAlt02 {
                rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<String>>())
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            values.push(rhs[rhs_len - (0)].ident.to_ascii_lowercase());
            out.item = Some(Box::new(values));
        }
        MiscRule::ProcedureOptDefaultAlt01 => out.item = None,
        MiscRule::ProcedureOptDefaultAlt02 => {
            out.item = rhs[rhs_len - (0)]
                .expr
                .clone()
                .map(|expr| Box::new(expr) as Box<dyn Any>)
        }
        MiscRule::ProcedureDeclAlt01 => {
            let Some(field_type) = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_types::types::FieldType>())
                .cloned()
            else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::ProcedureDecl {
                DeclNames: rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<String>>())
                    .cloned()
                    .unwrap_or_default(),
                DeclType: field_type,
                DeclDefault: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                    .cloned(),
            }));
        }
        MiscRule::ProcedureDeclAlt02 => {
            out.item = Some(Box::new(parser_ast::ProcedureCursor {
                node_text: Default::default(),
                CurName: rhs[rhs_len - (3)].ident.to_ascii_lowercase(),
                Selectstring: rhs[rhs_len - (0)].statement.take(),
            }))
        }
        MiscRule::ProcedureDeclAlt03 => {
            let errors = rhs[rhs_len - (1)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<Box<dyn Any>>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            out.item = Some(Box::new(parser_ast::ProcedureErrorControl {
                node_text: Default::default(),
                ControlHandle: rhs[rhs_len - (4)]
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default() as i32,
                ErrorCon: errors,
                Operate: rhs[rhs_len - (0)].statement.take(),
            }));
        }
        MiscRule::ProcedureHandlerTypeAlt01 | MiscRule::ProcedureHandlerTypeAlt02 => {
            out.item = Some(Box::new(if rule == MiscRule::ProcedureHandlerTypeAlt01 {
                1i32
            } else {
                2i32
            }))
        }
        MiscRule::ProcedureHcondListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .statement
                    .take()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ))
        }
        MiscRule::ProcedureHcondListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<Box<dyn parser_ast::Node>>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)].statement.take() {
                values.push(value);
            }
            out.item = Some(Box::new(values));
        }
        MiscRule::ProcedureHcondAlt01 => out.statement = rhs[rhs_len - (0)].statement.take(),
        MiscRule::ProcedureHcondAlt02
        | MiscRule::ProcedureHcondAlt03
        | MiscRule::ProcedureHcondAlt04 => {
            out.statement = Some(Box::new(parser_ast::ProcedureErrorCon {
                node_text: Default::default(),
                ErrorCon: match rule {
                    MiscRule::ProcedureHcondAlt02 => parser_ast::ProcedureErrorConType::SqlWarning,
                    MiscRule::ProcedureHcondAlt03 => parser_ast::ProcedureErrorConType::NotFound,
                    _ => parser_ast::ProcedureErrorConType::SqlException,
                },
            }))
        }
        MiscRule::ProcedurceCondAlt01 => {
            out.statement = Some(Box::new(parser_ast::ProcedureErrorVal {
                node_text: Default::default(),
                ErrorNum: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or_default()
                    .max(0) as u64,
            }))
        }
        MiscRule::ProcedurceCondAlt02 => {
            out.statement = Some(Box::new(parser_ast::ProcedureErrorState {
                node_text: Default::default(),
                CodeStatus: rhs[rhs_len - (0)].ident.clone(),
            }))
        }
        MiscRule::ProcedureOpenCurAlt01 => {
            out.statement = Some(Box::new(parser_ast::ProcedureOpenCur {
                node_text: Default::default(),
                CurName: rhs[rhs_len - (0)].ident.to_ascii_lowercase(),
            }))
        }
        MiscRule::ProcedureFetchIntoAlt01 => {
            out.statement = Some(Box::new(parser_ast::ProcedureFetchInto {
                node_text: Default::default(),
                CurName: rhs[rhs_len - (2)].ident.to_ascii_lowercase(),
                Variables: rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<String>>())
                    .cloned()
                    .unwrap_or_default(),
            }))
        }
        MiscRule::ProcedureCloseCurAlt01 => {
            out.statement = Some(Box::new(parser_ast::ProcedureCloseCur {
                node_text: Default::default(),
                CurName: rhs[rhs_len - (0)].ident.to_ascii_lowercase(),
            }))
        }
        MiscRule::ProcedureFetchListAlt01 | MiscRule::ProcedureFetchListAlt02 => {
            let mut values = if rule == MiscRule::ProcedureFetchListAlt02 {
                rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<String>>())
                    .cloned()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            values.push(rhs[rhs_len - (0)].ident.to_ascii_lowercase());
            out.item = Some(Box::new(values));
        }
        MiscRule::ProcedureDeclsOptAlt01 => out.item = Some(Box::new(Vec::<Box<dyn Any>>::new())),
        MiscRule::ProcedureDeclsOptAlt02 => out.item = rhs[rhs_len - (0)].item.take(),
        MiscRule::ProcedureDeclsAlt01 => {
            let value = rhs[rhs_len - (1)].item.take().or_else(|| {
                rhs[rhs_len - (1)]
                    .statement
                    .take()
                    .map(|node| node.into_any())
            });
            out.item = Some(Box::new(value.into_iter().collect::<Vec<Box<dyn Any>>>()));
        }
        MiscRule::ProcedureDeclsAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<Box<dyn Any>>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (1)].item.take().or_else(|| {
                rhs[rhs_len - (1)]
                    .statement
                    .take()
                    .map(|node| node.into_any())
            }) {
                values.push(value);
            }
            out.item = Some(Box::new(values));
        }
        MiscRule::ProcedureProcStmtsAlt01 => {
            out.item = Some(Box::new(Vec::<Box<dyn parser_ast::Node>>::new()))
        }
        MiscRule::ProcedureProcStmtsAlt02 | MiscRule::ProcedureProcStmt1sAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<Box<dyn parser_ast::Node>>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (1)].statement.take() {
                values.push(value);
            }
            out.item = Some(Box::new(values));
        }
        MiscRule::ProcedureProcStmt1sAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (1)]
                    .statement
                    .take()
                    .into_iter()
                    .collect::<Vec<Box<dyn parser_ast::Node>>>(),
            ))
        }
        MiscRule::ProcedureBlockContentAlt01 => {
            out.statement = Some(Box::new(parser_ast::ProcedureBlock {
                node_text: Default::default(),
                ProcedureVars: rhs[rhs_len - (2)]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<Vec<Box<dyn Any>>>().ok())
                    .map(|item| *item)
                    .unwrap_or_default(),
                ProcedureProcStmts: rhs[rhs_len - (1)]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<Vec<Box<dyn parser_ast::Node>>>().ok())
                    .map(|item| *item)
                    .unwrap_or_default(),
            }))
        }
        MiscRule::ProcedureIfstmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::ProcedureIfInfo {
                node_text: Default::default(),
                IfBody: rhs[rhs_len - (2)].statement.take(),
            }))
        }
        MiscRule::ProcedureIfAlt01 => {
            out.statement = Some(Box::new(parser_ast::ProcedureIfBlock {
                node_text: Default::default(),
                IfExpr: rhs[rhs_len - (3)].expr.clone().unwrap_or_default(),
                ProcedureIfStmts: rhs[rhs_len - (1)]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<Vec<Box<dyn parser_ast::Node>>>().ok())
                    .map(|item| *item)
                    .unwrap_or_default(),
                ProcedureElseStmt: rhs[rhs_len - (0)].statement.take(),
            }))
        }
        MiscRule::procedurceElseIfsAlt01 => out.statement = None,
        MiscRule::procedurceElseIfsAlt02 => {
            out.statement = Some(Box::new(parser_ast::ProcedureElseIfBlock {
                node_text: Default::default(),
                ProcedureIfStmt: rhs[rhs_len - (0)].statement.take(),
            }))
        }
        MiscRule::procedurceElseIfsAlt03 => {
            out.statement = Some(Box::new(parser_ast::ProcedureElseBlock {
                node_text: Default::default(),
                ProcedureIfStmts: rhs[rhs_len - (0)]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<Vec<Box<dyn parser_ast::Node>>>().ok())
                    .map(|item| *item)
                    .unwrap_or_default(),
            }))
        }
        MiscRule::ProcedureCaseStmtAlt01
        | MiscRule::ProcedureCaseStmtAlt02
        | MiscRule::ProcedureUnlabelLoopBlockAlt01 => {
            out.statement = rhs[rhs_len - (0)].statement.take()
        }
        MiscRule::SimpleWhenThenListAlt01 | MiscRule::SearchedWhenThenListAlt01 => {
            let Some(statement) = rhs[rhs_len - (0)].statement.take() else {
                return Ok(false);
            };
            if rule == MiscRule::SimpleWhenThenListAlt01 {
                let Ok(value) = statement
                    .into_any()
                    .downcast::<parser_ast::SimpleWhenThenStmt>()
                else {
                    return Ok(false);
                };
                out.item = Some(Box::new(vec![*value]));
            } else {
                let Ok(value) = statement
                    .into_any()
                    .downcast::<parser_ast::SearchWhenThenStmt>()
                else {
                    return Ok(false);
                };
                out.item = Some(Box::new(vec![*value]));
            }
        }
        MiscRule::SimpleWhenThenListAlt02 => {
            let mut values = rhs[rhs_len - (1)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<parser_ast::SimpleWhenThenStmt>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            let Some(statement) = rhs[rhs_len - (0)].statement.take() else {
                return Ok(false);
            };
            let Ok(value) = statement
                .into_any()
                .downcast::<parser_ast::SimpleWhenThenStmt>()
            else {
                return Ok(false);
            };
            values.push(*value);
            out.item = Some(Box::new(values));
        }
        MiscRule::SearchedWhenThenListAlt02 => {
            let mut values = rhs[rhs_len - (1)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<parser_ast::SearchWhenThenStmt>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            let Some(statement) = rhs[rhs_len - (0)].statement.take() else {
                return Ok(false);
            };
            let Ok(value) = statement
                .into_any()
                .downcast::<parser_ast::SearchWhenThenStmt>()
            else {
                return Ok(false);
            };
            values.push(*value);
            out.item = Some(Box::new(values));
        }
        MiscRule::SimpleWhenThenAlt01 | MiscRule::SearchWhenThenAlt01 => {
            let expr = rhs[rhs_len - (2)].expr.clone().unwrap_or_default();
            let statements = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<Box<dyn parser_ast::Node>>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            out.statement = Some(if rule == MiscRule::SimpleWhenThenAlt01 {
                Box::new(parser_ast::SimpleWhenThenStmt {
                    node_text: Default::default(),
                    Expr: expr,
                    ProcedureStmts: statements,
                }) as Box<dyn parser_ast::Node>
            } else {
                Box::new(parser_ast::SearchWhenThenStmt {
                    node_text: Default::default(),
                    Expr: expr,
                    ProcedureStmts: statements,
                }) as Box<dyn parser_ast::Node>
            });
        }
        MiscRule::ElseCaseOptAlt01 => out.item = None,
        MiscRule::ElseCaseOptAlt02 => out.item = rhs[rhs_len - (0)].item.take(),
        MiscRule::ProcedureSimpleCaseAlt01 => {
            out.statement = Some(Box::new(parser_ast::SimpleCaseStmt {
                node_text: Default::default(),
                Condition: rhs[rhs_len - (4)].expr.clone().unwrap_or_default(),
                WhenCases: rhs[rhs_len - (3)]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<Vec<parser_ast::SimpleWhenThenStmt>>().ok())
                    .map(|item| *item)
                    .unwrap_or_default(),
                ElseCases: rhs[rhs_len - (2)]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<Vec<Box<dyn parser_ast::Node>>>().ok())
                    .map(|item| *item)
                    .unwrap_or_default(),
            }))
        }
        MiscRule::ProcedureSearchedCaseAlt01 => {
            out.statement = Some(Box::new(parser_ast::SearchCaseStmt {
                node_text: Default::default(),
                WhenCases: rhs[rhs_len - (3)]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<Vec<parser_ast::SearchWhenThenStmt>>().ok())
                    .map(|item| *item)
                    .unwrap_or_default(),
                ElseCases: rhs[rhs_len - (2)]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<Vec<Box<dyn parser_ast::Node>>>().ok())
                    .map(|item| *item)
                    .unwrap_or_default(),
            }))
        }
        MiscRule::ProcedureUnlabelLoopStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::ProcedureWhileStmt {
                node_text: Default::default(),
                Condition: rhs[rhs_len - (4)].expr.clone().unwrap_or_default(),
                Body: rhs[rhs_len - (2)]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<Vec<Box<dyn parser_ast::Node>>>().ok())
                    .map(|item| *item)
                    .unwrap_or_default(),
            }))
        }
        MiscRule::ProcedureUnlabelLoopStmtAlt02 => {
            out.statement = Some(Box::new(parser_ast::ProcedureRepeatStmt {
                node_text: Default::default(),
                Body: rhs[rhs_len - (4)]
                    .item
                    .take()
                    .and_then(|item| item.downcast::<Vec<Box<dyn parser_ast::Node>>>().ok())
                    .map(|item| *item)
                    .unwrap_or_default(),
                Condition: rhs[rhs_len - (2)].expr.clone().unwrap_or_default(),
            }))
        }
        MiscRule::ProcedureLabeledBlockAlt01 | MiscRule::ProcedurelabeledLoopStmtAlt01 => {
            let label = rhs[rhs_len - (3)].ident.clone();
            let end = rhs[rhs_len - (0)].ident.clone();
            let error = !end.is_empty() && label != end;
            let label_end = if error { end } else { String::new() };
            let block = rhs[rhs_len - (1)].statement.take();
            out.statement = Some(if rule == MiscRule::ProcedureLabeledBlockAlt01 {
                Box::new(parser_ast::ProcedureLabelBlock {
                    node_text: Default::default(),
                    LabelName: label,
                    Block: block,
                    LabelError: error,
                    LabelEnd: label_end,
                }) as Box<dyn parser_ast::Node>
            } else {
                Box::new(parser_ast::ProcedureLabelLoop {
                    node_text: Default::default(),
                    LabelName: label,
                    Block: block,
                    LabelError: error,
                    LabelEnd: label_end,
                }) as Box<dyn parser_ast::Node>
            });
        }
        MiscRule::ProcedurceLabelOptAlt01 => out.ident.clear(),
        MiscRule::ProcedurceLabelOptAlt02 => out.ident = rhs[rhs_len - (0)].ident.clone(),
        MiscRule::ProcedureIterateAlt01 | MiscRule::ProcedureLeaveAlt01 => {
            out.statement = Some(Box::new(parser_ast::ProcedureJump {
                node_text: Default::default(),
                Name: rhs[rhs_len - (0)].ident.clone(),
                IsLeave: rule == MiscRule::ProcedureLeaveAlt01,
            }))
        }
        MiscRule::CreateProcedureStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::ProcedureInfo {
                node_text: Default::default(),
                IfNotExists: rhs[rhs_len - (5)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ProcedureName: rhs[rhs_len - (4)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                    .cloned()
                    .unwrap_or_default(),
                ProcedureParam: rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::StoreParameter>>())
                    .cloned()
                    .unwrap_or_default(),
                ProcedureBody: rhs[rhs_len - (0)].statement.take(),
            }))
        }
        MiscRule::PrepareSQLAlt01 => out.item = Some(Box::new(rhs[rhs_len - (0)].ident.clone())),
        MiscRule::PrepareSQLAlt02 => {
            out.item = rhs[rhs_len - (0)]
                .expr
                .clone()
                .map(|expr| Box::new(expr) as Box<dyn Any>)
        }
        MiscRule::ExecuteStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::ExecuteStmt {
                Name: rhs[rhs_len - (0)].ident.clone(),
                ..Default::default()
            }));
        }
        MiscRule::ExecuteStmtAlt02 => {
            let vars = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                .cloned()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::ExecuteStmt {
                node_text: Default::default(),
                Name: rhs[rhs_len - (2)].ident.clone(),
                UsingVars: vars,
            }));
        }
        MiscRule::DeallocateStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::DeallocateStmt {
                node_text: Default::default(),
                Name: rhs[rhs_len - (0)].ident.clone(),
            }));
        }
        MiscRule::RollbackStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::RollbackStmt::default()))
        }
        MiscRule::RollbackStmtAlt02 => {
            let completion = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::CompletionType>())
                .copied()
                .unwrap_or_default();
            out.statement = Some(Box::new(parser_ast::RollbackStmt {
                CompletionType: completion,
                ..Default::default()
            }));
        }
        MiscRule::RollbackStmtAlt03 | MiscRule::RollbackStmtAlt04 => {
            out.statement = Some(Box::new(parser_ast::RollbackStmt {
                SavepointName: rhs[rhs_len - (0)].ident.clone(),
                ..Default::default()
            }));
        }
        MiscRule::CompletionTypeWithinTransactionAlt01
        | MiscRule::CompletionTypeWithinTransactionAlt04 => {
            out.item = Some(Box::new(parser_ast::CompletionTypeChain))
        }
        MiscRule::CompletionTypeWithinTransactionAlt02
        | MiscRule::CompletionTypeWithinTransactionAlt06 => {
            out.item = Some(Box::new(parser_ast::CompletionTypeRelease))
        }
        MiscRule::CompletionTypeWithinTransactionAlt03
        | MiscRule::CompletionTypeWithinTransactionAlt05
        | MiscRule::CompletionTypeWithinTransactionAlt07 => {
            out.item = Some(Box::new(parser_ast::CompletionTypeDefault))
        }
        MiscRule::HelpStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::HelpStmt {
                node_text: Default::default(),
                Topic: rhs[rhs_len - (0)].ident.clone(),
            }));
        }
        MiscRule::TransactionCharsAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::VariableAssignment>>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(values));
        }
        MiscRule::TransactionCharsAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::VariableAssignment>>())
                .cloned()
                .unwrap_or_default();
            if let Some(next) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::VariableAssignment>>())
            {
                values.extend(next.clone());
            }
            out.item = Some(Box::new(values));
        }
        MiscRule::TransactionCharAlt04 => {
            let mut values = Vec::<parser_ast::VariableAssignment>::new();
            if let Some(as_of) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::AsOfClause>())
            {
                values.push(parser_ast::VariableAssignment {
                    Name: "tx_read_ts".to_owned(),
                    Value: as_of.TsExpr.clone(),
                    IsSystem: true,
                    ..Default::default()
                });
            }
            out.item = Some(Box::new(values));
        }
        MiscRule::TransactionCharAlt01
        | MiscRule::TransactionCharAlt02
        | MiscRule::TransactionCharAlt03 => {
            let (name, value) = match rule {
                MiscRule::TransactionCharAlt01 => {
                    ("tx_isolation", rhs[rhs_len - (0)].ident.clone())
                }
                MiscRule::TransactionCharAlt02 => ("tx_read_only", "0".to_owned()),
                _ => ("tx_read_only", "1".to_owned()),
            };
            out.item = Some(Box::new(vec![parser_ast::VariableAssignment {
                Name: name.to_owned(),
                Value: parser_ast::ExprNode::Value(value),
                IsSystem: true,
                ..Default::default()
            }]));
        }
        MiscRule::IsolationLevelAlt01 => out.ident = "REPEATABLE-READ".to_owned(),
        MiscRule::IsolationLevelAlt02 => out.ident = "READ-COMMITTED".to_owned(),
        MiscRule::IsolationLevelAlt03 => out.ident = "READ-UNCOMMITTED".to_owned(),
        MiscRule::IsolationLevelAlt04 => out.ident = "SERIALIZABLE".to_owned(),
        MiscRule::CallStmtAlt01 => {
            let Some(procedure) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::CallStmt {
                node_text: Default::default(),
                Procedure: procedure,
            }));
        }
        MiscRule::ProcedureCallAlt01
        | MiscRule::ProcedureCallAlt02
        | MiscRule::ProcedureCallAlt03
        | MiscRule::ProcedureCallAlt04 => {
            let (schema, name, args) = match rule {
                MiscRule::ProcedureCallAlt01 => (
                    parser_ast::CIStr::default(),
                    parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
                    Vec::new(),
                ),
                MiscRule::ProcedureCallAlt02 => (
                    parser_ast::NewCIStr(&rhs[rhs_len - (2)].ident),
                    parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
                    Vec::new(),
                ),
                MiscRule::ProcedureCallAlt03 => (
                    parser_ast::CIStr::default(),
                    parser_ast::NewCIStr(&rhs[rhs_len - (3)].ident),
                    rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                        .cloned()
                        .unwrap_or_default(),
                ),
                _ => (
                    parser_ast::NewCIStr(&rhs[rhs_len - (5)].ident),
                    parser_ast::NewCIStr(&rhs[rhs_len - (3)].ident),
                    rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                        .cloned()
                        .unwrap_or_default(),
                ),
            };
            out.expr = Some(parser_ast::ExprNode::Function(schema, name, args));
        }
        MiscRule::ValuesListAlt01 => {
            let row = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<parser_ast::ExprNode>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            out.item = Some(Box::new(vec![row]));
        }
        MiscRule::ValuesListAlt02 => {
            let mut rows = rhs[rhs_len - (2)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<Vec<parser_ast::ExprNode>>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            if let Some(row) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<parser_ast::ExprNode>>().ok())
                .map(|item| *item)
            {
                rows.push(row);
            }
            out.item = Some(Box::new(rows));
        }
        MiscRule::StatementAlt82
        | MiscRule::TraceableStmtAlt08
        | MiscRule::ExplainableStmtAlt08
        | MiscRule::BindableStmtAlt04
        | MiscRule::ProcedureStatementStmtAlt03
        | MiscRule::ProcedureCursorSelectStmtAlt03 => {
            let Some(statement) = take_subquery_statement(&mut rhs[rhs_len - (0)]) else {
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
        MiscRule::StatementListAlt01 | MiscRule::StatementListAlt02 => {
            if let Some(mut statement) = rhs[rhs_len - (0)].statement.take() {
                if let Some((encoding, text)) = yylex.statement_text() {
                    statement.SetText(Some(encoding), text.as_bytes());
                    statement.SetNoBackslashEscapes(
                        mysql::SQLMode(yylex.sql_mode_bits()).HasNoBackslashEscapesMode(),
                    );
                }
                parser_state.reducedStatementCount += 1;
                parser_state.allStatementsSemanticallyComplete &=
                    rhs[rhs_len - (0)].semantic_complete;
                parser_state.result.push(statement);
            }
        }
        MiscRule::UseStmtAlt01 => {
            out.statement = Some(Box::new(parser_ast::UseStmt {
                node_text: Default::default(),
                DBName: rhs[rhs_len - (0)].ident.clone(),
            }));
        }
        MiscRule::SpPdparamsAlt01 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .take()
                .and_then(|item| item.downcast::<Vec<parser_ast::StoreParameter>>().ok())
                .map(|item| *item)
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::StoreParameter>().ok())
                .map(|item| *item)
            {
                values.push(value);
            }
            out.item = Some(Box::new(values));
        }
        MiscRule::SpPdparamsAlt02 => {
            let values = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_ast::StoreParameter>().ok())
                .map(|item| vec![*item])
                .unwrap_or_default();
            out.item = Some(Box::new(values));
        }
        MiscRule::SpPdparamAlt01 => {
            let Some(field_type) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<parser_types::types::FieldType>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::StoreParameter {
                Paramstatus: rhs[rhs_len - (2)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<i32>())
                    .copied()
                    .unwrap_or(parser_ast::MODE_IN),
                ParamType: field_type,
                ParamName: rhs[rhs_len - (1)].ident.clone(),
            }));
        }
        MiscRule::SpOptInoutAlt01 | MiscRule::SpOptInoutAlt02 => {
            out.item = Some(Box::new(parser_ast::MODE_IN))
        }
        MiscRule::SpOptInoutAlt03 => out.item = Some(Box::new(parser_ast::MODE_OUT)),
        MiscRule::SpOptInoutAlt04 => out.item = Some(Box::new(parser_ast::MODE_INOUT)),
        MiscRule::DropProcedureStmtAlt01 => {
            let Some(name) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            out.statement = Some(Box::new(parser_ast::DropProcedureStmt {
                node_text: Default::default(),
                IfExists: rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
                ProcedureName: name,
            }));
        }
    }
    Ok(true)
}
