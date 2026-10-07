# `pkg/executor/admin.rs`

## 文件定位

`admin.rs` 属于 `astersql-executor` crate，并由 [`pkg/executor/lib.rs`](lib.rs) 的 `pub mod admin` 公开。它承载三类索引维护算法：`ADMIN CHECK INDEX` 的范围扫描、`ADMIN RECOVER INDEX` 的缺失索引回填，以及 `ADMIN CLEANUP INDEX` 的悬空索引删除。crate 归属和 Go 包映射分别由 [`pkg/executor/Cargo.toml`](Cargo.toml) 的包名 `astersql-executor`、`[lib] path = "lib.rs"` 与 `package.metadata.porting.go-package = "pkg/executor"` 确认。

当前 Rust 文件不是完整会话/存储实现：真实 DAG 下发、事务、键编码、分区解析等能力均通过 `AdminRuntime`、`AdminTransaction`、`AdminSelectResult` 注入。仓库搜索未发现这三个 trait 的具体 `impl`，也未发现其他 Rust 文件直接构造 `CheckIndexRangeExec`、`RecoverIndexExec` 或 `CleanupIndexExec`。因此，本文件应理解为已经移植的执行算法与适配契约；不能仅凭其被 `lib.rs` 导出，就断言三个 Rust 执行器已经接入生产构建链。

## 核心职责

- `CheckIndexRangeExec` 扫描完整索引范围，只输出 handle 落入任一半开区间 `[Begin, End)` 的索引行；它不修复数据。
- `RecoverIndexExec` 按最多 2048 行一批顺序扫描记录，计算索引值，批量探测现有索引键，并在新事务中锁定记录键后创建缺失索引；输出 `(新增索引数, 扫描行数)`。
- `CleanupIndexExec` 顺序扫描索引，按 handle 批量读取记录键，对不存在对应记录的索引项执行删除；输出删除数。
- `AdminRuntime` 与 `AdminTransaction` 把算法和具体 TiKV/DAG/session 实现隔离；`SelectRequest`、`DAGRequest`、`ScanExecutor` 是该边界上的内部请求模型。
- `Datum`、`Row`、`AdminChunk`、表/索引元数据及 `Handle` 是本文件自包含的轻量模型，用于表达 Go 版本中 `types.Datum`、`chunk`、`model`、`kv.Handle` 等概念。

这些职责直接对应 [`pkg/executor/admin.go`](admin.go) 的三个同名执行器，但 Rust 版本通过 trait 抽象依赖，而不是直接引用 Go 版本所用的 distsql、kv、table、session context 类型。

## 主要符号

### 公共数据与边界

- `AdapterResult<T>`：统一为 `Result<T, errors::SharedError>`；本文件所有可失败的运行时操作沿此类型传播。
- `Datum`、`Row::datum`、`AdminChunk`：表达扫描行和批次。`Row::datum` 显式检查列越界；`AdminChunk::Reset/AppendRows/AppendInt64/AppendUint64` 维护输出批。
- `TableInfo`、`IndexInfo`、`ColumnInfo`、`FieldType`：算法所需的最小元数据。`IndexInfo::{Global, Primary, HasCondition}` 控制全局索引、聚簇主键和部分索引分支。
- `Handle`：保存编码值、可选整数值和可选分区 ID。`Handle::partitioned` 为全局索引保留分区身份；私有 `successor` 为续扫提供严格后继。
- `DAGRequest`、`ScanExecutor`、`SelectRequest`：描述 IndexScan/TableScan/Limit 下推、时间区与 flags、快照时间戳、键范围、保序和并发度。
- `AdminSelectResult`：流式结果只有 `Next` 与 `Close`；`finish_select_result` 保证关闭，并优先保留扫描业务错误。
- `AdminTransaction`：暴露 `StartTS`、TopSQL/磁盘满选项、批量读、锁记录键、创建和删除索引。
- `AdminRuntime`：负责激活事务、运行新事务、发起 Select、编码/解码键、生成索引条目、计算生成列/部分索引、解析分区与日志。

### 三个执行器

