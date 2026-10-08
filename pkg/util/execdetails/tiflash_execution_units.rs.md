# `pkg/util/execdetails/tiflash_execution_units.rs`

## 文件定位

本文件实现按执行计划节点汇总 TiFlash/MPP 原始执行证据的 Rust 版本。它不是独立模块：`pkg/util/execdetails/internal/group1/lib.rs` 的 `runtime_stats_impl` 在包含 `runtime_stats.rs` 后紧接着 `include!("../../tiflash_execution_units.rs")`，因此本文件可以直接为 `RuntimeStatsColl` 增加方法，并使用同一作用域中的 `HashSet`、`tipb`、`getPlanIDFromExecutionSummary` 等名字。`astersql-util-execdetails-group1` 随后由 `pkg/util/execdetails/lib.rs` 通过 `execdetails` 模块统一重导出。

顶层 `pkg/util/execdetails/Cargo.toml` 声明的 crate 是 `astersql-util-execdetails`，其依赖 `internal/group1` 对应的 `astersql-util-execdetails-group1`。当前目标文件实际编入后者；`internal/group1/lib.rs` 还定义了一组简化的 `tipb` 兼容类型，因此当前 Rust 实现并未在这个 crate 中直接依赖生成的 TiDB protobuf 类型。这是理解其接线与迁移成熟度时必须保留的边界。

## 核心职责

文件承担三件事：

1. 用 `TiFlashExecutionUnits` 保存行数、哈希表规模、扫描字节和跨区域网络发送字节，并用 `Observed`、`Missing` 区分“明确观测到零”和“没有证据”。
2. 从单个 `tipb::ExecutorExecutionSummary` 提取上述证据，再按 plan ID 将多次报告安全累加到 `RuntimeStatsColl::tiFlashExecutionUnits`。
3. 对不可信汇总打上粘性的 `Invalid` 标记，包括同一批次出现重复 plan ID、任意 `u64` 累加溢出，以及总行数无法安全转换为下游使用的 `i64`。

它只保存原始证据，不计算最终 RU，也不证明任务覆盖完整性。文件头注释明确指出 presence 位描述收到的字段，而不是 TiFlash task 覆盖度；最终消费发生在 `pkg/executor/statement_ru_plan_walk.rs` 的 `snapshot_statement_ru_runtime_evidence`。

## 主要符号

- `TiFlashUnitFields = u8`：presence 位图类型。
- `TiFlashUnitRows`、`TiFlashUnitHash`、`TiFlashUnitScan`、`TiFlashUnitNetwork`：值分别为 `1`、`2`、`4`、`8`；私有常量 `ALL_FIELDS` 是四位之或。
- `TiFlashExecutionUnits`：可复制的值快照。六个 `u64` 数值字段分别是 `Rows`、`HashDistinctEntries`、`HashBuildRows`、`UserReadBytes`、`InnerZoneSendBytes`、`InterZoneSendBytes`；`Observed` 与 `Missing` 保存证据状态；`Invalid` 保存不可安全使用状态。
- `tiFlashExecutionUnits(&ExecutorExecutionSummary) -> TiFlashExecutionUnits`：私有的单摘要解析器，依据字段是否存在设置数值和 presence 位。
- `TiFlashExecutionUnits::merge(&mut self, other)`：私有累加器。presence 与 invalid 状态取或；每个数值用 `checked_add`，溢出时保持原值并置 `Invalid`；最终还校验 `Rows <= i64::MAX`。
- `RuntimeStatsColl::RecordTiFlashExecutionSummaries(&self, planIDs, summaries)`：公开写入口，过滤摘要并在互斥锁保护下按 plan ID 聚合。
- `RuntimeStatsColl::GetTiFlashExecutionUnits(&self, planID)`：公开读入口，返回复制出的不可变值快照和 found 标志，不把锁或内部 map 引用泄露给调用者。

文件没有 trait、枚举声明、条件编译项或显式错误类型；条件测试装配位于 `pkg/util/execdetails/lib.rs`。

