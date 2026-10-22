# `pkg/statistics/analyze.rs`

## 文件定位

用 `AnalyzeTableID::{GetStatisticsID, IsPartitionTable, String, Equals}` 集中实现物理统计 ID 选择、分区判定、日志展示和 Go 指针相等语义。

## 核心职责

- 用 `NonPartitionTableID == -1` 表示“非分区表”，让 `AnalyzeTableID` 能用同一结构表达逻辑表 ID 与可选物理分区 ID。
- 用 `AnalyzeTableID::{GetStatisticsID, IsPartitionTable, String, Equals}` 集中实现物理统计 ID 选择、分区判定、日志展示和 Go 指针相等语义。
- 用 `AnalyzeResult` 聚合一组列或索引产生的 `Histogram`、`CMSketch`、`TopN` 与 `FMSketch`；`IsIndex` 区分列结果和索引结果。
- 用 `AnalyzeResults` 聚合一次任务的错误、任务描述、多个分组结果、表级计数、统计版本及快照基线元数据。
- 用两层 `DestroyAndPutToPool` 显式释放 FM sketch 向量的 backing storage，并把每个直方图交给 `Histogram::DestroyAndPutToPool` 清理内部缓冲区。

本文件只保存数据和执行轻量清理，不扫描表、构建草图、持久化 `mysql.stats_*`，也不启动 ANALYZE worker。

## 主要符号

- `pub const NonPartitionTableID: i64 = -1`：非分区哨兵；`PartitionID` 不等于它时，结构被视为分区表标识。
- `pub struct AnalyzeTableID { TableID, PartitionID }`：`TableID` 是逻辑表 ID，`PartitionID` 是构建分区统计时使用的物理分区 ID。
  - `GetStatisticsID(&self) -> i64`：分区表返回 `PartitionID`，否则返回 `TableID`。
  - `IsPartitionTable(&self) -> bool`：仅比较 `PartitionID` 与哨兵。
  - `String(&self) -> String`：输出 `"<PartitionID> => <TableID>"`，保持 Go `fmt.Sprintf("%d => %v", ...)` 的字段顺序。
  - `Equals(left, right) -> bool`：参数使用 `Option<&AnalyzeTableID>` 模拟 Go 可空指针。相同地址（包括 `None`/`None`）立即相等；不同地址的两个 `Some` 再按两个 ID 值比较；仅一侧为空则不相等。
- `pub struct AnalyzeResult`：并行向量 `Hist`、`Cms`、`TopNs`、`Fms` 保存一组统计构件，`IsIndex: i32` 保留 Go 的 0/1 契约。类型本身没有验证这些向量长度一致。
- `AnalyzeResult::DestroyAndPutToPool(&mut self)`：用 `std::mem::take` 替换 `Fms`，确保容量也释放；随后逐个清理 `Hist`。它不会清空 `Hist` 容器，也不会显式清空 `Cms` 或 `TopNs`。
- `pub struct AnalyzeResults`：`Err: Option<astersql_errors::SharedError>` 和 `Job: Option<AnalyzeJob>` 携带失败或任务上下文；`Ars` 保存列组/索引组结果；`TableID`、`Count`、`StatsVer`、`Snapshot`、`BaseCount`、`BaseModifyCnt` 描述表级更新；`ForMVIndexOrGlobalIndex` 标记只应更新索引统计和版本的特殊任务。
- `AnalyzeResults::DestroyAndPutToPool(&mut self)`：遍历 `Ars`，把清理委托给每个 `AnalyzeResult`。

文件没有 trait、泛型、条件编译项或私有辅助函数；上述常量、结构和方法全部是公开 API。

## 执行流程

