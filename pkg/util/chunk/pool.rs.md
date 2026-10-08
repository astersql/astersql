# `pkg/util/chunk/pool.rs`

源文件：[pool.rs](pool.rs)。本文说明 `astersql-util-chunk` crate 中按列物理宽度复用 `Column` 的对象池实现；它不负责行计算、序列化或内存配额，只负责创建/回收构成 `Chunk` 的列缓冲区。

## 文件定位

- 本文件由 crate 入口 [`lib.rs`](lib.rs) 通过 `#[path = "pool.rs"] pub mod pool` 挂入 `astersql-util-chunk`；crate 边界及直接依赖声明在 [`Cargo.toml`](Cargo.toml)。`Pool` 使用的 `Chunk`、`Column`、`getFixedLen`、`newFixedLenColumn`、`newVarLenColumn` 和 `types::FieldType` 都由 crate 根重导出。
- 它位于 Chunk 内存构造链上：[`chunk.rs`](chunk.rs) 的 `NewChunkFromPoolWithCapacity` 调用 crate 内的 `getChunkFromPool` 包装，最终进入本文件的 `getChunkFromPool`；`Chunk::Destroy` 沿相反方向调用 `putChunkFromPool` 归还列。
- [`internal/group1/lib.rs`](internal/group1/lib.rs) 提供按值接收 `Vec<FieldType>`/`Chunk` 的适配函数，把调用转发给本文件按切片和可变引用接收的接口。这是 `chunk.rs` 经 `include!` 注入后的实际接线，不是另一套池实现。
- 此池优化的是具有相同 `initCap` 和物理列宽的反复分配。普通的 `New`、`NewChunkWithCapacity`、`NewEmptyChunk` 不经过它。

## 核心职责

1. `global_pool` 以 `initCap` 为键懒创建并共享 `Arc<Pool>`，使相同初始行容量的 Chunk 使用同一组列缓存。
2. `Pool` 按 `getFixedLen(FieldType)` 的结果将列分入五个桶：变长列的 `VarElemLen`，以及 4、8、16、40 字节定长列。
3. `GetChunk` 从每个对应桶取出一列；桶为空时按 `initCap` 新建列，然后组装一个空的 `Chunk`。
4. `PutChunk` 校验字段数与列数一致，逐列执行 `Column::reset`，再放回按字段物理宽度选定的桶；同时通过 `drain(..)` 清除原 Chunk 对列的所有权。
5. 该文件只实现显式复用。Chunk 必须走 `Chunk::Destroy(initCap, fields)` 或直接调用 `PutChunk` 才会归还列；普通 Rust `drop` 不会自动回池。

## 主要符号

- `static globalChunkPool: OnceLock<Mutex<HashMap<usize, Arc<Pool>>>>`：进程内按 `initCap` 分组的懒初始化全局表。`OnceLock` 只初始化表一次，表项持有共享池的强引用。
- `fn global_pool(initCap: usize) -> Arc<Pool>`：锁住全局表，查询或以 `NewPool(initCap)` 插入池，并克隆 `Arc` 后返回。中毒的互斥锁通过 `into_inner` 恢复数据而非传播错误。
- `pub fn getChunkFromPool(...) -> Box<Chunk>` / `pub fn putChunkFromPool(...)`：全局池入口，分别转发到对应容量池的 `GetChunk` 与 `PutChunk`。
- `pub struct Pool`：包含不可变的 `initCap` 和五个 `Mutex<Vec<Column>>`。每个物理宽度独立加锁，允许不同桶并行访问。
- `pub fn NewPool(initCap: usize) -> Box<Pool>`：建立一个所有桶均为空的独立池；返回 `Box` 保持 Go 风格构造接口，调用方可在需要共享时解包进 `Arc`。
- `fn cache(&self, width: usize)`：把 `VarElemLen/4/8/16/40` 映射到具体缓存桶；其他宽度会 `panic!`。
- `fn get_column(&self, width: usize) -> Column`：从桶尾 `pop`；没有缓存时，变长列走 `newVarLenColumn(initCap)`，定长列走 `newFixedLenColumn(width, initCap)`。
- `pub fn GetChunk(&self, fields: &[FieldType]) -> Box<Chunk>`：公开取用入口。它设置 `capacity == requiredRows == initCap`，并把选择向量设为 `None`、虚拟行数设为 0、`inCompleteChunk` 设为 `false`。
- `pub fn PutChunk(&self, fields: &[FieldType], chunk: &mut Chunk)`：公开归还入口。字段数不等于列数时先断言失败；成功后 Chunk 的 `columns` 为空。
- `pub fn cached_columns(&self) -> usize`：依次锁住五个桶并求缓存列总数，主要用于测试/诊断，不给出按桶细分结果或原子快照。
- `global_cached_columns_for_test`：仅在 `cfg(test)` 下暴露，用来观察指定 `initCap` 的全局池缓存数。