## 执行流程

单条摘要先经 `tiFlashExecutionUnits` 转换：存在 `NumProducedRows` 时记录行数并置 `TiFlashUnitRows`；哈希统计只有同时存在 `Size_` 且 `SizeKind` 为 `DistinctKeyCount` 或 `BuildRowCount` 时才分别写入对应字段并置同一个 `TiFlashUnitHash`，未知 kind 不作为错误，也不建立 hash 证据。扫描字节优先取 `TiflashScanContext.UserReadBytes`，只有前者缺失时才回退到 `ColumnarScanContext.UserReadBytes`；不会把 `MvccInputBytes` 加入用户读取字节。网络摘要的两个数值各自以缺失即零读取，但只有两者都存在才置 `TiFlashUnitNetwork`。最后以 `ALL_FIELDS & !Observed` 计算该条摘要的 `Missing`。

`RecordTiFlashExecutionSummaries` 的流程如下：

1. 空切片直接返回，不创建 map 项。
2. 锁住 `RuntimeStatsColl::tiFlashExecutionUnits`，并创建仅对本次调用有效的 `seen` 集合。
3. 跳过 `None`；用 `getPlanIDFromExecutionSummary` 解析 `ExecutorId` 最后一个下划线分段为 `i32`。
4. 跳过解析失败、非正数或不在调用方 `planIDs` gather 中的 ID，防止无关执行器贡献到其他 plan。
5. 为合格 ID 获取或创建默认汇总。同一批调用中第二次出现同一 ID 时只将已有项标为 `Invalid`，不合并重复值；首次出现时才解析并 `merge`。

不同调用批次中的相同 ID 会继续累加。这与“一个已消费响应或一个 direct task report”为一次调用的约定相匹配，跨路由和跨响应去重由调用者负责。Rust 生产调用证据包括 `pkg/distsql/select_result.rs::consume_response`：仅当 `mpp_reports_directly` 明确为 false 时，从响应路线记录 summaries，以避免与直接上报路线重复。

读取时，`GetTiFlashExecutionUnits` 在锁内查 map，命中则复制 `TiFlashExecutionUnits` 并返回 `(units, true)`，否则返回 `(default, false)`。`pkg/executor/statement_ru_plan_walk.rs::snapshot_statement_ru_runtime_evidence` 对去重后的 plan IDs 调用它，并仅在 found 时构造 `StatementRUPlanEvidence.tiflash`。

## 数据与状态

`Observed` 与 `Missing` 不是互补的全局最终状态。单摘要内二者互补，但多摘要合并时均采用按位或：某一批曾缺少行数、另一批又明确提供行数后，行数位会同时出现在 `Observed` 和 `Missing`。这保留了“至少一次观察到”和“至少一次缺失”的历史，测试 `go_merge_22_tiflash_units_preserve_presence_and_overflow` 明确覆盖该语义。

明确的零值与字段缺失不同：`Some(0)` 会置相应 `Observed` 位，而 `None` 只通过 `Missing` 表达。不存在 plan ID 的 map 项又与“存在一个全部字段缺失的摘要”不同，前者 found 为 false，后者 found 为 true 且 `Missing == 15`。

`Invalid` 是粘性状态，正常的后续证据不会清除它。数值溢出时该字段保持溢出前的值；例如两次累加 `u64::MAX` 扫描字节后仍保存 `u64::MAX` 并标记 invalid。`NewRuntimeStatsColl(Some(reuse))` 会清空整个 TiFlash map，因此 collector 复用不会把上一 statement 的相同 plan ID 证据带到下一 statement。

## 依赖与调用关系

下游依赖均来自包含点的同一模块作用域：

