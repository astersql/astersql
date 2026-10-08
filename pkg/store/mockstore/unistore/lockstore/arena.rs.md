# `pkg/store/mockstore/unistore/lockstore/arena.rs`

## 文件定位

本文件是 `astersql-store-mockstore-unistore-lockstore` crate 的分块内存分配层。crate 入口 `pkg/store/mockstore/unistore/lockstore/lib.rs` 将 `arena` 声明为公开模块并重导出其符号；`Cargo.toml` 把该 crate 对应到 Go 包 `pkg/store/mockstore/unistore/lockstore`，运行时依赖只有 `rand`，arena 本身只使用标准库。

直接上层是 `pkg/store/mockstore/unistore/lockstore/lockstore.rs` 的 `MemStore`：跳表节点的头、各层 next 地址、key 和 value 都被连续写入 arena 字节块。`MemStore::NewMemStore` 创建定位器，`MemStore::newNode` 分配并写入节点，节点读取路径通过 `arena::get` 取回字节，替换和删除路径通过 `arena::free` 归还块级引用。

## 核心职责

- 用 `arenaAddr` 把“块下标 + 块内偏移”编码进一个 `u64`，同时保留 `0` 作为 `nullArenaAddr`。
- 用 `arenaBlock` 提供固定容量、8 字节对齐的顺序分配。它不逐条回收空间，而是统计整个块仍有多少个已分配对象被引用。
- 用 `arena` 管理多个共享块、当前可写块和等待复用的空块；一个块写满后从可写队列移除，引用清零后延迟 100ms 才重新进入可写队列。
- 在容量不足时通过 `arena::grow` 生成新定位器：旧块由 `Arc` 共享，新定位器增加一个新块，因而旧地址仍可被新旧定位器解析。
- 为 Rust 的节点布局补充 `get_mut`，让单写者能把结构头、key 和 value 写到与 Go `[]byte` 等价的底层缓冲区。

本文件只管理内存与地址，不负责跳表排序、节点发布、迭代、持久化或外部同步；这些职责分别在 `lockstore.rs`、`iterator.rs` 和 `load_dump.rs`。

## 主要符号

- `arenaAddr(pub u64)`：逻辑地址。高 32 位保存 `blockIdx + 1`，低 32 位保存 `blockOffset`。`blockIdx()` 解码时减一，`blockOffset()` 取低 32 位；`newArenaAddr` 执行反向打包。
- `nullArenaAddr` / `nullBlockOffset`：分别表示定位器级分配失败（地址 `0`）和块级分配失败（`u32::MAX`）。调用方必须在解码或取切片前处理哨兵。
- `alignMask`：配合 `(length + 7) & alignMask` 将下一次分配起点向上对齐到 8 字节；其掩码同时将结果限制在低 32 位可编码范围内。
- `reuseSafeDuration`：100ms 的延迟复用窗口，用于降低已删除节点仍被无锁读者读取时遭覆盖的概率。
- `arena`：定位器，包含固定 `blockSize`、共享的 `blocks: Vec<Arc<arenaBlock>>`、后进先出的 `writableQueue` 和按进入顺序保存的 `pendingBlocks`。
- `pendingBlock`：记录待复用块下标及 `reusableTime`。
- `newArenaLocator`：创建一个块，并把块 0 放入可写队列。
- `arena::{get, get_mut}`：先校验块下标，再把地址和长度交给对应 `arenaBlock` 切片。
- `arena::alloc`：从可写队列尾块分配；块满则弹出并继续；没有可写块时只检查最早的 pending 块是否到期，否则返回空地址。
- `arena::free`：递减目标块的 `refCount`。当引用归零且 `length > buf_len()`（说明该块曾因一次失败分配越过容量）时，把块加入延迟队列并把 `length` 复位为 0。
- `arena::{grow, growInPlace}`：前者复制定位器元数据并通过 `Arc` 共享旧块，后者直接追加块。当前生产接线使用 `grow`；`growInPlace` 在目标目录中没有调用者。
- `arenaBlock`：用 `UnsafeCell<Vec<u8>>` 保存稳定的固定长度缓冲区，用 `Cell<u64>` 和 `Cell<usize>` 保存引用数及分配游标；手工实现 `Send`/`Sync`。
- `arenaBlock::alloc`：推进对齐后的游标，越界时保留越界后的 `length` 并返回失败哨兵，成功时递增 `refCount`。