## 执行流程

取用流程如下：

1. `NewChunkFromPoolWithCapacity(fields, initCap)` 把输入转换为 `Vec<FieldType>`，经 `internal/group1/lib.rs::getChunkFromPool` 进入本文件。
2. `global_pool(initCap)` 初始化或锁住全局表，按容量取得 `Arc<Pool>`；首次出现该容量时创建空池。
3. `Pool::GetChunk` 遍历字段。每个字段先由 [`codec.rs`](codec.rs) 的 `getFixedLen` 归类，再由 `get_column` 锁住相应桶并弹出缓存列；缓存为空才调用 [`column.rs`](column.rs) 的列构造函数。
4. 所得列被收集进一个新 `Chunk`。列本身应处于 `length == 0` 的可写状态，而已分配的 `Vec` 容量得以保留供后续追加复用。

归还流程如下：

1. 调用方消费完 Chunk 后调用 `Chunk::Destroy(initCap, fields)`；适配层把拥有的 `Chunk` 转成可变引用交给本文件。
2. `Pool::PutChunk` 先要求 `fields.len() == chunk.columns.len()`，防止按错误数量归还。
3. `fields` 与 `chunk.columns.drain(..)` 成对遍历。每列的 `reset` 清零逻辑长度、null bitmap 和数据，变长 offsets 只保留起始 0；已经分配的底层容量保留。
4. 根据对应字段重新计算宽度并把列压入该桶。循环结束后原 Chunk 不再持有任何列。

## 数据与状态

- `initCap` 同时决定新列的预分配规模以及新 Chunk 的 `capacity`、`requiredRows`。它也是全局表的唯一键；字段组合不是键，因为单列只按物理宽度复用。
- `getFixedLen` 当前把 Float 映射为 4；Tiny/Short/Int24/Long/Longlong/Double/Year/Duration 映射为 8；Date/Datetime/Timestamp 使用 `sizeTime`（当前池预期为 16）；NewDecimal 使用 `MyDecimalStructSize`（当前池预期为 40）；其余类型使用 `VarElemLen`。池的宽度集合必须和这套映射保持同步。
- 定长列的 `elemBuf` 长度等于元素宽度，`data` 初始容量约为 `initCap * width`；变长列的 offsets 初始含一个 0，数据容量使用列模块的估算元素宽度。相关构造细节在 `newFixedLenColumn`/`newVarLenColumn`。
- `Column::reset` 清除内容但不缩容，也不重建 `reference_id`，因此复用的核心收益是保留 allocation；池本身不统计命中率、字节数或最大缓存量。
- 全局 `HashMap` 和每个桶的 `Vec<Column>` 都没有淘汰上限。出现很多不同 `initCap`，或归还大量/曾膨胀很大的列时，内存会由全局强引用长期保留。

## 依赖与调用关系

- 上游生产调用边：`chunk.rs::NewChunkFromPoolWithCapacity → internal/group1/lib.rs::getChunkFromPool → pool.rs::getChunkFromPool → global_pool → Pool::GetChunk`。
- 上游归还调用边：`chunk.rs::Chunk::Destroy → internal/group1/lib.rs::putChunkFromPool → pool.rs::putChunkFromPool → global_pool → Pool::PutChunk`。
- 下游取用边（RustCodeGraph）：`global_pool → NewPool`，`getChunkFromPool → global_pool + GetChunk`，`GetChunk → get_column → cache`。
- 下游归还边（RustCodeGraph）：`putChunkFromPool → global_pool + PutChunk`，`PutChunk → Column::reset + cache`。其中 `Column::reset` 位于 [`column.rs`](column.rs)。
- 类型分类依赖 [`codec.rs`](codec.rs) 的 `VarElemLen`/`getFixedLen`；实际列分配依赖 [`column.rs`](column.rs) 的 `newFixedLenColumn`/`newVarLenColumn`。
- `Cargo.toml` 表明本文件属于 `astersql-util-chunk` 库（入口 `lib.rs`）。本文件自身只直接使用标准库的 `HashMap`、`Arc`、`Mutex`、`OnceLock`；字段类型来自该 crate 已声明的 `astersql-types-datum` 依赖并经 crate 根别名 `types` 使用。

