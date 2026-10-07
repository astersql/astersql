# `pkg/ddl/reorg_util.rs`

## 文件定位

本文件属于 `astersql-ddl` crate；`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"` 指向 crate 根，而 `pkg/ddl/lib.rs` 通过 `pub mod reorg_util;` 无条件公开本模块。它提供两类纯内存辅助逻辑：把重组（reorg）运行参数校验并冻结为元数据快照，以及汇总表/Region 的近似大小。

这里必须区分模块可见性与生产接线状态。RustCodeGraph 能定位本文件全部公开符号，但文件级反向引用只来自 `pkg/ddl/reorg_util_test.rs` 和 `pkg/ddl/backfilling_txn_executor_test.rs`，三个公开函数的 `callers`/`callees` 查询均没有返回生产调用边。因此当前 Rust 模块是可公开调用的独立模型，尚不能据此声称它已经进入 DDL job 提交、回填调度或 PD 查询主链。真实的 Go 生产主链在同路径 `pkg/ddl/reorg_util.go`。

从 DDL 语义看，这些值面向需要 reorg/backfill 的 job：Go 版本在建 job 时初始化 `job.ReorgMeta`，供后续 worker 或分布式任务消费；Rust 文件自身不创建 job、不驱动 schema state、不保存 checkpoint，也不更新 schema version。

## 核心职责

1. `ReorgVariables` 表示调用侧提供的并发度、批大小、写入限速、云存储和分布式开关，并给出确定的默认值。
2. `init_job_reorg_meta_from_variables` 对最基本的输入约束做同步校验，然后生成与输入解耦的 `InitializedReorgMeta` 快照。
3. `estimate_table_size_by_regions` 对多页 Region 统计逐项取 `approximate_size_mib` 与 `approximate_kv_size_mib` 的较大值，换算为字节后累加。
4. `get_table_size_by_id` 汇总调用者已经取得的物理分区大小，并显式保留 Go `int64` 的回绕语义。

本文件不负责访问 session、PD、KV store 或表元数据，也不负责分页、key range 编码、错误日志和统计回退；这些能力仍只存在于 Go 的 `initJobReorgMetaFromVariables`、`getTableSizeByID` 和 `estimateTableSizeByID` 中。

## 主要符号

- `ReorgVariables`：公开输入结构。字段 `worker_count`、`batch_size` 和 `max_write_speed` 是数值配置；`use_cloud_storage` 决定 `cloud_storage_uri` 是否有效；`distributed` 是分布式执行快照。其 `Default` 固定为 4 个 worker、批大小 256、无限速（0）、本地存储且非分布式。
- `InitializedReorgMeta`：公开输出结构。`concurrency` 从 `worker_count` 改名映射而来；其余运行参数被复制；`version` 当前总是 1，用于给后续元数据兼容演进留出版本位。它是本地简化结构，不是 `astersql_meta_model::group_3::DDLReorgMeta`。
- `ReorgMetaError`：同步、可比较的参数错误枚举，分别表示并发度为 0、批大小为 0、启用云存储却缺少 URI。
- `init_job_reorg_meta_from_variables(&ReorgVariables) -> Result<InitializedReorgMeta, ReorgMetaError>`：按固定顺序校验并构造快照；只在启用云存储时复制 URI。
- `RegionSize`：一个 Region 的两种 MiB 近似值，分别对应存储文件大小和 KV 数据大小的简化表示。
- `estimate_table_size_by_regions(&[Vec<RegionSize>]) -> i64`：把调用者组织好的分页结果展平、逐 Region 取最大值、转成字节并回绕累加。
- `get_table_size_by_id(&[i64]) -> i64`：对调用者提供的物理分区大小列表进行回绕求和；函数名沿用 Go 概念，但参数里没有 table ID，也不会自行查询表。

以上类型与函数均为 `pub`；文件中没有 trait、异步函数、条件编译项或模块级可变状态。唯一函数内常量是 `estimate_table_size_by_regions` 的 `MEBIBYTE = 1024 * 1024`。

## 执行流程

`init_job_reorg_meta_from_variables` 的流程是：

1. 先检查 `worker_count == 0`，命中时返回 `InvalidConcurrency`。
2. 再检查 `batch_size == 0`，命中时返回 `InvalidBatchSize`。多个字段同时非法时，由此校验顺序决定首个错误。
3. 当 `use_cloud_storage` 为真且 URI 为空字符串时，返回 `CloudStorageUriMissing`；该判断不做 trim，也不解析 URI。
4. 成功时复制数值和 `distributed`，把 `worker_count` 写入 `concurrency`，并把版本固定为 1。
5. 云存储启用时 clone 原 URI；未启用时强制写空字符串，防止无效配置泄漏到持久化快照。

`estimate_table_size_by_regions` 的流程是：展平所有页；对每个 `RegionSize` 取两个估计值的较大者；使用 `wrapping_mul(1024 * 1024)` 从 MiB 换算为字节；再使用 `wrapping_add` 累加。空页集合或全空页自然得到 0。

