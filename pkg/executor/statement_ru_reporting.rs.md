# `pkg/executor/statement_ru_reporting.rs`

## 文件定位

本文件属于 `astersql-executor` crate：`pkg/executor/Cargo.toml` 将库入口设为 `lib.rs`，`pkg/executor/lib.rs` 通过 `pub mod statement_ru_reporting` 公开本模块，并在 `#[cfg(test)]` 下装配独立测试 `statement_ru_reporting_test.rs`。模块位于 RU v2 语句核算链的“归属与发布”层：它不遍历物理计划，也不读取运行时统计，而是接收已经汇总的 `StmtUnits`/`StatementRUComputeUnits`，按 TiDB、TiKV、TiFlash 三个引擎计算 RU 归属，并把 full 模式的有界明细交给发布 sink。

直接上游是 `pkg/executor/statement_ru_result.rs` 与 `pkg/executor/statement_ru_plan_walk.rs`。前者的 `StatementRUCalculator::finalize` 调用 `statement_ru_engine_result`、追加 TiFlash 倍率并冻结 `StatementRUFullReport`；`publish_statement_ru_finalized_snapshot` 再调用 `publish_statement_ru_full_metrics`。后者在计划证据遍历和 EXPLAIN 累计中调用 `statement_ru_tiflash_ru`，并使用 `STATEMENT_RU_TIFLASH_MULTIPLIER`。因此本文件是“证据采集”与“最终指标/资源组发布”之间的纯数值边界。

## 核心职责

- 用 `StatementRUFailureReason` 把终止原因稳定映射成低基数的 status/reason 标签。
- 用 `StatementRUEngine`、`StatementRUOperator` 和固定二维数组保存 full 模式的引擎/算子原始单位，避免保存计划节点、SQL、表名或索引名。
- 在 `StatementRUFullReport::add_operator` 中把非 TiFlash 算子的 `scan_bytes`、`net_bytes` 从本地算子剥离并归属到 TiKV；TiFlash 证据保持在 TiFlash。
- 在 `StatementRUFullReport::add_statement_units` 中补记不属于单个物理算子的 frontend、语句写入和 KV 写入证据。
- 通过 `statement_ru_engine_result` 和 `statement_ru_tiflash_ru` 按配置权重计算三个引擎的原始 RU；10 倍 TiFlash 实验倍率由调用方应用，而不是在这两个函数内应用。
- 通过 `publish_statement_ru_full_metrics` 只发布 seen 且非零的单位序列，并在末尾发布一次成功语句计数。

## 主要符号

- `STATEMENT_RU_TIFLASH_MULTIPLIER: f64 = 10.0`：TiFlash 实验倍率。`StatementRUCalculator::finalize` 将原始 TiFlash RU 的额外 9 倍计入总 RU，并把 `engine_ru.tiflash` 放大 10 倍；`statement_ru_plan_walk.rs` 也用它修正 EXPLAIN 节点 RU。
- `StatementRUFailureReason`：六种终止结果。`label()` 返回 `not_finished`、`unsupported_plan`、`invalid_plan_or_evidence`、`statement_error`、`ineligible`、`panic`；`status()` 仅把 `Unsupported`/`Ineligible` 映射为 `skipped`，其余映射为 `failed`。
- `StatementRUEngine`：按 `#[repr(usize)]` 固定为 `TiDB`、`TiKV`、`TiFlash`，其判别值直接充当二维数组下标。
- `StatementRUOperator`：23 个有界类别，从普通算子 `Wrapper`/`Projection`/`HashJoin` 到语句级 `Frontend`/`KVWrite`。判别值同时索引 `StatementRUFullReport` 与发布器中的字符串表。
- `StatementRUFullReport`：`units: [[StmtUnits; 23]; 3]` 保存累计值，`seen: [[bool; 23]; 3]` 区分“从未观察”与“观察过但当前为零”。`Default` 将两张表全部清零。
- `StatementRUFullReport::add`：用 `StmtUnits::add` 累加指定格并设置 `seen`；即使传入全零单位，也会记录 seen。
- `StatementRUFullReport::add_operator`：实现本地算子与远端 scan/network 的归属拆分；TiFlash 是不拆分的特殊分支。
- `StatementRUFullReport::add_statement_units`：frontend 始终调用 `add`，写语句与 KV 写入只在对应值非零时加入。
- `StatementRUComputeUnits`：引擎侧计算投影，包含 CPU、hash 状态行、算子数、join 输出、scan、network 和跨 AZ network；它与完整 `StmtUnits` 分开，使本地算子工作和全局原始单位能按引擎重组。
- `StatementRUEngineResult`：最终三个引擎的 RU 数值容器；本文件返回的 TiFlash 值仍是未乘实验倍率的原始值。
- `statement_ru_engine_result`：以总 `StmtUnits`、三引擎 compute 数组和 `StmtWeights` 计算分引擎结果。
- `statement_ru_tiflash_ru`：TiFlash 七类 compute 单位的纯加权和，供引擎结算与计划节点 RU 共同复用。
- `publish_statement_ru_full_metrics`：将冻结报告转换为 `StatementRUPublicationSink::unit`/`statement` 调用；引擎、算子和单位名全部来自固定表。

