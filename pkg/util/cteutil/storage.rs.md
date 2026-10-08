# `pkg/util/cteutil/storage.rs`

## 文件定位

本文件属于 `astersql-util-cteutil` crate，是 CTE（公共表表达式）中间结果的临时存储层实现。crate 入口 [`lib.rs`](./lib.rs) 将本模块声明为私有 `mod storage`，再公开导出全部符号；[`Cargo.toml`](./Cargo.toml) 通过 `package.metadata.porting.go-package = "pkg/util/cteutil"` 标明其 Go 对照包。根 crate 还在 `pkg/lib.rs` 的 `util::cteutil` facade 中再次导出该 crate。

在 Rust 执行器侧，`pkg/executor/cte_table_reader.rs` 直接依赖这里的 `Storage` trait：`CTETableReaderExec::Next` 读取 `GetIter`、`NumChunks` 和 `GetChunk`，在递归轮次变化时从第 0 个 chunk 重新扫描，并复制列数据以免上层修改共享结果。`pkg/executor/cte.rs` 的主 CTE 流程则通过通用的 `CTEBackend::Storage` 及 `storage_*` 方法抽象存储操作；当前代码搜索没有找到该生产抽象直接调用 `NewStorageRowContainer` 的 Rust 接线，因此不能据此断言 `StorageRC` 已经是 `CTEBackend` 的生产实现。构造函数的明确 Rust 调用者目前位于本 crate 的独立测试。

## 核心职责

- `Storage` 定义一次填充、多读者消费的 CTE 临时表契约，包括引用计数生命周期、chunk/row 访问、完成与错误状态、迭代轮次，以及内存/磁盘统计和 spill 动作。
- `StorageRC` 用 `chunk::RowContainer` 保存真实数据；未打开时不分配容器，第一次 `OpenAndRef` 才创建容器。
- `StorageMutex` 用布尔状态和条件变量模拟 Go `sync.Mutex` 的分离式 `Lock`/`Unlock` API，使锁的占用可以跨越两个方法调用。
- `SwapData` 只交换 schema、chunk 大小和底层容器，刻意保留两侧各自的引用计数、完成标志、迭代号和错误状态。
- `Reopen` 关闭旧容器并创建新容器，而不是复用/重置旧容器，从而替换旧容器关联的 tracker 与 spill 元数据。

## 主要符号

- `pub trait Storage: Any`：公开对象安全接口。需要 `Any` 是因为 `SwapData` 经 `as_any_mut` 向下转型，确保只在相同的 `StorageRC` 实现之间交换数据。
- `struct StorageMutex`：内部互斥器，字段为 `syncutil::Mutex<bool>` 和 `parking_lot::Condvar`；`lock` 在已占用时循环等待，`unlock` 校验状态后唤醒一个等待者。
- `pub struct StorageRC`：具体实现。`err` 保存生产错误，`rc` 是可选 `RowContainer`，`tp`/`chkSize` 是重建容器所需的 schema 与容量，`refCnt` 管理打开引用，`iter` 表示消费者轮次，`mu` 提供显式互斥，`done` 表示填充结束。
- `pub fn NewStorageRowContainer(Vec<FieldType>, usize) -> StorageRC`：只保存 schema 和 chunk 大小，初始 `rc = None`、`refCnt = 0`、`iter = 0`、`done = false`，不会立即分配数据容器。
- `OpenAndRef` / `DerefAndClose`：管理底层容器与引用计数。首次打开创建容器并设为 1；重复打开递增；最后一次解引用关闭容器并清理可变元数据。
- `Add` / `GetChunk` / `GetRow` / `NumChunks` / `NumRows`：数据写入、定位读取和计数接口。`Add` 会忽略空 chunk；写入时克隆传入 chunk 后交给 `RowContainer::Add`。
- `GetMemTracker` / `GetDiskTracker` / `ActionSpill` / `GetMemBytes` / `GetDiskBytes`：透传 `RowContainer` 的资源追踪及落盘能力。`ActionSpillForTest` 额外暴露可等待的测试动作。
- `valid`：以 `refCnt > 0 && rc.is_some()` 定义有效存储；`row_container` 是内部快捷访问器，无容器时会 panic。

