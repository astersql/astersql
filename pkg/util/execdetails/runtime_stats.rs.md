# `pkg/util/execdetails/runtime_stats.rs`

## 文件定位

本文件实现 SQL 执行期统计的核心数据模型：它把根执行器本地采样、下推到 TiKV/TiFlash 的 Cop 任务摘要、事务提交/加锁明细、并发度和 RU 等异构信息，按物理计划节点 `planID` 聚合，并提供 EXPLAIN、慢查询及资源核算可消费的字符串或值快照。源码不是单独的 Rust 模块声明；`pkg/util/execdetails/internal/group1/lib.rs` 的 `runtime_stats_impl` 通过 `include!("../../runtime_stats.rs")` 编译它并 `pub use`，随后 `pkg/util/execdetails/lib.rs` 以 `execdetails_group_1::*` 对外再导出。

所属 crate 是 `astersql-util-execdetails`。其 `Cargo.toml` 将 `lib.rs` 作为入口，并依赖承载主体实现的 `astersql-util-execdetails-group1`、RUv2 与 util 子 crate，以及 protobuf/百分位计算依赖；因此本文件中的 `kv`、`tipb`、`Percentile`、`TiflashStats`、`FormatDuration` 和 `util::*` 均来自 `runtime_stats_impl` 外层的 `use crate::*`，而不是本文件自己的显式导入。

在应用链路中，`pkg/distsql/select_result.rs` 将远端响应的扫描、时间、线程池和 `ExecutorExecutionSummary` 写入 `RuntimeStatsColl`；`pkg/executor/statement_ru_plan_walk.rs` 读取根/Cop 行数快照进行 RU 归属；`pkg/session/runtime/scan_adapter_runtime.rs` 通过共享收集器登记提交统计；hash join/hash aggregate 执行器登记 `HashStateRuntimeStats`。这些统计最终由计划展示、语句上下文和进程信息持有者读取。

## 核心职责

1. 用 `RuntimeStats` trait 统一“类型编号、合并、克隆、格式化”协议。类型编号 `TpBasicRuntimeStats` 至 `TpWriteRuntimeStats` 是同一计划节点内查找同类统计并合并的稳定判别键。
2. 用 `BasicRuntimeStats` 记录根执行器的 open/next/close 耗时、循环次数与输出行数；用 `CopRuntimeStats`/`basicCopRuntimeStats` 聚合远端任务的耗时百分位、行数、循环、并发线程、扫描和 TiFlash 专项信息。
3. 用 `RuntimeStatsColl` 按 `planID` 管理 root、cop、Analyze 扫描字节、摘要覆盖期望、语句级 TiFlash 网络流量和 TiFlash execution units，并支持对象复用时清空状态。
4. 提供不暴露可变内部对象的证据快照：`RootRowsSnapshot`、`CopRowsSnapshot`、`HashStateRowsSnapshot`、`GetRootWriteCPUWork` 和扫描明细副本。
5. 格式化事务提交/锁键、并发度与 RU 信息，供 EXPLAIN/诊断文本直接拼接；空或未观测数据通常输出空串，避免产生虚假统计。

## 主要符号

