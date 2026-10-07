# `br/pkg/checkpoint/log_restore.rs`

## 文件定位

`log_restore.rs` 是 `astersql-br-pkg-checkpoint` crate 面向日志恢复场景的适配层。crate 入口 `br/pkg/checkpoint/lib.rs` 以 `pub mod log_restore` 装载本文件，并通过 `pub use log_restore::*` 展平公开符号；`br/pkg/checkpoint/Cargo.toml` 则把该 crate 标记为对齐 Go 包 `br/pkg/checkpoint` 的 library。它不实现 SQL 表或对象存储后端，而是在通用 `CheckpointRunner`、`LogMetaManager`/`SnapshotMetaManager` 与日志恢复所需的数据格式之间建立契约。

RustCodeGraph 的文件节点显示本文件被 `br/pkg/checkpoint/checkpoint_test.rs`、`br/pkg/restore/log_client/client.rs` 和 `br/pkg/task/stream_test.rs` 使用。仓库文本检索还显示 `br/pkg/checkpoint/parity_test.rs` 和独立的 `log_restore_test.rs` 直接覆盖其 API。生产侧当前可确认的直接使用集中在 `client.rs` 构造 `CheckpointMetadataForLogRestore`；Runner 启动和 `AppendRangeForLogRestore` 的 Rust 证据来自独立测试，不能据此宣称 Rust 生产主链已经调用这两个入口。

## 核心职责

本文件承担四组职责：

1. 用 `LogRestoreKeyType`、`LogRestoreValueType` 表示某个日志元数据分组内已经恢复的文件位置，并由 `valueMarshalerForLogRestore` 压缩成 `LogRestoreValueMarshaled`。
2. 通过 `StartCheckpointRunnerForLogRestore`（默认周期）或 `StartCheckpointLogRestoreRunnerForTest`（注入周期）启动由 manager 提供的通用 Runner；通过 `AppendRangeForLogRestore` 把单个已完成文件包装成 Runner 消息。
3. 定义跨运行保存的日志恢复元数据、snapshot+log 恢复相位以及 `GetCheckpointTaskInfo` 聚合视图，使恢复入口能判断是否可以重跑快照阶段。
4. 定义 ingest index 修复与外键更新 SQL 的持久化载荷，同时把“是否已在本进程找到/修复”的标志排除在 JSON 之外。

边界也很明确：实际的通道、线程、批量刷盘和收尾在 `checkpoint.rs`；SQL 表与外部存储的读写、存在性判断和清理在 `manager.rs`；本文件只负责日志恢复特有的类型和薄适配。

## 主要符号

- `pub type LogRestoreKeyType = String`：检查点分组键，通常对应一批日志文件的元数据键。
- `LogRestoreValueType { TableID, Goff, Foff }`：Runner 接收的稀疏完成项；分别记录下游表 ID、元数据 group 下标和 group 内 file 下标。
- `LogRestoreValueMarshaled { Goff, Foffs }`：落盘形态；`Foffs` 是 `TableID -> [Foff]`，JSON 字段固定为 `goff`、`foffs`。
- `valueMarshalerForLogRestore(&RangeGroup<...>) -> Result<Vec<u8>>`：按 `Goff`、再按 `TableID` 聚合文件下标，保留原 `GroupKey`，最后序列化为 JSON。
- `newTableCheckpointStorage(Box<dyn Session>, String)`：构造 `tableCheckpointStorage` 的薄包装；资源所有权被移入存储对象。
- `StartCheckpointLogRestoreRunnerForTest` / `StartCheckpointRunnerForLogRestore`：分别使用注入 tick 或 `DefaultTickDurationConfig()`，把日志 marshaler 交给 `LogMetaManager::StartCheckpointRunner`。
- `AppendRangeForLogRestore`：把一个 `(groupKey, tableID, goff, foff)` 包成单元素 `CheckpointMessage` 后调用 `CheckpointRunner::Append`。
- `CheckpointMetadataForLogRestore`：保存集群 ID、时间戳边界、GC 比例、TiKV 后台任务配置、快照数据量和 TiFlash 副本记录；可选字段使用 `skip_serializing_if` 保持 Go `omitempty` 语义。
- `RestoreProgress`：以 `repr(i32)` 表示恢复相位；自定义 serde 只接受数值 `0` 和 `1`，未知值报错。
- `CheckpointProgress`、`TaskInfoForLogRestore`：前者是进度持久化信封；后者合并可选 log 元数据、snapshot 元数据存在性和进度。`IdMapSaved` 仅在相位为 `InLogRestoreAndIdMapPersisted` 时返回真。
- `CheckpointIngestIndexRepairSQL`、`CheckpointForeignKeyUpdateSQL`、`CheckpointIngestIndexRepairSQLs`：保存可重放 SQL 及参数；运行时完成标志带 `#[serde(skip)]`，不会跨进程恢复。

