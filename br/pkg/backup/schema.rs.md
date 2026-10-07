# `br/pkg/backup/schema.rs`

## 文件定位

`schema.rs` 属于 Cargo 包 `astersql-br-pkg-backup`。该包以
`br/pkg/backup/lib.rs` 为 crate 根；`lib.rs` 通过 `pub mod schema` 装载本文件，
并用 `pub use schema::*` 把公开入口平铺到 crate API。`br/pkg/backup/Cargo.toml`
把该 crate 标为 Go 包 `br/pkg/backup` 的 library 移植，并且当前直接依赖只有
`serde` 与 `serde_json`；TiDB、KV、checkpoint、统计、label rule 和 meta writer
能力均由同 crate 的 `stubs.rs` 抽象提供。

本文件对应 Go 实现 `br/pkg/backup/schema.go`，负责把已经筛选、补齐过的库表元信息
转换成 `backuppb::Schema`：可选计算或复用 checksum、校验外部 checksum、导出统计、
读取 `merge_option` label rule，最后追加到 backup meta。上游 Rust 构造链是
`client.rs::BuildBackupRangeAndInitSchema` → `NewBackupSchemas`；checkpoint 续跑数据由
`client.rs::Client::BuildBackupRangeAndSchema` 调用 `SetCheckpointChecksum` 注入。

精确搜索当前 Rust 生产文件只找到上述“构造/注入”接线，`BackupSchemas` 的 Rust 调用
均位于独立测试文件；完整产品执行入口仍可在 Go 的 `br/pkg/task/backup.go` 中看到。
因此本文件是已实现并经过 Rust 测试组合验证的 schema 备份组件，但不能据现有调用边
宣称 Rust BR 命令主链已经调用它。

## 核心职责

- `Schemas` 保存延迟枚举库表的 `iterFunc`、供进度展示的 `size`，以及可选的
  `table id -> ChecksumItem` checkpoint 映射。构造阶段不执行备份。
- `Schemas::BackupSchemas` 建立 `AppendSchema` 写会话，收集库表单元，按给定并发度
  完成表级 checksum/匹配/label rule 工作，再按原始枚举顺序导出统计并写入 meta。
- `schemaInfo` 是一个库或一张表的内部工作记录，聚合库表 JSON、checksum 三元组、
  统计索引和表/分区的 merge 许可；空库以 `tableInfo == None` 表示。
- `calculateChecksum`、`matchChecksum`、`dumpStatsToJSON`、`encodeToSchema` 和
  `checkMergeOptionAllowed` 分别隔离计算、校验、统计持久化、wire 数据组装和 label
  rule 查询。
- 三个 `*_for_test` 公共函数只为独立测试暴露私有行为；它们不是产品主链入口。

## 主要符号

- `DefaultSchemaConcurrency: u64 = 64`：与 Go 默认 schema 并发常量同值。注意
  `BackupSchemas` 实际接收的 `concurrency` 是 `u32`，调用方需要自行做类型转换。
- `iterFuncTp`：线程安全的共享回调。它接收 `Storage` 和一个逐项回调；空库传
  `None`，普通表传 `Some(&TableInfo)`。`client.rs` 为解决借用对象不能进入
  `'static` 闭包的问题，会先物化 `(DBInfo, Option<TableInfo>)`，再构造该回调。
- `Schemas` / `NewBackupSchemas`：公开任务对象与构造器。`Len` 只返回构造时传入的
  估算值，不重新枚举，也不保证等于运行时成功写出的数量。
- `Schemas::SetCheckpointChecksum`：整体替换 checkpoint checksum 映射；命中的表
  在执行时直接复用 `Crc64xor/TotalKvs/TotalBytes`。
- `Schemas::BackupSchemas`：本文件的主入口。关键参数包括 meta writer、可选
  checkpoint runner、存储、可选统计句柄、备份时间戳、可选外部 checksum map、
  两级并发度、`skipChecksum` 和可选进度回调。
- `schemaInfo`：私有可变状态。`stats` 字段仍参与编码，但本文件的当前主流程只由
  `dumpStatsToJSON` 填充 `statsIndex`，没有给 `stats` 赋值，所以正常路径的
  protobuf `Stats` 为空。
