# `pkg/util/workloadrepo/utils.rs`

## 文件定位

本文件属于 `astersql-util-workloadrepo` crate。crate 入口 `pkg/util/workloadrepo/lib.rs` 以私有 `mod utils` 装入本文件，再通过 `pub use utils::*` 导出其中的公开函数；`pkg/util/workloadrepo/Cargo.toml` 表明该 crate 的直接外部依赖只有 `chrono`，并以 `pkg/util/workloadrepo` 为 Go 移植来源。

它位于工作负载仓库的配置与分区管理公共层：`table.rs::Worker::createAllTables` 用它生成新表的初始分区，`housekeeper.rs::{createPartition,dropOldPartition}` 用它维护分区，`worker.rs::Worker::setRepositoryDest` 用它规范化并校验目标配置。文件还为 `worker::worker` 增加保留天数设置方法。这里不执行后端 SQL，也不注册系统变量；SQL 执行和运行时启停分别由 `housekeeper.rs`、`table.rs` 与 `worker.rs` 完成。

## 核心职责

- 把本地日期稳定映射为 `pYYYYMMDD` 分区名，并严格执行反向解析（`generatePartitionName`、`parsePartitionName`）。
- 生成 MySQL `PARTITION BY RANGE(TO_DAYS(...))` 的初始定义，或在已有分区之后追加未来两天所需的范围（`generatePartitionDef`、`generatePartitionRanges`）。
- 模拟 Go `time.Date(..., time.Local)` 在午夜缺失或重复时的偏移查找行为（私有函数 `local_midnight`）。
- 将保留天数字符串解析为机器整数并写入 worker 状态（`Worker::setRetentionDays`）。
- 将仓库目的地转为小写，仅接受空串和 `table`（`validateDest`）。

这些职责对应 `pkg/util/workloadrepo/utils.go` 的同名逻辑；当前 Rust API 用 `String`/切片和 `Result<_, String>` 替代 Go 的 `strings.Builder`、表元数据与结构化错误。

## 主要符号

- `pub fn generatePartitionDef(output: &mut String, column: &str, now: DateTime<Local>) -> Result<(), String>`：追加完整的按日 RANGE 分区子句。它调用 `generatePartitionRanges(output, &[], now)`；若后者反常地报告“全部已存在”，返回 `could not generate partition ranges`。
- `pub fn generatePartitionName(time: DateTime<Local>) -> String`：按本地日期生成 `p%Y%m%d`，不读取时分秒。
- `pub fn parsePartitionName(part: &str) -> Result<DateTime<Local>, String>`：要求恰好为小写 `p` 加八位数字，显式检查月份、合法日和尾随文本，返回该日期按 Go 兼容规则构造的本地零点。
- `fn local_midnight(date: NaiveDate) -> DateTime<Local>`：先把朴素午夜当作 UTC 时间戳，连续查询两次本地 UTC 偏移，再得到 Go 兼容的本地时间。它是本文件唯一私有函数。
- `pub fn generatePartitionRanges(output: &mut String, existing: &[String], now: DateTime<Local>) -> Result<bool, String>`：以当天本地零点和 `existing.last()` 中较晚者为基线，按需追加 `now` 后第 1、2 天的分区定义；返回值 `true` 表示没有追加内容。
- `pub fn Worker::setRetentionDays(&self, value: &str) -> Result<(), String>`：解析 `isize`，转为 `i32` 后写入 `self.state`；范围合法性不在本层处理。
- `pub fn validateDest(orig: &str) -> Result<String, String>`：Unicode 逐字符小写化后接受 `""` 或 `"table"`，否则生成与 Go 格式对齐的错误文本。

文件没有模块级常量、类型、trait 或条件编译项；它通过 `crate::*` 使用 `repositoryDest`、`repositoryRetentionDays` 等 crate 内常量。

## 执行流程

新表创建链路是 `table.rs::Worker::createAllTables -> generatePartitionDef -> generatePartitionRanges -> generatePartitionName`。`generatePartitionDef` 先写入 ` PARTITION BY RANGE( TO_DAYS(<column>) ) (`，范围生成器从空的已有分区列表出发追加明日与后日两个 `VALUES LESS THAN` 定义，最后补右括号。元数据表传入 `BEGIN_TIME`，其它仓库表传入 `TS`。

日常补分区链路是 `housekeeper.rs::createPartition -> RepositoryBackend::partitions -> generatePartitionRanges`。范围生成器先以 `now` 当天本地零点为 `lastPart`；如果分区列表非空，只解析最后一个名称，并在该日期更晚时提升基线。随后逐个计算 `now.date_naive() + 1/2 天`，只为严格晚于基线的日期写入定义，以 `, ` 分隔。返回 `false` 时，调用方才拼装并执行 `ALTER TABLE ... ADD PARTITION`。

删旧分区链路是 `housekeeper.rs::dropOldPartition -> parsePartitionName`。每个名称被还原为本地日期，调用方用 `(now - time).num_days() >= retention` 判断是否删除。表就绪检查 `table.rs::checkTableExistsByIS` 同样解析最后一个分区，并要求其日期严格晚于 `now + 1 天`。