- `RuntimeStats: Any + Send + Sync`：动态统计接口。`String` 生成展示片段，`Merge` 合并同类值，`CloneBox` 复制 trait object，`Tp` 返回类型编号，`as_any` 支持 Rust downcast。`Send + Sync` 使包含这些对象的 `RuntimeStatsColl` 可被 `Arc` 跨线程共享。
- `BasicRuntimeStats`：原子字段 `executorCount/loopCount/consume/open/close/rows`。`Record` 记录一次 Next，`RecordOpen`/`RecordClose` 将阶段耗时同时计入总耗时，`SetRowNum` 可覆盖行数，`String` 在同 plan id 有多个执行器实例时使用 `total_` 前缀。它的 `Clone`/`CloneBox` 故意 panic，因为相同 executor id 应共享同一个基础统计，复制容易重复计数。
- `basicCopRuntimeStats`：单类 Cop 摘要累加器。`mergeExecSummary` 从 protobuf 可选字段累计迭代数、输出行、处理时间、并发度，并按 TiFlash 子消息的实际存在性懒建 `TiflashStats`；`Merge` 合并百分位样本与 TiFlash 统计。
- `CopRuntimeStats`：组合基本 Cop 统计、`ScanDetail`、`TimeDetail`、可选 `PoolTaskDetails`、store 类型及摘要证据计数。`String` 对单任务和多任务分别展示直接耗时或 max/min/avg/p80/p95；TiKV 追加扫描/时间/read-pool，TiFlash 追加线程、等待、网络与 columnar/scan context。
- `RootRuntimeStats`：一个可选 `BasicRuntimeStats` 加多个 `Box<dyn RuntimeStats>`；`String` 忽略空片段后用逗号连接。
- `RuntimeStatsColl`：核心聚合器。`rootStats` 与 `copStats` 是 plan id 索引，`sharedRootStats` 接受 `&self` 下的跨线程登记，`copResponseSummaryExpected` 保存“期望数+失效标记”，`analyzeScanBytes` 保存逐请求估算后的累计值。
- `NewRuntimeStatsColl`：创建或复用收集器。复用分支清空 root/shared/cop/Analyze/摘要期望/TiFlash units；源码当前没有重置 `stmtCopStats`，这是当前实现事实，扩展复用语义时必须连同 Go 对照和测试核查。
- `RegisterStats`/`RegisterStatsShared`：按 `(planID, Tp())` 找同类对象合并，否则追加；`GetRootStats` 会把 shared 暂存项移入 owned root，`GetRootStatsStringShared` 则在不消费 shared 项的情况下克隆并组合展示。
- `RecordCopStats`/`RecordOneCopTask`：远端统计入口。前者还合并扫描、时间与 read-pool；两者都可用 summary 的 executor id 尾段重定向 `planID`，更新摘要证据、Cop 基础统计和语句级 TiFlash 网络统计。
- `RootRowsSnapshot`/`CopRowsSnapshot`：区分“真实零行”“尚未观察”“摘要不完整/矛盾”。Cop 的 `Observed` 要求期望数与已见数均为正且已见数不超过期望数；`Complete` 还要求两者相等。
- `EstimateScanBytes`：按 `processedBytes / processedKeys * totalKeys` 对单个逻辑请求估算扫描字节，拒绝负值、无样本但有字节、零 total/bytes 及非有限结果。比例必须在各请求扫描明细被合并前计算。
- `WriteRuntimeStats` 与 `HashStateRuntimeStats`：分别保存写入 CPU work 和 hash-state 行数。后者用 CAS 累加；转换/加法溢出后写 `-1` 并永久视为无效。
- `RuntimeStatsWithConcurrencyInfo`：保存并发项，非正并发数格式化为 `OFF`；其 `Merge` 当前为空，因此同类型重复注册不会累计。
- `RuntimeStatsWithCommit`：合并并格式化 commit、普通 lock keys、shared lock keys。退避类型会去重排序；锁保护的最慢 RPC/退避列表与原子 resolve-lock/region 计数分别按其同步机制读取。
- `RURuntimeStats`/`ExplainRURuntimeStats`：前者展示语句级 RU v1 的 `RRU + WRU`，后者累计并展示算子 `SelfRU/CumRU`；全零时为空串。

## 执行流程

根执行器路径如下：执行器先通过 `GetBasicRuntimeStats(planID, true)` 获取或创建共享基础统计并增加 executor 数，运行期间调用 `RecordOpen`、`Record`、`RecordClose`；专项统计通过 `RegisterStats`，共享的 session/commit 路径则用 `RegisterStatsShared`。读取时 `GetRootStats` 把 shared 暂存项按类型归并，`RootRuntimeStats::String` 将基础统计与各专项片段串接。`GetRootRowsSnapshot` 只在至少一次 `Record` 后把行数标为 observed，因此“仅 SetRowNum(0)”与“真实执行后得到 0 行”可区分。

Cop 路径从 `pkg/distsql/select_result.rs` 进入：

1. `RecordCopStats` 先在原始 `planID` 下创建或取得 `CopRuntimeStats`；首次写入直接复制 scan/time，后续才调用 `Merge`，避免首条明细重复累计。
2. 非空 read-pool 明细与既有值合并。若 summary 包含形如 `table_scan_42` 的 executor id，`getPlanIDFromExecutionSummary` 解析最后一个下划线片段并将摘要改记到 plan 42；扫描/time 仍属于调用时传入的原 plan 项，这是与 Go 实现一致的分段行为。
3. `recordSummaryEvidence` 累加 produced rows 和摘要个数，`basicCopRuntimeStats::mergeExecSummary` 累加迭代、处理时间、线程和 TiFlash 子统计，`StmtCopRuntimeStats` 汇总整条语句的 TiFlash 网络信息。
4. 上游在消费响应时先用 `RecordExpectedCopResponseSummaries` 登记每个有效 plan id 应有的摘要槽；畸形向量用 `InvalidateCopResponseSummaries` 标记。`GetCopRowsSnapshot` 再把实际摘要数与期望数比较，供 RU plan walk 判断完整性。