## 执行流程

1. `MemStore::NewMemStore(blockSize)` 调用 `newArenaLocator`，创建块 0；随后 `setHeadNode` 也通过同一 arena 分配跳表头节点（`lockstore.rs` 的 `NewMemStore`、`setHeadNode`、`newNode`）。
2. `MemStore::newNode` 计算 `nodeHeaderSize + height * 8 + key.len() + value.len()`，调用 `arena::alloc`。块内起点按 8 字节对齐；成功时返回编码地址并增加块引用数。
3. 若当前定位器无块可写且没有已到期的 pending 块，`arena::alloc` 返回 `nullArenaAddr`。`newNode` 随即调用 `arena::grow`，用 `Arc` 共享已有块并追加一个新块，再由 `MemStore::setArena` 原子发布新定位器；旧定位器被放入 `retiredArenas`，直到 `MemStore` 析构才回收，避免并发读者仍持有旧定位器。
4. `newNode` 用 `arena::get_mut` 获得连续可变切片并写入节点头、next 数组、key 和 value。节点发布后，读取路径通过 `arena::get` 解析头部、key 和 value。
5. `MemStore::replace` 或 `DeleteWithHint` 在跳表链接改向之后调用 `arena::free`。每次释放只减少块引用数；只有“块已写满并被移出可写队列”且全部对象均释放时，块才进入 pending 队列。
6. 后续分配发现可写队列为空时检查 `pendingBlocks[0]`。到达 `reusableTime` 后，该块重新进入可写队列；其游标已清零，旧字节将在新分配写入时覆盖。

## 数据与状态

地址编码的不变量是：合法块下标编码为高 32 位的 `index + 1`，因此零值可作空地址；偏移必须可表示为 `u32`。`arenaAddr` 不携带分配长度，长度由节点头或调用上下文提供。

`arenaBlock.length` 是下一次分配的原始游标，而不是成功分配字节数。失败分配也会把它推进到容量之后；`free` 正是用 `length > buf_len()` 判断块曾写满。`refCount` 只在成功分配时加一，在节点被替换或删除时减一，因此它统计块内仍被跳表引用的节点数，而不是 `Arc` 强引用数。

`arena.blocks` 中的 `Arc` 保证 `grow` 后字节缓冲仍是同一份分配；`migration_arena_growth_shares_existing_blocks_like_go` 通过新定位器修改旧地址，再从旧定位器读到修改结果，验证这种共享关系。`writableQueue` 当前按栈使用；构造或增长时只加入新块，满块被弹出，到期 pending 块再压回。`pendingBlocks` 当前按向量头部消费，保持释放入队顺序。

## 依赖与调用关系

上游调用边（由 RustCodeGraph 和源码交叉核对）：

- `lockstore.rs::MemStore::NewMemStore -> newArenaLocator`。
- `lockstore.rs::MemStore::newNode -> arena::alloc -> arenaBlock::alloc`；失败时 `newNode -> arena::grow -> newArenaBlock`，成功后 `newNode -> arena::get_mut -> arenaBlock::get_mut`。
- `lockstore.rs::{node::getKey, node::getValue, node::getNextNode, MemStore::getNext, findGreater, findSpliceForLevel, getNode} -> arena::get -> arenaBlock::get`。
- `lockstore.rs::{MemStore::replace, MemStore::DeleteWithHint} -> arena::free`。
- `iterator.rs::Iterator` 经 `MemStore::getArena` 和节点方法读取 arena 内的 value。

