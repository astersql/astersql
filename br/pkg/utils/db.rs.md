# `br/pkg/utils/db.rs`

## 文件定位

`db.rs` 是 `astersql-br-pkg-utils` crate 中的 TiDB/TiKV 配置辅助模块，由 [`br/pkg/utils/lib.rs`](./lib.rs) 通过 `pub mod db` 暴露。它位于 BR 工具层与 `RestrictedSQLExecutor` 边界之间：调用方无需自行拼装 `SHOW CONFIG`/`SET CONFIG`，即可读取 Region 切分参数、临时调整 GC 与 RocksDB 后台任务数，并查询本进程是否存在日志备份任务。当前 Rust 生产调用证据集中在 [`br/pkg/task/stream.rs`](../task/stream.rs) 的流式恢复配置控制；其他公开函数仍为 Go API 对齐和后续接线保留。

该文件不建立数据库连接，也不拥有 session。所有 SQL 都通过调用方传入的 `&mut dyn RestrictedSQLExecutor` 执行；该 trait 及返回行、字段元数据是 [`br/pkg/utils/stubs.rs`](./stubs.rs) 提供的本地 SQL 边界。`Cargo.toml` 将本目录声明为 `astersql-br-pkg-utils` library，并直接依赖日志、错误、parser types 与 `bytesize`，没有 feature 条件控制本文件。

## 核心职责

1. `GetSplitSize`、`GetSplitKeys` 和 `GetRegionSplitInfo` 从 TiKV 配置读取 Region 拆分阈值；读取、转换或解析失败时使用稳定默认值，使备份切分规划能够继续。
2. `GetGcRatio`/`SetGcRatio` 读取或修改 `gc.ratio-threshold`；与 split 查询不同，这类错误必须返回给上层，因为禁用或恢复 GC 失败会影响集群行为。
3. `GetRocksDBMaxBackgroundJobs`/`SetRocksDBMaxBackgroundJobs` 为恢复期间临时限制 `rocksdb.max-background-jobs` 提供读写接口。
4. `LogBackupTaskCountInc`、`LogBackupTaskCountDec`、`CheckLogBackupTaskExist` 和 `IsLogBackupInUse` 维护并查询进程内日志备份任务计数。
5. `GetTidbNewCollationEnabled` 暴露与 Go 版本一致的系统变量键名，避免调用方重复硬编码。

## 主要符号

- `TidbNewCollationEnabled: &str = "new_collation_enabled"` 与 `GetTidbNewCollationEnabled() -> &'static str`：排序规则变量名常量及稳定访问函数。
- `GetRegionSplitInfo(ctx) -> (u64, i64)`：依次组合 `GetSplitSize` 与 `GetSplitKeys` 的结果；它不保证两次查询来自同一配置快照。
- `GetSplitSize(ctx) -> u64`：查询 `coprocessor.region-split-size`，默认值为 `96 * 1024 * 1024` 字节；用 `ByteSize` 解析人类可读容量，测试确认 `10MB` 为十进制 `10_000_000`。
- `GetSplitKeys(ctx) -> i64`：查询 `coprocessor.region-split-keys`，按十进制有符号整数解析，默认值为 `960_000`。
- `field_type_at(fields, 3) -> &FieldType`：私有元数据访问器，固定取得 `SHOW CONFIG` 第四列 `Value` 的字段类型；字段或 column 元数据缺失会 `expect` panic。
- `GetGcRatio(ctx) -> Result<Option<String>, SharedError>`：查询 GC ratio；空结果用 `Ok(None)` 明确表示配置缺失/不支持，而非执行失败。
- `DefaultGcRatioVal = "1.1"`、`DisabledGcRatioVal = "-1.0"`：默认 GC 比率与禁用 GC 的哨兵值。
- `SetGcRatio(ctx, ratio)`：参数化执行 GC 配置更新，成功后以 Warn 级别记录目标值；失败错误包含目标 ratio。
- `RocksDBMaxBackgroundJobsForRestore = "1"`：恢复期间使用的后台任务数。
- `GetRocksDBMaxBackgroundJobs(ctx) -> Result<String, SharedError>`：读取后台任务数；空结果返回空字符串。
- `SetRocksDBMaxBackgroundJobs(ctx, jobs)`：参数化更新后台任务数并记录审计日志；失败错误包含目标 jobs。
- `logBackupTaskCount: AtomicI32` 及四个计数相关函数：进程级共享状态，`Inc`/`Dec` 由调用方负责配对；`IsLogBackupInUse` 当前忽略保留的 context 参数，只委托计数检查。

## 执行流程

配置读取的共同流程是：构造限定 `type = 'tikv'` 的 `SHOW CONFIG` 字符串，通过 `ExecRestrictedSQL` 执行；若有结果，只读取 `rows[0]` 的第 4 列，并借助 `fields[3].column.FieldType` 将 Datum 转成字符串。多 TiKV 实例可能返回多行，但本模块把首行作为集群配置代表，不比较各实例是否一致。