## 执行流程

1. 调用方使用 `NewStorageRowContainer` 创建未打开对象，仅保留字段类型和期望 chunk 大小。
2. 第一次 `OpenAndRef` 创建 `RowContainer::New(tp, chkSize)`，将引用计数设为 1 并把迭代号复位为 0；后续调用只增加引用计数。
3. 生产者按 Go 约定可在 `Lock`/`Unlock` 之间检查 `Done`，通过 `Add` 写入非空 chunk，并用 `SetError` 或 `SetDone` 发布结果状态。这里的实现不自动加锁，也不强制调用顺序，协议由调用者维护。
4. 消费者通过 `GetChunk` 或 `GetRow(RowPtr)` 读取数据，通过 `GetIter` 判断是否进入新的递归轮次。直接消费者 `CTETableReaderExec::Next` 在轮次增加时重置本地 chunk 游标，并复制取出的 chunk 后交给上层。
5. 内存超过 tracker 限额时，调用方安装/触发 `ActionSpill`，具体异步落盘由 `RowContainer` 和 `SpillDiskAction` 完成；存储层只提供动作和 tracker 访问入口。
6. 新一轮填充可调用 `Reopen`：先关闭旧容器，清空 `iter`、`done`、`err`，再按原 schema/容量创建全新的容器；`refCnt` 保持不变，因此对象仍处于已打开状态。
7. 每个使用者结束时调用 `DerefAndClose`。引用数仍大于 0 时数据保留；降到 0 时先置为 -1，复位 `done`/`err`/`iter`，关闭并移除容器。之后若再使用，需要重新 `OpenAndRef`。

## 数据与状态

生命周期可概括为“未打开（`refCnt = 0` 或 -1，`rc = None`）→ 已打开（`refCnt > 0`，`rc = Some`）→ 最后一次解引用后关闭”。`valid` 同时检查计数和容器，避免仅凭其中一个状态工作。`DerefAndClose` 在非最后一个引用上不会清理数据或元数据；独立测试 `storage_reference_lifecycle_matches_go` 验证第一次解引用后 `done` 仍为真，而最后一次才清除 `done`、`iter` 和 `err`。

`tp`、`chkSize` 和 `rc` 属于数据/schema 一侧，参与 `SwapData`；`refCnt`、`done`、`iter`、`err`、`mu` 属于存储实例身份，不参与交换。`SwapData` 测试验证整数与字符串数据/schema 互换后，各实例原有的 `done` 和 `iter` 保持不变。

`GetMemBytes` 与 `GetDiskBytes` 不维护独立计数，而是即时读取相应 tracker 的 `BytesConsumed`。spill 测试表明落盘前数据计入内存 tracker，动作完成后当前内存消耗归零、磁盘消耗增加，chunk 内容仍可读取。

## 依赖与调用关系

下游依赖由 [`Cargo.toml`](./Cargo.toml) 明确声明：

- `astersql-util-chunk`：`Chunk`、`Row`、`RowPtr`、`RowContainer` 与 `SpillDiskAction`，承担实际数据存储和 spill。
- `astersql-util-memory`、`astersql-util-disk`：内存/磁盘 `Tracker`。
- `astersql-util-syncutil` 与 `parking_lot`：显式互斥状态和条件变量。
- `astersql-types`：字段类型 `FieldType`。
- crate 本地 `errors::Error`：把 `RowContainer` 的 `ChunkError` 转成统一字符串错误。

已核实的上游关系包括：

- `pkg/executor/cte_table_reader.rs::CTETableReaderExec::Next` → `Storage::{GetIter, NumChunks, GetChunk}`。
- `pkg/util/cteutil/storage_test.rs` 与 `migration_aster_unit_test.rs` → `NewStorageRowContainer` 及全部核心生命周期/读写/spill API。
- `pkg/executor/Cargo.toml` 声明 `astersql-util-cteutil` 依赖，`pkg/executor/lib.rs` 导出 `cte_table_reader` 模块。
- 根 `Cargo.toml` 以 `facade_util_cteutil` 引入本 crate，`pkg/lib.rs::util::cteutil` 再导出其 API。