- `tipb::ExecutorExecutionSummary` 及其 scan、network、hash 子结构来自 `internal/group1/lib.rs` 的简化 `tipb` 模块。
- `getPlanIDFromExecutionSummary` 来自 `runtime_stats.rs`，按 `ExecutorId` 最后一个 `_` 后缀解析 plan ID，例如 `TableScan_1 -> 1`。
- `RuntimeStatsColl::tiFlashExecutionUnits` 在 `runtime_stats.rs` 中定义为 `Mutex<HashMap<i32, TiFlashExecutionUnits>>`，构造和复用路径分别初始化、清空它。
- `HashSet`、`HashMap` 和 `Mutex` 由 `internal/group1/lib.rs` 的外围作用域导入。

RustCodeGraph 将目标文件列为被 `pkg/distsql/distsql_test.rs`、`pkg/executor/statement_ru_plan_walk_test.rs` 和本目录测试使用。生产源码搜索进一步确认写路径为 `pkg/distsql/select_result.rs::consume_response -> RecordTiFlashExecutionSummaries`，读路径为 `pkg/executor/statement_ru_plan_walk.rs::snapshot_statement_ru_runtime_evidence -> GetTiFlashExecutionUnits`。Go 侧还有 `pkg/executor/internal/mpp/local_mpp_coordinator.go` 的直接上报路线；当前 Rust 搜索未发现对应生产直接上报调用，所以不能把 Go 的两条采集路线都宣称为已在 Rust 接线。

## 错误处理与边界

本 API 不返回 `Result`。输入级异常采用过滤或状态位表达：无摘要、`None`、非法/非正 plan ID、gather 外 ID均被忽略；未知 hash kind 保持 hash 缺失；重复和算术/行数转换溢出通过 `Invalid` 暴露。调用者必须同时检查 found、presence 位和 `Invalid`，不能只读数值字段。

锁中毒使用 `expect("TiFlash units lock poisoned")`，会 panic，而不是恢复或返回错误。`planIDs.contains` 是线性查找，整体过滤成本约为摘要数乘 plan ID 数；当前实现适合 statement 局部的小集合，若扩大规模需先用基准或真实调用规模证明是否改为集合。

同一批重复 ID 被视为 invalid 且第二份不计数；跨批相同 ID则会累加。这个边界依赖调用者正确划分“单个已消费响应/单个 direct task report”，若调用方批次语义改变，必须同步复核重复判定。

## 并发与资源生命周期

TiFlash map 有独立 `Mutex`，记录与读取都持锁；`GetTiFlashExecutionUnits` 复制值后立即释放锁，调用者无法在锁外修改内部状态。单次记录在遍历全部 summaries 期间持续持锁，保证该批更新原子可见，但也意味着大型批次会延长竞争窗口。

本文件不创建线程、异步任务、通道、事务或外部资源。数据生命周期归属于 statement 的 `RuntimeStatsColl`；新建 collector 创建空 map，复用 collector 时 `NewRuntimeStatsColl` 在返回前清空 map。调用方负责报告路线去重和 collector 生命周期，本文件只负责 map 内串行化与本批重复检测。

## 与 Go 版本的对应关系

`pkg/util/execdetails/tiflash_execution_units.go` 是直接语义对照：常量位、数据字段、摘要提取优先级、`Missing` 计算、checked accumulation、`Rows > MaxInt64` 校验、gather 过滤、本批重复标 invalid 和值快照读取均一一对应。Rust 用 `Option` 表示 protobuf presence，用 `checked_add` 对应 Go 的 `n > MaxUint64-*dst`，用 `Mutex<HashMap<...>>` 对应 Go 在 `RuntimeStatsColl.mu` 下维护的 map。

需要注意两点当前实现差异。第一，Go 使用 `github.com/pingcap/tipb/go-tipb` 生成类型，Rust 当前使用 `internal/group1/lib.rs` 中标注为“简化版”的兼容结构；因此本文只能确认已声明字段的语义对齐，不能推断真实 protobuf oneof、编码或未知字段行为已经端到端对齐。第二，Go 生产代码同时有响应采集与 MPP coordinator 直接报告入口；当前 Rust 生产搜索仅确认响应采集入口，直接报告路线仍未验证。

