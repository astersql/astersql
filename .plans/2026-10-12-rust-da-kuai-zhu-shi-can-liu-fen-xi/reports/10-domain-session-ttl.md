# Domain、Session 与 TTL 注释残留审计

## 结论

- 候选总数：31。
- 分类计数：已实现重复档案 14；部分实现且注释仍描述缺口 1；完全未实现占位 0；非执行测试档案 4；合法说明/示例误报 12。
- 范围：`pkg/domain`、`pkg/session`、`pkg/sessionctx`、`pkg/sessiontxn`、`pkg/ttl`。
- 方法：逐块读取候选范围及后续可执行代码，用 RustCodeGraph 查询实现/调用关系，并核对同路径 Go 来源和同目录 Rust 测试。这里的“已实现”只表示候选块描述的行为已有对应实现和回归，不代表整个 Go 文件已经完整移植。

## Domain

候选：`pkg/domain/domain_sysvars.rs:21-166`

- 分类：已实现重复档案。
- Go 来源：`pkg/domain/domain_sysvars.go` 的 `initDomainSysVars` 及其统计缓存、PD 动态选项、外部时间戳、资源管控、低分辨率 TSO、schema cache 和熔断阈值回调。
- Rust 对应：`DomainSysVars::{set_stats_cache_capacity,set_pd_client_dynamic_option,set_external_timestamp,set_global_resource_control,set_circuit_breaker_error_rate_ratio}` 与 `Domain::init_domain_sys_vars`；当前可执行边界把 PD/资源组副作用保存为可观测状态，而不是复刻 Go 客户端对象。
- 缺失行为：候选所列各回调没有仅存在于注释中的分支；低分辨率 TSO/schema cache 由现有 Domain 配置路径承接。若以后接入真实 PD client，应另做行为计划，不能靠保留草稿驱动。
- 测试证据：`domain_sysvars_test.rs` 覆盖动态选项解析和 duration 单位、RPC mode 大小写、统计容量与熔断比率不额外 clamp。
- 风险/建议/置信度：中；可删除旧草稿，但删除前宜把“真实客户端副作用由边界适配”写成普通说明。高。

候选：`pkg/domain/extract.rs:23-591`

- 分类：已实现重复档案。
- Go 来源：同路径 `extract.go` 的 task 收集、表/视图依赖展开、binary plan 解码与 package dump。
- Rust 对应：`ExtractHandle::extract_task`、`ExtractSource`、`view_dependencies_from_sql`、`StatementRecord::is_valid` 和文件名辅助函数。
- 缺失行为：候选描述的可观察筛选、依赖展开、打包顺序均有执行路径；实际 zip/stmt-summary 接口被 `ExtractSource` 抽象，并非注释占位。
- 测试证据：`extract_test.rs` 覆盖 digest 筛选、覆盖记录顺序、反向时间窗透传、persistent statement summary 门禁及 Domain AST source。
- 风险/建议/置信度：低；可删除重复草稿，保留接口边界说明。高。

候选：`pkg/domain/historical_stats.rs:21-112`

- 分类：部分实现且注释仍描述缺口。
- Go 来源：同路径 `historical_stats.go` 的 channel 投递、表存在性检查与持久化 worker。
- Rust 对应：`HistoricalStatsWorker::{send_table_to_dump_historical_stats,dump_historical_stats,get_one_historical_stats_table}` 和 `HistoricalStatsStore`。
- 缺失行为：当前 worker 是同步、无后台 goroutine 生命周期的 bounded channel 门面；没有找到针对队列满、表不存在、store 错误传播与 dequeue 的独立 Rust 回归。
- 测试证据：RustCodeGraph 找到 Domain 生产调用，但目标目录没有 `HistoricalStatsWorker` 的直接测试；不能把编译或调用存在视为对齐证据。
- 风险/建议/置信度：中；先补队列和 store 分支测试，再决定是否删除草稿，后台调度若属其他提交则单列范围。高。

候选：`pkg/domain/infosync/info_test.rs:176-207`