RustCodeGraph 对 `NewStorageRowContainer` 的精确查询识别出 Go 与 Rust 两个定义；对目标文件的索引列出 57 个符号。但同名方法较多，精确 qualified caller/callee 查询未产生可用调用边，因此上述上游边由精确源码引用搜索和相邻模块读取补齐。

## 错误处理与边界

- 未打开或已完全关闭时调用 `DerefAndClose` 返回原样兼容的字符串错误 `"Storage not opend yet"`（保留了 Go 源码中的拼写）。`Add`、`GetChunk`、`GetRow` 在无效状态返回 `"Storage is not valid"`。
- `Reopen` 在 Rust 中额外显式检查 `rc` 是否存在，无效时返回 `"Storage is not valid"`；Go 版本会直接对空 `s.rc` 调用方法并因此 panic。这是 Rust 的安全边界增强。
- `SwapData` 通过 `Any` 下转型；若另一端不是 `StorageRC`，返回 `"cannot swap if underlying storages are different"`。
- `NumChunks`、`NumRows`、tracker、spill 和字节统计方法直接调用 `row_container()`，无效状态会 panic，而不是返回 `Result`。扩展调用方必须先完成 `OpenAndRef` 并保证最后一个 `DerefAndClose` 尚未发生。
- `GetChunk` 的越界下标和 `GetRow` 的非法 `RowPtr` 由下层 `RowContainer` 决定并通过 `?` 转成 `errors::Error`；本层不预先校验索引。
- `DerefAndClose` 在引用数归零时先把计数置为 -1并清状态，再调用 `RowContainer::Close`。若下层关闭失败，函数返回错误且对象已被 `valid` 判定为无效；这是需要调用方保留和上报的部分清理状态。
- 空 chunk 是成功的 no-op，既不创建额外数据块，也不增加行数；`migration_aster_unit_test.rs::empty_chunk_is_ignored` 覆盖这一不变量。

## 并发与资源生命周期

`StorageRC` 的数据与元数据变更本身不提供自动同步。`Storage::Lock`/`Unlock` 是调用协议的一部分：`StorageMutex::lock` 用 `while` 循环抵抗条件变量的虚假唤醒，成功后设置占用标志；`unlock` 在未持锁时会触发断言 panic，并只唤醒一个等待者。该锁不可重入，同一线程重复 `Lock` 而不 `Unlock` 会等待自己。

Rust 方法签名仍要求 `&mut self` 才能执行 `Add`、状态设置、打开/关闭和交换，因此跨线程共享可变存储还需要上层提供可变访问协调；仅调用 `Lock` 不会自动把后续方法绑定到锁守卫。测试 `lock_blocks_other_callers_until_unlock` 只证明两个 `&self` 锁调用之间的互斥和唤醒语义。

`RowContainer` 负责数据、tracker 和异步 spill 资源。`Reopen` 和最后一次 `DerefAndClose` 都先调用其 `Close`；前者随后创建新容器，后者移除容器。spill 是异步动作，测试通过 `WaitForTest` 等待完成；生产调用者不能仅凭触发动作就假设 tracker 与落盘数据已经稳定。

## 与 Go 版本的对应关系

Rust 文件逐项复刻 [`storage.go`](./storage.go) 的 `Storage`、`StorageRC` 和构造/生命周期/读写/tracker API。字段含义、首次打开、引用计数递增与归零清理、空 chunk no-op、只交换数据与 schema、`Reopen` 新建容器以及错误文本均保持一致。[`storage_test.rs`](./storage_test.rs) 对应 [`storage_test.go`](./storage_test.go) 的六组场景：基础生命周期、多次打开关闭、Add/GetChunk、spill、重复 Reopen 和 SwapData。

主要语言适配差异如下：