## 执行流程

1. `statement_ru_plan_walk.rs` 遍历物理计划和运行时统计，将证据累计到 `StatementRUCalculator.units`、三槽 `compute`，并在 full 模式向 `StatementRUFullReport` 的相应引擎/算子格写入单位。
2. 写入算子明细时，`add_operator` 对 TiDB/TiKV 输入先复制 `scan_bytes` 与 `net_bytes` 到 `remote`，把原单位中的这两项清零，再累计本地部分；只要远端两项之一非零，就将它们累计到 TiKV 的同一算子类别。TiFlash 输入则整体保留在 TiFlash。
3. `StatementRUCalculator::finalize` 读取当前 `StmtWeights`，先调用资源组模型计算总结果，再调用 `statement_ru_engine_result`。TiDB 接收本地 compute、排除 TiFlash 的 join 输出、frontend 与 write-statement；TiKV 接收自身 compute、排除 TiFlash 的 scan/net、write keys/bytes；TiFlash 由 `statement_ru_tiflash_ru` 独立计算。
4. `finalize` 在本文件之外应用 TiFlash 10 倍倍率并拒绝负数或非有限 RU。full 模式下它 clone 报告，对 clone 调用 `add_statement_units`，然后把该冻结副本放入 `StatementRUFinalizedSnapshot`；后续 live report 变化不会修改快照。
5. `publish_statement_ru_finalized_snapshot` 先发布资源组消费和总结果；快照包含 full report 时调用 `publish_statement_ru_full_metrics`。后者按固定数组顺序扫描，只处理 `seen` 格，只对 11 种单位中的非零值调用 `sink.unit`，最后调用 `sink.statement("success", calibration_state.label())`。
6. 失败快照不走成功发布器；`statement_ru_result.rs::publish_statement_ru_failure_safely` 使用本文件的 `status()`/`label()` 发布失败或跳过标签，并在外围隔离 sink panic。

## 数据与状态

`StatementRUFullReport` 的数组尺寸由私有常量 `ENGINE_COUNT = 3`、`OPERATOR_COUNT = 23` 固定。枚举顺序、数组尺寸以及 `publish_statement_ru_full_metrics` 中 `ENGINES`/`OPERATORS` 字符串顺序构成同一个不变量：新增、删除或重排枚举成员时必须同步全部位置，否则虽可能仍能编译，却会产生错误标签归属；越过数组边界则会 panic。

`units` 保存数值，`seen` 保存出现性。两者分离的意义是：从未观察的格不会发布；观察过但合计为零的格也不会发布 unit 指标，但仍保留“曾处理”的内部事实。累加依赖 `StmtUnits::add`，本文件不去重，因此调用方必须保证每份证据只记一次。语句级证据由 `finalize` 仅加入冻结 clone，从而避免反复 finalize 污染 live report，并保持 result/full 两种模式的数值结果一致。

引擎结果采用 `f64`。`statement_ru_engine_result` 会用总量减去 TiFlash 子量来得出 TiDB join 输出和 TiKV scan/net；本函数自身不钳制负值、不检查 NaN/Infinity。合法性边界位于 `StatementRUCalculator::finalize`，后者在倍率处理后统一拒绝负数和非有限结果。`StatementRUComputeUnits`/`StatementRUEngineResult` 均为 Copy 值对象，不持有计划、runtime stats 或外部资源。

## 依赖与调用关系

直接外部依赖只有 `astersql_resourcegroup::ruv2::model::{StmtUnits, StmtWeights}`；`pkg/executor/Cargo.toml` 以路径依赖 `../resourcegroup` 声明 `astersql-resourcegroup`，本模块没有专属 feature，crate 唯一列出的 `nextgen` feature 与本文件无直接条件编译关系。模块内没有 `cfg` 分支。

上游调用关系经 RustCodeGraph 与源码交叉核对：