展示路径中，`CopRuntimeStats::String` 会克隆百分位容器后查询分位数。单任务输出任务耗时和 loops；多任务输出分布及任务数。store 类型决定附加字段：TiFlash 使用线程和列存专项摘要，其他 store 使用扫描、time detail 和 read-pool。

事务路径中，`RuntimeStatsWithCommit::MergeCommitDetails` 首次接管详情并把 `TxnCnt` 置 1，后续调用 util 层 `Merge`；`MergeCommitStats` 合并整个统计对象。`String` 只展示非零字段，并在持有详情 mutex 时复制或格式化受保护字段，随后释放 guard 再读原子计数。

## 数据与状态

`RuntimeStatsColl` 的主要状态按 plan id 隔离，但一个 plan 下的动态统计以 `Tp()` 而不是 Rust 具体类型作为合并键；新增类型必须分配不冲突的编号。`rootStats` 保存 owned 数据，`sharedRootStats` 是共享引用下的缓冲区：字符串读取会克隆后合并，owned 读取会 `remove` 并消费缓冲区，避免重复计入。

`BasicRuntimeStats` 与 `HashStateRuntimeStats` 使用 `AtomicI32/AtomicI64`，单位分别是次数、行数和纳秒。所有访问均为 `Ordering::Relaxed`，只保证每个计数的原子性，不建立跨字段一致快照；例如 rows 与 loopCount 可能来自相邻但非同一瞬间的加载。`HashStateRuntimeStats` 用负数作为无效哨兵，一旦 CAS 计算溢出，后续 `AddRows` 直接返回。

Cop 摘要完整性有三类状态：默认值表示无证据；`ObservedSummaries > 0` 且不超过正的 `ExpectedSummaries` 表示至少有可用证据；两者相等才完整。失效标记、负行数或实际摘要多于期望都令结果不可用。计数使用 wrapping add，忠实保留源码行为而不承诺溢出检测。

`scanDetailObserved` 是 Rust 的 presence bridge：`GetCopScanDetail` 保留 Go 风格的“有 Cop 项即返回零值明细”，`GetObservedCopScanDetail` 只有确实传入过 scan 时才返回。`analyzeScanBytes` 只接受正 plan id、非负有限值，并按 plan 累加。

## 依赖与调用关系

上游生产调用证据包括：

- `pkg/distsql/select_result.rs` 调用 `RecordCopStats`、`RecordOneCopTask` 和 `EstimateScanBytes`，是远端 response 到统计集合的主要桥接点。
- `pkg/executor/statement_ru_plan_walk.rs` 调用 `GetRootRowsSnapshot`/`GetCopRowsSnapshot`，将已验证的行数证据用于算子 RU 遍历。
- `pkg/session/runtime/scan_adapter_runtime.rs` 调用 `RegisterStatsShared` 登记 `RuntimeStatsWithCommit`，对应 `StmtCtx`/process info 共享 `Arc<RuntimeStatsColl>` 的场景。
- `pkg/executor/aggregate/agg_hash_executor.rs`、`pkg/executor/join/hash_join_v1.rs` 和 `hash_join_v2.rs` 持有 `HashStateRuntimeStats` 与 `Arc<Mutex<RuntimeStatsColl>>`，在执行器生命周期结束时提交 typed hash-state 证据。
- `pkg/sessionctx/stmtctx/stmtctx.rs`、`pkg/session/sessmgr/processinfo.rs`、`pkg/distsql/context/context.rs` 保存或转递收集器；`pkg/planner/core/operator/physicalop/*` 读取实际行数/探测次数。

下游依赖来自 group1 crate 外层：`tipb::ExecutorExecutionSummary` 提供远端摘要，`kv::StoreType` 决定 TiKV/TiFlash 展示分支，`Percentile<Duration>` 计算分位数，`TiflashStats`/`TiFlashNetworkTrafficSummary` 合并列存信息，`util::ScanDetail`、`TimeDetail`、`PoolTaskDetails`、`CommitDetails`、`LockKeysDetails` 与 `RUDetails` 提供细分数据及自身的 `Merge/Clone/String`。