- 分类：合法说明/示例误报。
- Go 来源：`info_test.go` 的 TiFlash manager 测试意图。
- Rust 对应：这是可执行 `test_tiflash_manager` 内的 `BlockingCollector`、规则配置与断言，不是 raw archive。
- 缺失行为：无；候选范围本身参与编译执行。
- 测试证据：`test_tiflash_manager` 直接覆盖规则 CRUD、timeout circuit breaker 和分区配置。
- 风险/建议/置信度：低；扫描器应排除函数体内 trait/impl 测试替身。高。

候选：`pkg/domain/infosync/info_test.rs:523-567`

- 分类：合法说明/示例误报。
- Go 来源：`info_test.go` 的 etcd/topology 测试。
- Rust 对应：`MemoryEtcd` 的可执行 `EtcdClient` 实现；邻接 raw bytes 是 JSON fixture。
- 缺失行为：无；不是归档字符串。
- 测试证据：`etcd_injection_and_topology_decoding_match_go` 执行 put/get/prefix/delete 并断言 TiProxy/TiCDC/TiDB topology 解码。
- 风险/建议/置信度：低；把测试 fixture/trait impl 加入误报过滤。高。

候选：`pkg/domain/infosync/tiflash_manager.rs:286-346`

- 分类：合法说明/示例误报。
- Go 来源：`tiflash_manager.go` 的 mock TiFlash 支持结构。
- Rust 对应：`mockTiFlashTableInfo`、`MockTiFlash`、`NewMockTiFlash` 和 `HandleSetPlacementRule` 均为可执行代码。
- 缺失行为：范围内仅 `setUpMockTiFlashHTTPServer` 明示为空，但它不是被注释的代码，且内存 mock 的 observable rule 路径已执行。
- 测试证据：`infosync/info_test.rs::test_tiflash_manager` 与 `tiflash_progress_store_partition_excludes_compute_nodes` 使用该 mock。
- 风险/建议/置信度：中；HTTP server 空实现应另列实现缺口，不应算注释残留。高。

候选：`pkg/domain/plan_replayer.rs:26-601`

- 分类：已实现重复档案。
- Go 来源：同路径 Go 的 dump GC、task collector/status、channel handle 与 dump worker。
- Rust 对应：`DumpFileGcChecker`、`PlanReplayerTaskCollector`、`PlanReplayerDumpTaskStatus`、`PlanReplayerHandle` 和 `handle_dump_task`。
- 缺失行为：候选描述的文件期限、任务去重、running/finished 状态和发送/关闭均有执行实现；SQL/session 访问经 trait 边界注入。
- 测试证据：`plan_replayer_test.rs` 覆盖 capture/default retention，`plan_replayer_handle_test.rs` 覆盖队列、状态和 dumper 成败。
- 风险/建议/置信度：低；可删除重复草稿，保留常量/保留期说明。高。

候选：`pkg/domain/plan_replayer_dump.rs:23-138`

- 分类：已实现重复档案。
- Go 来源：同路径 Go 的表/视图收集、archive 文件布局、schema/stats/config/binding/explain dump。
- Rust 对应：`dump_plan_replayer_info`、`PlanReplaySource`、`ReplayArchive::{write}`、encode/decode 与 `extract_table_references`。
- 缺失行为：候选涉及的输出类别均由 source trait 和 archive path 实现；外键/视图依赖通过 dependency hooks 扩展。
- 测试证据：`plan_replayer_dump_test.rs` 覆盖布局、view filtering、encoded plan、capture meta、CTE/alias 和 zip round trip。
- 风险/建议/置信度：低；可删除重复草稿。高。

候选：`pkg/domain/ru_stats.rs:23-284`

- 分类：已实现重复档案。
- Go 来源：同路径 Go 的每日 RU delta、持久化、批量 GC 和 insert SQL 生成。
- Rust 对应：`RuStatsWriter`、`generate_rows`、`generate_sql`、`get_last_expected_time`。
- 缺失行为：候选描述的 interval 对齐、group ID 匹配和 SQL 转义均在执行实现中；存储由 `RuStatsBackend` 注入。
- 测试证据：`ru_stats_test.rs` 覆盖 delta/重复 group、整数 SQL、writer 调用次序、UTC interval 与 GC。
- 风险/建议/置信度：低；可删除旧草稿。高。

