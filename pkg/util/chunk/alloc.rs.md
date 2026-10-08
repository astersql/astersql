# `pkg/util/chunk/alloc.rs`

## 文件定位

本文件实现 `astersql-util-chunk` crate 内的 `Chunk`/`Column` 对象池。它不是独立模块：`pkg/util/chunk/internal/group1/lib.rs` 的 `alloc_impl` 通过 `include!("../../alloc.rs")` 编译本文件，再由 `pub use alloc_impl::*` 和 crate 根 `pkg/util/chunk/lib.rs` 的 `pub use group_1::*` 对外导出。crate 的包名、入口和本地类型依赖分别由 `pkg/util/chunk/Cargo.toml` 中的 `name = "astersql-util-chunk"`、`[lib] path = "lib.rs"`、`types = .../types/internal/datum` 声明。

在完整程序中，已核实的生产接线是 `cmd/tidb-server/main.rs::setGlobalVars`（其所在初始化流程）调用 `chunk::InitChunkAllocSize`，把 `TiDBMaxReuseChunk` 和 `TiDBMaxReuseColumn` 写入本文件的全局默认上限。RustCodeGraph 将本文件识别为 57 个符号、被 31 个文件经模块/符号关系使用；但仓库内对 `chunk::NewAllocator()` 的精确 Rust 文本引用目前只出现在测试中。因此，本文件提供执行器热路径可用的池化基础设施，但不能仅凭 crate 被众多模块依赖，就断言 Rust SQL 主链已经广泛实例化该分配器。

## 核心职责

- `Allocator` 定义三阶段协议：`Alloc` 取得一个 `ChunkRef`，`CheckReuseAllocSize` 报告配置上是否允许缓存，`Reset` 结束一轮使用并回收本轮被追踪的对象。
- `allocator` 同时管理空闲 `Chunk` 容器和按列物理类型尺寸分桶的 `Column`；其目标是保留向量容量，减少重复堆分配，而不是限制调用者可分配的对象总数。
- `poolColumnAllocator` 负责列的创建、登记、按 `typeSize` 复用和缓存上限；`checkColumnType` 在回收时阻止类型已改变、显式禁止复用或占用过大的列进入错误缓存桶。
- `syncAllocator` 用互斥锁串行化同一个底层分配器的三个接口操作；`reuseHookAllocator` 在底层首次报告“具有复用配额”时执行一次 hook；`emptyAllocator` 提供完全不缓存的替代实现。
- `InitChunkAllocSize` 设置后续新建池的默认上限，`MaxCachedLen` 设置列缓存的数据容量门槛。它们影响资源复用策略，不改变 `Chunk` 的行语义。

## 主要符号

- `pub type ChunkRef = Arc<Mutex<Chunk>>`：用共享所有权和内部互斥表达 Go 的 `*Chunk`。调用者访问行列内容时必须成功取得锁。
- `pub trait Allocator`：公开分配器抽象。`Alloc(fields, capacity, maxChunkSize)` 把初始容量钳制为两者较小值，并把 `requiredRows` 设为 `maxChunkSize`。
- `maxFreeChunks`、`maxFreeColumnsPerType`：可由 `InitChunkAllocSize` 修改的进程级默认值，初始分别为 64 和 256；函数先把 `u32` 输入钳制到 `i32::MAX`。`NewAllocator` 在创建时快照这些值，既有实例不会随之后的配置调用改变。
- `MaxCachedLen`：初始为 16 KiB。`columnList::push` 只登记 `data.capacity() < MaxCachedLen` 的列；`checkColumnType` 还会拒绝容量大于该值的变长列。
- `allocator { allocated, free, columnAlloc, freeChunk }`：`allocated` 追踪本轮将在 `Reset` 中处理的共享 Chunk，`free` 保存可再次使用的空 Chunk，`columnAlloc` 保存按尺寸分桶的列，`freeChunk` 是实例化时的 Chunk 缓存上限。
- `columnList { freeColumns, allocColumns }`：前者是可立即弹出的列，后者是本轮已发出、等待 `Reset` 判定的列；`Len` 返回两者之和。
- `poolColumnAllocator::{NewColumn, NewSizeColumn, put}`：`NewColumn` 由 `getFixedLen` 求尺寸、取得列并登记同源快照；`NewSizeColumn` 优先从对应空闲桶弹出，若其 `data.capacity()` 小于请求计数则改为新建；`put` 忽略不可复用或无有效尺寸的列，并受每桶登记上限约束。
- `checkColumnType`：变长列要求桶 ID 为 `VarElemLen`、`elemBuf` 为空且数据容量不过大；定长列要求 `elemBuf` 非空且其容量等于原桶 ID。
- `NewSyncAllocator` / `syncAllocator`、`NewReuseHookAllocator` / `reuseHookAllocator`、`NewEmptyAllocator` / `emptyAllocator`：分别是串行化、一次性观测和禁用缓存三种包装/替代策略。
- `cached_*`、`all_column_counts`、`allocated_column_count`：暴露缓存计数、容量和地址等观测信息，现有调用主要用于独立测试验证。