RustCodeGraph 将本文件标记为被 12 个文件引用，并能精确定位 `RuntimeStatsColl`、`RecordCopStats`、`EstimateScanBytes` 等 Rust/Go 同名符号；由于实现经 `include!` 注入，图的 `callers/callees` 未返回这些方法的边，故上述调用关系由索引的引用文件列表和生产源码精确引用共同核验。

## 错误处理与边界

该模块没有 `Result` 型业务错误；无效输入通常被忽略或编码为返回标志/哨兵。`EstimateScanBytes` 返回 `(0.0, false)` 表示估算不可用；空 processed keys 且空 bytes 是合法零值。executor id 解析失败时保留调用方给出的 plan id。`RecordAnalyzeScanBytes` 忽略非正 plan id、负数、NaN 和无穷值。

锁中毒使用 `expect(...)` 直接 panic，消息区分 runtime collector、shared stats、TiFlash units、commit detail 与 lock-key detail。`BasicRuntimeStats::Clone` 也明确 panic，这是禁止误复制的契约而非未实现占位。动态 `Merge` 先 `downcast_ref`；类型不匹配时静默不合并，但正常注册路径还要求相同 `Tp()`，因此类型编号冲突会造成数据被忽略，是新增统计类型的兼容风险。

格式化只输出已观察或非零字段。`RuntimeStatsWithCommit::formatLockKeysDetails` 可能在首个有效字段是 lock RPC 时保留前导逗号（现有 Rust 测试锁定了该字符串）；修改格式必须同步核对 Go 输出和所有 golden/assertion。`CopRuntimeStats::String` 是 `&mut self`，尽管当前只克隆百分位状态后读取；调用方需要可变统计或克隆副本。

## 并发与资源生命周期

`RuntimeStatsColl` 同时存在 `&mut self` API 与内部 mutex。常规 distsql 路径通常把集合包在外层 `Arc<Mutex<_>>` 后调用可变方法；session/process-info 共享路径无法取得 `&mut`，因此 `RegisterStatsShared` 使用 `sharedRootStats: Mutex<_>`。`GetRootStatsStringShared` 的锁顺序是 `mu` 后 `sharedRootStats`；复用和 `GetRootStats` 也遵循这一顺序，扩展时不得反向获取以免死锁。

`RuntimeStats` 的 `Send + Sync` 保证 trait object 可随 `RuntimeStatsColl` 进入 `Arc`。不过 `RuntimeStatsColl` 多个普通 getter 返回内部引用，并依赖调用时借用规则而非 guard 生命周期；它们适合 owned/外部加锁路径。需要跨线程安全读取时应优先增加值快照或使用现有 shared 字符串接口，不应泄露新的内部可变引用。

原子统计选择 relaxed ordering，因为这里只累计观测值，不以计数器发布其他内存状态。多字段输出不是事务快照；若新功能要求字段间强一致，应在同一 mutex 下维护和读取，而不是单纯提高某一个 atomic 的 ordering。

