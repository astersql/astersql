# `pkg/domain/infosync/region.rs`

## 文件定位

`region.rs` 属于 `astersql-domain-infosync` crate，是全局 `InfoSyncer` 与 PD HTTP Region/调度接口之间的一层轻量门面。模块由 `pkg/domain/infosync/lib.rs` 以 `mod region; pub use region::*;` 纳入并重导出；crate 的边界和依赖由 `pkg/domain/infosync/Cargo.toml` 定义。文件不负责建立网络连接、编码表键或解释 SQL，而是接收已经准备好的字节 key range、引擎名或调度器参数，从全局同步器取得 `PdHttpClient` 后转发请求。

当前 Rust 仓库中，五个公开函数除 `pkg/domain/infosync/region_test.rs` 外没有直接调用点；因此它们是已实现、已重导出的移植 API，但不能据此声称 Rust SQL 执行链已经接线。Go 对照实现则由 `pkg/executor/show_placement.go`、`pkg/executor/show.go` 和 `pkg/executor/distribute.go` 调用。

## 核心职责

- `PlacementScheduleState` 把 PD 返回的复制状态字符串压缩成三态领域值，并为 SQL 展示提供稳定字符串。
- `GetReplicationState` 查询半开 key range `[startKey, endKey)` 的 Region 复制进度；PD 客户端不存在时刻意降级为 `Pending`。
- `GetRegionDistributionByKeyRange` 查询给定范围和引擎的 Region/peer 分布。
- `GetSchedulerConfig`、`CreateSchedulerConfigWithInput`、`CancelSchedulerJob` 分别读取调度器配置、创建或更新调度任务、取消指定 job。
- 所有外部操作都通过 `pkg/domain/infosync/types.rs` 中的 `PdHttpClient` trait 完成，使真实客户端与测试替身共享同一边界。

该文件不缓存结果、不重试、不做超时控制，也不构造表或分区范围；这些策略属于调用方或具体客户端实现。

## 主要符号

- `pub enum PlacementScheduleState`：`Copy + Clone + Default + Eq` 的三态枚举。默认值为 `PlacementScheduleStatePending`；另两项为 `InProgress` 与 `Scheduled`。
- `PlacementScheduleState::String(self) -> &'static str`：将三态映射为 `PENDING`、`INPROGRESS`、`SCHEDULED`。注意 `Scheduled` 对应 PD 原始值 `REPLICATED`，展示字符串并非原样返回。
- `GetReplicationState(startKey: Vec<u8>, endKey: Vec<u8>) -> Result<PlacementScheduleState>`：唯一允许 PD 客户端缺失并成功回退的入口；未知状态字符串也回退为 `Pending`。
- `GetRegionDistributionByKeyRange(startKey: Vec<u8>, endKey: Vec<u8>, engine: &str) -> Result<RegionDistributions>`：把两个 `Vec<u8>` 移入 `KeyRange`，再调用 `PdHttpClient::get_region_distribution`。
- `GetSchedulerConfig(schedulerName: &str) -> Result<ConfigValue>`：返回可表达布尔、数字、字符串、数组、对象和空值的 `ConfigValue`。
- `CreateSchedulerConfigWithInput(schedulerName: &str, input: &HashMap<String, ConfigValue>) -> Result<()>`：不修改输入映射，直接交给 `PdHttpClient::create_scheduler`。
- `CancelSchedulerJob(schedulerName: &str, jobID: u64) -> Result<()>`：将名称和无符号 job ID 交给客户端取消。

文件没有模块级常量、结构体、trait、异步函数或条件编译项。

## 执行流程

五个入口首先调用 `getGlobalInfoSyncer()`。该函数在 `pkg/domain/infosync/info.rs` 中从全局 `RwLock<Option<Arc<InfoSyncer>>>` 克隆 `Arc`；未初始化时返回 `Error::NotInitialized`。

随后入口读取 `InfoSyncer::pdHTTPCli`，在读锁保护下克隆 `Option<Arc<dyn PdHttpClient>>`，并立即结束锁的借用。`pkg/domain/infosync/region_test.rs` 的测试客户端会在回调中尝试获取同一槽位的写锁，验证真正调用 trait 方法时读锁已经释放，避免自锁或无谓地覆盖整个网络请求生命周期。

`GetReplicationState` 的分支如下：

1. 全局同步器不存在：传播 `NotInitialized`。
2. PD HTTP 客户端不存在：返回 `Ok(Pending)`，不发请求。
3. 客户端存在：构造 `KeyRange` 并调用 `get_regions_replicated_state`；客户端错误原样传播。
4. 返回值为 `REPLICATED` 时映射到 `Scheduled`，为 `INPROGRESS` 时映射到 `InProgress`，其他任何字符串（包括 `PENDING` 和空字符串）映射到 `Pending`。

