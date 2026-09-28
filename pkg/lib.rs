// Copyright 2026 AsterSQL.

#![allow(non_snake_case, non_upper_case_globals)]

#[cfg(test)]
mod lib_test;

#[path = "../build/config.rs"]
pub mod build;

#[cfg(test)]
#[path = "../build/config_test.rs"]
mod build_config_test;

#[cfg(test)]
#[path = "../build/linter/allrevive/analyzer_test.rs"]
mod build_linter_allrevive_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/assertionapi/analyzer_test.rs"]
mod build_linter_assertionapi_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/bootstrap/analyzer_test.rs"]
mod build_linter_bootstrap_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/constructor/analyzer_test.rs"]
mod build_linter_constructor_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/copyloopvar/analyzer_test.rs"]
mod build_linter_copyloopvar_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/deferrecover/analyzer_test.rs"]
mod build_linter_deferrecover_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/durationcheck/analyzer_test.rs"]
mod build_linter_durationcheck_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/errcheck/analyzer_test.rs"]
mod build_linter_errcheck_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/etcdconfig/analyzer_test.rs"]
mod build_linter_etcdconfig_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/filepermission/checker_test.rs"]
mod build_linter_filepermission_checker_test;

#[cfg(test)]
#[path = "../build/linter/forbidigo/analyzer_test.rs"]
mod build_linter_forbidigo_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/forcetypeassert/analysis_test.rs"]
mod build_linter_forcetypeassert_analysis_test;

#[cfg(test)]
#[path = "../build/linter/gci/analysis_test.rs"]
mod build_linter_gci_analysis_test;

#[cfg(test)]
#[path = "../build/linter/gofmt/analyzer_test.rs"]
mod build_linter_gofmt_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/gosec/analysis_test.rs"]
mod build_linter_gosec_analysis_test;

#[cfg(test)]
#[path = "../build/linter/ineffassign/analyzer_test.rs"]
mod build_linter_ineffassign_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/intrange/analyzer_test.rs"]
mod build_linter_intrange_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/linter_test.rs"]
mod build_linter_linter_test;

#[cfg(test)]
#[path = "../build/linter/lll/analyzer_test.rs"]
mod build_linter_lll_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/makezero/analyzer_test.rs"]
mod build_linter_makezero_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/mirror/analyzer_test.rs"]
mod build_linter_mirror_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/misspell/analyzer_test.rs"]
mod build_linter_misspell_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/prealloc/analyzer_test.rs"]
mod build_linter_prealloc_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/predeclared/analysis_test.rs"]
mod build_linter_predeclared_analysis_test;

#[cfg(test)]
#[path = "../build/linter/printexpression/analyzer_test.rs"]
mod build_linter_printexpression_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/revive/analyzer_test.rs"]
mod build_linter_revive_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/rowserrcheck/analyzer_test.rs"]
mod build_linter_rowserrcheck_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/staticcheck/analyzer_test.rs"]
mod build_linter_staticcheck_analyzer_test;

#[cfg(test)]
#[path = "../build/linter/toomanytests/analyze_test.rs"]
mod build_linter_toomanytests_analyze_test;

#[cfg(test)]
#[path = "../build/linter/util/exclude_test.rs"]
mod build_linter_util_exclude_test;

#[cfg(test)]
#[path = "../build/linter/util/util_test.rs"]
mod build_linter_util_util_test;

#[cfg(test)]
#[path = "../build/linter/unconvert/analysis_test.rs"]
mod build_linter_unconvert_analysis_test;

// Public compatibility façade over canonical AsterSQL packages.
//
// `pkg` 根 crate：将各 canonical AsterSQL 包以与 Go `pkg/...` 路径相近的模块树再导出，
// 供迁移期兼容依赖与集成测试使用。子模块多为 `pub use facade_*::*` 门面转发。