## 执行流程

写入文件范围的流程如下：恢复代码为一个已完成文件调用 `AppendRangeForLogRestore`；函数创建单元素 `CheckpointMessage` 并进入 `CheckpointRunner::Append`。`Append` 先检查 `Context` 是否取消、Runner 是否已有异步错误或已经关闭，然后向无界 `appendCh` 发送。通用主循环按 key 合并消息，在 flush 时调用本文件的 `valueMarshalerForLogRestore`：先形成 `Goff -> TableID -> Vec<Foff>`，再把每个 `Goff` 变成一项 `LogRestoreValueMarshaled`，最终序列化整个 `RangeGroup`。manager 创建的表存储或外部存储负责真正落盘。

Runner 启动时，正式入口使用默认 flush/checksum/retry 配置；测试入口把 flush 与 checksum 周期都覆盖为传入的 `tick`。`TableMetaManager::StartCheckpointRunner` 会一次性 `take` 专用 `runnerSe`，构造 `tableCheckpointStorage` 并启动主循环；`StorageMetaManager` 构造外部 checkpoint storage，可携带 cipher。两者对日志恢复都把 lock tick 设为 `Duration::ZERO`。

读取任务概况时，`GetCheckpointTaskInfo` 依次执行：若 log progress 存在则加载进度；若 log metadata 存在则加载元数据；若传入 snapshot manager 则探测 snapshot metadata。全部成功后才构造结果。无 progress 时默认 `InSnapshotRestore`，无 log metadata 时 `Metadata` 为 `None`，未传 snapshot manager 时 `HasSnapshotMetadata` 保持 `false`。

## 数据与状态

范围数据存在两种形态。内存/消息形态是一文件一条的 `LogRestoreValueType`；落盘形态按 `Goff` 和 `TableID` 聚合多个 `Foff`，减少重复字段。聚合使用 `HashMap`，因此 JSON 数组中不同 `Goff` 的顺序以及 map 键迭代顺序不是稳定协议；消费者应按键解释内容，不应依赖输出顺序。函数不去重，同一完成项重复追加会保留重复 `Foff`。

`CheckpointMetadataForLogRestore` 是任务级持久状态。除 `RocksDBMaxBackgroundJobs`、`SnapshotRestoreDataSize` 和空 `TiFlashItems` 可在序列化时省略外，其余字段即使为默认值也会写出；反序列化均允许缺失字段回落到默认值。`RestoreProgress` 的不变量是：`0` 表示仍可从 snapshot restore 重试；只有 id-map 已持久化后才能写入 `1`，随后恢复流程必须跳过 snapshot restore，避免 rename 等 meta-kv 已落库后再次创建重复表。

SQL 修复载荷把 SQL 文本、JSON 参数和对象标识持久化，但 `OldIndexIDFound`、`IndexRepaired`、`OldForeignKeyFound`、`ForeignKeyUpdated` 只属于当前进程。重启后这些布尔值恢复为默认 `false`，调用者必须重新判定运行时完成状态。

## 依赖与调用关系

上游方面，`br/pkg/checkpoint/lib.rs` 公开本模块；`br/pkg/restore/log_client/client.rs::LoadOrCreateCheckpointMetadataForLogRestore` 构造本文件的元数据类型；`br/pkg/checkpoint/checkpoint_test.rs` 与 `parity_test.rs` 调用聚合查询、测试 Runner 和 append API；`br/pkg/checkpoint/log_restore_test.rs` 验证进度 JSON。RustCodeGraph 对文件的 “used by” 结果还列出 `br/pkg/task/stream_test.rs` 对元数据类型的使用。