1. ANALYZE 上游应先构造 `AnalyzeTableID`。调用 `GetStatisticsID` 时，以 `PartitionID != -1` 为唯一分支条件选择分区 ID 或逻辑表 ID；`IsPartitionTable` 使用同一不变量。
2. 单个列组或索引组的统计构件被装入一个 `AnalyzeResult`。Go 注释给出的 v2 语义是：列组结果与索引组结果分别成为 `Ars` 中的元素；Rust 容器不主动组装或校验这种布局。
3. 多个 `AnalyzeResult` 连同任务错误、计数、版本和快照基线被装入 `AnalyzeResults`。`ForMVIndexOrGlobalIndex` 只记录策略标志，本文件不执行“跳过表级 count/snapshot 更新”的持久化分支。
4. 消费完成后，调用最外层 `AnalyzeResults::DestroyAndPutToPool`，它依次调用内层清理；内层先释放全部 FM sketch 存储，再清空每个直方图的 `Bounds`、`Buckets`、`Scalars`（下游实现见 `pkg/statistics/histogram.rs`）。
5. 清理方法可再次调用：第一次后 `Fms` 为空且直方图内部向量已清空，后续遍历仍是安全的空操作；但本文件没有用类型系统强制消费者必须清理。

## 数据与状态

`AnalyzeTableID` 的核心不变量是：非分区表必须把 `PartitionID` 写成 `NonPartitionTableID`，而不是 0 或 `TableID`。一旦写入其他值，`GetStatisticsID` 和 `IsPartitionTable` 都会把对象解释为分区表。`String` 总是先显示 `PartitionID`，即使它是 `-1`。

`AnalyzeResult` 拥有四个 `Vec`，不像 Go 的 `[]*T` 那样允许元素为 `nil`。Rust 版本使用值元素，销毁时能够以 `&mut Histogram` 逐一清理。`std::mem::take(&mut Fms)` 把字段替换为新的零容量空向量；`pkg/statistics/analyze_test.rs` 明确断言清理后 `len == 0` 且 `capacity == 0`。

`AnalyzeResults` 的 `Snapshot` 是分析开始时的快照时间戳，`BaseCount` 和 `BaseModifyCnt` 是同一时点从 `mysql.stats_meta` 读取的基线。它们为并发 ANALYZE 的过期结果判定和快照增量合并提供输入，但本文件不读取数据库、不比较时间戳。`ForMVIndexOrGlobalIndex` 表示多值索引或全局索引不应改写表级行数和 snapshot；该字段的完整 Go 理由记录在 `pkg/statistics/analyze.go` 第 102–119 行。

## 依赖与调用关系

crate 内部依赖通过 `use crate::{AnalyzeJob, CMSketch, FMSketch, Histogram, TopN}` 引入：`AnalyzeJob` 来自 `analyze_jobs.rs`，四种统计结构分别由统计 crate 的其他生产模块定义。外部依赖只有 `astersql_errors::SharedError`；`pkg/statistics/Cargo.toml` 以本地路径 `../errors` 声明 `astersql-errors`。

已验证的下游调用边是 `AnalyzeResults::DestroyAndPutToPool -> AnalyzeResult::DestroyAndPutToPool -> Histogram::DestroyAndPutToPool`。后者当前清空直方图的边界、桶和标量数组。`AnalyzeTableID` 方法没有 I/O 或下游服务调用。

当前 Rust 仓库内的直接上游证据来自测试：`statistics_test.rs::analyze_table_id_selects_partition_and_compares_values` 调用 ID 选择、分区判定和相等比较；`analyze_test.rs::destroy_analyze_result_releases_fm_sketch_storage` 调用清理；`integration_test.rs::analyze_snapshot_metadata_is_retained` 构造聚合结果。RustCodeGraph 将本文件列为被 `analyze_test.rs` 和 `statistics_test.rs` 使用，仓库级 `rg` 另确认 integration test 的构造。没有找到生产 Rust 模块构造本文件这些类型的证据；storage 中的同名类型是不同定义。

## 错误处理与边界

本文件的方法均不返回 `Result`，不会自行生成错误。`AnalyzeResults::Err` 只是可选错误载体；是否短路、记录或释放结果由消费者决定。`SharedError` 的具体错误分类不在此处解释。

边界行为包括：`Equals(None, None)` 为真，`Equals(None, Some(_))` 为假；两个不同地址但字段相同的对象为真；分区 ID 为任何非 `-1` 值时均被当作分区；空 `Ars`、空 `Hist` 和空 `Fms` 的清理都是空操作。`DestroyAndPutToPool` 不检查四组统计向量是否对齐，也不重置 `Count`、`Snapshot`、`Err`、`Job`、`Cms`、`TopNs` 或 `IsIndex`，调用方不能把它理解为把整个对象恢复默认值。