/// 自动递增（auto-increment / auto-random）ID 分配服务门面。
pub mod autoid_service {
    pub use facade_autoid_service::*;
}
/// SQL 绑定（plan binding）信息管理门面。
pub mod bindinfo {
    pub use facade_bindinfo::*;
    pub mod tests {
        pub use facade_bindinfo_tests::*;
    }
}
/// 全局配置、部署模式与内核类型（classic / nextgen）门面。
pub mod config {
    pub use facade_config::*;
    pub mod configtypes {
        pub use facade_config_configtypes::*;
    }
    pub mod deploymode {
        pub use facade_config_deploymode::*;
    }
    pub mod kerneltype {
        pub use facade_config_kerneltype::*;
    }
}
/// DDL（数据定义语言）执行、作业与相关测试子模块门面。
pub mod ddl {
    pub use facade_ddl::*;
    pub mod bdr {
        pub use facade_ddl_bdr::*;
    }
    pub mod copr {
        pub use facade_ddl_copr::*;
    }
    pub mod ingest {
        pub use facade_ddl_ingest::*;
        pub mod testutil {
            pub use facade_ddl_ingest_testutil::*;
        }
    }
    pub mod jobsubmit {
        pub use facade_ddl_jobsubmit::*;
    }
    pub mod label {
        pub use facade_ddl_label::*;
    }
    pub mod logutil {
        pub use facade_ddl_logutil::*;
    }
    pub mod mock {
        pub use facade_ddl_mock::*;
    }
    pub mod notifier {
        pub use facade_ddl_notifier::*;
    }
    pub mod placement {
        pub use facade_ddl_placement::*;
    }
    pub mod resourcegroup {
        pub use facade_ddl_resourcegroup::*;
    }
    pub mod schematracker {
        pub use facade_ddl_schematracker::*;
    }
    pub mod schemaver {
        pub use facade_ddl_schemaver::*;
    }
    pub mod serverstate {
        pub use facade_ddl_serverstate::*;
    }
    pub mod session {
        pub use facade_ddl_session::*;
    }
    pub mod systable {
        pub use facade_ddl_systable::*;
    }
    pub mod testargsv1 {
        pub use facade_ddl_testargsv1::*;
    }
    pub mod tests {
        pub mod adminpause {
            pub use facade_ddl_tests_adminpause::*;
        }
        pub mod fail {
            pub use facade_ddl_tests_fail::*;
        }
        pub mod fastcreatetable {
            pub use facade_ddl_tests_fastcreatetable::*;
        }
        pub mod fk {
            pub use facade_ddl_tests_fk::*;
        }
        pub mod indexmerge {
            pub use facade_ddl_tests_indexmerge::*;
        }
        pub mod metadatalock {
            pub use facade_ddl_tests_metadatalock::*;
        }
        pub mod multivaluedindex {
            pub use facade_ddl_tests_multivaluedindex::*;
        }
        pub mod partition {
            pub use facade_ddl_tests_partition::*;
        }
        pub mod serial {
            pub use facade_ddl_tests_serial::*;
        }
        pub mod tiflash {
            pub use facade_ddl_tests_tiflash::*;
        }
    }
    pub mod testutil {
        pub use facade_ddl_testutil::*;
    }
    pub mod util {
        pub use facade_ddl_util::*;
    }
}
/// 分布式 SQL（DistSQL）请求构建与上下文门面。
pub mod distsql {
    pub use facade_distsql::*;
    pub mod context {
        pub use facade_distsql_context::*;
    }
}
/// Domain：实例级元数据、信息同步与后台服务门面。
pub mod domain {
    pub use facade_domain::*;
    pub mod affinity {
        pub use facade_domain_affinity::*;
    }
    pub mod crossks {
        pub use facade_domain_crossks::*;
    }
    pub mod globalconfigsync {
        pub use facade_domain_globalconfigsync::*;
    }
    pub mod infosync {
        pub use facade_domain_infosync::*;
    }
    pub mod metrics {
        pub use facade_domain_metrics::*;
    }
    pub mod serverinfo {
        pub use facade_domain_serverinfo::*;
    }
    pub mod sqlsvrapi {
        pub use facade_domain_sqlsvrapi::*;
        pub mod mock {
            pub use facade_domain_sqlsvrapi_mock::*;
        }
    }
}
/// 数据导出格式（如 Parquet）解析与测试工具门面。
pub mod dumpformat {
    pub mod parquetfile {
        pub use facade_dumpformat_parquetfile::*;
    }
    pub mod parsedef {
        pub use facade_dumpformat_parsedef::*;
    }
    pub mod testutils {
        pub use facade_dumpformat_testutils::*;
    }
}
/// DXF（分布式执行框架）示例、调度与计量门面。
pub mod dxf {
    pub mod example {
        pub use facade_dxf_example::*;
    }
    pub mod framework {
        pub use facade_dxf_framework::*;
        pub mod dxfmetric {
            pub use facade_dxf_framework_dxfmetric::*;
        }
        pub mod dxfutil {
            pub use facade_dxf_framework_dxfutil::*;
        }
        pub mod handle {
            pub use facade_dxf_framework_handle::*;
        }
        pub mod integrationtests {
            pub use facade_dxf_framework_integrationtests::*;
        }
        pub mod metering {
            pub use facade_dxf_framework_metering::*;
        }
        pub mod mock {
            pub use facade_dxf_framework_mock::*;
            pub mod execute {
                pub use facade_dxf_framework_mock_execute::*;
            }
        }
        pub mod planner {
            pub use facade_dxf_framework_planner::*;
        }
        pub mod proto {
            pub use facade_dxf_framework_proto::*;
        }
        pub mod scheduler {
            pub use facade_dxf_framework_scheduler::*;
            pub mod mock {
                pub use facade_dxf_framework_scheduler_mock::*;
            }
        }
        pub mod schstatus {
            pub use facade_dxf_framework_schstatus::*;
        }
        pub mod storage {
            pub use facade_dxf_framework_storage::*;
        }
        pub mod taskexecutor {
            pub use facade_dxf_framework_taskexecutor::*;
            pub mod execute {
                pub use facade_dxf_framework_taskexecutor_execute::*;
            }
        }
        pub mod testutil {
            pub use facade_dxf_framework_testutil::*;
        }
    }
    pub mod importinto {
        pub use facade_dxf_importinto::*;
        pub mod conflictedkv {
            pub use facade_dxf_importinto_conflictedkv::*;
        }
        pub mod jobhistory {
            pub use facade_dxf_importinto_jobhistory::*;
        }
        pub mod mock {
            pub use facade_dxf_importinto_mock::*;
        }
        pub mod taskkey {
            pub use facade_dxf_importinto_taskkey::*;
        }
    }
    pub mod operator {
        pub use facade_dxf_operator::*;
    }
}
/// 错误上下文（error context）传播门面。
pub mod errctx {
    pub use facade_errctx::*;
}
/// MySQL / TiDB 错误号常量门面。
pub mod errno {
    pub use facade_errno::*;
}
/// 通用错误类型与构造门面。
pub mod errors {
    pub use facade_errors::*;
}
/// 执行器（executor）：物理计划算子执行门面。
pub mod executor {
    pub use facade_executor::*;
    pub mod aggfuncs {
        pub use facade_executor_aggfuncs::*;
    }
    pub mod aggregate {
        pub use facade_executor_aggregate::*;
    }
    pub mod importer {
        pub use facade_executor_importer::*;
    }
    pub mod internal {
        pub mod applycache {
            pub use facade_executor_internal_applycache::*;
        }
        pub mod builder {
            pub use facade_executor_internal_builder::*;
        }
        pub mod calibrateresource {
            pub use facade_executor_internal_calibrateresource::*;
        }
        pub mod exec {
            pub use facade_executor_internal_exec::*;
        }
        pub mod mpp {
            pub use facade_executor_internal_mpp::*;
        }
        pub mod pdhelper {
            pub use facade_executor_internal_pdhelper::*;
        }
        pub mod querywatch {
            pub use facade_executor_internal_querywatch::*;
        }
        pub mod testutil {
            pub use facade_executor_internal_testutil::*;
        }
        pub mod util {
            pub use facade_executor_internal_util::*;
        }
        pub mod vecgroupchecker {
            pub use facade_executor_internal_vecgroupchecker::*;
        }
    }
    pub mod join {
        pub use facade_executor_join::*;
        pub mod joinversion {
            pub use facade_executor_join_joinversion::*;
        }
        pub mod test {
            pub mod indexjoin {
                pub use facade_executor_join_test_indexjoin::*;
            }
            pub mod mergejoin {
                pub use facade_executor_join_test_mergejoin::*;
            }
        }
    }
    pub mod lockstats {
        pub use facade_executor_lockstats::*;
    }
    pub mod metrics {
        pub use facade_executor_metrics::*;
    }
    pub mod mppcoordmanager {
        pub use facade_executor_mppcoordmanager::*;
    }
    pub mod sortexec {
        pub use facade_executor_sortexec::*;
    }
    pub mod staticrecordset {
        pub use facade_executor_staticrecordset::*;
    }
    pub mod test {
        pub mod admintest {
            pub use facade_executor_test_admintest::*;
        }
        pub mod aggregate {
            pub use facade_executor_test_aggregate::*;
        }
        pub mod analyzetest {
            pub use facade_executor_test_analyzetest::*;
            pub mod columns {
                pub use facade_executor_test_analyzetest_columns::*;
            }
            pub mod memorycontrol {
                pub use facade_executor_test_analyzetest_memorycontrol::*;
            }
            pub mod options {
                pub use facade_executor_test_analyzetest_options::*;
            }
            pub mod panictest {
                pub use facade_executor_test_analyzetest_panictest::*;
            }
        }
        pub mod autoidtest {
            pub use facade_executor_test_autoidtest::*;
        }
        pub mod cte {
            pub use facade_executor_test_cte::*;
        }
        pub mod ddl {
            pub use facade_executor_test_ddl::*;
        }
        pub mod distsqltest {
            pub use facade_executor_test_distsqltest::*;
        }
        pub mod executor {
            pub use facade_executor_test_executor::*;
        }
        pub mod fktest {
            pub use facade_executor_test_fktest::*;
        }
        pub mod indexmergereadtest {
            pub use facade_executor_test_indexmergereadtest::*;
        }
        pub mod infoschema {
            pub use facade_executor_test_infoschema::*;
        }
        pub mod issuetest {
            pub use facade_executor_test_issuetest::*;
        }
        pub mod jointest {
            pub use facade_executor_test_jointest::*;
            pub mod hashjoin {
                pub use facade_executor_test_jointest_hashjoin::*;
            }
        }
        pub mod loaddatatest {
            pub use facade_executor_test_loaddatatest::*;
        }
        pub mod loadremotetest {
            pub use facade_executor_test_loadremotetest::*;
        }
        pub mod memtest {
            pub use facade_executor_test_memtest::*;
        }
        pub mod oomtest {
            pub use facade_executor_test_oomtest::*;
        }
        pub mod passwordtest {
            pub use facade_executor_test_passwordtest::*;
        }
        pub mod plancache {
            pub use facade_executor_test_plancache::*;
        }
        pub mod planreplayer {
            pub use facade_executor_test_planreplayer::*;
        }
        pub mod recovertest {
            pub use facade_executor_test_recovertest::*;
        }
        pub mod seqtest {
            pub use facade_executor_test_seqtest::*;
        }
        pub mod showtest {
            pub use facade_executor_test_showtest::*;
        }
        pub mod simpletest {
            pub use facade_executor_test_simpletest::*;
        }
        pub mod splittest {
            pub use facade_executor_test_splittest::*;
        }
        pub mod tiflashtest {
            pub use facade_executor_test_tiflashtest::*;
        }
        pub mod txn {
            pub use facade_executor_test_txn::*;
        }
        pub mod unstabletest {
            pub use facade_executor_test_unstabletest::*;
        }
        pub mod writetest {
            pub use facade_executor_test_writetest::*;
        }
    }
    pub mod unionexec {
        pub use facade_executor_unionexec::*;
    }
    pub mod windows {
        pub use facade_executor_windows::*;
    }
}
/// 表达式求值与内建函数门面。
pub mod expression {
    pub use facade_expression::*;
    pub mod aggregation {
        pub use facade_expression_aggregation::*;
    }
    pub mod exprctx {
        pub use facade_expression_exprctx::*;
    }
    pub mod expropt {
        pub use facade_expression_expropt::*;
    }
    pub mod exprstatic {
        pub use facade_expression_exprstatic::*;
    }
    pub mod integration_test {
        pub use facade_expression_integration_test::*;
    }
    pub mod sessionexpr {
        pub use facade_expression_sessionexpr::*;
    }
    pub mod test {
        pub mod constantpropagation {
            pub use facade_expression_test_constantpropagation::*;
        }
        pub mod multivaluedindex {
            pub use facade_expression_test_multivaluedindex::*;
        }
    }
}
/// 扩展点注册与加载门面。
pub mod extension {
    pub use facade_extension::*;
    pub mod extensionimpl {
        pub use facade_extension_extensionimpl::*;
    }
}
/// 外部工作负载客户端门面。
pub mod extworkload {
    pub use facade_extworkload::*;
    pub mod client {
        pub use facade_extworkload_client::*;
    }
}
/// 结果集等格式化输出门面。
pub mod format {
    pub mod textrow {
        pub use facade_format_textrow::*;
    }
}
/// 数据导入 SDK 门面。
pub mod importsdk {
    pub use facade_importsdk::*;
    pub mod mock {
        pub use facade_importsdk_mock::*;
    }
}
/// 信息模式（Information Schema）目录视图门面。
pub mod infoschema {
    pub use facade_infoschema::*;
    pub mod context {
        pub use facade_infoschema_context::*;
    }
    pub mod internal {
        pub use facade_infoschema_internal::*;
    }
    pub mod issyncer {
        pub use facade_infoschema_issyncer::*;
        pub mod mdldef {
            pub use facade_infoschema_issyncer_mdldef::*;
        }
    }
    pub mod isvalidator {
        pub use facade_infoschema_isvalidator::*;
    }
    pub mod metrics {
        pub use facade_infoschema_metrics::*;
    }
    pub mod perfschema {
        pub use facade_infoschema_perfschema::*;
    }
    pub mod test {
        pub mod cachetest {
            pub use facade_infoschema_test_cachetest::*;
        }
        pub mod clustertablestest {
            pub use facade_infoschema_test_clustertablestest::*;
        }
        pub mod infoschemav2test {
            pub use facade_infoschema_test_infoschemav2test::*;
        }
    }
    pub mod validatorapi {
        pub use facade_infoschema_validatorapi::*;
    }
}
/// 数据摄取（ingest）管道门面。
pub mod ingestor {
    pub use facade_ingestor::*;
    pub mod engineapi {
        pub use facade_ingestor_engineapi::*;
    }
    pub mod errdef {
        pub use facade_ingestor_errdef::*;
    }
    pub mod globalsort {
        pub use facade_ingestor_globalsort::*;
    }
    pub mod ingestcli {
        pub use facade_ingestor_ingestcli::*;
        pub mod mock {
            pub use facade_ingestor_ingestcli_mock::*;
        }
    }
    pub mod ingestctrl {
        pub use facade_ingestor_ingestctrl::*;
    }
    pub mod ingestmetric {
        pub use facade_ingestor_ingestmetric::*;
    }
    pub mod simplesst {
        pub use facade_ingestor_simplesst::*;
    }
    pub mod testutils {
        pub use facade_ingestor_testutils::*;
    }
}
/// Keyspace（键空间）多租户隔离工具门面。
pub mod keyspace {
    pub use facade_keyspace::*;
}
/// KV 事务、键值抽象与存储适配门面。
pub mod kv {
    pub use facade_kv::*;
}
/// Lightning 高速导入工具相关模块门面。
pub mod lightning {
    pub mod backend {
        pub use facade_lightning_backend::*;
        pub mod encode {
            pub use facade_lightning_backend_encode::*;
        }
        pub mod kv {
            pub use facade_lightning_backend_kv::*;
        }
        pub mod tidb {
            pub use facade_lightning_backend_tidb::*;
        }
    }
    pub mod common {
        pub use facade_lightning_common::*;
    }
    pub mod config {
        pub use facade_lightning_config::*;
    }
    pub mod duplicate {
        pub use facade_lightning_duplicate::*;
    }
    pub mod importdef {
        pub use facade_lightning_importdef::*;
    }
    pub mod log {
        pub use facade_lightning_log::*;
    }
    pub mod manual {
        pub use facade_lightning_manual::*;
    }
    pub mod membuf {
        pub use facade_lightning_membuf::*;
    }
    pub mod metric {
        pub use facade_lightning_metric::*;
    }
    pub mod mydump {
        pub use facade_lightning_mydump::*;
    }
    pub mod tikv {
        pub use facade_lightning_tikv::*;
    }
    pub mod verification {
        pub use facade_lightning_verification::*;
    }
    pub mod worker {
        pub use facade_lightning_worker::*;
    }
}
/// 锁相关工具门面。
pub mod lock {
    pub use facade_lock::*;
    pub mod context {
        pub use facade_lock_context::*;
    }
}
/// 元数据读写（schema meta）门面。
pub mod meta {
    pub use facade_meta::*;
    pub mod autoid {
        pub use facade_meta_autoid::*;
    }
    pub mod metabuild {
        pub use facade_meta_metabuild::*;
    }
    pub mod metadef {
        pub use facade_meta_metadef::*;
    }
    pub mod model {
        pub use facade_meta_model::*;
    }
    pub mod tidbvar {
        pub use facade_meta_tidbvar::*;
    }
}
/// 元数据服务客户端门面。
pub mod metaservice {
    pub use facade_metaservice::*;
}
/// 监控指标定义与注册门面。
pub mod metrics {
    pub use facade_metrics::*;
    pub mod common {
        pub use facade_metrics_common::*;
    }
}
/// 对象存储抽象门面。
pub mod objstore {
    pub use facade_objstore::*;
    pub mod compressedio {
        pub use facade_objstore_compressedio::*;
    }
    pub mod mockobjstore {
        pub use facade_objstore_mockobjstore::*;
    }
    pub mod objectio {
        pub use facade_objstore_objectio::*;
    }
    pub mod ossstore {
        pub use facade_objstore_ossstore::*;
        pub mod mock {
            pub use facade_objstore_ossstore_mock::*;
        }
    }
    pub mod recording {
        pub use facade_objstore_recording::*;
    }
    pub mod s3like {
        pub use facade_objstore_s3like::*;
        pub mod mock {
            pub use facade_objstore_s3like_mock::*;
        }
    }
    pub mod s3store {
        pub use facade_objstore_s3store::*;
        pub mod mock {
            pub use facade_objstore_s3store_mock::*;
        }
    }
    pub mod storeapi {
        pub use facade_objstore_storeapi::*;
    }
}
/// Owner（主节点）竞选与管理门面。
pub mod owner {
    pub use facade_owner::*;
}
/// 参数标记与绑定门面。
pub mod param {
    pub use facade_param::*;
}
/// SQL 解析器及 mysql 方言子模块门面。
pub mod parser {
    pub use facade_parser::*;
    pub mod ast {
        pub use facade_parser_ast::*;
    }
    pub mod auth {
        pub use facade_parser_auth::*;
    }
    pub mod charset {
        pub use facade_parser_charset::*;
    }
    pub mod duration {
        pub use facade_parser_duration::*;
    }
    pub mod format {
        pub use facade_parser_format::*;
    }
    pub mod mysql {
        pub use facade_parser_mysql::*;
    }
    pub mod opcode {
        pub use facade_parser_opcode::*;
    }
    pub mod terror {
        pub use facade_parser_terror::*;
    }
    pub mod test_driver {
        pub use facade_parser_test_driver::*;
    }
    pub mod tidb {
        pub use facade_parser_tidb::*;
    }
    pub mod types {
        pub use facade_parser_types::*;
    }
    pub mod util {
        pub use facade_parser_util::*;
    }
}
/// 优化器 / 执行计划（logical & physical plan）门面。
pub mod planner {
    pub use facade_planner::*;
    pub mod cardinality {
        pub use facade_planner_cardinality::*;
    }
    pub mod cascades {
        pub use facade_planner_cascades::*;
        pub mod base {
            pub use facade_planner_cascades_base::*;
            pub mod cascadesctx {
                pub use facade_planner_cascades_base_cascadesctx::*;
            }
        }
        pub mod r#impl {
            pub use facade_planner_cascades_impl::*;
        }
        pub mod memo {
            pub use facade_planner_cascades_memo::*;
        }
        pub mod old {
            pub use facade_planner_cascades_old::*;
        }
        pub mod pattern {
            pub use facade_planner_cascades_pattern::*;
        }
        pub mod rule {
            pub use facade_planner_cascades_rule::*;
            pub mod apply {
                pub mod decorrelateapply {
                    pub use facade_planner_cascades_rule_apply_decorrelateapply::*;
                }
            }
            pub mod join {
                pub use facade_planner_cascades_rule_join::*;
            }
            pub mod ruleset {
                pub use facade_planner_cascades_rule_ruleset::*;
            }
        }
        pub mod task {
            pub use facade_planner_cascades_task::*;
        }
        pub mod util {
            pub use facade_planner_cascades_util::*;
        }
    }
    pub mod core {
        pub use facade_planner_core::*;
        pub mod access {
            pub use facade_planner_core_access::*;
        }
        pub mod base {
            pub use facade_planner_core_base::*;
        }
        pub mod casetest {
            pub use facade_planner_core_casetest::*;
            pub mod binaryplan {
                pub use facade_planner_core_casetest_binaryplan::*;
            }
            pub mod cascades {
                pub use facade_planner_core_casetest_cascades::*;
            }
            pub mod cbotest {
                pub use facade_planner_core_casetest_cbotest::*;
            }
            pub mod ch {
                pub use facade_planner_core_casetest_ch::*;
            }
            pub mod correlated {
                pub use facade_planner_core_casetest_correlated::*;
            }
            pub mod dag {
                pub use facade_planner_core_casetest_dag::*;
            }
            pub mod enforcempp {
                pub use facade_planner_core_casetest_enforcempp::*;
            }
            pub mod flatplan {
                pub use facade_planner_core_casetest_flatplan::*;
            }
            pub mod hint {
                pub use facade_planner_core_casetest_hint::*;
            }
            pub mod index {
                pub use facade_planner_core_casetest_index::*;
            }
            pub mod indexmerge {
                pub use facade_planner_core_casetest_indexmerge::*;
            }
            pub mod instanceplancache {
                pub use facade_planner_core_casetest_instanceplancache::*;
            }
            pub mod join {
                pub use facade_planner_core_casetest_join::*;
            }
            pub mod logicalplan {
                pub use facade_planner_core_casetest_logicalplan::*;
            }
            pub mod mpp {
                pub use facade_planner_core_casetest_mpp::*;
            }
            pub mod parallelapply {
                pub use facade_planner_core_casetest_parallelapply::*;
            }
            pub mod partition {
                pub use facade_planner_core_casetest_partition::*;
            }
            pub mod physicalplantest {
                pub use facade_planner_core_casetest_physicalplantest::*;
            }
            pub mod plancache {
                pub use facade_planner_core_casetest_plancache::*;
            }
            pub mod planstats {
                pub use facade_planner_core_casetest_planstats::*;
            }
            pub mod pushdown {
                pub use facade_planner_core_casetest_pushdown::*;
            }
            pub mod rule {
                pub use facade_planner_core_casetest_rule::*;
            }
            pub mod scalarsubquery {
                pub use facade_planner_core_casetest_scalarsubquery::*;
            }
            pub mod schema {
                pub use facade_planner_core_casetest_schema::*;
            }
            pub mod tpcds {
                pub use facade_planner_core_casetest_tpcds::*;
            }
            pub mod tpch {
                pub use facade_planner_core_casetest_tpch::*;
            }
            pub mod vectorsearch {
                pub use facade_planner_core_casetest_vectorsearch::*;
            }
            pub mod windows {
                pub use facade_planner_core_casetest_windows::*;
            }
        }
        pub mod constraint {
            pub use facade_planner_core_constraint::*;
        }
        pub mod cost {
            pub use facade_planner_core_cost::*;
        }
        pub mod generator {
            pub mod hash64_equals {
                pub use facade_planner_core_generator_hash64_equals::*;
            }
            pub mod plan_cache {
                pub use facade_planner_core_generator_plan_cache::*;
            }
            pub mod shallow_ref {
                pub use facade_planner_core_generator_shallow_ref::*;
            }
        }
        pub mod issuetest {
            pub use facade_planner_core_issuetest::*;
        }
        pub mod joinorder {
            pub use facade_planner_core_joinorder::*;
        }
        pub mod metrics {
            pub use facade_planner_core_metrics::*;
        }
        pub mod operator {
            pub mod baseimpl {
                pub use facade_planner_core_operator_baseimpl::*;
            }
            pub mod logicalop {
                pub use facade_planner_core_operator_logicalop::*;
                pub mod logicalop_test {
                    pub use facade_planner_core_operator_logicalop_logicalop_test::*;
                }
            }
            pub mod physicalop {
                pub use facade_planner_core_operator_physicalop::*;
            }
        }
        pub mod partidx {
            pub use facade_planner_core_partidx::*;
        }
        pub mod resolve {
            pub use facade_planner_core_resolve::*;
        }
        pub mod rule {
            pub use facade_planner_core_rule::*;
            pub mod util {
                pub use facade_planner_core_rule_util::*;
            }
        }
        pub mod stats {
            pub use facade_planner_core_stats::*;
        }
        pub mod tests {
            pub mod analyze {
                pub use facade_planner_core_tests_analyze::*;
            }
            pub mod cte {
                pub use facade_planner_core_tests_cte::*;
            }
            pub mod extractor {
                pub use facade_planner_core_tests_extractor::*;
            }
            pub mod null {
                pub use facade_planner_core_tests_null::*;
            }
            pub mod partition {
                pub use facade_planner_core_tests_partition::*;
            }
            pub mod pointget {
                pub use facade_planner_core_tests_pointget::*;
            }
            pub mod prepare {
                pub use facade_planner_core_tests_prepare::*;
            }
            pub mod redact {
                pub use facade_planner_core_tests_redact::*;
            }
            pub mod rewriter {
                pub use facade_planner_core_tests_rewriter::*;
            }
            pub mod subquery {
                pub use facade_planner_core_tests_subquery::*;
            }
        }
    }
    pub mod extstore {
        pub use facade_planner_extstore::*;
    }
    pub mod funcdep {
        pub use facade_planner_funcdep::*;
    }
    pub mod implementation {
        pub use facade_planner_implementation::*;
    }
    pub mod indexadvisor {
        pub use facade_planner_indexadvisor::*;
    }
    pub mod memo {
        pub use facade_planner_memo::*;
    }
    pub mod planctx {
        pub use facade_planner_planctx::*;
    }
    pub mod plannersession {
        pub use facade_planner_plannersession::*;
    }
    pub mod property {
        pub use facade_planner_property::*;
    }
    pub mod util {
        pub use facade_planner_util::*;
        pub mod coretestsdk {
            pub use facade_planner_util_coretestsdk::*;
        }
        pub mod coreusage {
            pub use facade_planner_util_coreusage::*;
        }
        pub mod costusage {
            pub use facade_planner_util_costusage::*;
        }
        pub mod domainmisc {
            pub use facade_planner_util_domainmisc::*;
        }
        pub mod fixcontrol {
            pub use facade_planner_util_fixcontrol::*;
        }
        pub mod partitionpruning {
            pub use facade_planner_util_partitionpruning::*;
        }
        pub mod tablesampler {
            pub use facade_planner_util_tablesampler::*;
        }
        pub mod utilfuncp {
            pub use facade_planner_util_utilfuncp::*;
        }
    }
}
/// 插件加载与管理门面。
pub mod plugin {
    pub use facade_plugin::*;
    pub mod conn_ip_example {
        pub use facade_plugin_conn_ip_example::*;
    }
}
/// 权限检查与管理门面。
pub mod privilege {
    pub use facade_privilege::*;
    pub mod conn {
        pub use facade_privilege_conn::*;
    }
    pub mod privileges {
        pub use facade_privilege_privileges::*;
        pub mod ldap {
            pub use facade_privilege_privileges_ldap::*;
        }
    }
}
/// 资源组（Resource Group）门面。
pub mod resourcegroup {
    pub use facade_resourcegroup::*;
    pub mod runaway {
        pub use facade_resourcegroup_runaway::*;
    }
    pub mod tests {
        pub use facade_resourcegroup_tests::*;
    }
}
/// 资源管理器门面。
pub mod resourcemanager {
    pub use facade_resourcemanager::*;
    pub mod pool {
        pub use facade_resourcemanager_pool::*;
        pub mod spool {
            pub use facade_resourcemanager_pool_spool::*;
        }
        pub mod workerpool {
            pub use facade_resourcemanager_pool_workerpool::*;
        }
    }
    pub mod poolmanager {
        pub use facade_resourcemanager_poolmanager::*;
    }
    pub mod scheduler {
        pub use facade_resourcemanager_scheduler::*;
    }
    pub mod util {
        pub use facade_resourcemanager_util::*;
    }
}
/// SQL 协议服务端门面。
pub mod server {
    pub use facade_server::*;
    pub mod err {
        pub use facade_server_err::*;
    }
    pub mod handler {
        pub use facade_server_handler::*;
        pub mod extractorhandler {
            pub use facade_server_handler_extractorhandler::*;
        }
        pub mod optimizor {
            pub use facade_server_handler_optimizor::*;
        }
        pub mod tests {
            pub use facade_server_handler_tests::*;
        }
        pub mod tikvhandler {
            pub use facade_server_handler_tikvhandler::*;
        }
        pub mod ttlhandler {
            pub use facade_server_handler_ttlhandler::*;
        }
    }
    pub mod internal {
        pub use facade_server_internal::*;
        pub mod column {
            pub use facade_server_internal_column::*;
        }
        pub mod dump {
            pub use facade_server_internal_dump::*;
        }
        pub mod handshake {
            pub use facade_server_internal_handshake::*;
        }
        pub mod parse {
            pub use facade_server_internal_parse::*;
        }
        pub mod resultset {
            pub use facade_server_internal_resultset::*;
        }
        pub mod testserverclient {
            pub use facade_server_internal_testserverclient::*;
        }
        pub mod testutil {
            pub use facade_server_internal_testutil::*;
        }
        pub mod util {
            pub use facade_server_internal_util::*;
        }
    }
    pub mod metrics {
        pub use facade_server_metrics::*;
    }
    pub mod tests {
        pub use facade_server_tests::*;
        pub mod commontest {
            pub use facade_server_tests_commontest::*;
        }
        pub mod cursor {
            pub use facade_server_tests_cursor::*;
        }
        pub mod servertestkit {
            pub use facade_server_tests_servertestkit::*;
        }
        pub mod standby {
            pub use facade_server_tests_standby::*;
        }
        pub mod tls {
            pub use facade_server_tests_tls::*;
        }
    }
}
/// 会话生命周期与语句执行门面。
pub mod session {
    pub use facade_session::*;
    pub mod cursor {
        pub use facade_session_cursor::*;
    }
    pub mod metrics {
        pub use facade_session_metrics::*;
    }
    pub mod sessionapi {
        pub use facade_session_sessionapi::*;
    }
    pub mod sessmgr {
        pub use facade_session_sessmgr::*;
    }
    pub mod syssession {
        pub use facade_session_syssession::*;
    }
    pub mod test {
        pub use facade_session_test::*;
        pub mod bootstraptest {
            pub use facade_session_test_bootstraptest::*;
        }
        pub mod bootstraptest2 {
            pub use facade_session_test_bootstraptest2::*;
        }
        pub mod clusteredindextest {
            pub use facade_session_test_clusteredindextest::*;
        }
        pub mod common {
            pub use facade_session_test_common::*;
        }
        pub mod meta {
            pub use facade_session_test_meta::*;
        }
        pub mod nontransactionaltest {
            pub use facade_session_test_nontransactionaltest::*;
        }
        pub mod privileges {
            pub use facade_session_test_privileges::*;
        }
        pub mod resourcegrouptest {
            pub use facade_session_test_resourcegrouptest::*;
        }
        pub mod schematest {
            pub use facade_session_test_schematest::*;
        }
        pub mod temporarytabletest {
            pub use facade_session_test_temporarytabletest::*;
        }
        pub mod txn {
            pub use facade_session_test_txn::*;
        }
        pub mod variable {
            pub use facade_session_test_variable::*;
        }
        pub mod vars {
            pub use facade_session_test_vars::*;
        }
    }
    pub mod txninfo {
        pub use facade_session_txninfo::*;
    }
}
/// 会话上下文（session context）变量与状态门面。
pub mod sessionctx {
    pub use facade_sessionctx::*;
    pub mod sessionstates {
        pub use facade_sessionctx_sessionstates::*;
    }
    pub mod slowlogrule {
        pub use facade_sessionctx_slowlogrule::*;
    }
    pub mod stmtctx {
        pub use facade_sessionctx_stmtctx::*;
    }
    pub mod sysproctrack {
        pub use facade_sessionctx_sysproctrack::*;
    }
    pub mod vardef {
        pub use facade_sessionctx_vardef::*;
    }
    pub mod variable {
        pub use facade_sessionctx_variable::*;
        pub mod tests {
            pub use facade_sessionctx_variable_tests::*;
            pub mod slowlog {
                pub use facade_sessionctx_variable_tests_slowlog::*;
            }
        }
    }
}
/// 会话事务管理门面。
pub mod sessiontxn {
    pub use facade_sessiontxn::*;
    pub mod internal {
        pub use facade_sessiontxn_internal::*;
    }
    pub mod isolation {
        pub use facade_sessiontxn_isolation::*;
        pub mod metrics {
            pub use facade_sessiontxn_isolation_metrics::*;
        }
    }
    pub mod staleread {
        pub use facade_sessiontxn_staleread::*;
    }
}
/// 备节点 / 待机相关门面。
pub mod standby {
    pub use facade_standby::*;
}
/// 统计信息收集与使用门面。
pub mod statistics {
    pub use facade_statistics::*;
    pub mod asyncload {
        pub use facade_statistics_asyncload::*;
    }
    pub mod handle {
        pub use facade_statistics_handle::*;
        pub mod autoanalyze {
            pub use facade_statistics_handle_autoanalyze::*;
            pub mod exec {
                pub use facade_statistics_handle_autoanalyze_exec::*;
            }
            pub mod priorityqueue {
                pub use facade_statistics_handle_autoanalyze_priorityqueue::*;
                pub mod calculatoranalysis {
                    pub use facade_statistics_handle_autoanalyze_priorityqueue_calculatoranalysis::*;
                }
                pub mod intervaltimezone {
                    pub use facade_statistics_handle_autoanalyze_priorityqueue_intervaltimezone::*;
                }
            }
            pub mod refresher {
                pub use facade_statistics_handle_autoanalyze_refresher::*;
            }
        }
        pub mod cache {
            pub use facade_statistics_handle_cache::*;
            pub mod internal {
                pub use facade_statistics_handle_cache_internal::*;
                pub mod lfu {
                    pub use facade_statistics_handle_cache_internal_lfu::*;
                }
                pub mod mapcache {
                    pub use facade_statistics_handle_cache_internal_mapcache::*;
                }
                pub mod testutil {
                    pub use facade_statistics_handle_cache_internal_testutil::*;
                }
            }
            pub mod metrics {
                pub use facade_statistics_handle_cache_metrics::*;
            }
        }
        pub mod ddl {
            pub use facade_statistics_handle_ddl::*;
            pub mod testutil {
                pub use facade_statistics_handle_ddl_testutil::*;
            }
        }
        pub mod globalstats {
            pub use facade_statistics_handle_globalstats::*;
        }
        pub mod handletest {
            pub use facade_statistics_handle_handletest::*;
            pub mod analyze {
                pub use facade_statistics_handle_handletest_analyze::*;
            }
            pub mod initstats {
                pub use facade_statistics_handle_handletest_initstats::*;
            }
            pub mod lockstats {
                pub use facade_statistics_handle_handletest_lockstats::*;
            }
            pub mod statstest {
                pub use facade_statistics_handle_handletest_statstest::*;
            }
        }
        pub mod history {
            pub use facade_statistics_handle_history::*;
        }
        pub mod initstats {
            pub use facade_statistics_handle_initstats::*;
        }
        pub mod internal {
            pub use facade_statistics_handle_internal::*;
        }
        pub mod lockstats {
            pub use facade_statistics_handle_lockstats::*;
        }
        pub mod logutil {
            pub use facade_statistics_handle_logutil::*;
        }
        pub mod metrics {
            pub use facade_statistics_handle_metrics::*;
        }
        pub mod storage {
            pub use facade_statistics_handle_storage::*;
        }
        pub mod syncload {
            pub use facade_statistics_handle_syncload::*;
        }
        pub mod types {
            pub use facade_statistics_handle_types::*;
        }
        pub mod updatetest {
            pub use facade_statistics_handle_updatetest::*;
        }
        pub mod usage {
            pub use facade_statistics_handle_usage::*;
            pub mod collector {
                pub use facade_statistics_handle_usage_collector::*;
            }
            pub mod indexusage {
                pub use facade_statistics_handle_usage_indexusage::*;
            }
            pub mod predicatecolumn {
                pub use facade_statistics_handle_usage_predicatecolumn::*;
            }
        }
        pub mod util {
            pub use facade_statistics_handle_util::*;
            pub mod test {
                pub use facade_statistics_handle_util_test::*;
            }
        }
    }
    pub mod util {
        pub use facade_statistics_util::*;
    }
}
/// 存储驱动与 TiKV 客户端封装门面。
pub mod store {
    pub use facade_store::*;
    pub mod copr {
        pub use facade_store_copr::*;
        pub mod copr_test {
            pub use facade_store_copr_copr_test::*;
        }
        pub mod metrics {
            pub use facade_store_copr_metrics::*;
        }
    }
    pub mod driver {
        pub use facade_store_driver::*;
        pub mod backoff {
            pub use facade_store_driver_backoff::*;
        }
        pub mod error {
            pub use facade_store_driver_error::*;
        }
        pub mod options {
            pub use facade_store_driver_options::*;
        }
        pub mod txn {
            pub use facade_store_driver_txn::*;
        }
    }
    pub mod gcworker {
        pub use facade_store_gcworker::*;
    }
    pub mod helper {
        pub use facade_store_helper::*;
    }
    pub mod mockstore {
        pub use facade_store_mockstore::*;
        pub mod mockcopr {
            pub use facade_store_mockstore_mockcopr::*;
        }
        pub mod mockstorage {
            pub use facade_store_mockstore_mockstorage::*;
        }
        pub mod teststore {
            pub use facade_store_mockstore_teststore::*;
        }
        pub mod unistore {
            pub use facade_store_mockstore_unistore::*;
            pub mod client {
                pub use facade_store_mockstore_unistore_client::*;
            }
            pub mod config {
                pub use facade_store_mockstore_unistore_config::*;
            }
            pub mod cophandler {
                pub use facade_store_mockstore_unistore_cophandler::*;
            }
            pub mod lockstore {
                pub use facade_store_mockstore_unistore_lockstore::*;
            }
            pub mod metrics {
                pub use facade_store_mockstore_unistore_metrics::*;
            }
            pub mod pd {
                pub use facade_store_mockstore_unistore_pd::*;
            }
            pub mod server {
                pub use facade_store_mockstore_unistore_server::*;
            }
            pub mod tikv {
                pub use facade_store_mockstore_unistore_tikv::*;
                pub mod dbreader {
                    pub use facade_store_mockstore_unistore_tikv_dbreader::*;
                }
                pub mod kverrors {
                    pub use facade_store_mockstore_unistore_tikv_kverrors::*;
                }
                pub mod mvcc {
                    pub use facade_store_mockstore_unistore_tikv_mvcc::*;
                }
                pub mod pberror {
                    pub use facade_store_mockstore_unistore_tikv_pberror::*;
                }
            }
            pub mod util {
                pub mod lockwaiter {
                    pub use facade_store_mockstore_unistore_util_lockwaiter::*;
                }
            }
        }
    }
    pub mod pdtypes {
        pub use facade_store_pdtypes::*;
    }
}
/// 结构化 KV 封装门面。
pub mod structure {
    pub use facade_structure::*;
}
/// 表抽象与读写接口门面。
pub mod table {
    pub use facade_table::*;
    pub mod tables {
        pub use facade_table_tables::*;
        pub mod test {
            pub mod partition {
                pub use facade_table_tables_test_partition::*;
            }
        }
        pub mod testutil {
            pub use facade_table_tables_testutil::*;
        }
    }
    pub mod tblctx {
        pub use facade_table_tblctx::*;
    }
    pub mod tblsession {
        pub use facade_table_tblsession::*;
    }
    pub mod temptable {
        pub use facade_table_temptable::*;
    }
}
/// 表数据编解码（table codec）门面。
pub mod tablecodec {
    pub use facade_tablecodec::*;
    pub mod rowindexcodec {
        pub use facade_tablecodec_rowindexcodec::*;
    }
}
/// 遥测上报门面。
pub mod telemetry {
    pub use facade_telemetry::*;
}
/// 集成测试工具包门面。
pub mod testkit {
    pub use facade_testkit::*;
    pub mod analyzehelper {
        pub use facade_testkit_analyzehelper::*;
    }
    pub mod ddlhelper {
        pub use facade_testkit_ddlhelper::*;
    }
    pub mod external {
        pub use facade_testkit_external::*;
    }
    pub mod testenv {
        pub use facade_testkit_testenv::*;
    }
    pub mod testfailpoint {
        pub use facade_testkit_testfailpoint::*;
    }
    pub mod testflag {
        pub use facade_testkit_testflag::*;
    }
    pub mod testfork {
        pub use facade_testkit_testfork::*;
    }
    pub mod testmain {
        pub use facade_testkit_testmain::*;
    }
    pub mod testsetup {
        pub use facade_testkit_testsetup::*;
    }
    pub mod testutil {
        pub use facade_testkit_testutil::*;
    }
}
/// TiDB 实例管理门面。
pub mod tidbmanager {
    pub use facade_tidbmanager::*;
}
/// 定时任务门面。
pub mod timer {
    pub use facade_timer::*;
    pub mod api {
        pub use facade_timer_api::*;
    }
    pub mod metrics {
        pub use facade_timer_metrics::*;
    }
    pub mod runtime {
        pub use facade_timer_runtime::*;
    }
    pub mod tablestore {
        pub use facade_timer_tablestore::*;
    }
}
/// TTL（生存时间）清理任务门面。
pub mod ttl {
    pub mod cache {
        pub use facade_ttl_cache::*;
    }
    pub mod client {
        pub use facade_ttl_client::*;
    }
    pub mod metrics {
        pub use facade_ttl_metrics::*;
    }
    pub mod session {
        pub use facade_ttl_session::*;
    }
    pub mod sqlbuilder {
        pub use facade_ttl_sqlbuilder::*;
    }
    pub mod ttlworker {
        pub use facade_ttl_ttlworker::*;
        pub mod integrationtest {
            pub use facade_ttl_ttlworker_integrationtest::*;
        }
    }
}
/// SQL 类型系统门面。
pub mod types {
    pub use facade_types::*;
    pub mod parser_driver {
        pub use facade_types_parser_driver::*;
    }
}
/// 通用工具库（切片、集合、格式化等）门面。
pub mod util {
    pub use facade_util::*;
    pub mod admin {
        pub use facade_util_admin::*;
    }
    pub mod arena {
        pub use facade_util_arena::*;
    }
    pub mod backoff {
        pub use facade_util_backoff::*;
    }
    pub mod benchdaily {
        pub use facade_util_benchdaily::*;
    }
    pub mod bitmap {
        pub use facade_util_bitmap::*;
    }
    pub mod breakpoint {
        pub use facade_util_breakpoint::*;
    }
    pub mod cdcutil {
        pub use facade_util_cdcutil::*;
    }
    pub mod cgmon {
        pub use facade_util_cgmon::*;
    }
    pub mod cgroup {
        pub use facade_util_cgroup::*;
    }
    pub mod channel {
        pub use facade_util_channel::*;
    }
    pub mod checksum {
        pub use facade_util_checksum::*;
    }
    pub mod chunk {
        pub use facade_util_chunk::*;
    }
    pub mod codec {
        pub use facade_util_codec::*;
    }
    pub mod collate {
        pub use facade_util_collate::*;
        pub mod ucadata {
            pub use facade_util_collate_ucadata::*;
            pub mod generator {
                pub use facade_util_collate_ucadata_generator::*;
            }
        }
        pub mod ucaimpl {
            pub use facade_util_collate_ucaimpl::*;
        }
    }
    pub mod column_mapping {
        pub use facade_util_column_mapping::*;
    }
    pub mod compress {
        pub use facade_util_compress::*;
    }
    pub mod config {
        pub use facade_util_config::*;
    }
    pub mod context {
        pub use facade_util_context::*;
    }
    pub mod cpu {
        pub use facade_util_cpu::*;
    }
    pub mod cpuprofile {
        pub use facade_util_cpuprofile::*;
        pub mod testutil {
            pub use facade_util_cpuprofile_testutil::*;
        }
    }
    pub mod cteutil {
        pub use facade_util_cteutil::*;
    }
    pub mod dbterror {
        pub use facade_util_dbterror::*;
        pub mod exeerrors {
            pub use facade_util_dbterror_exeerrors::*;
        }
        pub mod plannererrors {
            pub use facade_util_dbterror_plannererrors::*;
        }
    }
    pub mod dbutil {
        pub use facade_util_dbutil::*;
        pub mod dbutiltest {
            pub use facade_util_dbutil_dbutiltest::*;
        }
    }
    pub mod ddl_checker {
        pub use facade_util_ddl_checker::*;
    }
    pub mod deadlockhistory {
        pub use facade_util_deadlockhistory::*;
    }
    pub mod deeptest {
        pub use facade_util_deeptest::*;
    }
    pub mod disjointset {
        pub use facade_util_disjointset::*;
    }
    pub mod disk {
        pub use facade_util_disk::*;
    }
    pub mod disttask {
        pub use facade_util_disttask::*;
    }
    pub mod domainutil {
        pub use facade_util_domainutil::*;
    }
    pub mod encrypt {
        pub use facade_util_encrypt::*;
    }
    pub mod engine {
        pub use facade_util_engine::*;
    }
    pub mod errmsg {
        pub use facade_util_errmsg::*;
    }
    pub mod etcd {
        pub use facade_util_etcd::*;
    }
    pub mod execdetails {
        pub use facade_util_execdetails::*;
    }
    pub mod expensivequery {
        pub use facade_util_expensivequery::*;
    }
    pub mod extsort {
        pub use facade_util_extsort::*;
    }
    pub mod fastrand {
        pub use facade_util_fastrand::*;
    }
    pub mod filter {
        pub use facade_util_filter::*;
    }
    pub mod format {
        pub use facade_util_format::*;
    }
    pub mod gctuner {
        pub use facade_util_gctuner::*;
    }
    pub mod gcutil {
        pub use facade_util_gcutil::*;
    }
    pub mod generatedexpr {
        pub use facade_util_generatedexpr::*;
    }
    pub mod generic {
        pub use facade_util_generic::*;
    }
    pub mod globalconn {
        pub use facade_util_globalconn::*;
    }
    pub mod hack {
        pub use facade_util_hack::*;
    }
    pub mod hint {
        pub use facade_util_hint::*;
    }
    pub mod httputil {
        pub use facade_util_httputil::*;
    }
    pub mod importer {
        pub use facade_util_importer::*;
    }
    pub mod injectfailpoint {
        pub use facade_util_injectfailpoint::*;
    }
    pub mod intest {
        pub use facade_util_intest::*;
    }
    pub mod intset {
        pub use facade_util_intset::*;
    }
    pub mod israce {
        pub use facade_util_israce::*;
    }
    pub mod keydecoder {
        pub use facade_util_keydecoder::*;
    }
    pub mod kvcache {
        pub use facade_util_kvcache::*;
    }
    pub mod linter {
        pub mod constructor {
            pub use facade_util_linter_constructor::*;
        }
    }
    pub mod logutil {
        pub use facade_util_logutil::*;
        pub mod consistency {
            pub use facade_util_logutil_consistency::*;
        }
    }
    pub mod mathutil {
        pub use facade_util_mathutil::*;
    }
    pub mod memory {
        pub use facade_util_memory::*;
    }
    pub mod memoryusagealarm {
        pub use facade_util_memoryusagealarm::*;
    }
    pub mod metricsutil {
        pub use facade_util_metricsutil::*;
    }
    pub mod mock {
        pub use facade_util_mock::*;
    }
    pub mod mvmap {
        pub use facade_util_mvmap::*;
    }
    pub mod naming {
        pub use facade_util_naming::*;
    }
    pub mod nocopy {
        pub use facade_util_nocopy::*;
    }
    pub mod paging {
        pub use facade_util_paging::*;
    }
    pub mod parser {
        pub use facade_util_parser::*;
    }
    pub mod partialjson {
        pub use facade_util_partialjson::*;
    }
    pub mod password_validation {
        pub use facade_util_password_validation::*;
    }
    pub mod plancodec {
        pub use facade_util_plancodec::*;
    }
    pub mod ppcpuusage {
        pub use facade_util_ppcpuusage::*;
    }
    pub mod prefetch {
        pub use facade_util_prefetch::*;
    }
    pub mod printer {
        pub use facade_util_printer::*;
    }
    pub mod profile {
        pub use facade_util_profile::*;
    }
    pub mod promutil {
        pub use facade_util_promutil::*;
    }
    pub mod queue {
        pub use facade_util_queue::*;
    }
    pub mod ranger {
        pub use facade_util_ranger::*;
        pub mod context {
            pub use facade_util_ranger_context::*;
        }
    }
    pub mod redact {
        pub use facade_util_redact::*;
    }
    pub mod regexpr_router {
        pub use facade_util_regexpr_router::*;
    }
    pub mod regionsplit {
        pub use facade_util_regionsplit::*;
    }
    pub mod replayer {
        pub use facade_util_replayer::*;
    }
    pub mod resourcegrouptag {
        pub use facade_util_resourcegrouptag::*;
    }
    pub mod rowDecoder {
        pub use facade_util_rowDecoder::*;
    }
    pub mod rowcodec {
        pub use facade_util_rowcodec::*;
    }
    pub mod schemacmp {
        pub use facade_util_schemacmp::*;
    }
    pub mod selection {
        pub use facade_util_selection::*;
    }
    pub mod sem {
        pub use facade_util_sem::*;
        pub mod compat {
            pub use facade_util_sem_compat::*;
        }
        pub mod v2 {
            pub use facade_util_sem_v2::*;
        }
    }
    pub mod serialization {
        pub use facade_util_serialization::*;
    }
    pub mod servermemorylimit {
        pub use facade_util_servermemorylimit::*;
    }
    pub mod set {
        pub use facade_util_set::*;
    }
    pub mod signal {
        pub use facade_util_signal::*;
    }
    pub mod size {
        pub use facade_util_size::*;
    }
    pub mod skip {
        pub use facade_util_skip::*;
    }
    pub mod sli {
        pub use facade_util_sli::*;
    }
    pub mod slice {
        pub use facade_util_slice::*;
    }
    pub mod sqlescape {
        pub use facade_util_sqlescape::*;
    }
    pub mod sqlexec {
        pub use facade_util_sqlexec::*;
        pub mod mock {
            pub use facade_util_sqlexec_mock::*;
        }
    }
    pub mod sqlkiller {
        pub use facade_util_sqlkiller::*;
    }
    pub mod stmtsummary {
        pub use facade_util_stmtsummary::*;
        pub mod v2 {
            pub use facade_util_stmtsummary_v2::*;
            pub mod tests {
                pub use facade_util_stmtsummary_v2_tests::*;
            }
        }
    }
    pub mod stringutil {
        pub use facade_util_stringutil::*;
    }
    pub mod syncutil {
        pub use facade_util_syncutil::*;
    }
    pub mod sys {
        pub mod linux {
            pub use facade_util_sys_linux::*;
        }
        pub mod storage {
            pub use facade_util_sys_storage::*;
        }
    }
    pub mod systimemon {
        pub use facade_util_systimemon::*;
    }
    pub mod table_filter {
        pub use facade_util_table_filter::*;
    }
    pub mod table_router {
        pub use facade_util_table_router::*;
    }
    pub mod table_rule_selector {
        pub use facade_util_table_rule_selector::*;
    }
    pub mod tableutil {
        pub use facade_util_tableutil::*;
    }
    pub mod texttree {
        pub use facade_util_texttree::*;
    }
    pub mod tiflash {
        pub use facade_util_tiflash::*;
    }
    pub mod tiflashcompute {
        pub use facade_util_tiflashcompute::*;
    }
    pub mod tikvutil {
        pub use facade_util_tikvutil::*;
    }
    pub mod timeutil {
        pub use facade_util_timeutil::*;
    }
    pub mod tls {
        pub use facade_util_tls::*;
    }
    pub mod topsql {
        pub use facade_util_topsql::*;
        pub mod collector {
            pub use facade_util_topsql_collector::*;
            pub mod mock {
                pub use facade_util_topsql_collector_mock::*;
            }
        }
        pub mod reporter {
            pub use facade_util_topsql_reporter::*;
            pub mod metrics {
                pub use facade_util_topsql_reporter_metrics::*;
            }
            pub mod mock {
                pub use facade_util_topsql_reporter_mock::*;
            }
        }
        pub mod state {
            pub use facade_util_topsql_state::*;
        }
        pub mod stmtstats {
            pub use facade_util_topsql_stmtstats::*;
        }
    }
    pub mod traceevent {
        pub use facade_util_traceevent::*;
        pub mod test {
            pub use facade_util_traceevent_test::*;
        }
    }
    pub mod tracing {
        pub use facade_util_tracing::*;
    }
    pub mod trxevents {
        pub use facade_util_trxevents::*;
    }
    pub mod versioninfo {
        pub use facade_util_versioninfo::*;
    }
    pub mod vitess {
        pub use facade_util_vitess::*;
    }
    pub mod watcher {
        pub use facade_util_watcher::*;
    }
    pub mod workloadrepo {
        pub use facade_util_workloadrepo::*;
    }
    pub mod zeropool {
        pub use facade_util_zeropool::*;
    }
}
/// 工作负载学习门面。
pub mod workloadlearning {
    pub use facade_workloadlearning::*;
}