## 执行流程

1. 服务启动阶段可先调用 `InitChunkAllocSize`。`NewAllocator` 随后创建空的 Chunk 列表，初始化 `poolColumnAllocator.pool`，并把当时的两个全局上限保存到实例字段。
2. `allocator::Alloc` 先从 `free` 尾部弹出 Chunk；没有命中时构造新的 `Arc<Mutex<Chunk>>`。持锁期间，它设置 `capacity = min(capacity, maxChunkSize)`、`requiredRows = maxChunkSize`，再为每个字段调用 `poolColumnAllocator::NewColumn`。
3. 列分配先按 `getFixedLen(field)` 定位桶。若有空闲列，则弹出；容量不足时丢弃该候选并调用 `newColumn` 重建。返回列之前，`reference_clone` 生成相同 `reference_id` 的登记快照，`put` 将其放进该桶的 `allocColumns`。
4. `Alloc` 仅在 `allocated.len() < freeChunk` 时克隆 `Arc` 到 `allocated`，从而把本轮可回收的 Chunk 数限制在缓存上限内；无论是否被追踪，Chunk 都会返回给调用者。
5. `allocator::Reset` 排空 `allocated`。对每个 Chunk 加锁，取走实际 `columns`，调用 `Chunk::resetForReuse` 清除 selection、行数、容量、`requiredRows` 和不完整标志，并在未达到上限时把空容器放进 `free`。取出的列以 `reference_root()` 去重后暂存于 `live_columns`。
6. `Reset` 再逐桶 `take` 掉 `allocColumns`。若能按 `reference_id` 找到仍在 Chunk 中的实际列，就用实际值替换登记快照；否则沿用快照。通过每桶上限和 `checkColumnType` 后调用 `Column::reset()` 清空逻辑内容并压入 `freeColumns`，不合格对象在循环末尾释放。
7. 下一轮 `Alloc` 可同时复用空 Chunk 容器和列的底层容量。`emptyAllocator::Alloc` 则直接按相同容量规则构造 Chunk，并对每个字段调用普通 `NewColumn`，不登记也不回收。

## 数据与状态

池的状态分成“当前空闲”和“本轮发出”两层：Chunk 使用 `free`/`allocated`，每个列尺寸桶使用 `freeColumns`/`allocColumns`。两个 `Vec` 都按尾部执行 LIFO 复用。`pool` 的键来自 `getFixedLen`：`VarElemLen` 标识变长列，其他正值对应固定元素缓冲尺寸。