- `match_checksum_for_test`、`encode_schema_for_test`、
  `check_merge_option_allowed_for_test`：分别覆盖 checksum 聚合、protobuf 编码和
  merge rule 判定的测试桥接函数。

文件中没有 trait 定义、宏或条件编译项；公开面由常量、类型别名、`Schemas`、构造器、
三个方法和三个测试桥接函数组成，其余实现均为私有。

## 执行流程

1. `BackupSchemas` 记录总耗时，以 `metautil::AppendSchema` 调用
   `MetaWriter::StartWriteMetasAsync`。
2. 调用 `iterFunc`，把每个库表对克隆为 `schemaInfo`。系统库由
   `utils::IsSysDB` 识别，并把克隆后的库名改成 `utils::TemporaryDBName`，不修改
   迭代器持有的原对象。此阶段只收集 `units`；迭代错误立即返回。
3. 每个 `unit` 被送入 `WorkerPool::ApplyOnErrorGroup`。空库跳过所有表级工作；有表
   且未设置 `skipChecksum` 时，优先读取 checkpoint，未命中才调用
   `calculateChecksum`，随后在给定 `checksumMap` 存在时调用 `matchChecksum`。
4. 不论是否跳过 checksum，只要有表都会调用 `checkMergeOptionAllowed`。该查询失败
   被有意降级：保留默认 `false` 和空分区映射，备份继续。
5. worker 把 `(原始序号, Result<schemaInfo>)` 写入共享 `Mutex<Vec<_>>`；
   `wait_jobs` 等待全部线程，主线程再按序号排序。该排序使 Rust 的 meta 写出顺序
   可重复，而 Go 版本在 worker 内直接发送，并不提供同样的顺序承诺。
6. 对每个成功结果，若 checksum 来自现场计算且存在 checkpoint runner，则调用
   `FlushChecksum`；若存在统计句柄，则从 `MetaWriter::NewStatsWriter` 获取表级 writer，
   经 `PersistStatsBySnapshot` 和 `BackupStatsDone` 生成 `StatsIndex`。
7. `encodeToSchema` 用 `serde_json` 编码库、可选表和可选内联 stats，复制 checksum、
   stats index 与 merge 标志；随后 `MetaWriter::Send(MetaPayload::Schema(...))`。
   每次发送成功后才调用一次 `Progress::Inc`。
8. 全部单元处理后检查 `wait_jobs` 的聚合错误，记录 `summary::CollectDuration`，最后
   调用 `FinishWriteMetas`。任何更早返回的错误都不会走到该收尾调用。

`calculateChecksum` 使用 `NewExecutorBuilder(table, backupTS)`，固定请求来源为
`ExplicitTypeBR`，把 `copConcurrency` 传给 executor，并将执行结果的 checksum、KV 数
和字节数写回当前 `schemaInfo`。

`matchChecksum` 先读取主表 ID 的期望值，再对每个分区按 Go 语义聚合：CRC64 使用
异或，KV 数和字节数使用加法。映射中缺失的条目按零处理；三项任一不等即失败。

`checkMergeOptionAllowed` 用小写库名、表名和分区名生成 rule ID，以
`utils::LabelRuleBatchSize`（当前 stub 为 64）分批调用 `infosync::GetLabelRules`。
只有同时满足 `Key == "merge_option"` 与 `Value == "allow"` 才允许合并；分区结果
以原始大小写名称 `def.Name.O` 为键，未允许的分区不会写入 map。

## 数据与状态

`Schemas` 的 checkpoint map 在调用开始前整体注入，`BackupSchemas(&self, ...)` 运行时
只读它。为满足 `'static` worker 闭包，每个任务克隆 context、checkpoint map 和可选
checksum map，并从 `Storage` 各取一个 client 与 codec。此设计换取简单所有权边界，
代价是表数较多时会重复克隆映射；新增大规模 checkpoint 功能时需要关注内存与复制成本。

`schemaInfo` 的不变量如下：

- `tableInfo == None` 表示空库；此时 checksum、统计和 merge 查询均跳过，但库 JSON
  仍会作为一条 schema 写出，`Table`/`Stats` 为零长度字节数组。