候选：`pkg/domain/runaway.rs:21-66`

- 分类：已实现重复档案。
- Go 来源：同路径 Go 的 resource group controller/runaway manager Domain 接线。
- Rust 对应：`ResourceGroupRuntime`、`Domain::{bind_runaway_manager,runaway_manager}` 和 `ControllerResourceGroupCatalog`。
- 缺失行为：注释中的获取/绑定与 controller catalog 行为已实现；外部控制器细节属于依赖边界。
- 测试证据：`runaway_test.rs` 覆盖 runtime controller、catalog lookup 和 Domain manager 绑定。
- 风险/建议/置信度：低；可删重复代码形注释。高。

候选：`pkg/domain/schema_checker.rs:23-119`

- 分类：已实现重复档案。
- Go 来源：同路径 Go 的 schema version 校验、Unknown 重试和错误映射。
- Rust 对应：`SchemaChecker::{check,check_by_schema_version}`、`SchemaValidator`、`SchemaCheckResult/Error`。
- 缺失行为：无候选内独有分支；包括最终 Unknown 后 sleep 和零重试边界。
- 测试证据：`schema_checker_test.rs` 覆盖参数透传、Unknown→Fail、耗尽重试与零重试。
- 风险/建议/置信度：低；可删除重复草稿。高。

候选：`pkg/domain/sysvar_cache.rs:24-304`

- 分类：已实现重复档案。
- Go 来源：同路径 Go 的 global/session sysvar cache rebuild、配置覆盖、validation/set-global callback。
- Rust 对应：`SysVarCache::{rebuild,rebuild_with_policy,global_var,session_cache}`。
- 缺失行为：候选的 scope、skip-session-init、config initialized 与 callback 过滤均有实现。
- 测试证据：`sysvar_cache_test.rs` 覆盖过滤、默认值/配置覆盖、copy isolation 和 internal stmt-summary callback 重放。
- 风险/建议/置信度：低；可删除重复草稿。高。

候选：`pkg/domain/test_helper.rs:23-142`

- 分类：已实现重复档案。
- Go 来源：同路径 Go 的 Domain 测试 helper。
- Rust 对应：`DomainTestHelper` 的 info schema 注入、table/id/partition must helper 和 schema enumeration。
- 缺失行为：无候选内独有 observable helper；panic contract 被保留。
- 测试证据：`test_helper_test.rs` 覆盖大小写查表、partition、schema enumeration、missing table 与越界 panic。
- 风险/建议/置信度：低；可删除重复草稿。高。

候选：`pkg/domain/topn_slow_query.rs:24-280`

- 分类：已实现重复档案。
- Go 来源：同路径 Go 的 top-N heap、recent queue、过期清理、internal/external 分类和关闭。
- Rust 对应：`TopNSlowQueries`、`SlowQueryHeap`、`SlowQueryQueue` 与 `take_last_n`。
- 缺失行为：候选的 heap replacement、strict expiry boundary、queue capacity 和 close 都有实现。
- 测试证据：`topn_slow_query_test.rs` 覆盖上述四类行为。
- 风险/建议/置信度：低；可删除重复草稿。高。

## Session 与 Session Context

候选：`pkg/session/test/clusteredindextest/main_test.rs:16-23`

- 分类：合法说明/示例误报。
- Go 来源：`clusteredindextest/main_test.go::TestMain`。
- Rust 对应：普通文件级说明，解释 schema lease 和 AsyncCommit 测试边界，不是被注释函数。
- 缺失行为：说明指出 failpoint/goleak 未由此文件执行；这是测试运行时差异，不是隐藏代码块。
- 测试证据：下方 `clusteredindextest_harness_shortens_schema_lease_and_exposes_bootstrap_names` 验证可观察配置。
- 风险/建议/置信度：低；保留或压缩说明，扫描器排除 prose。高。