Region 参数读取采用“尽量可用”路径。`GetSplitSize` 在 SQL 错误、Datum 转换错误、容量解析错误或无行时返回 96 MiB；`GetSplitKeys` 在相同类别的错误或空结果时返回 960,000。前述错误路径记录 Warn（空结果除外）。`GetRegionSplitInfo` 连续调用这两个函数，因此可能得到真实值与默认值的混合结果。

GC 与 RocksDB 写入采用“必须可观测”路径。`SetGcRatio` 和 `SetRocksDBMaxBackgroundJobs` 将字符串放入 `Vec<Box<dyn Any>>` 作为绑定参数，调用 `SET CONFIG`；执行失败先转成 `SharedError`，再由 `Annotate` 补充目标配置和值，成功后才写 Warn 日志。读取 GC 时空行映射为 `None`，读取 RocksDB jobs 时空行映射为空字符串，这两种返回契约由各自调用方判断。

已接线的 Rust 主链见 `br/pkg/task/stream.rs`：`DisableGC` 创建 session、保存旧 ratio、写入 `DisabledGcRatioVal`，再返回持有该 session 的恢复闭包；`KeepRocksDBMaxBackgroundJobsLow` 保存旧值，非空时写入 `RocksDBMaxBackgroundJobsForRestore` 并返回恢复闭包，空值时返回无操作闭包。恢复逻辑因此可以在操作前调整配置，并在清理阶段使用旧值复原。

## 数据与状态

配置值在本文件内都以字符串跨越 SQL 边界；只有 split size 与 split keys 在返回前分别转换成 `u64` 和 `i64`。代码不缓存 TiKV 配置，每次 getter 都执行查询；setter 也不保存旧值，保存和恢复责任属于 `br/pkg/task/stream.rs` 等上层生命周期控制器。

唯一可变全局状态是 `AtomicI32 logBackupTaskCount`。`LogBackupTaskCountInc` 和 `LogBackupTaskCountDec` 先做原子加减，再单独 load 当前值用于日志。`CheckLogBackupTaskExist` 只判断计数是否大于零；计数不是跨进程或跨节点状态，也不与 SQL session 绑定。代码没有负数保护、上溢保护或 RAII guard，正确性依赖调用方严格配对。

## 依赖与调用关系

上游方面，RustCodeGraph 将 `br/pkg/task/stream.rs` 和 `br/pkg/utils/db_test.rs` 标识为本文件的直接使用者。生产侧 `DisableGC` 调用 `GetGcRatio`、`SetGcRatio`，`KeepRocksDBMaxBackgroundJobsLow` 调用 RocksDB getter/setter。独立测试覆盖所有当前 Rust API 的主要契约。仓库搜索未发现其他 Rust 生产调用，因此 Region、日志任务计数和排序规则帮助函数在 Rust 侧目前尚未接入对应的完整 Go 主链。

下游方面，所有配置函数依赖 `crate::stubs::RestrictedSQLExecutor`、`Row::GetDatum`、`Datum::ToString` 和 `ResultField` 元数据；日志通过 `astersql-br-pkg-logutil` 的 `log`、`Field`、`ShortError` 输出；错误通过 `astersql-errors::{SharedError, Annotate}` 归一化；容量字符串由 `bytesize::ByteSize` 解析；字段类型来自 `astersql-parser-types::FieldType`。

Go 生产调用补充了迁移目标证据：`br/pkg/task/stream.go` 使用 Region、GC 和 RocksDB API；`br/pkg/task/backup.go`、`restore.go` 使用排序规则变量名；`br/pkg/streamhelper/advancer.go` 配对增减任务计数；`pkg/telemetry/data_feature_usage.go` 查询日志备份是否占用。这些 Go 调用不能视为 Rust 已接线行为。

## 错误处理与边界

- split size/keys 将 SQL、字符串转换和解析错误降级为默认值；这是显式容错策略，不应在扩展时误改成传播错误。
- GC/RocksDB getter 传播 SQL 与 Datum 转换错误；setter 还为错误添加配置名和值，便于定位部分恢复失败。
- 所有非空读取都假设 `SHOW CONFIG` 有至少四列且 Value 列具有 column 元数据。`field_type_at` 在这一协议被破坏时 panic；`db_test.rs::split_size_rejects_missing_value_field_metadata_like_go` 固化了该边界。
- 读取只使用首行，未检测不同 TiKV 实例配置漂移。若未来要求一致性检查，需要定义冲突策略，而不能直接改变现有首行语义。
- `GetSplitKeys` 接受任何可解析的 `i64`，包括零或负数；本层没有业务范围校验。setter 同样不校验 ratio/jobs 字符串，校验由 TiDB/TiKV 或上层负责。
- 日志任务计数允许减到负数；负数会被 `CheckLogBackupTaskExist` 视为不存在。调用方必须保证一次开始对应一次结束。

## 并发与资源生命周期