下游只依赖 `std::cell::{Cell, UnsafeCell}`、`std::sync::Arc` 和 `std::time::{Duration, Instant}`。该实现没有 I/O、锁、通道或异步任务。`Cargo.toml` 中的 `rand` 被 `lockstore.rs` 用于随机层高，不是 arena 自身依赖；`tempfile`、`testsetup` 仅为测试依赖。

## 错误处理与边界

- 这是无 `Result` 的内部快速路径。分配不足以 `nullBlockOffset` / `nullArenaAddr` 表达；块下标越界和切片范围越界直接 panic。Rust 的 panic 对应 Go `arena.get` 对非法块下标调用 `Fatalf` 的不可恢复语义，但具体终止机制不同。
- `nullArenaAddr.blockIdx()` 会在减一时下溢；所有调用者必须先判断空地址。当前跳表 next 读取路径遵守该约束，但 API 本身没有类型层保护。
- `free` 假定地址合法且每次成功分配恰好释放一次；重复释放会使 `u64` 引用数下溢，释放遗漏则阻止块复用。
- `newNode` 只在首次失败后增长一次，并假定单条目能放入 `blockSize`。若条目仍大于新块，第二次分配仍返回空地址，随后的 `get_mut` 会失败。`MemStore::MaxEntrySize` 仅提供上限估计，调用链没有在本文件内返回可恢复错误。
- `alignMask` 和地址格式隐含块内偏移小于 `2^32`、块下标可装入高 32 位等前提；配置超大块或极多块不受显式检查。
- 延迟 100ms 是基于预期读路径很短的工程窗口，不是读者生命周期追踪或形式化内存回收保证；长时间持有旧节点切片仍可能越过窗口。

## 并发与资源生命周期

设计契约是单写多读。写者独占修改 `arena`、分配元数据和尚未发布的字节区；读者只读取已经通过跳表原子 next 指针发布的节点。`arenaBlock` 因此用 `UnsafeCell`/`Cell` 获得内部可变性，并以 `unsafe impl Send + Sync` 声明可跨线程共享。安全性依赖上层严格保持单写者、发布前完成节点初始化，以及读者不写缓冲区；这些条件不是由 `arenaBlock` 类型系统强制保证的。

增长时，块缓冲由 `Arc` 共享，所以定位器替换不会移动节点字节。`MemStore::setArena` 用 `AtomicPtr::swap(SeqCst)` 发布新定位器，并把旧定位器保留在 `retiredArenas`；`MemStore::drop` 最终释放当前和所有退役定位器。块只有在所有相关 `Arc` 都析构后才真正释放。

删除或替换节点时，先完成跳表链接改向，再递减块引用。写满块在引用清零后等待 `reuseSafeDuration` 才能重写，以降低并发读者仍访问旧节点的风险。该机制没有后台计时器：只有之后调用 `arena::alloc` 且可写队列为空时，才检查并激活已到期块。

## 与 Go 版本的对应关系

`pkg/store/mockstore/unistore/lockstore/arena.go` 是逐项对照来源。Rust 保留了 Go 的地址位布局、8 字节对齐、`nullBlockOffset`/`nullArenaAddr` 哨兵、可写队列、100ms pending 窗口、失败后保留越界 `length`、块级引用计数，以及 `grow` 共享旧块并只让新块可写的行为。

主要语言适配如下：