配置链路中，`worker.rs::Worker::setRepositoryDest` 先调用 `validateDest`；规范化结果为 `table` 时启动仓库，为空时停止。`setRetentionDays` 当前由 Rust 测试直接覆盖；Go 对照中它作为全局系统变量 `tidb_workload_repository_retention_days` 的 `SetGlobal` hook，取值范围 `0..=365` 由 sysvar 层先行约束。

## 数据与状态

分区生成只修改调用者传入的 `String`。成功时内容是在原字符串末尾追加，不会清空已有前缀；`generatePartitionRanges` 在解析最后一个已有分区失败时尚未写入新内容。`existing` 被视为按日期升序排列，函数只观察最后一项；更早的非法名称不会被解析，这一行为由 `utils_test.rs::partition_ranges_preserve_append_skip_and_error_contracts` 明确覆盖。

日期状态使用 `chrono::{DateTime<Local>,NaiveDate,Duration}`。生成窗口固定为 `now` 的本地日历日之后第 1、2 天，而非从传入瞬间增加 24/48 小时；这使 SQL 边界保持 `YYYY-MM-DD` 日历语义。`local_midnight` 的结果在 DST 跳变日可能不是墙钟 `00:00`，这是为匹配 Go `time.Date` 而保留的兼容行为。

worker 的可变状态位于 `Worker.state: Mutex<_>`。`setRetentionDays` 在成功解析后持锁写入 `retentionDays`；失败时不获取锁、不改变原值。`validateDest` 和所有分区函数没有共享可变状态。

## 依赖与调用关系

RustCodeGraph 将本文件标为 9 个符号，并列出直接使用文件 `utils_test.rs`、`worker.rs`、`worker_test.rs`；对生产引用的文本核验还确认了 `table.rs` 和 `housekeeper.rs` 的调用点。主要关系如下：

- 上游：`table.rs::Worker::createAllTables` 调用 `generatePartitionDef`；`table.rs::checkTableExistsByIS` 调用 `parsePartitionName`。
- 上游：`housekeeper.rs::createPartition` 调用 `generatePartitionRanges`；`housekeeper.rs::dropOldPartition` 调用 `parsePartitionName`。
- 上游：`worker.rs::Worker::setRepositoryDest` 调用 `validateDest`；`worker_test.rs` 还直接使用名称生成、目的地校验和保留天数设置。
- 文件内部：`generatePartitionDef -> generatePartitionRanges`；`generatePartitionRanges -> parsePartitionName/generatePartitionName/local_midnight`；`parsePartitionName -> local_midnight`。
- 下游库：仅使用 `chrono` 做本地时间、朴素日期、日历加法和格式化；标准库承担字符串构造、整数解析和互斥锁中毒处理。

`lib.rs` 的通配再导出使这些公开自由函数成为 crate API；`setRetentionDays` 则是私有 worker 类型的固有方法，供 crate 内部或同 crate 测试使用。

## 错误处理与边界

所有可失败公开入口都以 `Result<_, String>` 返回错误。`parsePartitionName` 拒绝错误前缀、非 ASCII 数字、不足或多余字符、月份越界、非法日期；例如非闰年的 `p20260229` 会得到 `day out of range`。内部数字解析只在已经确认全为 ASCII 数字后 `unwrap`，因此该处不会因用户输入触发 panic。

`generatePartitionRanges` 只解析 `existing.last()`。最后一项非法时返回错误并保持输出未变；列表前部非法但最后一项合法时不会报错。调用者必须保证分区顺序，否则“最后一个即最晚一个”的假设可能漏建分区。日期加两天使用 `checked_add_signed(...).unwrap()`，极端接近 `NaiveDate` 上界的输入会 panic；正常数据库时间范围内未见额外防护。

`local_midnight` 对 `timestamp_opt(...).unwrap()` 的结果做强假设：系统本地时区必须能为相关 Unix 时间戳提供唯一映射。该实现专门处理午夜 DST 跳变的 Go 兼容结果，但没有把超出 chrono/平台范围转成错误。

`setRetentionDays` 接受 `isize` 语法，包括前导 `+`；空白、浮点文本和超出 `isize` 的值报错。成功后使用 Rust `as i32` 的截断/环绕语义匹配 Go `int32(n)`，所以本层可把 `2147483648` 写为 `i32::MIN`。生产合法范围必须由上游 sysvar 层保证。锁中毒使用 `unwrap`，会 panic 而非返回错误。

`validateDest` 的比较不做 trim，因此带空白的值无效。错误消息将变量名限制为 64 个字符、规范化后的值限制为 200 个字符，以对应 Go `%-.64s`/`%-.200s` 的显示边界；Rust 当前按字符截断，不按字节截断。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、数据库事务或后端连接。分区与目的地函数是同步计算；传入的 `&mut String` 独占借用保证同一输出缓冲区不会被并发写入。

