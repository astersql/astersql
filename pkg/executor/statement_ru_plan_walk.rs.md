# `pkg/executor/statement_ru_plan_walk.rs`

## 文件定位

`statement_ru_plan_walk.rs` 属于 `astersql-executor` crate（`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"`），由 `pkg/executor/lib.rs` 以公开模块 `statement_ru_plan_walk` 导出。它位于一条语句 RU v2 终结链路的中间：上游 `statement_ru_result.rs::install_statement_ru_owner_at_boundary` 创建语句级 owner，`adapter.rs::ExecStmt::finishStatementRU` 在语句终结时冻结证据并调用 `calculate_statement_ru_forest`，本文件再把 typed flat physical-plan forest 和运行时快照折算成 `StatementRUFinalizedSnapshot`。结果的对外发布在 `statement_ru_result.rs`，不在本文件内。

这不是通用物理计划执行器，而是语句执行完成后的“证据遍历与 RU 计费”层。它依赖 `astersql-planner-core` 的 `TypedFlatPhysicalPlan`/`TypedFlatOperator`、`astersql-util-execdetails` 的值快照、`astersql-resourcegroup` 的 `StmtUnits` 模型，以及同 crate 的 `statement_ru_result` 和 `statement_ru_reporting`。

## 核心职责

1. 维护一条语句的终结所有权：`StatementRUOwner` 分别记录“第一个 session 结果”和 root EOF，并保证 calculation setup 只被一个终结调用者取走。
2. 将可变、会在 cleanup 期间消失的运行时统计复制为 `StatementRURuntimeEvidence`、`StatementRUPlanEvidence`、`StatementRUPointSnapshot` 和 `StatementRUWriteSnapshot` 等值对象，避免 calculator 保留 collector/RPC/commit-detail 指针。
3. 验证预序展平树的规范结构，然后以子节点优先顺序遍历 Main、CTE 和 ScalarSubQuery 树；任一树不完整就不产生最终值。
4. 根据物理算子类型、执行位置（TiDB root、TiKV cop、TiFlash MPP）和快照证据计算 CPU work、scan bytes、hash-state rows、join-output rows、network bytes 等 `StmtUnits`。
5. 在 full-report/EXPLAIN RU 路径上，同步按 engine/operator 归属增量，并生成每个 occurrence 的 `self_ru` 和 `cum_ru`；主树 root 所有的语句级单位不会重复分摊到 CTE/标量子查询。

## 主要符号

- `StatementRUFinalOutcome::{Unknown, Success, Failure}` 是 `AtomicU32` 的语义枚举；`StatementRUOwner::record_final_outcome_with_setup` 用 CAS 实现 first-record-wins，首次 failure 同时消费 setup。
- `StatementRUOperatorState::{Unknown, Complete, Unsupported, Invalid}` 和 `StatementRUOperatorResult { state, output_rows }` 表示单个计划 occurrence 的可计算性及输出行数。`merge_statement_ru_operator_state` 保证 `Invalid` 优先于 `Unsupported`，只有两边均 `Complete` 才完成。
- `StatementRUOwner` 内含 `Mutex<Option<StatementRUCalculationSetup>>`、outcome/root-EOF 原子量和安装时的 restricted/TTL/cursor 快照。`take_terminal_setup` 是终结的 one-shot 门，`abort` 仅消费该门。
- `snapshot_statement_ru_writes` 只复制 `CommitDetails.WriteKeys/WriteSize`；`snapshot_statement_ru_runtime_evidence` 对 plan ID 去重，复制 root/cop rows、scan detail、write CPU、analyze bytes、hash-state 和 TiFlash units，并且只在 RU metrics 未 bypass 时取 TiKV response bytes。
- `StatementRUPointSnapshot::state` 区分非法证据、没有 payload producer 的有效零工作、scan detail 不完整的 unsupported，以及可分类的 complete。
- `calculate_statement_ru_forest[_with_operators]` 是 forest 级入口；后者还可填充 `StatementRUExplainResult` 的 Main/CTE/ScalarSubQuery 结果数组。
- `validate_statement_ru_flat_tree` 验证 legacy `FlatPlanTree`，`validate_statement_ru_typed_flat_tree` 验证实际计算所用 typed tree；它们要求每个 child index 恰好指向前一棵子树后的下一个预序项。
- `calculate_statement_ru_plan[_with_operators]` 先验证整树与可选结果 slice 长度，再交给内部递归 `calculate_statement_ru_plan_child_first`。
- `collect_statement_ru_join_units`、`collect_statement_ru_aggregation_units`、`collect_statement_ru_reader_scan_bytes`、`collect_statement_ru_point_lookup_evidence`、`collect_statement_ru_mpp_network`、`collect_statement_ru_mpp_scan_bytes` 和 `collect_statement_ru_shuffle_units` 是主要证据适配器。
- `merge_statement_ru_unit_delta` 先在 calculator 拷贝上校验四个可变单位，所有字段均合法且累加不溢出时才整体提交，避免留下半个算子的费用。