下游方面，`valueMarshalerForLogRestore` 依赖 `checkpoint.rs::RangeGroup` 和 `serde_json::to_vec`；启动函数调用 `manager.rs::LogMetaManager::StartCheckpointRunner`；append 函数调用 `checkpoint.rs::CheckpointRunner::Append`；任务查询调用 `LogMetaManager` 的 progress/metadata API 和可选 `SnapshotMetaManager::ExistsCheckpointMetadata`。元数据字段依赖 `stubs.rs` 的 `CIStr`、`TiFlashReplicaInfo`、`Context`、`Session` 和统一 `Result`。

crate 的直接第三方依赖由 `Cargo.toml` 声明，其中本文件直接使用 `serde`、`serde_json`；线程通道、加密和摘要依赖由相邻的 Runner/存储实现消费。本文件没有条件编译项；独立测试由 `lib.rs` 的 `#[cfg(test)] #[path = "log_restore_test.rs"]` 挂载。

## 错误处理与边界

`valueMarshalerForLogRestore` 唯一显式失败点是 JSON 序列化，错误通过 crate 的 `Result` 和 `?` 原样上抛；若它在 flush 中失败，`checkpoint.rs` 会把整批 flush 视为失败。`AppendRangeForLogRestore` 完整保留 Runner 的取消、异步刷盘错误、关闭状态和通道关闭错误，不吞错也不重试。

`GetCheckpointTaskInfo` 在每个存在性检查或加载操作上使用 `?`，因此按固定读取顺序遇到第一个错误就返回，不会给出部分成功的 `TaskInfoForLogRestore`。它不验证 metadata 中时间戳之间的业务关系，也不强制进度单调；这些约束应由写入方维持。

`RestoreProgress::deserialize` 对 `0`、`1` 之外的整数返回 `invalid restore progress ...`，避免把未来或损坏状态默认为可重跑快照。相反，metadata 的缺失字段有意使用默认值以兼容旧 JSON。`valueMarshalerForLogRestore` 接受空 group 并可序列化为空 `groups`，也不会检查负数 ID/下标；调用者负责保证这些值来自有效元数据索引。

## 并发与资源生命周期

本文件自身没有锁、线程或异步任务。并发语义来自 `CheckpointRunner`：append/checksum 使用 crossbeam 无界通道，聚合状态受 `Mutex` 保护，后台工作由 `std::thread` 驱动，错误保存在共享错误槽并反馈给后续 append。无界 append 通道意味着突发写入的背压主要体现为内存增长，扩展批量追加时需要关注生产速度与 flush 周期。

manager 决定资源所有权。表后端在首次启动 Runner 时取走专用 session，第二次启动会得到 `runner session missing`；Runner 的 `WaitForFinish(ctx, flush)` 只发送一次 done、join 全部线程并关闭存储，表存储因而关闭其 session。外部存储 manager 不持有 session，`Close` 为空操作。调用者必须在生命周期结束时调用 `WaitForFinish`，并依据是否需要最终刷盘选择 `flush`，否则不能假定内存中的完成项已经持久化。