- `StatementRUCalculator::finalize`（`statement_ru_result.rs`）调用 `statement_ru_engine_result`，并对 full report clone 调用 `add_statement_units`。
- `calculate_statement_ru_plan_child_first` 等计划遍历路径（`statement_ru_plan_walk.rs`）调用 `statement_ru_tiflash_ru`，并在 EXPLAIN 自身/子树结果中使用倍率常量。
- `publish_statement_ru_finalized_snapshot`（`statement_ru_result.rs`）在快照含 report 时调用 `publish_statement_ru_full_metrics`。
- `adapter.rs` 与 plan-walk/result 终止路径构造 `StatementRUFailureReason`，再由 `publish_statement_ru_failure_safely` 消费其标签。

下游方面，计算函数只读取权重与值对象；发布函数依赖 `statement_ru_result.rs::StatementRUPublicationSink`。生产 `StatementRUContextSink` 最终把 consumption 交给 session/resource-group reporter，把 results/unit/statement 写入 `astersql_metrics::ru_v2` 指标，并把校准快照交回 runtime context。这个抽象也使独立 Rust 测试能用内存 sink 验证标签和发布次数。

## 错误处理与边界

本文件所有 API 都是无 `Result` 的纯值计算或 sink 调用。它不自行捕获 panic：发布隔离由上游 `publish_statement_ru_finalized_snapshot`/`publish_statement_ru_failure_safely` 使用 `catch_unwind` 实现；若直接调用 `publish_statement_ru_full_metrics`，sink panic 会向调用者传播。

边界规则包括：无 `snapshot.report` 时 full 发布器立即返回，连 success 计数也不发；有 report 时，即使没有任何非零 unit，仍发布一次 success/calibration-state。unit 的 `0.0` 被跳过，而 NaN 因 `NaN != 0.0` 会被传给 sink，但正常生产快照已在 `finalize` 检查最终 RU；本文件并未逐字段验证 report 单位。负权重、负证据或 TiFlash 子量超过总量也不会在这里被修正，最终是否合法由 finalize 的 RU 结果检查决定。

非 TiFlash 的 `add_operator` 只转移 scan/net，不转移 `cross_az_net_bytes`；这与当前 Go 实现一致。`add_statement_units` 即使 frontend bytes 为零也把 TiDB/Frontend 标成 seen，但发布器仍跳过零值序列。固定下标和固定标签表是刻意的低基数设计，不应替换为 SQL 文本、plan ID、表名或索引名等无界标签。

## 并发与资源生命周期

本文件不创建线程、锁、任务、通道或事务，也不拥有外部资源。所有类型都是普通值；`StatementRUFullReport` 通过 `&mut self` 更新，Rust 借用规则阻止同一实例在安全代码中无同步并发写入。跨线程共享若有需要，锁与所有权必须由调用方提供。

报告生命周期由 `StatementRUCalculator` 管理：result 模式的 `report` 为 `None`，不分配二维报告；full 模式创建有界数组。finalize clone 当前报告、只在 clone 上追加语句级单位，再把副本移入快照，因此发布阶段读取的是冻结值，不借用物理计划或 runtime stats，也不受后续 live evidence 修改影响。`publish_statement_ru_full_metrics` 仅同步借用 sink/snapshot；生产 sink 内部的指标初始化锁和 panic 隔离均位于 `statement_ru_result.rs`，不属于本文件职责。

## 与 Go 版本的对应关系

主要数据结构与公式直接对应 `pkg/executor/statement_ru_reporting.go`：Rust 的 engine/operator 枚举、compute/result/full-report、`add`、`add_operator`、`statement_ru_engine_result`、`statement_ru_tiflash_ru`、`add_statement_units` 和 full metrics 发布，分别对应 Go 的同名 camelCase 类型/方法。Rust 测试名中的 `go_merge_195`/`go_merge_197` 也明确固定了这些移植语义。

已对齐的关键行为包括：非 TiFlash scan/net 归 TiKV；TiFlash 工作完整留在 TiFlash；TiDB 的 join output 与 TiKV 的 scan/net 扣除 TiFlash 部分；frontend/write/KV-write 只记到规定引擎/类别；full report 跳过 unseen 与零值指标；成功原因使用 calibration state；unsupported/ineligible 为 skipped，其余失败原因是 failed。

Rust 与 Go 的边界划分并非逐函数一比一。Go 文件还包含 `statementRUOperatorForPlan`、`statementRUFailed`、`statementRUTerminalFailure`、`publishStatementRUFailureSafely`；Rust 将物理计划到 operator 的映射放在 `statement_ru_plan_walk.rs`，把终止判定和安全发布放在 `statement_ru_result.rs`/调用路径。Go `publishStatementRUFullMetrics` 直接写全局 Prometheus 指标且接收非可选 report；Rust 通过 sink 解耦，并显式在 `None` 时返回。Rust 的 public 类型/函数也比 Go 包内私有符号更开放，扩展时需把它们视为 crate API。