## 执行流程

1. `statement_ru_result.rs::install_statement_ru_owner` 在编译/计划边界判定语句是否合格，将 setup 和安装时 session 分类封装进 `Arc<StatementRUOwner>`。
2. 执行期间 `adapter.rs::ExecStmt::RecordStatementRUFinalOutcome` 记录第一个 session 成败，`recordStatementRURootEOF` 独立记录 root 已穷尽。成功本身不等于 EOF，因为客户端可在读完前正常关闭 record set。
3. `ExecStmt::finishStatementRU` 先 `take_terminal_setup`，使 panic、错误或重入都无法重试；然后检查 final outcome、terminal error、安装时与当前 session 资格、root EOF，再冻结 runtime evidence。
4. commit 在 adapter 内直接用 write snapshot 终结；其他可计算计划被展平为 `TypedFlatPhysicalPlan`，并进入 `calculate_statement_ru_forest`。
5. forest 入口要求 root EOF，拒绝空 Main 树，并防止一份旧的 statement-wide point snapshot 被多个 point occurrence 重复计费。它只一次加入 TiKV cop response bytes，根据主根类型加入 write keys/bytes 及 write-statement 标识。
6. forest 按 Main、CTEs、ScalarSubQueries 顺序遍历，每棵树先验证 canonical preorder，再递归子节点。子节点状态合并成功后，当前 occurrence 根据 root/TiKV/TiFlash 行数证据和算子支持矩阵增加 units。
7. 支持的核心算子包括 write/analyze/commit wrapper，join/aggregation，projection/selection/limit/union-scan/window/sort/top-N，TiKV/TiFlash readers 和 scans，point lookup，MPP exchange，shuffle，以及 dual/memtable/CTE/union/sequence/lock/apply 等 ownership wrapper。类型、child 数量、label 或执行 site 不符合合同时返回 `Unsupported`；矛盾数值或树结构返回 `Invalid`。
8. 完成 occurrence 会增加 `operator_num`，将它自己的 delta 归属到 TiDB/TiKV/TiFlash engine，可选写入 full report 和 EXPLAIN 的 self/cumulative RU。所有树均 complete 后 `StatementRUCalculator::finalize` 才产生快照。

## 数据与状态

owner 状态被特意拆成三部分：`final_outcome` 用 Acquire/AcqRel CAS 保证首次结果不可改写，`root_eof` 用 Release/Acquire 独立发布穷尽状态，`setup: Mutex<Option<_>>` 保证只有首个 terminal/abort 拿到计算权。三个 `*_at_install` 布尔值保留安装时资格，防止延迟 terminal 期间 session 标记被恢复后误发布。

`StatementRURuntimeEvidence` 是计算边界的只读值快照：`plans` 按 plan ID 保存每个 occurrence 的行数和 scan/hash/network 证据；`points` 为多个 point lookup 提供按 ID 匹配的快照，`point` 只是单 occurrence 的兼容退路；`writes`、`tikv_response_bytes`、`frontend_compile_bytes` 是语句级证据。“快照缺失”通常表示生产者未提供数据并计为零，而负数、不自洽的 coverage 标志或 `NaN`/infinity 是 `Invalid`；这两者不能混同。

`StatementRUCalculator` 是遍历期的累加器。算子通常先构造局部 `StmtUnits` delta 再原子式合并；归属数组 `compute[TiDB/TiKV/TiFlash]` 只记录 occurrence 自身 delta。EXPLAIN 的 `self_ru` 使用当前算子增量，`cum_ru` 使用整棵子树增量，两者都加入对应 TiFlash premium；只有 Main root 加入 frontend/write/network 等 root-owned units。

## 依赖与调用关系