`get_table_size_by_id` 只是对 `partition_sizes` 逐项执行 `i64::wrapping_add`。它假定调用者已经完成物理表/分区枚举和每个分区的大小估算。

## 数据与状态

所有数据都由值类型或借用切片承载，没有全局变量、缓存或隐式单例。输入借用只持续到函数返回；输出结构拥有自己的 `String`，因此后续修改原 `ReorgVariables` 不会改变已经生成的快照。

`InitializedReorgMeta.version = 1` 是当前唯一的格式版本事实，但文件中没有反序列化、升级或分支兼容逻辑。若它未来真正映射到持久化 job 元数据，需要先核对 `astersql_meta_model::group_3::DDLReorgMeta` 的字段和兼容规则，不能仅凭这里的注释推断已经具备跨版本迁移能力。

大小单位边界很明确：`RegionSize` 字段为 MiB，`estimate_table_size_by_regions` 返回字节；`get_table_size_by_id` 不带单位转换，要求输入与期望输出使用同一单位。负数和溢出不会被拒绝，而是遵循有符号 `i64` 回绕运算。

## 依赖与调用关系

Rust 内部依赖仅为标准库隐式能力（`Vec`、`String`、迭代器、`Result` 和整数运算），没有使用 `pkg/ddl/Cargo.toml` 中列出的外部 crate。模块由 `pkg/ddl/lib.rs` 第 91 行公开，测试模块由同文件第 257～258 行在 `cfg(test)` 下装配。

RustCodeGraph 查询得到的关键节点为：

- `reorg_util.rs::init_job_reorg_meta_from_variables`（源码第 81 行）；
- `reorg_util.rs::estimate_table_size_by_regions`（源码第 117 行）；
- `reorg_util.rs::get_table_size_by_id`（源码第 130 行）；
- `ReorgVariables`、`InitializedReorgMeta`、`ReorgMetaError` 三个公开数据符号。

这些节点当前没有生产 caller/callee 边。`pkg/ddl/reorg_util_test.rs` 直接覆盖全部三个函数；RustCodeGraph 的文件级“used by”还列出 `pkg/ddl/backfilling_txn_executor_test.rs`，但该文件实际使用的是另一组 `ReorgMeta`/`DDLReorgMeta` 辅助逻辑，不能把它当作本文件函数的直接行为测试。

Go 对照的生产关系更完整：`pkg/ddl/reorg_util.go::initJobReorgMetaFromVariables` 被 `pkg/ddl/executor.go`、`pkg/ddl/modify_column.go` 等建 job 路径调用；它进一步读取 session variables、按 job 类型决定参数、查询表大小和执行 CPU、计算资源槽位，最后写入 `job.ReorgMeta`。`getTableSizeByID` 调用 `estimateTableSizeByID`，后者通过 PD HTTP 按 128 个 Region 分页查询。

## 错误处理与边界

初始化函数使用精确的 `ReorgMetaError` 返回可判定错误，不 panic。它只验证三个条件：并发度和批大小非零、云存储启用时 URI 非空；不限制最大值、不验证写入速度、不验证 URI 格式，也不约束 `distributed` 与云存储的组合。

两个大小函数不返回错误。空输入返回 0；负值被保留；乘法和加法明确使用 wrapping 运算，因此 debug/release 构建行为一致，并贴近 Go `int64` 的二进制回绕。代价是错误统计或溢出不会被暴露给调用者。

与 Go 相比，Rust 文件没有以下边界处理：store 未实现 `helper.Storage`、PD client 获取失败、PD 请求失败、Region `EndKey` 十六进制解码失败、单分区估算为 0 时回退 `GetPDRegionStats`。Go 的 `getTableSizeByID` 对多数错误记录警告并返回/继续使用 0，`estimateTableSizeByID` 则把 PD 与解码错误向上传递；这些语义尚未落入当前 Rust API。

## 并发与资源生命周期

本文件没有线程、锁、原子变量、channel、async task、事务或 I/O。三个函数只读取不可变借用，并返回拥有所有权的值，因此本身可重入；能否跨线程使用取决于这些普通字段的自动 trait，文件没有额外同步协议。

这里的 `concurrency` 只是数据，不会创建或调整 worker；`distributed` 也只是布尔快照，不会启动分布式任务。Region 分页的 client、网络连接和 key-range 游标生命周期均位于 Go `estimateTableSizeByID`，Rust 聚合函数只消费已经物化的 `Vec<Vec<RegionSize>>`，因此内存占用与全部页的总 Region 数量线性相关。

从完整 DDL 生命周期看，本文件既不持久化 checkpoint，也不承担 owner failover、取消/回滚、delete-range GC、MDL 或 schema 同步。若未来接入生产链，这些职责应继续由 job/worker 框架承担，而不是塞入纯计算辅助函数。

## 与 Go 版本的对应关系

`pkg/ddl/reorg_util.go` 是权威对照，但当前 Rust 不是逐函数完整复刻：