值得注意的移植边界是：Go 的 `AnalyzeResult`/`AnalyzeResults` 使用指针切片，Rust 使用值向量；Go receiver 可理论上遇到 nil 指针，而 Rust 的 `&mut self` 不可为空。Rust `IsIndex` 为 `i32`、`StatsVer` 为 `i32`，对应 Go 的 `int`，跨接口时应确认宽度转换。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、channel 或事务。结构也没有在本文件显式实现 `Send`/`Sync`；能否跨线程取决于全部字段的自动 trait，不能仅凭本文件断言并发安全。

生命周期完全由所有权管理：`AnalyzeResults` 拥有 `Ars`，每个 `AnalyzeResult` 拥有四组统计对象。正常 Rust drop 最终也会释放内存，但显式清理用于对齐 Go 的对象池/GC 生命周期，并允许在外层对象仍存活时提前释放 FM sketch 容量和直方图内部缓冲区。直方图清理当前只是清空内部 `Vec`，源码注释称其为“归还池（占位）”，所以文档不声称存在真实的全局对象池。

快照字段体现的是并发任务协议而非本地同步：上游在分析开始时捕获 `Snapshot` 和基线计数，下游持久化层据此避免旧任务覆盖新统计。当前本文件仅承载这些值，未接线的 Rust 主链不能由这里单独证明。

## 与 Go 版本的对应关系

Rust 用 `Option<&AnalyzeTableID>` 表达 Go 的 nil receiver/参数组合，并先比较引用地址，复现 `h == t` 的快速路径。

资源释放有一项有意实现细节：Go 通过 `a.Fms = nil` 允许 backing array 被 GC；Rust 不能用 `Vec::clear`，因为它会保留容量，因此使用 `std::mem::take`。

语义尚未完整接线：Go 的 executor 创建这些结果并通过 channel 交给保存 worker，异常路径调用 `DestroyAndPutToPool`；Go storage 用 `TableID.GetStatisticsID()` 和 `ForMVIndexOrGlobalIndex` 决定持久化行为。

测试对应也不完全等价：Rust `integration_test.rs::analyze_snapshot_metadata_is_retained` 只验证字段保留；Go `integration_test.go::TestAnalyzeSnapshot` 会执行 SQL、读取 `mysql.stats_meta`，验证 count、snapshot 与 histogram version。后者的端到端行为不能由当前 Rust 测试替代。

## 扩展指南

- 修改表/分区识别时，应同时修改 `GetStatisticsID`、`IsPartitionTable`、`Equals` 和 `String`，并扩充独立文件 `pkg/statistics/statistics_test.rs`；不要把测试内嵌回生产源文件。
- 新增统计构件字段时，应决定它属于单组 `AnalyzeResult` 还是任务级 `AnalyzeResults`，同步 Go 对照结构，并明确 `DestroyAndPutToPool` 是否需要提前释放其资源。涉及并行向量时还应定义长度/索引对应不变量。
- 修改 FM sketch 或直方图清理策略时，应同步 `pkg/statistics/analyze_test.rs` 与 `pkg/statistics/histogram.rs` 的资源语义；若引入真实对象池，要说明重复清理、池容量和跨线程访问规则。
- 修改 `Snapshot`、`BaseCount`、`BaseModifyCnt` 或 `ForMVIndexOrGlobalIndex` 时，必须同时审查 Go storage 的过期判断/表级更新分支，以及 Rust 当前独立的 `pkg/statistics/handle/storage/stats_read_writer.rs::AnalyzeResults`，防止两套契约继续漂移。
- 若要完成 Rust 主链接线，优先复用并统一本文件类型，而不是再增加同名结构；随后增加独立 executor/storage 测试，覆盖正常表、分区表、过期 snapshot、MV 索引、全局索引、错误结果和销毁路径。该工作超出本说明任务范围。
- 性能风险主要来自大向量的额外复制、清理时的线性遍历和过早/过晚释放；兼容风险主要来自 `-1` 哨兵、Go `int` 到 Rust `i32` 的宽度、以及字段/序列化边界变化。