- 上游安装：`statement_ru_result.rs::install_statement_ru_owner_at_boundary` 调用 `StatementRUOwner::new`，`install_statement_ru_owner` 将 owner 存入 `ExecStmt.StatementCtx`。
- 上游生命周期：`adapter.rs::StatementRUFailureGuard::drop`、`ExecStmt::RecordStatementRUFinalOutcome`、`abortStatementRU`、`recordStatementRURootEOF` 和 `finishStatementRU` 调用 owner API；`finishStatementRU` 调用 `calculate_statement_ru_forest`。
- 计划依赖：`astersql-planner-core::{FlatPlanTree, TypedFlatPhysicalPlan, TypedFlatOperator}` 提供 forest 与 occurrence 元数据，`astersql-planner-core-base::Plan` 和 `astersql-planner-core-operator-physicalop` 用于 downcast 及算子合同判定，`astersql-kv::StoreType` 区分 TiKV/TiFlash site。这些都在 `pkg/executor/Cargo.toml` 中以 workspace path dependency 声明，本文件没有专属 feature gate。
- 证据依赖：`astersql-util-execdetails` 提供 `RuntimeStatsColl`、root/cop/hash/TiFlash 快照、`ScanDetail`、`RUV2Metrics` 与 `CommitDetails`。`snapshot_statement_ru_runtime_evidence` 是从 live collector 进入值证据的边界。
- 下游模型：`astersql-resourcegroup::ruv2::model::{StmtUnits, calculate}` 进行单位到 RU 的折算；`statement_ru_result::{StatementRUCalculator, classify_scan_evidence, classify_statement_ru_plan}` 负责分类与最终化；`statement_ru_reporting` 提供 engine/operator 归属、TiFlash 倍率和失败原因。
- RustCodeGraph 对 `calculate_statement_ru_forest_with_operators` 显示直接下游包括 `statement_ru_explain_tree`、`calculate_statement_ru_plan_with_operators`、`classify_statement_ru_plan`、report `add` 和 calculator `finalize`；其直接上游是 `calculate_statement_ru_forest`。

## 错误处理与边界

该模块遵循 fail-closed：只有完整、结构合法、所有 occurrence 可支持的 forest 才返回 finalized snapshot。`Invalid` 表示不可信的证据或结构（空/非 canonical tree，负数，NaN/infinity，行数越界，child 数或 label 矛盾，结果 slice 长度错误）；`Unsupported` 表示当前算子/site/证据 coverage 不在支持矩阵内。`statement_ru_failed` 将 unsupported 映射为对应 failure reason，其余非完整状态按 invalid 处理。

重要边界包括：root EOF 是必要条件；多 point occurrence 必须使用按 plan ID 的证据；full outer join、TiKV cop join、非 MPP 的非 root join 不支持；MPP 只支持 hash join；TopN root 使用 `offset + count` 且检查溢出，cop/MPP TopN 不允许 offset；point lookup 在 write statement 中不重复计读证据；ExchangeReceiver 不再收取 network work，只由 sender 计费；MPP reader 对实际 TableScan 递归求 scan bytes，防止 reader/scan 重复计费。

`Mutex` 中毒时 `take_terminal_setup` 会取回 inner value 并继续 one-shot 消费，而不将毒化变成二次终结机会。本文件不直接 panic-catch；生产终结调用者 `adapter.rs::finishStatementRU` 在其外层使用 `catch_unwind`，将 panic 降级为 failure 且不影响原有 terminal bookkeeping。

## 并发与资源生命周期

`StatementRUOwner` 是可通过 `Arc` 共享的语句级所有者。outcome CAS 使并发的 success/failure 竞争只接受第一个；setup 的 `Option::take` 使并发 terminal 只有一个 consumer；root EOF 可与 outcome 独立记录。`statement_ru_plan_walk_test.rs::statement_ru_concurrent_terminal_has_one_consumer` 和 `statement_ru_concurrent_outcomes_keep_the_first_record` 直接验证这两个不变量。

计算本身是单个 `&mut StatementRUCalculator` 的同步深度优先遍历，不创建线程、task 或 channel。递归 `depth` 上限与 tree length 相同，配合 canonical-tree 验证防止环和无界递归。快照设计确保 cleanup 可在终结后释放 runtime collector、RPC response 和 commit details，calculator/finalized snapshot 不延长它们的生命周期。CTE definition 在 flatten 阶段按 storage ID 去重，forest 只遍历共享生产者一次，消费者 occurrence 仍在各自所属树中保留行数。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/statement_ru_plan_walk.go`。Rust 符号基本按 snake_case 一对一移植：`statementRUOwner` ↔ `StatementRUOwner`，`calculateStatementRUInternal`/forest 遍历 ↔ `calculate_statement_ru_forest_with_operators`，`calculateStatementRUPlanChildFirst` ↔ `calculate_statement_ru_plan_child_first`，`collectStatementRUJoinUnits`/`collectStatementRUAggregationUnits`/reader/point/shuffle helpers ↔ 同名 Rust helpers，`mergeStatementRUOperatorState` 和 `mergeStatementRUUnitDelta` 也保留了同样的优先级与整体提交语义。

Rust 与 Go 的可观察合同保持一致：first-record-wins，语句成功和 root EOF 分离，Main/CTE/ScalarSubQuery 各遍历一次，只对 Main root 附加 statement-wide units，子节点优先，读/写/join/agg/sort/TopN/MPP 的公式和支持 site 一致，无效或不支持证据都不发布部分结果。Go 用 `sync.Once` 和内嵌 setup；Rust 用 `Mutex<Option<Setup>>` 表达同一 one-shot 所有权。Go 直接从 `RuntimeStatsColl` 读取数据；Rust 将 live-source 读取提前到 `StatementRURuntimeEvidence` 快照边界，以值语义执行后续遍历。

`pkg/executor/statement_ru_plan_walk_test.rs` 的测试名中保留 `go_merge_187/195/197` 等迁移线索，并与 Go 的 `statement_ru_plan_walk_test.go`、`statement_ru_plan_walk_integration_test.go` 和 benchmark 文件对应。Rust 测试与生产源文件分离，由 `pkg/executor/lib.rs` 的 `#[cfg(test)] mod statement_ru_plan_walk_test;` 接入。