- `CheckIndexRangeExec::{Open, Next, Close, buildDAGPB, constructIndexScanPB}`：建立全索引扫描并过滤 handle。
- `RecoverIndexExec::{Open, Next, backfillIndex, backfillIndexInTxn, fetchRecoverRows, batchMarkDup, buildIndexedValues}`：组织恢复索引的跨事务批循环与单事务写入。
- `buildRecoverIndexKeyRanges`：生成记录前缀内从首条记录或指定 handle 严格后继开始的单个半开键范围。
- `CleanupIndexExec::{Open, init, Next, cleanTableIndex, fetchIndex, batchGetRecord, deleteDanglingIdx}`：组织悬空索引清理循环。
- `extractIdxVals`：从扫描行前 `index_value_len` 列复制索引值，同时检查字段类型和行宽是否足够。

`backfillResult` 和 `recoverRows` 虽声明为 `pub`，但其命名与用途表明它们只是恢复流程的批次状态；扩展时不应把这种可见性误当成稳定的跨 crate API 承诺。

## 执行流程

### 检查指定 handle 范围

1. `CheckIndexRangeExec::Open` 按 `IndexInfo.Columns[*].Offset` 解析索引列，追加 `_tidb_rowid` handle 列，调用 `buildDAGPB` 构造单个 `IndexScan`。
2. 它通过 `ActivateTransaction` 获取快照时间戳，用 `FullIndexRange`、`keep_order = true` 创建 `ScanKind::CheckIndex` 请求，并保存 `Select` 结果。
3. `Next` 反复拉取源 chunk，从输出 schema 的最后一列读整数 handle，仅保留满足任一 `Begin <= handle < End` 的行。某批过滤为空时继续拉取，而不是错误地向上游报告扫描结束。
4. `Close` 关闭并清空保存的结果。注意该 Rust 实现假定最后一列 handle 可转成 `i64`，与其人工追加 `_tidb_rowid` 的设计一致。

### 恢复缺失索引

1. `RecoverIndexExec::Open` 打开基础执行器、缓存列类型，把批大小固定为 2048，并预分配行与索引值缓冲。
2. `Next` 对 common-handle 聚簇主键直接返回 `0 0`；普通表调用一次 `backfillIndex`，分区表则逐分区 `ResolvePartition`、`ResolveWritableIndex` 后累计新增数和扫描数。
3. `backfillIndex` 每轮通过 `RunInNewTxn` 创建事务，设置 TopSQL 选项，调用 `backfillIndexInTxn`。每累计扫描 50,000 行记录一次进度；本批为零行或当前 handle 没有后继时结束，否则以下一轮的起点续扫。
4. `backfillIndexInTxn` 以事务 `StartTS` 构造 `TableScan + Limit(batchSize)`，`keep_order = true`、`concurrency = 1`。`buildRecoverIndexKeyRanges` 使用记录前缀和严格后继，避免重复扫描上一批最后一行。
5. `fetchRecoverRows` 构造 handle；全局索引用 `physicalID` 包装分区 handle。部分索引条件不满足的行仍增加扫描计数和推进当前 handle，防止全不匹配批次被误判为结束。生成列通过 `EvalGeneratedColumn` 求值，普通列直接从行取值，并收集 restored data。
6. `batchMarkDup` 对每行可能产生的多个索引条目执行一次批量读。任何已存在条目都会保持该行 `skip = true`；唯一索引指向不同 handle 时只记录不一致日志而不覆盖。
7. 对确实缺失的行先 `LockKey(record_key)`，再以 `ignore_assertion = true`、`duplicate_check_skip = true` 调用 `CreateIndex`，最后返回新增数和扫描数。

### 清理悬空索引

1. `CleanupIndexExec::Open` 打开基础执行器并调用 `init`。`init` 要求 `batchSize > 0`，缓存数组元素类型，清空 handle 映射，从 `MinimumIndexKey` 初始化续扫键。
2. `Next` 对 common-handle 聚簇主键直接返回 `0`。分区表的本地索引逐分区初始化、清理；全局索引和非分区表走单次 `cleanTableIndex`。
3. `cleanTableIndex` 每轮开启新事务，允许磁盘处于 almost-full 状态并设置 TopSQL；随后 `fetchIndex`、`batchGetRecord`、`deleteDanglingIdx` 在同一事务内完成。
4. `fetchIndex` 从 `IndexRangeAfter(lastIdxKey)` 开始，以 `IndexScan + Limit(batchSize)`、保序且单并发读取。它按 handle 聚合一个或多个索引值组，并用 `GenIndexEntries` 得到真实索引键作为下一批游标。
5. `batchGetRecord` 把每个 handle 编码成记录键并批量读取。`deleteDanglingIdx` 只处理记录不存在的键，重新解码分区与 handle 后删除该 handle 下的全部索引值组；删除数每达到一个批大小记录进度。
6. 本轮读不到索引行时结束；否则清空批状态但保留 `lastIdxKey` 继续下一轮，最终向调用方输出累计删除数。