候选：`pkg/session/test/clusteredindextest/main_test.rs:25-25`

- 分类：合法说明/示例误报。
- Go 来源：同上。
- Rust 对应：单行 rustdoc 声称存在 `_GO_DRAFT_ARCHIVE`，但文件中不存在该字符串常量；实际是过时说明，不是档案本体。
- 缺失行为：failpoint/goleak harness 仍未执行，应作为测试基础设施建议，而非注释代码迁移项。
- 测试证据：同文件 executable test 覆盖 lease、AsyncCommit 和 bootstrap 常量。
- 风险/建议/置信度：中；修正文案可另建清理任务。高。

候选：`pkg/session/test/common/main_test.rs:16-19`

- 分类：合法说明/示例误报。
- Go 来源：`session/test/common/main_test.go::TestMain`。
- Rust 对应：文件级测试边界说明；无代码声明体。
- 缺失行为：failpoint/goleak 未接线，但 AsyncCommit 配置可执行。
- 测试证据：`common_harness_clears_async_commit_timing_windows`。
- 风险/建议/置信度：低；保留为迁移边界说明。高。

候选：`pkg/session/test/common/main_test.rs:21-21`

- 分类：合法说明/示例误报。
- Go 来源：同上。
- Rust 对应：过时 rustdoc，未发现实际 `_GO_DRAFT_ARCHIVE` 常量。
- 缺失行为：仅基础设施差异，非注释块行为。
- 测试证据：同文件 executable test。
- 风险/建议/置信度：中；建议后续修正文案。高。

候选：`pkg/session/test/nontransactionaltest/main_test.rs:16-19`

- 分类：合法说明/示例误报。
- Go 来源：`nontransactionaltest/main_test.go::TestMain`。
- Rust 对应：文件级说明，后接 `configure_nontransactionaltest_harness` 可执行 helper。
- 缺失行为：failpoint/goleak 没有包级入口；不是被归档代码。
- 测试证据：`nontransactionaltest_harness_clears_async_commit_timing_windows`。
- 风险/建议/置信度：低；保留说明或单独处理 harness 基础设施。高。

候选：`pkg/session/test/session_test.rs:16-1286`

- 分类：非执行测试档案。
- Go 来源：`pkg/session/session_test.go` 的 schema checker、bootstrap SQL、SQL mode、cached table、prepare、snapshot、identity、request source 等场景。
- Rust 对应：文件 1301 行后有 `BootstrapSqlRuntime` 和 15 个可执行测试；跨模块场景另由注释列出的 runtime/cached-table/hint/sessmgr/DDL/KV 测试承接。
- 缺失行为：档案远大于本文件执行区；未建立逐个 Go test 到 Rust test 的机械映射，完整 mockstore 事务重试/DDL 并发等不能由局部 mock 证明。
- 测试证据：本文件覆盖 bootstrap parse/execute error、SQL mode、PK auto increment、rollback、schema checker SQL、cached table、prepare、snapshot、identity/request source/DB names；相关生产回归分散在注释所列文件。
- 风险/建议/置信度：高；保留档案，后续按 Go 测试名生成覆盖矩阵，未映射场景拆成聚焦移植任务。高。

候选：`pkg/session/test/tidb_test.rs:21-108`

- 分类：已实现重复档案。
- Go 来源：`pkg/session/tidb_test.go` 的 parse warning 与 `KeyNeedToLock` 分支表。
- Rust 对应：`WarningParser` 驱动 `Parse`，并直接调用生产 `KeyNeedToLock`；另有 `GetRows4Test` 回归。
- 缺失行为：候选明确的 warning/error、row/index/delete/presume/NeedLocked/temp-index 分支均在可执行测试中。
- 测试证据：`parse_error_warn_forwards_warning_and_preserves_error`、`keys_need_lock_matches_go_branch_matrix`。
- 风险/建议/置信度：低；可删除重复测试草稿。高。

候选：`pkg/session/test/vars/vars_test.rs:21-385`