独立测试对应关系：Rust `statement_ru_reporting_test.rs` 覆盖引擎公式、TiFlash 独立归属、远端单位拆分、失败标签以及全部 11 种 unit 标签；Rust `statement_ru_result_test.rs` 与 `statement_ru_plan_walk_test.rs` 覆盖 finalize、倍率、冻结报告和调用链。Go `statement_ru_reporting_test.go`/`statement_ru_result_test.go` 额外覆盖 full/result 模式、冻结后 live evidence 变化、指标隔离、失败矩阵和 Prometheus 实际序列。

## 扩展指南

- 新增引擎时，必须同步 `StatementRUEngine`、`ENGINE_COUNT`、`StatementRUComputeUnits` 的承载方式、`statement_ru_engine_result` 公式、发布器 `ENGINES`、计划遍历归属、资源组 reporter 接口以及独立 Rust/Go 对照测试；三元素数组解构也必须调整。
- 新增或重分类算子时，同步 `StatementRUOperator`、`OPERATOR_COUNT`、发布器 `OPERATORS`、`statement_ru_plan_walk.rs` 的 plan-to-operator 映射，并在 `statement_ru_plan_walk_test.rs` 验证真实计划分类。不要引入 SQL/对象名作为标签。
- 新增计量单位时，同步 `StmtUnits`/`StmtWeights` 的上游模型、compute 投影、引擎公式、full publisher 单位表、校准/守恒检查与 `statement_ru_reporting_test.rs` 的全标签测试。先决定它属于本地引擎、远端存储还是语句级一次性证据。
- 修改 TiFlash 计费时，区分“原始 TiFlash RU 公式”和“实验倍率”两个层次；同时检查 `StatementRUCalculator::finalize` 与 `statement_ru_plan_walk.rs` 的 EXPLAIN premium，避免重复或遗漏倍率。
- 修改失败分类时，同步 `label()`/`status()`、adapter/plan-walk/result 的构造点，以及 Rust `go_merge_195_failure_status_matches_go_metric_labels` 和 Go failure matrix。
- 测试继续放在独立的 `pkg/executor/statement_ru_reporting_test.rs`、`statement_ru_result_test.rs` 或 `statement_ru_plan_walk_test.rs`，不要内嵌到生产源文件。兼容风险集中在指标标签和计费归属，正确性风险集中在重复计数、负差值与倍率重复应用，性能风险主要是 full 模式固定报告的 clone 和逐非零 unit 的 sink 调用。

## 验证依据

- Rust 源与装配：`pkg/executor/statement_ru_reporting.rs`、`pkg/executor/lib.rs:23-30`、`pkg/executor/Cargo.toml`（package `astersql-executor`、库入口 `lib.rs`、路径依赖 `astersql-resourcegroup`）。目标包未发现 `doc.go`。
- Rust 直接调用链：`pkg/executor/statement_ru_result.rs:118-178,386-497`；`pkg/executor/statement_ru_plan_walk.rs:835-932,1318-1325`；失败原因的 adapter 使用点位于 `pkg/executor/adapter.rs:967-1029`。
- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点、1,848,419 边；`query/node/callers/callees` 核实 `StatementRUFullReport`、`statement_ru_engine_result`、`statement_ru_tiflash_ru`、`publish_statement_ru_full_metrics`、`StatementRUFailureReason`。图明确给出 `statement_ru_engine_result -> statement_ru_tiflash_ru`、计划遍历与 engine result 对 TiFlash 函数的调用，以及 `publish_statement_ru_finalized_snapshot -> publish_statement_ru_full_metrics -> sink.unit/sink.statement`。按路径执行 `files --filter` 未命中，因此文件全貌与未被图识别的方法调用用源码和 `rg` 补证。
- Rust 测试：`pkg/executor/statement_ru_reporting_test.rs`；相关链路测试 `pkg/executor/statement_ru_result_test.rs`、`pkg/executor/statement_ru_plan_walk_test.rs`。这些测试分别固定公式/标签、finalize/发布和计划证据行为。
- Go 对照：`pkg/executor/statement_ru_reporting.go`、`pkg/executor/statement_ru_reporting_test.go`、`pkg/executor/statement_ru_result.go`、`pkg/executor/statement_ru_result_test.go`、`pkg/executor/statement_ru_plan_walk.go`。
- 本任务只新增说明文档，按计划不运行 Cargo。结构验证确认目标文件存在且恰好包含规定的 11 个二级标题；人工复核覆盖了文件存在原因、运行流程、安全扩展点、当前边界与 Go/Rust 差异。