## 错误处理与边界

- API 不返回 `Result`。宽度不属于 `VarElemLen/4/8/16/40` 时，`cache` 立即 `panic!("unsupported chunk column width …")`。因此扩充 `getFixedLen` 的固定宽度集合时，必须同步修改本文件。
- `PutChunk` 在字段数与列数不同时通过 `assert_eq!` 恐慌。这避免静默遗漏列，但不能验证每个字段的物理宽度是否真与当前列布局一致；调用方必须传回创建该 Chunk 时相同顺序和语义的字段列表。
- 所有锁都对 poison 采用 `poisoned.into_inner()`。一次持锁恐慌不会永久封锁池，但随后继续使用的状态是否满足语义不变量取决于恐慌发生点；当前修改操作很短，且 `PutChunk` 的数量断言发生在取得桶锁之前。
- `GetChunk` 若在处理中恐慌，已从先前桶弹出的列随正在构造的临时集合被正常释放，而不是自动放回池；这是性能损失，不是内存安全问题。
- `cached_columns` 是逐桶读取，其他线程可在两次加锁之间取还列，因此并发场景下只是观测值，不是某一瞬间的一致快照。
- `initCap` 为 0 在类型上允许；构造列时不会预留行数据，Chunk 的容量和 requiredRows 也为 0。本文件不负责拒绝这种调用。

## 并发与资源生命周期

- `Pool` 的所有可变缓存状态都在 `Mutex` 内，`initCap` 只读，因此可由 `Arc<Pool>` 跨线程共享。五个独立桶降低不同宽度间的锁竞争；同宽度列仍串行 `pop/push`。
- 全局表使用单个 `Mutex<HashMap<...>>`，每次全局取还都会取得排他锁并克隆 `Arc`，随后才在具体池中操作。锁不会跨 `GetChunk`/`PutChunk` 的列处理阶段持有。
- `get_column` 在桶为空时仍持有桶锁完成列分配，可避免同一桶多个线程同时把“空”误判为需要复用同一对象；代价是首次/耗尽后的分配会阻塞该宽度的其他线程。
- 归还时每一列分别取得一次桶锁。同一 Chunk 含多个相同宽度字段时不会批量锁住桶；这保持实现简单，但高并发宽表可能产生额外锁开销。
- 全局表由 `OnceLock` 存活到进程结束，表中的 `Arc` 又使所有按容量创建的池持续存活。桶中的 `Vec` 持有列及其保留容量，既无后台清理，也无显式关闭方法。
- `PutChunk` 通过 `drain(..)` 转移列所有权；归还后继续使用原 Chunk 只会看到零列。再次使用应重新从池取 Chunk，不能把已归还的 Chunk 当作原布局对象。
- [`pool_test.rs`](pool_test.rs) 用 4 个线程同步首次取用、随后各执行 100 次往返，验证共享池不会重复交出同一列，并在全部归还后缓存列数等于 `字段数 × 并发数`。

## 与 Go 版本的对应关系