- 分类：已实现重复档案。
- Go 来源：`pkg/session/test/vars/vars_test.go` 的 sysvar registry、transaction propagation 和 validation 场景。
- Rust 对应：`MemoryGlobal`、`SessionVars` 及五个真实测试调用生产 registry/validation hooks。
- 缺失行为：候选的 case-insensitive registry、依赖顺序、removed variable、TiKV/advancer hooks、index-join/timestamp boolean 规则已有断言。
- 测试证据：文件 453 行后的五个 `#[test]`。
- 风险/建议/置信度：低；可删除重复草稿。高。

候选：`pkg/sessionctx/sessionstates/session_states_test.rs:21-1921`

- 分类：非执行测试档案。
- Go 来源：`session_states_test.go` 全文存于 `GO_REFERENCE`。
- Rust 对应：`paramInfo` 之后四个可执行 serde/session-state 测试；另有 `session_states_1_aster_unit_test.rs` 的综合状态/令牌回归。
- 缺失行为：只证明 prepared statement JSON/base64、omitempty/null 和 round trip；Go 原文中的完整迁移资源/变量组合尚无逐用例映射。
- 测试证据：`prepared_statement_state_uses_go_json_shape_and_base64_types`、`session_states_json_round_trip_matches_go_tags_and_omitempty` 等。
- 风险/建议/置信度：高；当前不可删除 `GO_REFERENCE`，先建立用例对照并移植未覆盖状态字段。高。

候选：`pkg/sessionctx/sessionstates/session_token_test.rs:21-395`

- 分类：已实现重复档案。
- Go 来源：`session_token_test.go` 的证书/签名算法、用户名、有效期、grace/reload/concurrency 场景。
- Rust 对应：文件 414 行后直接使用生产 token API 和真实 OpenSSL 临时证书。
- 缺失行为：候选主要可观察矩阵均被执行区覆盖。
- 测试证据：JSON/base64、无证书、RSA/RSA-PSS/ECDSA/Ed25519、验证失败、Unicode case folding、lifetime、grace、reload 并发共九组测试。
- 风险/建议/置信度：中；可删除 Go 原文，但应先保留测试矩阵摘要以防算法覆盖信息丢失。高。

候选：`pkg/sessionctx/variable/tests/session_test.rs:20-1149`

- 分类：非执行测试档案。
- Go 来源：`variable/session_test.go` 全文 `GO_REFERENCE`。
- Rust 对应：文件 1158 行后有十个生产类型测试，覆盖 user vars、delta/savepoint、runtime filters、plan cache、system variable 等核心子集。
- 缺失行为：Go 原文的 slow-log 全格式/TiFlash RU、row-id shard、hook/chunk 并发、cloud storage URI 和 performance-schema SQL 等没有在本文件逐项执行。
- 测试证据：`go_test_set_system_variable_success_and_error_matrix`、`go_test_transaction_context_delta_savepoint_and_cleanup`、`go_test_user_vars_and_relevant_optimizer_state` 等。
- 风险/建议/置信度：高；保留档案并把未覆盖函数逐项拆分，不能因十个测试存在就宣称整个原文已迁移。高。

候选：`pkg/sessionctx/variable/tests/variable_test.rs:22-22`

- 分类：合法说明/示例误报。
- Go 来源：下一候选的 `variable_test.go` 参考文本。
- Rust 对应：单行 rustdoc 仅说明 `GO_REFERENCE` 的性质，不含代码。
- 缺失行为：不适用。
- 测试证据：下一候选的执行测试用于判断覆盖，而非本说明行。
- 风险/建议/置信度：低；保留说明并让扫描器合并相邻 raw archive metadata。高。

候选：`pkg/sessionctx/variable/tests/variable_test.rs:23-834`