## 数据与状态

- 三个执行器都是有状态、可变的拉取算子。`RecoverIndexExec.done` 与 `CleanupIndexExec.done` 保证结果只生成一次；后续 `Next` 返回空批。
- 恢复流程的进度由 `current_handle` 驱动，清理流程由 `lastIdxKey` 驱动；两者都依赖有序扫描与“严格后继”才能既不重复也不遗漏。
- `recoverRows`、`idxValsBufs`、`idxKeyBufs`、`batchKeys` 和清理侧 `idxValues` 是跨方法复用的批缓冲。Rust 实现会克隆部分 `Vec<Datum>`，语义正确但不完全复现 Go slice 的容量复用效率。
- 全局索引的 handle 必须含分区 ID，否则不同分区的相同基础 handle 会碰撞。恢复侧在 `fetchRecoverRows` 包装，清理侧从全局索引扫描行末列读取分区 ID 后包装。
- `idxValues: HashMap<Handle, Vec<Vec<Datum>>>` 允许同一 handle 对应多个索引值组，这是多值索引清理所需的数据形状。
- `FieldType::ArrayType` 用于清理扫描列；恢复侧 `columnsTypes` 保留原列类型。`extractIdxVals` 当前实际按位置复制 Datum，`field_types` 只参与长度防御。
- `CheckIndexRangeExec.startKey`、`RecoverIndexExec.idxKeyBufs` 等字段在当前 Rust 文件中未被读到；它们反映 Go 结构或后续适配预留，不能据此推导额外行为。

## 依赖与调用关系

上游静态关系是 `pkg/executor/lib.rs -> pub mod admin`。RustCodeGraph 将目标文件识别为 112 个符号，并能定位三种同名 Rust/Go struct；但仓库级 `rg` 未找到三个 runtime trait 的实现或执行器的外部 Rust 构造点，所以目前没有可确认的生产 Rust 调用边。`pkg/executor/test/admintest/admin_test.rs` 通过 `TestKit` 执行 SQL，另有内存 `AdminTable` 模型测试；它验证用户可见语义，但源码中没有直接引用本文件的 trait/struct。

文件内部主要调用链为：

- `CheckIndexRangeExec::Open -> buildDAGPB -> constructIndexScanPB -> AdminRuntime::{ActivateTransaction, FullIndexRange, Select}`，之后 `Next -> AdminSelectResult::Next`。
- `RecoverIndexExec::Next -> backfillIndex -> AdminRuntime::RunInNewTxn -> backfillIndexInTxn -> buildTableScan/fetchRecoverRows/batchMarkDup -> AdminTransaction::{BatchGetValue, LockKey, CreateIndex}`。
- `CleanupIndexExec::Next -> cleanTableIndex -> AdminRuntime::RunInNewTxn -> fetchIndex/batchGetRecord/deleteDanglingIdx -> AdminTransaction::{BatchGetValue, DeleteIndex}`。

直接 Rust crate 依赖只有源码显式使用的 `astersql-errors`（`Cargo.toml` 路径依赖 `../errors`）与标准库 `HashMap`、`Arc`；distsql、kv、table 等具体系统依赖被 trait 方法签名抽象为本地数据结构。Go 对照实现则直接依赖 `distsql`、`kv`、`tablecodec`、`tables`、`expression`、`chunk` 和 session context。

## 错误处理与边界