另外四个入口在取得同步器后要求客户端必须存在，否则返回 `Error::PdHttpClientMissing`；存在时各自只调用一次对应 trait 方法并直接返回结果。文件本身没有额外转换、重试或补偿步骤。

## 数据与状态

本文件自身没有可变静态数据。共享状态来自 `InfoSyncer::pdHTTPCli: RwLock<Option<Arc<dyn PdHttpClient>>>`，其初始化发生在 `GlobalInfoSyncerInit`；调用时克隆 `Arc`，所以一次请求使用的是取出时的客户端快照，之后槽位被替换不会使当前调用失效。

`KeyRange` 定义在 `pkg/domain/infosync/types.rs`，表达含起点、不含终点的字节区间。`GetReplicationState` 和分布查询取得 `Vec<u8>` 的所有权，构造范围后不再保留原始键。`RegionDistributions` 包含 Region 总数和 `Store ID -> peer 数` 映射。`ConfigValue` 是无标签 serde 枚举，用于跨越调度器 JSON 的异构值边界；输入映射以共享引用传入，生命周期只覆盖同步调用。

`PlacementScheduleState` 的关键不变量是：所有未识别或不可查询的“非错误状态”均保守地表现为 `Pending`，只有精确的 `REPLICATED` 才表示完成。

## 依赖与调用关系

直接上游边界是 `pkg/domain/infosync/lib.rs` 的公开重导出。RustCodeGraph 对 `region.rs` 建立了 13 个符号，并显示文件级依赖来自多个模块；但定向搜索未找到 Rust 生产源码对五个公开函数的调用，实际 Rust 调用证据目前仅在 `pkg/domain/infosync/region_test.rs`。

直接下游关系为：

- 所有函数 -> `getGlobalInfoSyncer` -> 全局 `InfoSyncer`。
- 所有函数 -> `InfoSyncer::pdHTTPCli` -> `Arc<dyn PdHttpClient>`。
- `GetReplicationState` -> `PdHttpClient::get_regions_replicated_state`。
- `GetRegionDistributionByKeyRange` -> `PdHttpClient::get_region_distribution`。
- `GetSchedulerConfig` -> `PdHttpClient::get_scheduler_config`。
- `CreateSchedulerConfigWithInput` -> `PdHttpClient::create_scheduler`。
- `CancelSchedulerJob` -> `PdHttpClient::cancel_scheduler_job`。

`Cargo.toml` 表明该 crate 直接依赖带固定 tag `v0.4.2-aster.10` 的 `tikv-client`，但本文件并不直接引用该 crate；它只依赖 crate 内统一定义的类型和 trait。实际网络协议如何实现不在本文件中，且 `PdHttpClient` 的默认方法当前返回 `Error::External("... is unsupported")`，具体实现必须显式覆盖所需方法。

## 错误处理与边界

- 全局同步器未初始化时，五个入口均通过 `?` 返回 `Error::NotInitialized`。
- `GetReplicationState` 把“客户端缺失”视为可用但尚未完成，返回 `Ok(Pending)`；客户端实际返回错误时仍传播错误。未知状态字符串不报协议错误，而是保守回退为 `Pending`。
- 其余四个入口把“客户端缺失”视为操作不可执行，返回 `Error::PdHttpClientMissing`。
- `RwLock::read()` 使用 `unwrap()`；若锁曾被持有者 panic 污染，这些入口会 panic，而不是转换成 crate 的 `Error`。
- 本层不验证 `startKey <= endKey`、空范围、`engine` 枚举、调度器名称、输入字段或 job ID；约束及拒绝行为由调用方和客户端/PD 决定。
- trait 默认实现为 `unsupported` 错误。因此“客户端存在”不等于目标能力可用，调用者必须处理下游 `Result`。

## 并发与资源生命周期

`PdHttpClient: Send + Sync`，并由 `Arc` 共享；入口函数本身是同步函数，不生成线程、任务、future、通道或后台资源。全局同步器和客户端槽位分别用 `Arc` 与 `RwLock` 管理并发访问。

锁的关键生命周期是“读取、克隆、释放、调用”：代码先从 `pdHTTPCli.read()` 得到 guard，克隆 `Option<Arc<_>>` 后再匹配或报错，trait 回调期间不持有 guard。`region_calls_match_go_without_holding_the_client_slot_lock` 通过回调内 `try_write()` 明确验证这一点。这样允许测试或管理代码并发替换客户端，也避免客户端实现回入 infosync 时死锁。