独立测试形成互证：Rust 的 `pkg/util/execdetails/tiflash_execution_units_test.rs` 覆盖 presence、跨批累加、本批重复、columnar 回退、TiFlash scan 优先、无关 ID、未知 hash kind、网络字段不完整、`u64`/`i64` 边界及 collector 复用清空；Go 的 `tiflash_execution_units_test.go` 覆盖同类契约，并额外体现生成 protobuf 类型的实际字段形态。

## 扩展指南

新增一种执行单位时，应同时修改 `TiFlashUnitFields` 位、`ALL_FIELDS`、`TiFlashExecutionUnits` 字段、`tiFlashExecutionUnits` 提取逻辑和 `merge` 的溢出安全累加；还要同步 Go 文件及两侧独立测试。位图当前是 `u8` 且已使用四位，扩展前需确认序列化/调用方是否依赖具体位值，不能重排现有常量。

若新增摘要来源或 direct-report 路线，应接入 `RecordTiFlashExecutionSummaries`，并确保一次调用准确代表一个去重边界；路线之间的互斥/去重仍应留在上游，而不是在此处用数值猜测。应扩展 `pkg/distsql/distsql_test.rs` 或对应新入口的独立测试，证明同一报告不会从两条路线重复进入。

若替换简化 `tipb` 类型为真实 Rust protobuf，需在独立依赖仓库按仓库规则完成移植和带 tag 引用，并重点验证 presence、oneof scan context、未知 enum 以及整数范围；不可在本 crate 内复制 vendor。任何行为修改应保留测试与源文件分离，并同步 `pkg/util/execdetails/tiflash_execution_units_test.rs`，不要把测试嵌入生产文件。

兼容风险主要是 presence 位含义、同批重复策略和 Go/Rust 字段形态漂移；正确性风险是溢出后误用保留值或忽略 `Invalid`；性能风险集中在持锁遍历和 `planIDs.contains` 的线性过滤。扩展完成后应先证明 Go/Rust 边界测试一致，再验证上游采集和下游 RU 快照。

## 验证依据

- RustCodeGraph `status`：本仓库索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录与文件已索引。
- RustCodeGraph `node --file pkg/util/execdetails/tiflash_execution_units.rs --offset 1 --limit 260`：读取目标文件全部 165 行，确认常量、结构体、解析器、合并器及两个公开方法。
- RustCodeGraph `query TiFlashExecutionUnits`、`query RecordTiFlashExecutionSummaries`、`query GetTiFlashExecutionUnits`：确认 Rust/Go 对应定义；`explore` 确认目标方法的测试调用摘要。精确 `callers/callees` CLI 对这些同名 impl 方法未返回可用边，因此调用关系另由已索引源码节点和精确 `rg` 使用点交叉核验，未采用模糊同名 `Merge` 结果。
- RustCodeGraph 节点：`runtime_stats.rs` 的 `RuntimeStatsColl`、`NewRuntimeStatsColl`、`getPlanIDFromExecutionSummary`；`internal/group1/lib.rs` 的简化 `tipb` 与 include 接线；`distsql/select_result.rs::consume_response`；`statement_ru_plan_walk.rs::snapshot_statement_ru_runtime_evidence`。
- 配置与模块：`pkg/util/execdetails/Cargo.toml`、`pkg/util/execdetails/internal/group1/Cargo.toml`、`pkg/util/execdetails/lib.rs`、`pkg/util/execdetails/internal/group1/lib.rs`。
- Go 对照：`pkg/util/execdetails/tiflash_execution_units.go`、`pkg/util/execdetails/runtime_stats.go`。
- 独立测试：`pkg/util/execdetails/tiflash_execution_units_test.rs`、`pkg/util/execdetails/tiflash_execution_units_test.go`；另核对上游使用点 `pkg/distsql/distsql_test.rs` 与 `pkg/executor/statement_ru_plan_walk_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构检查要求目标文件存在且恰有十一个固定二级标题。