- `Datum::as_i64` 拒绝非整数 handle，并拒绝超过 `i64` 的无符号值；`Row::datum`、索引列 offset 和 schema 最后一列都有显式边界检查。
- `finish_select_result` 总会调用 `Close`。扫描失败且关闭也失败时保留扫描错误；扫描成功但关闭失败时返回关闭错误。
- `CheckIndexRangeExec::Next` 在未 `Open` 时返回明确错误；`Close` 可在无结果时安全调用。
- 恢复循环拒绝“事务成功但未写回 `backfillResult`”以及“扫描了行却没有当前 handle”的不一致状态。handle 到达表示空间末端时安全停止。
- 部分索引条件为假不是错误，但必须计入扫描进度。唯一索引键已被不同 handle 占用时记录告警并跳过，不覆盖冲突数据。
- 恢复写索引前锁记录键，以缩小并发删除/更新导致的竞态；创建选项跳过重复检查的前提，是同一事务前已完成批量存在性探测。
- 清理拒绝零批大小、缺少全局索引分区 ID、无法生成索引键、记录键与 `idxValues` 不一致等状态。只在批量读确认记录键不存在时删除索引。
- 聚簇主键索引不是独立可修复/可清理的二级结构：Rust `Next` 返回零计数。Go 测试中某些构建阶段会更早拒绝相应 SQL；调用层需保持这一兼容边界。
- 所有 runtime/transaction/select 错误均用 `?` 原样向上传播；本文件没有重试策略，事务重试语义属于 `RunInNewTxn` 的具体实现契约。

## 并发与资源生命周期

- `AdminRuntime: Send + Sync` 且以 `Arc<dyn AdminRuntime>` 共享；`AdminSelectResult: Send` 可跨线程所有权边界，但执行器自身依赖 `&mut self` 串行推进，文件内没有启动线程或异步任务。
- 恢复和清理的维护扫描明确设置 `concurrency = 1`，并要求 `keep_order = true`。这是基于游标续扫的正确性条件，也对应 Go 版本为避免带 Limit 的无谓多 Region 扫描而设单并发。
- 每个恢复/清理批次运行在一个新事务中。批次提交后才推进 handle/index-key 游标；错误直接终止，不在本文件内吞掉或继续。
- `CheckIndexRangeExec` 持有跨多次 `Next` 的 select 结果，必须调用 `Close`。恢复/清理的临时 select 由 `finish_select_result` 在每批结束时关闭。
- `RecoverIndexExec::Open/Close` 与 `CleanupIndexExec::Open/Close` 委托基础执行器生命周期。`CleanupIndexExec::Open` 在 `init` 失败时主动尝试关闭基础执行器；该关闭错误被忽略以保留初始化错误。
- 缓冲区容量与批大小绑定；默认恢复批大小为 2048，清理批大小由构造方注入。增大批次会同时增大事务写集合、批量读键集合和内存占用。

## 与 Go 版本的对应关系

[`pkg/executor/admin.go`](admin.go) 是逐函数对照来源：三个执行器、恢复/清理主循环、DAG 的 Scan+Limit 形状、单并发保序扫描、50,000 行恢复日志阈值、TopSQL/磁盘满选项、分区与全局索引处理均在 Rust 中保留。

主要适配差异如下：

- Go 类型来自真实执行栈（`exec.BaseExecutor`、`distsql.SelectResult`、`kv.Transaction`、`table.Index`）；Rust 用本地模型和三个 trait 表达同一能力。因此 Rust 算法可被隔离验证，但具体请求编码、事务重试与表实现必须由尚未发现的适配层提供。
- Go 的 `defer terror.Call(result.Close)` 对应 Rust `finish_select_result`；Rust 额外定义了“业务错误优先于关闭错误”的确定规则。
- Go 使用 `kv.Handle.Next`、`PrefixNext` 和 `HandleMap`；Rust 分别使用 `Handle::successor`、`prefix_next` 与 `HashMap<Handle, ...>`。Rust 的非整数 handle 后继是字节字典序递增，适配方必须确保与实际 key codec 的排序语义一致。
- Go `buildIndexedValues` 懒建 expression columns 并直接求虚拟列；Rust 将求值委托给 `AdminRuntime::EvalGeneratedColumn`。Go 的 restored data、部分索引谓词、索引 KV 迭代同样分别映射为 runtime 方法。
- Go 复用 `idxKeyBufs` 生成索引键；Rust 字段仍存在，但 `GenIndexEntries` 返回自有 `IndexEntry`，当前没有使用该字段。
- Go `extractIdxVals` 借助字段类型从 chunk 解码；Rust 行已存成 `Datum`，所以只复制前 N 列并做长度检查。