`GetCheckpointTaskInfo` 是同步串行读取，没有跨 manager 的事务快照：progress、log metadata 和 snapshot metadata 可能来自不同读取时刻。若需要强一致视图，必须在 manager/调用层增加协议，不能只调整返回结构。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/checkpoint/log_restore.go`。Rust 保留了 Go 的公开类型和函数命名、JSON 字段名、稀疏到压缩形态的两级聚合、Runner 配置方式、任务信息读取顺序，以及 `RestoreProgress` 的数值含义。`newTableCheckpointStorage` 在 Go 返回指针，在 Rust 返回拥有 session 的具体值；随后均由 Runner/manager 管理生命周期。

主要语言差异有三点。第一，Go 的 `Goff`/`Foff` 是 `int`，Rust 使用 `i64`；跨语言数据需要避免超出实际索引范围。第二，Go 枚举由 `int` 的默认 JSON 编码得到数值，Rust 用手写 serde 明确固定为 `0/1` 并拒绝未知值，兼容性更严格。第三，Go 的 `nil` snapshot manager 对应 Rust 的 `Option<&dyn SnapshotMetaManager>`，Go 的 metadata 指针对应 Rust 的 `Option<CheckpointMetadataForLogRestore>`。

Go 测试 `br/pkg/checkpoint/checkpoint_test.go` 验证 metadata/progress/修复 SQL 往返、`GetCheckpointTaskInfo` 和嵌套偏移的 Runner 写入；Rust 的 `checkpoint_test.rs` 保留相同测试意图并覆盖表/外部存储两类 manager。`log_restore_test.rs` 额外锁定数值 JSON。当前 Rust 结构为实实现而非空桩，但生产调用覆盖并不等同于 Go：仓库检索未发现 Rust 生产代码直接调用正式 Runner 启动或 append 入口，因此不能将 Go 的完整运行接线自动推断为 Rust 现状。

## 扩展指南

新增日志完成项字段时，应同时修改 `LogRestoreValueType`、`LogRestoreValueMarshaled` 和 `valueMarshalerForLogRestore`，明确新字段如何参与聚合，并同步 Go 文件与 `checkpoint_test.rs`/`parity_test.rs` 的 round-trip 断言。若改变落盘 JSON，必须评估已有 Go/Rust checkpoint 的双向兼容；不要依赖 `HashMap` 顺序，若协议要求确定性输出则需引入排序并增加字节级测试。

新增任务级元数据字段时，在 `CheckpointMetadataForLogRestore` 上明确 Go 字段名、默认值和省略规则，并同步 `br/pkg/checkpoint/log_restore.go`、两种 manager 的往返测试以及 legacy JSON 测试。新增恢复相位风险更高：必须同步 Rust 的 `Serialize`/`Deserialize`、Go 常量、`IdMapSaved` 或新的判定方法，并验证旧进程面对新数值时的行为。

扩展任务信息聚合应从 `GetCheckpointTaskInfo` 接入，并保留“存在性检查后再加载”和首错返回语义；如需一致快照，应先扩展 manager 抽象。新增 Runner 入口应继续把具体存储留给 `LogMetaManager`，不要在本文件复制表/对象存储实现。测试逻辑应放在独立的 `log_restore_test.rs`、`checkpoint_test.rs` 或 `parity_test.rs`，不得嵌入生产源文件。

性能上重点关注每批 `HashMap`/`Vec` 分配、重复 Foff、无界 append 通道和 JSON 体积；正确性上重点关注 id-map 相位不可回退、未知进度值拒绝、跨语言字段名及可选字段兼容；资源上需覆盖取消、刷盘失败、重复启动和 `WaitForFinish` 收尾。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter br/pkg/checkpoint` 确认目标、Go 对照及测试均已索引；目标文件节点列出 30 个符号及 “used by” 文件；精确 `query` 区分了 Go/Rust 的 `GetCheckpointTaskInfo`、`StartCheckpointRunnerForLogRestore`、`AppendRangeForLogRestore`。`callers/callees` 命令在本地连续超时且没有输出，因此调用关系另由文件节点和直接调用点检索交叉核对。
- 源码：`br/pkg/checkpoint/log_restore.rs`（全部 326 行）、`br/pkg/checkpoint/checkpoint.rs`（消息、Runner、Append、收尾）、`br/pkg/checkpoint/manager.rs`（traits 与两种 manager 实现）、`br/pkg/checkpoint/lib.rs`（模块与测试挂载）。
- 边界配置：`br/pkg/checkpoint/Cargo.toml`（crate 类型、Go 包映射、serde/serde_json 等依赖）。
- Go 对照：`br/pkg/checkpoint/log_restore.go`；Go 测试证据来自 `br/pkg/checkpoint/checkpoint_test.go`。
- Rust 测试：`br/pkg/checkpoint/log_restore_test.rs`、`br/pkg/checkpoint/checkpoint_test.rs`、`br/pkg/checkpoint/parity_test.rs`；生产/上层引用还核对了 `br/pkg/restore/log_client/client.rs`、`br/pkg/task/stream.rs` 和 `br/pkg/task/stream_test.rs` 的文本调用点。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务指定的结构命令，确保文件存在且恰有十一个固定二级标题；人工复核重点是区分本文件职责、通用 Runner/manager 职责和当前 Rust 生产接线证据。