- 分类：非执行测试档案。
- Go 来源：`variable/variable_test.go` 的 registry、各种 validate、scope/default/setter/cache/timezone/dependency 测试。
- Rust 对应：842 行后七个可执行测试覆盖动态默认值、registry/error code、注册恢复、数值/枚举/duration/float、本地值类型/scope/skip-cache、relaxed/time/dependency。
- 缺失行为：Go 原文中的 deprecation、noop、instance config/scope、internal session variable、全部默认值可设置等没有逐项可运行映射。
- 测试证据：`go_test_sys_var_registry_and_dynamic_defaults` 至 `go_test_relaxed_validation_time_and_dependency_order`。
- 风险/建议/置信度：高；保留原文，先补缺失行为测试再清理。高。

## Session Transaction

候选：`pkg/sessiontxn/isolation/main_test.rs:16-43`

- 分类：合法说明/示例误报。
- Go 来源：isolation Go suite 的真实 testkit/mockstore harness。
- Rust 对应：英文/中文架构说明，后接可执行 `MockRuntime`、`TxnAssert`，并供 sibling isolation tests 使用。
- 缺失行为：真实 TiKV/PD oracle 集成明确未在此 fixture 建模，但这属于测试边界，不是被注释实现。
- 测试证据：本文件三个测试及 `optimistic/readcommitted/repeatable_read/serializable*_test.rs` 使用该 fixture 驱动真实状态机。
- 风险/建议/置信度：低；保留关键边界说明，扫描器过滤 prose。高。

候选：`pkg/sessiontxn/staleread/main_test.rs:18-44`

- 分类：合法说明/示例误报。
- Go 来源：staleread suite `TestMain` 与 testkit/mockstore 场景。
- Rust 对应：共享 fixture 说明，后接可执行 `MockBackend`；不是归档代码。
- 缺失行为：真实 SQL expression/KV request 集成由 trait 边界替代，说明中已显式限定。
- 测试证据：`externalts_test.rs`、`processor_test.rs`、`provider_test.rs` 广泛使用 fixture 覆盖 stale-read 状态机。
- 风险/建议/置信度：低；保留说明。高。

候选：`pkg/sessiontxn/txn_manager_test.rs:18-48`

- 分类：合法说明/示例误报。
- Go 来源：`txn_manager_test.go` 的 testkit/mockstore manager 测试。
- Rust 对应：共享 `MockManager/MockProvider` fixture 的架构说明，后续均为执行代码。
- 缺失行为：完整 KV transaction machinery 不在该 crate 边界，非注释实现缺口。
- 测试证据：本文件八个 helper/dispatch 测试及 `txn_context_test.rs` 等复用 fixture。
- 风险/建议/置信度：低；保留边界说明，扫描器过滤 prose。高。

## TTL

候选：`pkg/ttl/sqlbuilder/sql.rs:22-511`

- 分类：已实现重复档案。
- Go 来源：`pkg/ttl/sqlbuilder/sql.go` 的 Datum 格式化、builder 状态机、table/index scan generator 与 delete SQL。
- Rust 对应：`FormatSQLDatum`、`SQLBuilder`、`ScanQueryGenerator`、`NewIndexScanQueryGenerator`、`BuildDeleteSQL`。
- 缺失行为：候选的 identifier/string/bytes escaping、expire predicate、range、index hint、common handle 和 batch delete 均有执行实现。
- 测试证据：`sqlbuilder/main_test.rs` 与 `sqlbuilder/sql_test.rs` 覆盖格式化、状态错误、scan/delete、expire、index/common-handle 路径。
- 风险/建议/置信度：低；可删除重复草稿，保留 SQL 状态机的不变量说明。高。

## 后续处置优先级

1. 先为 `HistoricalStatsWorker` 补直接分支测试，确认是否还需要后台 worker 生命周期，再清理其草稿。
2. 为三个 `GO_REFERENCE` 大文件和 `session/test/session_test.rs` 建立 Go 测试名到 Rust 测试名的矩阵，未覆盖场景逐项移植；在此之前不得删除档案。
3. 可批量清理 14 个“已实现重复档案”，但每次清理仍应运行对应目录的现有测试。
4. 调整扫描启发式：排除可执行函数体、测试 fixture raw JSON/SQL、普通架构 prose，并把单行 archive metadata 与相邻 raw string 合并，避免重复候选。