测试对照：Rust [`pkg/executor/test/admintest/admin_test.rs`](test/admintest/admin_test.rs) 覆盖真实 SQL 无损维护结果、多值索引、缺失/悬空项、分区/全局索引身份、生成列与部分索引等用户语义；Go [`pkg/executor/test/admintest/admin_test.go`](test/admintest/admin_test.go) 更直接覆盖 `TestAdminRecoverIndex`、`TestAdminRecoverMVIndex`、`TestAdminCleanupMVIndex`、聚簇索引、分区表、全局索引和部分索引限制，并通过真实 `table.Index` 人工制造损坏。由于 Rust 测试未直接构造本文件执行器，trait 适配契约仍缺少同目录独立单元测试证据。

## 扩展指南

- 新增扫描字段或改变输出 schema 时，必须同步 `columns`/`cols` 的构造、`OutputOffsets`、handle 列位置、`BuildHandle` 与对应 `SelectResult` 解码；优先新增独立测试文件，不要把测试内嵌进 `admin.rs`。
- 扩展索引种类时，重点审查 `buildIndexedValues`、`GenIndexEntries`、restored data、全局索引的分区 handle、同一 handle 多值分组以及唯一索引冲突策略。多值索引必须允许一行产生多个 `IndexEntry`。
- 改动续扫逻辑时必须保持严格单调：恢复以记录键的严格后继开始，清理以最后索引键之后开始，并保持有序、单并发扫描。需要覆盖整数 handle 上界、common handle 字节进位和空批。
- 新增 runtime 实现时，应逐项映射 `AdminRuntime`/`AdminTransaction`，明确事务是否自动重试、`BatchGetValue` 是否读当前事务视图、锁键语义、Select 的 Close 保证和错误优先级；并在 builder 中添加可追踪的执行器构造边。
- 对性能的主要风险是批量键/Datum 克隆、`HashMap` 聚合和批大小引起的事务膨胀。优化缓冲复用时不能改变 `skip` 的“所有生成条目均缺失才回填”语义。
- 建议增加独立 Rust 单元测试（例如同目录 `admin_test.rs` 并由 `lib.rs` 的 `#[cfg(test)] mod admin_test;` 接入），使用假 `AdminRuntime`/事务/Select 覆盖：空批后继续、关闭错误优先级、部分索引全跳过仍推进、唯一索引错 handle、全局索引同 handle 不同分区、批边界续扫、零 batchSize 和事务错误传播。现有 `pkg/executor/test/admintest/admin_test.rs` 应继续承担 SQL 级兼容回归。
- 修改 Go/Rust 对齐行为时，应同步核对 `pkg/executor/admin.go` 与 `pkg/executor/test/admintest/admin_test.go`，但不要为了文档或局部算法对齐递归补建完整运行时子系统。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/admin.rs` 将目标识别为含 112 个符号的 Rust 文件。
- RustCodeGraph `node --file pkg/executor/admin.rs --offset ...`：完整读取 1–1266 行，核对所有数据类型、trait、三个执行器及辅助函数；`query` 分别定位 Rust/Go 的 `CheckIndexRangeExec`、`RecoverIndexExec`、`CleanupIndexExec` 与 `buildRecoverIndexKeyRanges`。
- RustCodeGraph `node --file pkg/executor/admin.go --offset ...`：完整读取 1–919 行，核对 Go 主流程、DAG、事务、分区/全局索引、错误与资源关闭语义。
- 配置与入口：读取 `pkg/executor/Cargo.toml` 和 `pkg/executor/lib.rs`；目标包没有 `pkg/executor/doc.go`。
- 测试：读取 `pkg/executor/test/admintest/admin_test.rs` 的 Rust SQL/内存语义用例，并检查 `pkg/executor/test/admintest/admin_test.go` 的 recover、cleanup、多值、聚簇、分区、全局及部分索引回归；另由搜索确认 `pkg/executor/test/executor/executor_test.rs` 与 `pkg/executor/test/writetest/write_test.rs` 存在 SQL 级引用。
- 接线核验：仓库搜索 `impl AdminRuntime/AdminTransaction/AdminSelectResult`、执行器外部构造和 `admin::...` 引用，无匹配；这支持“模块已公开、算法存在、具体 trait 适配未发现”的边界结论。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证目标存在且恰含 11 个固定二级标题，并人工复核没有把 Go 行为或测试覆盖误写为已确认的 Rust 生产接线。