- checksum 的三个字段来自同一个 checkpoint 项或同一次 executor 响应，不能只更新
  其中一部分；`matchChecksum` 也把三项作为一个整体验证。
- `statsIndex` 由 `StatsWriter::BackupStatsDone` 返回；有无统计句柄不应改变库表 JSON
  或 checksum。
- merge 标志默认保守地拒绝；只有显式 `allow` rule 才置真。label 服务错误同样保持
  拒绝状态。
- `size` 与实际 `units.len()` 没有运行时绑定，正确性依赖构造者传入一致计数。

输出状态属于注入对象：`MetaWriter` 管理 append 会话，`CheckpointRunner` 接收新算出的
checksum，`Progress` 统计成功发送数，`summary` 收集总耗时。本文件自身不持有文件句柄、
网络连接或跨调用缓存。

## 依赖与调用关系

上游关系：

- `br/pkg/backup/lib.rs` 声明并再导出 `schema`。
- `br/pkg/backup/client.rs::BuildBackupRangeAndInitSchema` 调用
  `BuildBackupSchemas` 物化库表，再把结果封装进 `NewBackupSchemas`。
- `br/pkg/backup/client.rs::Client::BuildBackupRangeAndSchema` 在检测到 checkpoint meta
  时调用 `Schemas::SetCheckpointChecksum`。
- 当前 Rust 非测试源码未找到 `.BackupSchemas(...)` 调用；测试调用位于
  `schema_test.rs`、`schema_merge_option_test.rs`、`client_test.rs` 和
  `parity_test.rs`。Go 产品入口 `br/pkg/task/backup.go` 调用 Go 版方法。

下游关系：

- `stubs::checksum::NewExecutorBuilder` 和 `KvClient`：表级 checksum。
- `stubs::checkpoint::{ChecksumItem, CheckpointRunner}`：续跑复用与新结果刷新。
- `stubs::statistics::Handle`、`metautil::StatsWriter`：快照统计导出。
- `stubs::label::NewRuleID` 与 `stubs::infosync::GetLabelRules`：表/分区 merge rule。
- `stubs::metautil::{MetaWriter, MetaPayload, AppendSchema}` 与
  `stubs::backuppb::Schema`：备份元数据输出边界。
- `stubs::{WorkerPool, wait_jobs}`：线程创建、并发上限近似和错误聚合；
  `utils::{IsSysDB, TemporaryDBName, LabelRuleBatchSize}`：系统库兼容与批量限制。

RustCodeGraph 的按文件节点确认本文件被 `schema_test.rs`、`parity_test.rs`、`stubs.rs`
等多个文件引用，并确认 `NewBackupSchemas` 的 Rust 定义与测试调用。图工具对 impl 方法的
`callers/callees` 查询在本次分析中超时，因此上述具体生产/测试调用边由精确 `rg` 搜索和
相邻源码核对补齐。

## 错误处理与边界

- `iterFunc`、checksum builder/executor、checksum 不匹配、checkpoint flush、统计导出、
  JSON 编码、meta send 和 finish 的错误都向上传播；部分位置用 `Error::Trace` 包装。
- worker 内的原始错误保存在 `computed`，线程返回的只是通用
  `"schema task failed"`。排序消费时会优先返回对应单元的原始错误；若没有原始单元错误
  但 `wait_jobs` 失败（例如线程 panic），则返回聚合错误。
- `matchChecksum` 对缺失主表或分区记录使用零值，而不是报“条目缺失”；因此只有本地
  实际 checksum 非零时才会表现为 mismatch。这一点是 Go 对齐语义，改变时需要兼容评估。
- `checkMergeOptionAllowed` 的直接调用会传播 infosync 错误；主流程明确吞掉该错误，
  将其解释为“不允许 merge”。这是一处故障降级边界，不应误改为默认允许。
- `skipChecksum` 同时跳过计算、checkpoint 复用、checkpoint flush 和外部 checksum
  匹配，但不跳过 label rule、统计、编码和 meta 写出。