- Go `arenaAddr` 是 `uint64` 别名；Rust 用可比较、可复制的元组结构 `arenaAddr(pub u64)`。
- Go `[]*arenaBlock` 天然共享指针；Rust 用 `Vec<Arc<arenaBlock>>` 明确共享所有权。迁移测试验证增长前后的定位器观察同一旧块。
- Go 的 `[]byte` 同时提供读写视图；Rust 分成 `get(&self) -> &[u8]` 和写路径使用的 `get_mut`。后者最终从 `UnsafeCell<Vec<u8>>` 构造可变切片。
- Go 的普通字段可在指针方法中修改；Rust 用 `Cell` 保留 `arenaBlock::alloc(&self)` 的共享引用接口，并手工声明线程可共享。
- Go `grow` 返回新 `*arena` 并由上层原子替换；Rust `grow` 返回值对象，再由 `MemStore::setArena(Box<arena>)` 发布和延迟回收旧定位器。Rust 另外提供未在当前生产路径使用的 `growInPlace`。
- Go 越界块下标使用日志 `Fatalf`；Rust 使用 `panic!`。两边的块内切片越界同样属于不可恢复的调用方错误。

## 扩展指南

- 修改地址布局或对齐规则时，应成套调整 `arenaAddr::{blockIdx, blockOffset}`、`newArenaAddr`、`alignMask` 和所有序列化/指针布局假设，并在 `migration_aster_unit_test.rs` 增加边界偏移、最大块号和对齐用例。
- 修改复用策略时，应保持“满块离开可写队列—引用归零—安全窗口到期—重新可写”的状态机；重点同步 `arena::{alloc, free}`、`pendingBlock` 与 `lockstore_test.rs::test_mem_store`。若需要严格读者安全，应引入可证明的 epoch/hazard-pointer 等方案，而不只是调整固定时长。
- 修改增长策略时，必须维持旧节点地址稳定，并同步审查 `MemStore::{newNode, setArena, drop}` 的定位器发布和回收。`migration_arena_growth_shares_existing_blocks_like_go` 是最低限度的共享性回归测试。
- 修改可变访问或并发模型时，必须重新证明 `UnsafeCell` 返回切片不会别名冲突，并审查 `unsafe impl Send/Sync`。测试应放在独立的 `migration_aster_unit_test.rs` 或 `lockstore_test.rs`，不要嵌入生产源文件。
- 为超大条目增加可恢复错误时，接入点是 `arenaBlock::alloc`、`arena::alloc` 与 `MemStore::newNode`；需要同时定义 Go 兼容行为，避免第二次分配失败后再解码空地址。
- `growInPlace` 当前没有生产调用者。若计划启用，应先证明它与 `AtomicPtr` 定位器发布、并发读者持有旧定位器的模型兼容，并添加独立测试；不要仅因实现存在就假定其已接线。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的 Rust/Go 文件均在索引中。
- 目标实现：`pkg/store/mockstore/unistore/lockstore/arena.rs`，核对了全部 258 行及其中 20 个索引符号。
- crate 边界：`pkg/store/mockstore/unistore/lockstore/Cargo.toml`、`pkg/store/mockstore/unistore/lockstore/lib.rs`。
- 生产调用链：`pkg/store/mockstore/unistore/lockstore/lockstore.rs` 的 `NewMemStore`、节点读取方法、`replace`、`newNode`、`getArena`/`getArenaMut`、`setArena`、`DeleteWithHint`；以及 `iterator.rs` 对 `getArena` 的读取引用。
- Go 对照：`pkg/store/mockstore/unistore/lockstore/arena.go` 全文；`lockstore_test.go::TestMemStore` 验证删除、等待 100ms 后重新插入时块数基本稳定，并含单写多读工作负载。
- Rust 独立测试：`migration_aster_unit_test.rs::{migration_arena_matches_go_alignment_growth_and_delayed_reuse, migration_arena_growth_shares_existing_blocks_like_go}`；`lockstore_test.rs::test_mem_store` 验证大批量删除及延迟复用，`test_replace` 和 `test_mem_store_concurrent` 覆盖释放入口所在的替换/并发场景。
- 本任务是纯文档分析，按计划未运行 Cargo。交付检查仅执行任务指定的 11 个固定章节结构验证，并人工复核本文没有把未接线的 `growInPlace` 或固定延迟窗口描述成更强保证。