Rust 的 `Chunk` 按值保存 `Column`，与 Go 的 `[]*Column` 不同。本文件为此依赖 `Column.reference_id`、`reference_clone` 和 `reference_root`：分配时登记同源快照，回收时再用 Chunk 内的实际列替换快照。`HashMap::entry(...).or_insert_with(...)` 对同一引用只保留一个实际列，避免 `Chunk::MakeRef` 后把同一列重复放入池。`pkg/util/chunk/alloc_test.rs::allocator_does_not_cache_duplicate_column_references` 验证了该不变量。

缓存上限是“保留多少对象”的上限，不是“允许分配多少对象”的限额。超过 `freeChunk` 的 Chunk 不进入 `allocated`；超过 `freeColumnsPerType`、`avoidReusing = true`、类型尺寸已改变或数据容量过大的列不会进入可复用队列。列的 `Vec` 容量被保留以换取后续吞吐，逻辑长度、null bitmap、offset 等内容由 `Column::reset` 清空。

## 依赖与调用关系

上游直接入口包括：

- `cmd/tidb-server/main.rs` 调用 `chunk::InitChunkAllocSize`，把服务配置灌入本文件的默认缓存上限。
- `pkg/util/chunk/alloc_test.rs` 调用 `NewAllocator` 并验证基础复用、引用去重和类型变化后的拒收。
- `pkg/util/chunk/alloc_1_aster_unit_test.rs` 调用 `InitChunkAllocSize`、`NewAllocator` 和 `NewReuseHookAllocator`，验证地址/容量复用、上限、超大变长列以及一次性 hook；该文件由 `internal/group1/lib.rs` 在 `cfg(test)` 下挂接。
- `pkg/util/sqlexec/migration_aster_unit_test.rs` 与 `pkg/sessionctx/variable/tests/session_test.rs` 也直接构造 `chunk::NewAllocator`，提供跨 crate 测试使用证据。

主要下游依赖是同 crate 的 `Chunk`、`Column`、`ColumnAllocator`、`getFixedLen`、`newColumn`、`NewColumn` 和 `VarElemLen`。其中 `Chunk::resetForReuse` 定义容器回收后的空状态；`Column::{reset,typeSize,reference_clone,reference_root}` 定义列清理、分桶和引用身份语义。标准库依赖为 `Arc`/`Mutex`（共享与互斥）、`Once`（hook 至多一次）和 `HashMap`（列分桶及 Reset 去重）。本文件自身不返回 crate 的 `Result`，也没有 I/O 依赖。

`pkg/util/chunk/Cargo.toml` 表明该 crate 被构造成独立库，且直接使用 `astersql-types-datum` 提供字段类型；仓库中多个执行、规划、会话和编码 crate 依赖整个 `astersql-util-chunk`，但这些 crate 依赖只能证明 Chunk crate 的系统位置，不能替代对本文件具体构造函数调用的证明。

## 错误处理与边界

本 API 没有可恢复的 `Result` 错误通道。`Arc<Mutex<Chunk>>` 或 `syncAllocator` 的互斥锁若因持锁线程 panic 而 poisoned，后续 `.lock().unwrap()` 会继续 panic。传入 `capacity > maxChunkSize` 时会静默钳制；`capacity` 或 `maxChunkSize` 为零时仍创建合法的零容量布局。

`InitChunkAllocSize` 防止 `u32` 配置超过 Go `int32` 对照上限，但对零值不报错：两类缓存均为零时，`CheckReuseAllocSize` 返回 false，分配仍然成功，只是不应保留对象。`reuseHookAllocator` 的触发条件是底层“配置上存在任一复用配额”，不是已经实际命中 `free`/`freeColumns`，所以非零默认配置下第一次 `Alloc` 前就会触发 hook。

列回收有多重保护：`avoidReusing` 拒绝借用 RPC 等外部存储的列；固定/变长布局与原桶不一致时拒绝；登记阶段对 `data.capacity()` 使用严格小于 `MaxCachedLen` 的条件；回收阶段变长列容量大于阈值也会拒绝。调用者若在一次使用周期中改变列的类型，`Reset` 不会把它迁移到新桶，而是从原桶丢弃，测试 `allocator_drops_columns_whose_type_changed` 覆盖此行为。