- `calculateChecksum`、`matchChecksum`、`dumpStatsToJSON` 和 merge 检查都要求存在表；
  私有方法若被错误地用于空库会返回 `"nil tableInfo"`。正常主流程在调用前已分支。
- `StartWriteMetasAsync` 本身不返回 `Result`；启动后若发生任何提前错误，当前实现不会
  调用 `FinishWriteMetas`。调用方或 writer 实现不能假设失败路径总有成对 finish。
- label 查询、checksum client 与统计实现来自本地 `stubs.rs` 的精简接口。当前 crate
  并未直接依赖真实 TiDB/TiKV crate，不能把内存 stub 测试等同于真实网络与持久化验证。

## 并发与资源生命周期

Rust 实现分两阶段：先同步收集所有 `units`，再建立 worker。`WorkerPool` 把零并发修正为
1，并通过线程句柄链限制同时运行数量；`wait_jobs` join 所有句柄，把线程 panic 转换为
`"worker panicked"`。各 worker 只拥有自己的 `schemaInfo`，共享写入点只有
`Arc<Mutex<Vec<(usize, Result<schemaInfo>)>>>`，所以 worker 阶段不会并发调用 meta writer。

主线程在 join 后排序并串行执行 checkpoint flush、stats dump、encode、send 和 progress
更新。这保证输出次序等于迭代次序，也意味着统计导出和 meta I/O 不受 `concurrency`
并行化；`copConcurrency` 只传入单表 checksum executor。与 Go 版在 worker 内完成 stats、
merge 检查和 `Send` 相比，Rust 版的并发范围更窄，性能比较必须单独测量，不能由语义测试
推出吞吐等价。

context 被逐 worker 克隆，但本地 `Context`/worker 抽象并不证明真实 Go errgroup 的取消
传播语义。`Storage::GetClient`、`GetCodec` 返回拥有所有权的 trait object，任务结束即释放；
`StatsWriter` 每张表由 meta writer 新建并在该表导出结束后释放；本文件不启动常驻后台任务。

## 与 Go 版本的对应关系

逐项对应关系可由 `br/pkg/backup/schema.go` 核对：常量、`schemaInfo` 字段、`iterFuncTp`、
`Schemas`、构造/设置/长度方法，以及五个私有处理步骤均保留。系统库临时改名、checkpoint
优先、checksum 聚合、stats index、label rule 批处理与原始分区名作为 map key等关键行为
也保持一致。

已确认的实现差异：

- Go 在 `iterFunc` 回调中直接向 errgroup 调度任务；Rust 先收集并克隆全部单元，再调度。
- Go worker 内完成 checksum flush、stats、merge 查询、编码、发送和进度；Rust worker
  只完成 checksum/匹配/merge 查询，join 后按序完成其余步骤。
- Go 使用 tracing 与日志记录单表计算、stats 失败、merge 查询降级和 checksum mismatch；
  Rust 当前只保留结果与 summary 计时，没有等价日志/tracing。
- Go 将 errgroup 派生 context 传给 checksum/merge 查询；Rust 使用克隆的原 context，
  本地 worker stub 不提供同等的“首错取消其它任务”保证。
- Go 依赖真实 TiDB/TiKV、PD infosync、meta writer 和 checkpoint runner；Rust crate 由
  `stubs.rs` 提供精简实现。Cargo 元数据也明确说明使用 local traits/stubs。
- Rust 额外公开三个 `*_for_test` 桥接函数，以遵守测试与生产源文件分离的仓库约定。

这些差异不应被描述成 Go 逻辑的删除：核心数据变换和失败判定仍存在；但接入真实依赖、
取消行为、观测性和并发吞吐仍需在更上层迁移任务中验证。

## 扩展指南

- 新增 schema 输出字段：先扩展 `schemaInfo` 与 `encodeToSchema`，同步
  `stubs::backuppb::Schema`，再在独立的 `schema_test.rs` 或专用测试文件断言普通表与
  空库编码；同时核对 Go `schema.go` 和恢复侧消费者的兼容性。不要把测试写回本文件。
- 修改 checksum：以 `calculateChecksum` 和 `matchChecksum` 为入口，必须保留分区 CRC
  异或、计数相加及 checkpoint 三元组一致性；同步覆盖单表、分区、缺失 map 条目、
  mismatch 与 `skipChecksum`。