## 扩展指南

- 新增可计费物理算子时，首先在 `calculate_statement_ru_plan_child_first` 增加明确的 downcast 分支，规定支持 site、child 数量/标签、输出行证据和 units 公式；同时在 `statement_ru_reporting.rs::StatementRUOperator` 中确认分类，并在 Go 同路径保持逻辑对齐。
- 新增运行时证据时，不要让 calculator 保留 live object。先向 `StatementRUPlanEvidence` 或专用 snapshot 增加值字段，在 `snapshot_statement_ru_runtime_evidence`/`adapter.rs::SnapshotStatementRUEvidence` 冻结，再由 collector helper 消费。必须定义“缺失”、“有效零值”、“coverage 不完整”和“非法”的不同语义。
- 修改 tree/forest 布局时，同步修改 `validate_statement_ru_typed_flat_tree`、root-owned units 归属和 `StatementRUExplainResult` 索引规则；不能仅让某个测试树通过而破坏 canonical preorder 不变量。
- 修改 join/aggregation/reader/point/MPP/shuffle 公式时，优先修改对应 `collect_*` helper，使 delta 先局部校验再合并，避免主遍历分支中出现半提交。性能风险主要来自每 occurrence 对 `evidence.plans` 的线性查找、递归深度和 EXPLAIN 路径的重复 RU 折算；引入索引或缓存时必须保持 occurrence 身份和 forest 顺序。
- 测试应继续放在独立的 `pkg/executor/statement_ru_plan_walk_test.rs`，并对齐 Go 的 `statement_ru_plan_walk_test.go`。至少覆盖：成功公式、不支持 site/形状、负数与溢出、缺失证据、多 occurrence 不重计、full-report/EXPLAIN 归属，以及并发 terminal 的 one-shot 性。若改变真实 SQL 边界，再同步 Go integration tests 所验证的生产接线。

## 验证依据

- 源码全文：`pkg/executor/statement_ru_plan_walk.rs`（1521 行）；直接生产入口：`pkg/executor/adapter.rs::ExecStmt::finishStatementRU`、`RecordStatementRUFinalOutcome`、`recordStatementRURootEOF`；owner 安装：`pkg/executor/statement_ru_result.rs::install_statement_ru_owner_at_boundary` 和 `install_statement_ru_owner`。
- crate 与模块边界：`pkg/executor/Cargo.toml` 声明 `astersql-executor` 以 `lib.rs` 为入口，并声明 planner-core/base/physicalop、resourcegroup、kv、util-execdetails 等 path dependencies；`pkg/executor/lib.rs` 公开本模块并仅在 test cfg 下接入独立 Rust 测试文件。该目录没有 `doc.go`，因此包级事实以 Cargo/lib 和生产调用点为准。
- RustCodeGraph：`status` 报告 11,467 个已索引文件、307,296 个节点和 1,848,419 条边；`query calculate_statement_ru_plan --kind function` 命中 plan 入口、with-operators 入口和 child-first 递归；`query StatementRUOwner` 命中 Rust owner 及 `statement_ru_result.rs::install_statement_ru_owner_at_boundary`；`node calculate_statement_ru_forest_with_operators` 核对了直接上下游。`files --filter pkg/executor/statement_ru_plan_walk` 未返回路径，因此文件全貌、Cargo/Go/测试与跨文件调用点按技能规则用 `rg` 和直接读取补齐。
- Go 对照：`pkg/executor/statement_ru_plan_walk.go`；Go 边界和回归：`pkg/executor/statement_ru_plan_walk_test.go`、`statement_ru_plan_walk_integration_test.go`、`statement_ru_plan_walk_bench_test.go`。
- Rust 独立测试：`pkg/executor/statement_ru_plan_walk_test.rs` 覆盖 typed plan bridge/forest、write snapshot、terminal calculator、owner first-outcome/one-shot、flat-tree validation、sort/delta/state merge、runtime evidence、reader/point/child-first、row/write/TopN、join/aggregation、MPP/CTE/shuffle/exchange、生产 terminal 发布与并发竞争。本任务为纯文档分析，按计划不运行 Cargo；通过固定章节结构命令验证文档形状，并人工核对本文的职责、流程和扩展点均能回链到上述符号与文件。