- Rust `init_job_reorg_meta_from_variables` 对应 Go `initJobReorgMetaFromVariables` 的“读取配置并形成 reorg 元数据”概念，仅保留参数校验与快照。Go 还依据 job 类型设置 reorg/dist 参数，处理多 schema 子任务、bootstrap/system DB 限制、NextGen 资源估算、fast reorg、target scope、最大节点数、failpoint、日志和 `job.ReorgMeta` 写入。
- Rust `estimate_table_size_by_regions` 对应 Go `estimateTableSizeByID` 的核心聚合公式：每个 Region 取 `max(ApproximateSize, ApproximateKvSize)`，按 MiB 转字节并累加。Rust 不构造表 handle key range、不编码 Region range、不分页访问 PD，也不推进 `EndKey`。
- Rust `get_table_size_by_id` 只复刻 Go `getTableSizeByID` 的最终分区求和性质。Go 函数自己识别分区表、逐物理 ID 估算、记录错误，并在估算为 0 时查询 region stats 回退。
- Rust 当前新增了 Go 函数没有以同形 API 暴露的三个参数错误；反过来，Go 的零并发/零 batch 值会交给 `DDLReorgMeta` setter 及既有配置语义处理，因此接入前必须核实是否真的应拒绝 0，不能把当前简化验证直接替换 Go 行为。

`pkg/ddl/reorg_util_test.go` 验证真实 PD 分页适配层使用 128 的 limit、正确的表 key range、两种 Region 大小取大值，以及相邻的行大小采样逻辑。`pkg/ddl/reorg_util_test.rs` 用本地 mock 重建了分页/采样场景，并直接验证 Rust 聚合、回绕和参数快照；其中分页 client 与行大小函数属于测试侧模型，不是本 Rust 生产文件提供的 API。

## 扩展指南

若只扩展快照字段，应同时修改 `ReorgVariables`、`InitializedReorgMeta`、构造分支和 `pkg/ddl/reorg_util_test.rs::reorg_meta_initialization_validates_and_snapshots_every_variable`，并明确默认值、非法组合以及未启用功能时是否清空敏感字段。若改变 `version`，还必须先确定持久化载体及旧 job 的读取兼容策略。

若接入真实 DDL 主链，优先复用 `astersql_meta_model::group_3::DDLReorgMeta`，并以 Go `initJobReorgMetaFromVariables` 的 job 类型分派和配置语义为基准；不要让本地 `InitializedReorgMeta` 成为第二套漂移的数据模型。接线测试必须放在独立测试文件中，至少覆盖普通 add-index、需要/不需要 reorg 的 modify-column、多 schema job、system DB 禁用分布式任务，以及资源估算失败路径。

若移植 PD 查询，应把“查询/分页”和“纯聚合”分层：保留 `estimate_table_size_by_regions` 作为可测试核心，在新适配层中实现 table handle key range、codec、128 条分页、空页/终止 key 判断和错误传播，并同步 `pkg/ddl/reorg_util_test.go` 的范围及分页断言。需要警惕一次性物化所有页的内存成本；生产实现更适合流式累加。

若调整数值语义，必须同步 `production_size_aggregation_preserves_go_signed_arithmetic` 和 `production_region_aggregation_uses_each_regions_larger_estimate`。从 wrapping 改为 checked/saturating 会改变 Go 对齐行为，应作为显式兼容决策，而不是普通重构。

## 验证依据

- 源码：`pkg/ddl/reorg_util.rs`，完整读取 132 行，核对 4 个公开结构/枚举、3 个公开函数、默认实现和所有运算分支。
- 模块与 crate：`pkg/ddl/lib.rs` 的 `pub mod reorg_util`、`#[cfg(test)] mod reorg_util_test`；`pkg/ddl/Cargo.toml` 的 crate 名、lib 路径和 Go package porting 元数据。
- Rust 测试：`pkg/ddl/reorg_util_test.rs`，覆盖两种估计值取大、MiB 换算、负数与溢出回绕、三个输入错误、完整快照及禁用云存储时清空 URI。
- Go 对照：`pkg/ddl/reorg_util.go` 的 `initJobReorgMetaFromVariables`、`getTableSizeByID`、`estimateTableSizeByID`；`pkg/ddl/reorg_util_test.go::TestEstimateTableSizeByIDUsesMaxApproximateSizes` 的 key range、分页 limit、聚合和采样断言。
- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件；`node --file` 读取目标 Rust/Go/测试/模块源码；`query` 定位三个函数及三类主要数据符号；对三个函数运行 `callers`/`callees` 未得到生产调用边；目标文件的索引反向引用为两个 Rust 测试文件。
- DDL 背景边界：读取 `pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`，仅用其定位 job/owner/reorg 框架，并以本文件及测试确认实际实现，没有把背景文档当作生产接线证据。
- 本任务是纯文档分析，按任务约束不运行 Cargo；最终以固定 11 章节结构检查和人工事实复核为验收。