由于接口同步执行，取消、超时和重试不能由本层驱动；相应能力必须在具体客户端、调用线程或未来显式引入的上下文参数中实现。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/domain/infosync/region.go`。Rust 保留了 Go 的公开名称和主要分支：三态枚举、展示字符串、PD 客户端缺失时复制状态回退、状态字符串映射，以及四个调度/分布转发入口。

主要差异如下：

- Go 每个函数显式接收 `context.Context`；Rust API 没有 context 参数，因此本层不能表达调用级取消或 deadline。
- Go 使用 PD HTTP 包的 `KeyRange`、`RegionDistributions` 与 `any`；Rust 使用 crate 内的 `KeyRange`、简化的 `RegionDistributions` 和强类型 `ConfigValue`。
- Go 的复制状态在请求错误或空状态时返回 `Pending` 加原错误；Rust 使用 `?` 传播错误，并仅对成功返回的字符串映射，空字符串会成为 `Ok(Pending)`。
- Go 的客户端缺失错误由 PD `errs.ErrClientGetLeader.FastGenByArgs(...)` 构造，部分函数参数不同；Rust 统一为 `Error::PdHttpClientMissing`。
- Go 生产调用链已经接线：`show_placement.go` 查询复制状态，`show.go` 查询分布和 job 配置，`distribute.go` 创建与取消 job。Rust 当前未发现对应生产调用者，不能把 Go 的完整 SQL 行为视作 Rust 已接线行为。

相关 Go 行为测试主要位于 `pkg/executor/distribute_table_test.go`，覆盖调度器配置、创建、取消和 Region 分布在 SQL 执行器中的使用；目标目录没有直接命中这些 API 的 `*_test.go`。Rust 独立测试 `pkg/domain/infosync/region_test.rs` 聚焦本门面的转发、缺失客户端语义和锁生命周期。

## 扩展指南

新增 PD Region 或调度 API 时，应先在 `pkg/domain/infosync/types.rs` 的 `PdHttpClient` 增加清晰的同步边界与数据类型，再在本文件增加只负责获取客户端和转发的公开函数。保持“先克隆客户端再调用”的锁生命周期，不要在持有 `pdHTTPCli` guard 时执行外部代码。

若扩展复制状态，必须同步检查 `PlacementScheduleState`、`String` 和 `GetReplicationState` 的字符串映射，并评估 Go 兼容性与 SQL 展示值；不能仅添加枚举项而让未知值静默改变语义。若需要超时、取消或重试，应先决定 Rust 全链路的上下文模型，不能只在此门面局部模拟 Go `context.Context`。

测试应继续放在独立文件 `pkg/domain/infosync/region_test.rs`，不要嵌入生产源文件。至少覆盖：参数完整转发、客户端错误传播、缺失客户端的差异化策略、未知 PD 状态、锁在回调前释放，以及新增 `ConfigValue` 形态。若把 API 接入 Rust executor，还应在对应 executor 的独立测试中覆盖 SQL 可见行为，并与 `pkg/executor/distribute_table_test.go` 等 Go 测试意图对齐。

兼容性风险主要来自 PD 状态字符串和调度 JSON schema；正确性风险来自把未知状态误判为完成、错误的 key range 编码或持锁调用；性能风险较小，但每次调用都经过全局锁读取和一次 `Arc` 克隆，且实际网络延迟完全由客户端承担。

## 验证依据

- RustCodeGraph `status`：索引有效，覆盖 11,467 个文件；`files --filter pkg/domain/infosync` 确认 `region.rs`、`region_test.rs`、Go 对照和模块文件均被索引。
- RustCodeGraph `node --file pkg/domain/infosync/region.rs`：读取完整 114 行源码并确认 13 个符号；`query` 分别定位 `PlacementScheduleState`、`GetReplicationState`、`GetRegionDistributionByKeyRange`、`GetSchedulerConfig`、`CreateSchedulerConfigWithInput`、`CancelSchedulerJob`。`callers/callees` 精确查询在本仓库超时，因此具体边以源码视图和定向 `rg` 复核，未把超时结果当作“无调用”。
- 源码与模块边界：`pkg/domain/infosync/region.rs`、`pkg/domain/infosync/lib.rs`、`pkg/domain/infosync/info.rs`、`pkg/domain/infosync/types.rs`、`pkg/domain/infosync/error.rs`、`pkg/domain/infosync/Cargo.toml`。
- Rust 测试：`pkg/domain/infosync/region_test.rs`，覆盖五个 trait 转发、key range/engine/name/input/job ID、缺失客户端分支、默认展示值和回调期间锁已释放。
- Go 对照与调用证据：`pkg/domain/infosync/region.go`、`pkg/executor/show_placement.go`、`pkg/executor/show.go`、`pkg/executor/distribute.go`、`pkg/executor/distribute_table_test.go`。
- 定向 Rust 调用搜索未发现测试之外的五个函数调用点；该限制已在“文件定位”“依赖与调用关系”和“与 Go 版本的对应关系”中明确记录。