调用协议要求在合适的批次边界执行 `Reset`。未调用 `Reset` 不会使 Rust 失去内存安全，但池不会获得本轮对象；在仍有外部 `ChunkRef` 时调用 `Reset` 会修改同一个共享 Chunk，因此业务层必须保证回收时已经结束使用。

## 并发与资源生命周期

`allocator` 的方法需要 `&mut self`，但返回的 `ChunkRef` 可以跨线程共享；每次读写 Chunk 都由其内部 `Mutex` 保护。普通 `allocator` 并不为多个所有者并发调用设计；需要共享分配器时应使用 `syncAllocator`，它用单个 `Mutex<Box<dyn Allocator + Send>>` 覆盖 `Alloc`、`CheckReuseAllocSize` 和 `Reset` 的完整转调，因而这些操作彼此串行。Go 测试 `TestSyncAllocator` 对应验证高并发分配/重置；当前读取到的 Rust 独立测试未提供同等压力用例。

`reuseHookAllocator.once` 保证其包装器生命周期内 hook 最多执行一次。其构造参数是 `Box<dyn Allocator>` 和 `Box<dyn Fn()>`，没有额外的 `Send + Sync` 约束；需要跨线程共享时必须由更外层类型满足相应线程安全边界，不能仅凭内部 `Once` 推断整个包装器线程安全。