- [`pool.go`](pool.go) 同样以 `initCap` 建全局池表，`Pool` 同样含变长与 4/8/16/40 五类列池，`GetChunk`/`PutChunk` 的字段宽度分派和 Chunk 初始容量语义一致。Rust 的独立测试 [`pool_test.rs`](pool_test.rs) 对照 Go 的 [`pool_test.go`](pool_test.go) 覆盖构造、列布局和归还后列引用清空。
- Go 全局表用 `RWMutex`，命中时只持读锁；Rust 当前用单个排他 `Mutex` 并通过 `entry` 原子查找/插入。两者保证每个容量有共享池，但并发争用特征不同。
- Go 每桶使用 `sync.Pool`，运行时可在 GC 时丢弃缓存对象；Rust 使用 `Mutex<Vec<Column>>`，对象采用 LIFO 且不会被运行时主动回收。因此 Rust 的复用更可预测，但长期内存保留风险更高。
- Go `GetChunk` 的 `switch` 对未知宽度没有赋列，最终可能留下空列引用；Rust `cache` 对未知宽度立即恐慌，较早暴露 `getFixedLen` 与池桶不同步。
- Go `PutChunk` 按 `fields` 索引 `chk.columns[i]`，数量不符会越界或留下列；Rust 先显式断言数量相等，再 drain 所有列，失败模式更清晰。
- Rust 特有的 `cached_columns`、`global_cached_columns_for_test` 是诊断/测试辅助；Go 原实现没有等价计数 API。Rust 测试还把 Go benchmark 的 `RunParallel` 意图固化成确定性的并发单元测试，并验证 `Chunk::Destroy` 的全局回收接线。

## 扩展指南

- 新增固定物理宽度时，应先核对 `codec.rs::getFixedLen`，再给 `Pool` 增加对应桶，并同步更新 `NewPool`、`cache`、`cached_columns`；同时在独立的 [`pool_test.rs`](pool_test.rs) 添加该字段类型的取用、归还及并发覆盖，不能把测试写回生产文件。
- 若改变 Chunk 的字段或初始化不变量，应同步检查 `Pool::GetChunk` 对 `sel`、`capacity`、`requiredRows`、`numVirtualRows`、`inCompleteChunk` 的构造，以及 `Chunk::Destroy` 的归还接线。
- 若增加缓存上限、按字节淘汰或收缩策略，主要接入点是 `get_column`/`PutChunk` 和全局表生命周期；需要明确处理膨胀后的列容量，并补充内存保留与并发压力测试。与 Go 语义对齐时还要考虑 `sync.Pool` 可被 GC 清空这一差异。
- 若想降低全局命中路径竞争，可调整 `globalChunkPool` 的同步策略，但必须保留“同一个 `initCap` 只发布一个共享池”的不变量，并为并发首次创建添加回归测试。
- 调用方新增池化路径时，取用和归还必须携带相同的 `initCap` 与字段顺序；最安全的接线位置是现有 `NewChunkFromPoolWithCapacity` 和 `Chunk::Destroy`，避免绕开转换/所有权约束。
- 性能风险主要来自全局互斥、逐列桶锁、锁内首次分配和无上限缓存；兼容风险主要来自宽度表不同步及错误字段布局。任何优化都应保留五类物理布局、归还后 Chunk 零列、列内容清空和跨线程唯一所有权。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 `pkg/util/chunk/pool.rs`；文件节点显示 161 行、12 个符号，并给出 `global_pool → NewPool`、`getChunkFromPool → global_pool/GetChunk`、`putChunkFromPool → global_pool/PutChunk`、`GetChunk → get_column → cache`、`PutChunk → cache` 等调用边。
- 已读生产源码：[`pool.rs`](pool.rs)、[`chunk.rs`](chunk.rs) 中 `NewChunkFromPoolWithCapacity`/`Chunk::Destroy`、[`internal/group1/lib.rs`](internal/group1/lib.rs) 中两个适配入口、[`codec.rs`](codec.rs) 中 `VarElemLen`/`getFixedLen`、[`column.rs`](column.rs) 中列构造与 `Column::reset`、crate 入口 [`lib.rs`](lib.rs) 及 [`Cargo.toml`](Cargo.toml)。
- 已读 Go 对照：[`pool.go`](pool.go)，核对全局表、五个 `sync.Pool`、构造/取用/归还流程；[`pool_test.go`](pool_test.go)，核对构造、布局、清空和并行 benchmark 的原始意图。
- 已读 Rust 独立测试：[`pool_test.rs`](pool_test.rs)，覆盖全部五种池宽、容量/布局、reset 与列所有权释放、4 线程往返以及 `Destroy` 到全局池的复用链。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另以任务指定命令验证本文恰好包含 11 个固定二级章节，并人工复核所有行为结论均可追溯到上述符号或文件。