唯一的同步状态变更在 `Worker::setRetentionDays`：解析在锁外完成，成功后临时获取 `state` 互斥锁，完成一次字段赋值即释放。失败路径不触碰状态。分区 DDL 的重试、后端资源和 owner 协调属于调用方 `housekeeper.rs`、`table.rs` 和 `worker.rs`，不由本文件持有。

本地时区来自进程环境。`utils_test.rs::midnight_transition_matches_go_date` 明确要求在单独进程以 `TZ=America/Sao_Paulo` 运行，避免并行测试修改进程全局时区；生产代码自身不会修改 `TZ`。

## 与 Go 版本的对应关系

`pkg/util/workloadrepo/utils.go` 提供六个同名入口，Rust 还把 Go 时间构造的兼容细节拆为 `local_midnight`。名称格式、未来两天窗口、`allExisted` 布尔语义、目的地允许值以及保留天数的 `int32` 转换均保持一致。

主要接口差异是：Go `generatePartitionRanges` 接收 `*model.TableInfo`，从其分区定义中取最后一项；Rust 接收已经由 `RepositoryBackend::partitions` 提取的 `&[String]`，因而不直接依赖 TiDB model。Go 使用结构化 `errWrongValueForVar`，Rust用字符串复现关键消息。Go 的 `setRetentionDays` 接收 `context.Context` 并锁住 `worker` 本体；Rust 无 context 参数，锁的是 `Worker.state`。

Go `init` 在 `worker.go` 中注册目的地与保留天数系统变量，并为保留天数声明 `0..=365`；当前 Rust `lib.rs` 只组装并再导出 crate，未在本文件注册 sysvar。因此不能把“Rust 方法存在”等同于“已经接入完整 SQL 系统变量注册链”。

独立 Rust 测试 `utils_test.rs` 比 Go 同目录测试更直接地固定了解析错误文本、非闰年、严格长度、追加前缀、只读最后分区、整数溢出转换和 DST 午夜行为。集成式语义由 `worker_test.rs::{TestSettingSQLVariables,TestCreatePartition,TestDropOldPartitions,TestAddNewPartitionsOnStart}` 覆盖；Go 对照位于 `worker_test.go` 的同类测试及分区辅助函数。

## 扩展指南

调整分区前瞻天数或命名规则时，应从 `generatePartitionRanges`、`generatePartitionName` 与 `parsePartitionName` 一起修改，并同步检查 `table.rs::checkTableExistsByIS` 的“覆盖到明天之后”不变量、`housekeeper.rs` 的创建/删除逻辑以及 Go 对照实现。必须在独立的 `utils_test.rs` 增加边界用例，不应把测试嵌入生产文件；还应同步评估 `worker_test.rs` 的建表、启动和分区维护场景。

若改变 `existing` 的输入契约，优先在 `RepositoryBackend::partitions` 边界明确排序，而不是默认本函数可从无序列表取最后项。若希望本函数自行寻找最大日期，需要决定遇到任一非法名称时是失败还是忽略，并与 Go `TableInfo.Partition.Definitions` 的顺序和错误行为保持一致。

新增仓库目的地时，扩展点是 `validateDest` 和 `worker.rs::Worker::setRepositoryDest`：前者定义规范化/错误契约，后者必须实现相应生命周期。新增保留期规则时，不要仅收紧 `setRetentionDays`；应与系统变量元数据的最小/最大值及 Go hook 同步，避免绕过 sysvar 的内部调用出现语义分叉。

涉及时间逻辑的改动需保留 DST 专项测试，并在隔离进程中设置时区。涉及用户可见错误时，应同时检查 Go 的结构化错误格式、Rust 字符/字节截断差异和调用方是否依赖精确文本。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，本目标已索引为 9 个符号；`files --filter pkg/util/workloadrepo` 与文件节点查询确认目标及直接使用文件。对 `generatePartitionRanges`、`validateDest` 等执行了 `query`，精确 `callers/callees` 未返回边详情，故用索引点名文件和文本引用补齐调用证据。
- 目标与 crate：`pkg/util/workloadrepo/utils.rs`、`pkg/util/workloadrepo/Cargo.toml`、`pkg/util/workloadrepo/lib.rs`。
- 生产调用：`pkg/util/workloadrepo/table.rs`、`pkg/util/workloadrepo/housekeeper.rs`、`pkg/util/workloadrepo/worker.rs`。
- Go 对照：`pkg/util/workloadrepo/utils.go`、`pkg/util/workloadrepo/worker.go`。
- 测试证据：`pkg/util/workloadrepo/utils_test.rs`、`pkg/util/workloadrepo/worker_test.rs`、`pkg/util/workloadrepo/worker_test.go`。
- 本任务是纯文档分析，未修改 Rust、Go 或 Cargo，也未运行 Cargo。交付前按任务文件命令确认目标文件存在且恰有 11 个固定二级章节，并人工复核文档覆盖文件存在原因、运行链路、安全扩展点及已知边界。