对象生命周期以语句为主。`NewRuntimeStatsColl(Some(old))` 复用已分配的 maps 并清空语句状态；调用方不得在复用后继续把旧 plan id 的引用或快照视为有效。trait object 的克隆仅用于组合 shared/owned 展示，`BasicRuntimeStats` 不允许进入这条克隆路径。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/execdetails/runtime_stats.go`，核心布局与流程基本逐项对应：`RuntimeStats` 接口、root/cop/basic 统计、按 plan id 聚合、summary executor id 重定向、Cop 展示、提交/锁键/RU 格式化，以及扫描字节和行数证据快照都有同名实现。Rust 采用 `Option` 代替 nil、`Box<dyn RuntimeStats>` 代替 Go interface、`AtomicI*` 代替 `atomic.Int*`、RAII mutex guard 代替手动 Lock/Unlock。

Rust 为可运行的跨线程对象模型增加了 `as_any`、`CloneBox` 和 trait 的 `Send + Sync`；还增加 `sharedRootStats`、`RegisterStatsShared`、`GetRootStatsStringShared`，用以支持 `Arc<RuntimeStatsColl>` 下的 commit 统计登记。`GetObservedCopScanDetail`/`scanDetailObserved` 是 presence-aware bridge，而 Go 兼容 getter 仍保留零值语义。Rust `GetRootWriteCPUWork` 和 hash-state getter通过 downcast 返回 typed 快照。

Go 测试 `pkg/util/execdetails/execdetails_test.go` 验证 Cop/TiFlash 汇总、摘要覆盖、扫描估算、root rows、read-pool、事务格式等语义；Rust 对应测试集中在独立文件 `pkg/util/execdetails/execdetails_test.rs` 和 group1 crate 挂载的 `pkg/util/execdetails/execdetails_1_aster_unit_test.rs`。后者额外覆盖共享收集器、同类型合并、plan id 重定向、退避去重和 RU clone/merge。测试没有内嵌在生产文件，符合仓库约束。

## 扩展指南

新增一种 root 专项统计时，应在本文件分配唯一 `Tp*` 常量，实现完整 `RuntimeStats`（特别是同类型 `Merge`、深/值克隆和空值 `String`），再从实际执行器生命周期调用 `RegisterStats` 或 `RegisterStatsShared`。同时在独立 Rust 测试中验证两次注册会合并而非重复展示、shared/owned 读取一致、空统计不污染输出；若是 Go 移植，还需同步 `runtime_stats.go` 的类型编号与格式语义。

扩展 Cop 摘要字段时，优先修改 `basicCopRuntimeStats::mergeExecSummary` 及其 `Merge/String`，并明确字段属于 plan 级、store 级还是 statement 级。TiFlash protobuf 子消息必须继续按 presence 懒建统计，避免“缺失”被误判为全零。涉及摘要覆盖的改动应同时验证真实零行、部分响应、额外摘要、畸形向量和 plan id 重定向。

增加并发读 API 时，优先返回标量或 owned snapshot，仿照 `GetRootRowsSnapshot`/`GetObservedCopScanDetail`，不要把 mutex 内部引用暴露到 guard 外。若需要可共享写入，沿用 `sharedRootStats` 模式并保持 `mu -> 子锁` 的锁顺序。增加原子复合状态前先确认是否允许弱一致观察。

修改显示格式或事务字段时，应同步 `pkg/util/execdetails/execdetails_test.rs`、`execdetails_1_aster_unit_test.rs` 与 Go 的 `execdetails_test.go`；关注单位（内部纳秒）、零值过滤、标点顺序、退避去重排序及 mutex/atomic 字段。修改对象复用时应覆盖所有字段，特别检查 `stmtCopStats` 和 `tiFlashExecutionUnits` 是否按预期重置。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/util/execdetails` 找到目标 Rust/Go 文件与独立测试；`node --file pkg/util/execdetails/runtime_stats.rs` 完整检查了 1,659 行实现并报告 174 个符号、12 个引用文件；`query RuntimeStatsColl`、`query RuntimeStats`、`query RecordCopStats --json`、`query RegisterStatsShared --json`、`query EstimateScanBytes --json` 核对了关键签名及 Go 对照。`callers/callees` 因 `include!` 未产生方法边，此限制用下列源码引用补证。
- 编译与导出边界：`pkg/util/execdetails/Cargo.toml`、`pkg/util/execdetails/lib.rs`、`pkg/util/execdetails/internal/group1/lib.rs`；后者的 `runtime_stats_impl` 是目标文件的真实编译入口。
- 生产调用：`pkg/distsql/select_result.rs`、`pkg/executor/statement_ru_plan_walk.rs`、`pkg/session/runtime/scan_adapter_runtime.rs`、`pkg/sessionctx/stmtctx/stmtctx.rs`、`pkg/session/sessmgr/processinfo.rs`、`pkg/executor/aggregate/agg_hash_executor.rs`、`pkg/executor/join/hash_join_v1.rs`、`pkg/executor/join/hash_join_v2.rs`。
- Go 对照：`pkg/util/execdetails/runtime_stats.go`，重点核对 `RuntimeStatsColl`、`EstimateScanBytes`、root/Cop snapshots、`RecordCopStats`、`RecordOneCopTask`、并发和 commit/RU 统计。
- 独立测试：`pkg/util/execdetails/execdetails_test.rs`、`pkg/util/execdetails/execdetails_1_aster_unit_test.rs`、`pkg/util/execdetails/tiflash_execution_units_test.rs`，以及 Go 的 `pkg/util/execdetails/execdetails_test.go`、`tiflash_execution_units_test.go`。覆盖真实零行与摘要完整性、无效扫描估算、首次 Cop 明细不重复、TiKV/TiFlash 格式、共享/owned 合并、hash-state 溢出、事务/锁键/RU 格式和复用清理。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行任务规定的 11 章节结构检查，并人工复核所有行为描述均可回溯到上述符号、调用点或测试。