- 修改 checkpoint：同时审查 `SetCheckpointChecksum`、worker 的命中分支、join 后的
  `FlushChecksum` 条件，以及 `client.rs::Client::BuildBackupRangeAndSchema` 的注入点，
  防止重复计算或把历史值再次 flush。
- 修改统计：通过 `MetaWriter::NewStatsWriter` 获取 writer，不要绕过 writer 的内联/
  外部文件策略；扩展 `schema_test.rs::backup_schemas_uses_meta_writer_stats_factory` 以及
  有/无 stats handle 的等价性断言。
- 修改 merge rule：以 `checkMergeOptionAllowed` 为唯一判定点，维持 `.L` 生成 rule ID、
  `.O` 输出分区键、批量上限和“失败默认拒绝”；同步
  `schema_merge_option_test.rs` 与 Go 对应测试。
- 扩大并发范围或改变写出顺序前，先确认 `MetaWriter`、`StatsWriter` 和 checkpoint runner
  的线程安全及顺序契约，并新增确定性的并发/错误测试。当前测试依赖有序写出，Go 本身
  则没有相同排序保证。
- 要接入 Rust 产品主链，应在 task/命令层增加真正的 `BackupSchemas` 调用并补集成验证；
  不能用当前测试调用替代接线证据。真实依赖移植还必须遵守仓库关于外部 Rust 依赖独立
  上游仓库、提交和 tag 的约束，不能复制到 vendor 或用本地 patch。

兼容风险集中在 protobuf 字段、JSON 形状、系统库命名、分区 map 键大小写和错误降级；
性能风险集中在映射克隆、全量预收集、线程创建以及 join 后串行 stats/meta I/O。

## 验证依据

本说明读取并交叉核对了以下直接证据：

- 目标实现：`br/pkg/backup/schema.rs`（532 行），包括全部公开/私有符号和主流程。
- crate 边界：`br/pkg/backup/Cargo.toml`、`br/pkg/backup/lib.rs`。
- Rust 上游：`br/pkg/backup/client.rs` 中的 `Client::BuildBackupRangeAndSchema`、
  `BuildBackupRangeAndInitSchema`、`BuildBackupSchemas`。
- 本地依赖语义：`br/pkg/backup/stubs.rs` 中的 `Storage`、`MetaWriter`、
  `CheckpointRunner`、`Progress`、`statistics::Handle`、`WorkerPool` 与 `wait_jobs`。
- Go 对照：`br/pkg/backup/schema.go`、`br/pkg/backup/client.go`，以及产品调用位置
  `br/pkg/task/backup.go`。
- Rust 独立测试：`br/pkg/backup/schema_test.rs` 覆盖空库/缺库、单表和多表、checksum、
  stats writer 工厂、系统库临时改名；`schema_merge_option_test.rs` 覆盖 rule ID、分区和
  全流程默认拒绝；`parity_test.rs` 覆盖编码、单表/分区 checksum、mismatch、显式 allow、
  `Len`、默认并发、进度和 writer finish；`client_test.rs` 提供构造链组合证据。
- Go 测试：`br/pkg/backup/schema_test.go` 与
  `br/pkg/backup/schema_merge_option_test.go`，用于核对原测试意图和移植语义。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；
  `node --file br/pkg/backup/schema.rs` 成功读取完整目标并报告引用文件；
  `node NewBackupSchemas` 同时定位 Go/Rust 定义，并给出 Rust 测试调用 trail。
  对 impl 方法的 `callers/callees` 查询本次未在 30 秒内返回，故未把它当作已验证图边。
- 精确文本搜索：确认 Rust 侧构造、checkpoint 注入和所有 `.BackupSchemas` 调用位置，
  并确认当前非测试 Rust 文件没有执行该方法的调用点。

本任务是纯文档分析，按计划未运行 Cargo。结构检查应要求本文档存在，且固定的十一个
二级标题各出现一次；人工复核还需确认没有把 stub 能力、测试调用或 Go 产品接线误写成
Rust 当前生产主链事实。