配置函数借用可变 executor，调用期间由 Rust 借用规则阻止同一个 executor 被并发可变访问；本文件不创建线程、异步任务、锁、事务或连接。它也不负责恢复已修改配置：上层 `DisableGC`/`KeepRocksDBMaxBackgroundJobsLow` 把 session 移入闭包以延长其生命周期，并显式调用闭包恢复旧值。

任务计数使用 `Ordering::Relaxed`。这里只需要对单个计数器的原子读改写和“当前是否大于零”的近似探测，不用它发布或同步其他内存状态，所以没有 acquire/release 顺序保证。加减后的日志 load 与刚才的 fetch 操作之间可以穿插其他线程更新，日志值用于观测而非精确事件序号。

## 与 Go 版本的对应关系

Rust 函数、常量和 SQL 字符串整体逐项对应 [`br/pkg/utils/db.go`](./db.go)。默认 split size、默认 split keys、GC 常量、RocksDB 恢复值、只取 `SHOW CONFIG` 第四列/首行以及日志任务计数语义均保持一致。Rust 用 `Option<String>` 区分 GC 配置空结果，等价表达 Go 的空字符串加 nil error；RocksDB getter则直接保留 Go 的空字符串契约。

实现边界存在几处值得注意的差异。Go 为受限 SQL 显式传入 `kv.InternalTxnBR`，Rust 当前向本地 stub 传 `Default::default()` context；这反映 Cargo 注释所述的精简 SQL/KV 边界，而不是完整事务来源接线。Go 的 `units.FromHumanSize` 在 Rust 中由 `bytesize::ByteSize` 替代，现有对照测试验证了 `10MB` 的关键语义。Go 的 atomic 类型来自 `go.uber.org/atomic`，Rust 使用标准库 `AtomicI32` 和 Relaxed ordering。Rust setter SQL 使用 `?` 绑定占位符，而 Go 文本写作 `%?`；两边测试均通过 mock 参数写回验证值传递契约。

测试对应关系以 [`br/pkg/utils/db_test.rs`](./db_test.rs) 和 [`br/pkg/utils/db_test.go`](./db_test.go) 为准。Rust 测试除 Go 主路径外，还覆盖 SQL 错误、非法值默认降级、空结果、缺少字段元数据 panic、公开常量，以及 RocksDB getter/setter 错误注解。

## 扩展指南

新增 TiKV 配置 getter 时，应先确定它属于 split 类的容错默认策略，还是 GC/RocksDB 类的错误传播策略；复用 `field_type_at` 时必须维持 `SHOW CONFIG` 四列协议，并在 `br/pkg/utils/db_test.rs` 增加独立测试。新增 setter 应使用参数绑定、在错误中包含配置键和值，并记录成功变更；若涉及临时修改，还要在上层生命周期中保存旧值并保证所有退出路径恢复。

若把尚未接线的 Go 行为迁移到 Rust，应在相应生产模块接入而不是把主流程塞入本工具文件：Region 参数对应流式恢复切分，排序规则对应 backup/restore session，任务计数对应 stream helper 的任务开始/结束，遥测只读取状态。计数生命周期最好由独立 guard 在直接调用侧保证，但改变公开行为前需与 Go 的显式 Inc/Dec 语义和独立测试同步。

变更容量解析器、默认值、首行选择或 Relaxed ordering 都有兼容或并发风险。特别是解析器必须继续覆盖十进制 `MB`；配置写入必须保留失败传播，否则可能静默留下禁用 GC 或受限 RocksDB 并发。测试逻辑应继续留在同目录独立的 `db_test.rs`，不要内嵌到生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/utils/db.rs` 确认目标文件已索引；`node --file br/pkg/utils/db.rs --offset 1 --limit 500` 列出完整 302 行及 26 个符号，并报告直接使用文件为 `br/pkg/task/stream.rs`、`br/pkg/utils/db_test.rs`；`callees GetSplitSize`/`callees GetGcRatio` 验证 `field_type_at`、`ExecRestrictedSQL`、`ShortError` 等下游边。
- 源码：[`br/pkg/utils/db.rs`](./db.rs)；crate 入口与测试挂载：[`br/pkg/utils/lib.rs`](./lib.rs)；crate 边界与依赖：[`br/pkg/utils/Cargo.toml`](./Cargo.toml)。
- Rust 生产调用：[`br/pkg/task/stream.rs`](../task/stream.rs) 的 `DisableGC` 与 `KeepRocksDBMaxBackgroundJobsLow`；独立 Rust 测试：[`br/pkg/utils/db_test.rs`](./db_test.rs)。
- Go 对照：[`br/pkg/utils/db.go`](./db.go)、[`br/pkg/utils/db_test.go`](./db_test.go)，以及仓库搜索得到的 `br/pkg/task/stream.go`、`backup.go`、`restore.go`、`br/pkg/streamhelper/advancer.go`、`pkg/telemetry/data_feature_usage.go` 调用点。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务指定的固定章节命令进行结构验证，并人工核对仅新增本说明文档、未修改 Rust/Go/Cargo/`plan.md`。