- Go 构造器返回 `*StorageRC`，Rust 返回拥有所有权的 `StorageRC`；Go 字段类型是指针切片，Rust 使用值类型 `Vec<FieldType>`。
- Go 依赖接口动态分派和类型断言；Rust 使用 `dyn Storage`、`Any` 与 `downcast_mut`。
- Go `syncutil.Mutex` 天然允许 `Lock`/`Unlock` 分离；Rust 不能让普通 guard 跨方法存活，因而引入 `StorageMutex { Mutex<bool>, Condvar }` 模拟该契约。
- Go 的 `GetDiskTracker` 源码签名写作 `*memory.Tracker`，Rust 明确返回 `Arc<disk::Tracker>`，与 Rust `RowContainer` 的类型边界相符。
- Rust `Add` 克隆 chunk 再传给拥有型下层 API；Go 直接传递 `*chunk.Chunk`。
- Rust 的 `Reopen` 对未打开状态返回错误，避免 Go 空指针式失败；其余关键行为由 Rust/Go 对照测试共同验证。

## 扩展指南

- 新增存储元数据时，先决定它属于“随数据交换”还是“存储实例身份”。若属于前者，同步修改 `SwapData`；若属于可重开状态，同步修改 `Reopen` 和最后一次 `DerefAndClose` 的复位逻辑。
- 新增 trait 方法必须同步 `StorageRC` 实现、`pkg/executor/cte_table_reader.rs` 等 trait 对象消费者、根 facade 可见性，以及独立测试；不要把 Rust 单元测试内嵌到 `storage.rs`。
- 改动生命周期时重点保持 `valid` 不变量和多引用语义：非最后一次解引用不得清数据，最后一次必须关闭资源并清可变元数据，重新打开应获得新容器。
- 改动 spill/tracker 时同步检查 `RowContainer` 的动作所有权和异步完成契约，并扩展 `storage_test.rs::TestSpillToDisk` 或 `migration_aster_unit_test.rs::spill_action_moves_rows_to_disk_without_changing_results`；性能风险主要是 chunk 克隆、重建容器和磁盘 I/O。
- 引入新的 `Storage` 实现时，当前 `SwapData` 会拒绝与 `StorageRC` 互换。若需要异构交换，应显式设计兼容协议，不能绕过类型检查；同时评估 `as_any_mut` 暴露的对象安全边界。
- 若要把 `StorageRC` 接入 `pkg/executor/cte.rs::CTEBackend`，应在 executor 的具体 backend 中逐项实现 `storage_*` 适配，并验证线程共享方式；当前仅存在抽象边界，不能用测试构造调用代替生产接线证据。
- 兼容性风险集中在现有错误文本、`SwapData` 不交换元数据、`Reopen` 保持引用计数、无效状态下部分 API panic 等既有约定；修改时应与 Go 文件和两组独立 Rust 测试一起审查。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点，目标目录的 `storage.rs` 含 57 个符号；`node --file pkg/util/cteutil/storage.rs` 读取了完整 346 行源码；精确 `query NewStorageRowContainer --json` 和 `query StorageRC --json` 同时定位 Go/Rust 定义；qualified callers/callees 未返回边，故没有据此虚构调用关系。
- 生产源码：[`storage.rs`](./storage.rs)（`Storage`、`StorageMutex`、`StorageRC` 及全部实现），`pkg/executor/cte_table_reader.rs`（直接 trait 消费者），`pkg/executor/cte.rs`（`CTEBackend` 抽象与 CTE 主流程），`pkg/executor/builder.rs`（CTE 存储/读取器构建边界）。
- crate/装配：[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、`pkg/executor/Cargo.toml`、根 `Cargo.toml` 和 `pkg/lib.rs`。
- Go 对照：[`storage.go`](./storage.go) 与 [`storage_test.go`](./storage_test.go)。
- Rust 独立测试：[`storage_test.rs`](./storage_test.rs) 和 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)，覆盖引用计数、chunk/row 读写、空块、状态复位、数据交换、spill 与显式互斥。
- 精确文本搜索确认：Rust 生产代码中 `Storage` 的直接使用位于 `pkg/executor/cte_table_reader.rs`；`NewStorageRowContainer` 的明确 Rust 调用目前仅见于上述独立测试。任务为纯文档分析，按计划未运行 Cargo。