全局 `maxFreeChunks`、`maxFreeColumnsPerType` 和 `MaxCachedLen` 是 `static mut`，访问位于 `unsafe` 块中，没有原子或锁保护。现有生产证据是在启动初始化阶段写入，再由新分配器读取。并发修改这些全局值会构成未同步共享可变状态，扩展时不应把 `InitChunkAllocSize` 当作运行期并发调参 API。资源释放由所有权完成：未缓存对象在离开作用域时释放，缓存对象由分配器持有，分配器销毁时连同底层 `Vec` 容量一起释放。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/chunk/alloc.go`，Rust 保留了 Go 的 `Allocator` 三方法协议、64/256 默认上限、16 KiB 列阈值、按 `typeSize` 分桶、LIFO 复用、Chunk/Column 分层回收、`sync.Mutex` 包装、`sync.Once` hook 和空分配器。`InitChunkAllocSize` 也保留 Go 对 `math.MaxInt32` 的钳制。

关键表示差异如下：Go 返回 `*Chunk` 并在 Chunk 中保存 `[]*Column`；Rust 返回 `Arc<Mutex<Chunk>>`，Chunk 内保存 `Vec<Column>`。因此 Rust 的 `Reset` 不能直接照搬 Go 的指针列表，而要先从被追踪 Chunk 中取出实际列，再按 `reference_id` 与分配时快照合并。该适配保持 Go 的两项重要语义：引用列不会被重复缓存，使用期间改变类型的列仍按原始分桶接受校验而不会迁桶。

Go 的 `emptyAllocator.Alloc` 直接调用 `New(fields, capacity, maxChunkSize)`；Rust 内联了等价构造过程并返回共享互斥引用。Go `NewSyncAllocator` 返回接口类型，Rust 返回具体 `Box<syncAllocator>`，同时为该类型实现 `Allocator`。Rust 还增加了缓存计数、容量和地址观测方法，主要服务测试。

测试覆盖并非完全对称。Go `alloc_test.go` 覆盖多字段物理布局、Chunk/Column 数量上限、`avoidReusing`、类型变化、hook 和 1000 goroutine 的同步包装压力；Rust `alloc_test.rs` 与 `alloc_1_aster_unit_test.rs` 覆盖基础/地址/容量复用、引用去重、类型变化、超大变长列和 hook，但当前没有读到与 Go `TestSyncAllocator`、完整多类型布局、`avoidReusing` 解码路径同等级的 Rust 用例。这些是测试证据缺口，不应描述成实现缺失。

## 扩展指南

- 修改默认缓存策略时，优先调整 `InitChunkAllocSize`、`NewAllocator` 和 `MaxCachedLen`，并明确“全局默认值”与“既有实例快照”的关系；同步更新 `cmd/tidb-server/main.rs` 的配置接线证据以及独立 Rust 测试。
- 增加新的列布局或 `getFixedLen` 返回类别时，必须同步审查 `poolColumnAllocator::{NewSizeColumn,put}` 与 `checkColumnType`，确保新类型不会进入错误桶；至少在 `pkg/util/chunk/alloc_test.rs` 添加类型变化、容量不足和 Reset 后复用的用例。Rust 单元测试应继续保存在独立测试文件，不要内嵌回 `alloc.rs`。
- 修改引用/共享语义时，要联动 `pkg/util/chunk/column.rs` 的 `reference_id` 系列方法、`pkg/util/chunk/chunk.rs::resetForReuse` 以及 `allocator::Reset` 的 `live_columns` 去重流程，并保留 `allocator_does_not_cache_duplicate_column_references` 回归。
- 若要支持运行期动态调参或真正并发共享默认配置，应先把三个 `static mut` 改造成有同步语义的状态，并决定既有分配器是否即时生效；这会改变当前 Go 对齐行为和性能模型，不能只局部修改 setter。
- 若要扩大 Rust 主链使用，调用点应在拥有明确批次生命周期的位置构造分配器，并在所有借用结束后调用 `Reset`；跨线程共享则使用 `syncAllocator` 或提供同等串行化。新增接线需要独立测试证明对象复用不会早于消费者结束。
- 对齐 Go 测试时，优先补 `syncAllocator` 并发压力、`avoidReusing` 解码路径、缓存上限和多字段物理布局；不要用仅检查编译或零断言测试替代行为证据。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标 `pkg/util/chunk/alloc.rs` 含 57 个符号；`files --filter pkg/util/chunk` 确认 Rust/Go 源与独立测试；`node --file pkg/util/chunk/alloc.rs --offset 1 --limit 500` 及 `--offset 494 --limit 80` 覆盖全部 538 行；精确 `query` 确认 `Allocator`、`NewAllocator`、`NewSyncAllocator`、`NewReuseHookAllocator` 和 `checkColumnType` 的目标定义。`callers/callees` 对目标符号在 30 秒内未返回，因此调用点改用精确 `rg` 核验，未把超时结果当作调用图证据。
- 源码与装配：`pkg/util/chunk/alloc.rs`；`pkg/util/chunk/internal/group1/lib.rs` 的 `alloc_impl`/测试模块；`pkg/util/chunk/lib.rs` 的 crate 导出；`pkg/util/chunk/chunk.rs::Chunk::resetForReuse`；`pkg/util/chunk/column.rs` 的 `ColumnAllocator`、列构造、类型尺寸和引用身份方法。
- crate 与生产入口：`pkg/util/chunk/Cargo.toml`；`cmd/tidb-server/main.rs` 中对 `chunk::InitChunkAllocSize` 的启动配置调用。仓库中不存在 `pkg/util/chunk/doc.go`，因此没有额外的目标包 `doc.go` 契约可读。
- Go 对照：`pkg/util/chunk/alloc.go` 与 `pkg/util/chunk/alloc_test.go`，逐项核对接口、阈值、分桶、Reset、同步包装、hook、空实现及边界测试意图。
- Rust 测试：`pkg/util/chunk/alloc_test.rs`、`pkg/util/chunk/alloc_1_aster_unit_test.rs`，以及直接使用入口的 `pkg/util/sqlexec/migration_aster_unit_test.rs`、`pkg/sessionctx/variable/tests/session_test.rs`。本任务是纯文档分析，按计划未运行 Cargo；文档结构由任务指定命令验证。
