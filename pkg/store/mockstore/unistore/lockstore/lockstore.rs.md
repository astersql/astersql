# `pkg/store/mockstore/unistore/lockstore/lockstore.rs`

## 文件定位

本文件是 `astersql-store-mockstore-unistore-lockstore` crate 的核心数据结构实现，定义 arena-backed 跳表 `MemStore`。它面向 UniStore/mock TiKV 的事务锁键值保存场景，提供按字节序有序的查找、插入、替换和删除；有序迭代与持久化分别由同 crate 的 `iterator.rs` 和 `load_dump.rs` 以 `impl MemStore` 扩展（`lib.rs` 统一声明并再导出这些模块）。

crate 边界由 `pkg/store/mockstore/unistore/lockstore/Cargo.toml` 确认：库入口是 `lib.rs`，运行时外部依赖只有 `rand = 0.8`，`package.metadata.porting.go-package` 指向同目录 Go 包。生产接线中，`pkg/store/mockstore/unistore/server/server.rs` 在创建 UniStore server 时以 `MemStore::NewMemStore(LOCK_STORE_ARENA_SIZE)` 构造并用 `Arc<MemStore>` 保存；`pkg/store/mockstore/unistore/tikv/mvcc/db_writer.rs` 的 `DBBundle`/`DBSnapshot` 也把它定义为共享锁存储视图。当前 Rust 搜索证据只显示生产侧的构造与共享，没有显示这些生产模块直接调用 `Put`/`Get`/`Delete`；这些 CRUD 行为目前由本 crate 的独立测试直接覆盖，不能据此宣称完整 MVCC 写链已经接通。

## 核心职责

- 用最多 16 层的跳表维护 key 的字节序，底层链包含全部节点，高层链加速定位（`maxHeight`、`findGreater`、`findLess`、`findSpliceForLevel`）。
- 把节点头、变长 next 地址数组、key 和 value 紧凑写入 arena 字节块，并以 `arenaAddr` 而非进程裸指针持久化链关系（`nodeHeader`、`node`、`newNode`）。
- 保持“单写多读”模型：写 API 要求 `&mut self`，已发布的 next 和 arena locator 用顺序一致原子操作供读者观察（`setNextAddr`、`getNextAddr`、`setArena`、`getArena`）。
- 通过 `Hint` 缓存各层 splice，连续写入或删除时尽量避免从最高层重新搜索（`calculateRecomputeHeight`、`PutWithHint`、`DeleteWithHint`）。
- 在替换或删除后把旧分配归还 arena，由 `arena.rs` 的引用计数和 100ms 延迟窗口控制块复用，避免内存只增不减（`replace`、`DeleteWithHint`、`arena::free`）。

## 主要符号

- `MemStore`：主存储。`height` 是当前有效层数，`head` 是最大高度哨兵，`arenaPtr` 指向当前 arena locator，`retiredArenas` 延迟保留旧 locator，`rand` 生成随机层高，`length` 记录逻辑条目数。`NewMemStore` 返回 `Box<MemStore>` 并调用 `setHeadNode` 完成初始化。
- `nodeHeader`：`#[repr(C)]` 固定头，记录自身 `arenaAddr`、层高、key 长度和 value 长度。`nodeHeaderSize` 来自 `size_of::<nodeHeader>()`。
- `node`：`#[repr(C)]` 的变长节点起点。`nextsBase` 只是 next 数组首槽，`nextsAddr` 通过裸指针运算访问其余槽；`nodeLen`、`getKey`、`getValue` 按布局解析 arena 字节。
- `entry`：查找返回的轻量对象，包含节点裸指针和已复制的 key；空节点以 null 指针和空 `Vec` 表示。
- `Hint`：`prev`/`next` 各有 `maxHeight + 1` 个节点指针，额外一格保存从 `listHeight` 开始搜索的哨兵；`height` 标记缓存见过的表高。`Hint::new` 产生全空缓存。
- 查询入口：`Get` 精确查找；`findGreater(key, allowEqual)`、`findLess(key, allowEqual)` 分别实现大于侧和小于侧定位；`findLast` 找最大键。这些内部查询同时供 `iterator.rs` 的 `Next`、`Prev`、`Seek*` 使用。
- 写入口：`Put`/`Delete` 是无 hint 包装；`PutWithHint`/`DeleteWithHint` 是实际变更路径。`replace` 处理同 key 覆盖，`newNode` 负责 arena 分配和布局。
- 生命周期入口：`getArena`/`getArenaMut` 取得当前 locator，`setArena` 原子发布增长后的 locator，`Drop` 释放当前 locator 和全部 retired locator。

## 执行流程

构造流程从 `NewMemStore` 开始：创建一个 `newArenaLocator(arenaBlockSize)`，初始化高度为 1、长度为 0和基于当前时间的随机源，再由 `setHeadNode` 分配一个高度为 `maxHeight` 的空哨兵，并把所有 next 初始化为 `nullArenaAddr`。

精确读取 `Get` 调用 `findGreater(key, true)`。后者从当前最高层和 head 开始：后继 key 较小时向右移动，后继大于目标或不存在时下降一层，相等且允许相等时立即返回。命中后，`entry::getValue` 按节点头中的长度从 arena 拷贝 value；`Get` 清空调用者的 `buf`、复用其容量写入，并返回一个新的 `Vec` 克隆。未命中返回 `None`。

插入/替换由 `PutWithHint` 完成：

1. `calculateRecomputeHeight` 检查 hint 的表高、相邻节点连接和 key 是否仍落在缓存区间，算出必须重算的最低层数范围。
2. 从需重算范围的高层向低层调用 `findSpliceForLevel`，填充每层 `prev`/`next` 并识别同 key 节点。
3. 同 key 已存在时，`replace` 分配同高度新节点，继承旧节点 next，逐层让前驱指向新节点，最后 `free` 旧分配；逻辑长度不变，返回 `false`。
4. 新 key 则由 `randomHeight` 以每层约 1/4 的晋升概率选择高度，`newNode` 写入完整节点。若新高度超过表高，先原子更新高度。
5. 新节点从第 0 层向上接入：先设置自身后继，再原子发布前驱指向。完成后 `length += 1`，返回 `true`。自底向上的顺序保证读者不会先在高层发现一个尚未接入底层的节点。

删除由 `DeleteWithHint` 复用相同 hint 校验和 splice 搜索。未找到返回 `false`；找到后从节点最高层向第 0 层依次把前驱 next 改成被删节点的后继，使任意中间时刻仍保持可遍历结构，然后释放 arena 分配、递减长度并返回 `true`。删除不会降低 `MemStore.height`，因此表高是历史最高层数而非当前最高节点的精确高度。

`newNode` 计算 `nodeHeaderSize + height * 8 + key.len() + value.len()`，首次分配失败时调用 `arena::grow`，由 `setArena` 发布新 locator 后重试一次；随后写 header、key、value。旧 locator 进入 `retiredArenas`，其共享 `Arc<arenaBlock>` 使已有块和节点地址保持有效。

## 数据与状态

跳表不变量是每层 key 严格递增，且高层节点也出现在所有更低层。head 不代表用户条目，查找函数明确不返回 head。空后继统一编码为 `nullArenaAddr(0)`；非空地址高 32 位编码 block index + 1，低 32 位是块内偏移（直接依赖 `arena.rs`）。

节点内存布局为 `[nodeHeader][height 个 u64 next][key][value]`。next 存 arena 地址，只有临时的 `head`、`entry.node`、`Hint.prev/next` 使用裸指针。key/value 读取均复制成 `Vec<u8>`，因此公开读取结果和 iterator 缓存不长期借用 arena 字节。`keyLen` 是 `u16`、`valLen` 是 `u32`，调用者必须把长度限制在可表示范围且总节点大小不超过 arena block；源码没有显式返回尺寸错误。

`length` 只在新 key 插入和成功删除时变化；替换不改变长度。`MaxEntrySize` 返回 `blockSize - nodeHeaderSize - getHeight() * 8`，是按当前表高计算的估计上限，注释也只承诺超出时“很可能失败”，并非带错误返回的强校验 API。

`Hint` 只对其观察过的同一 `MemStore` 和仍有效的结构位置有意义。每次使用前 `calculateRecomputeHeight` 会验证连接与 key 区间，但结构中没有 store 身份字段；不得把一个 store 的 hint 传给另一个 store，也不得在缺乏单写者串行保证时并发修改同一个 hint。

## 依赖与调用关系

下游直接依赖分为三类：

- `super::arena::{arena, arenaAddr, newArenaLocator, nullArenaAddr}` 提供分配、地址解析、增长和延迟复用；`MemStore` 的所有节点地址与生命周期建立在这些行为上。
- `std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU64}` 发布表高、arena locator 和 next；`std::ptr`/`std::mem` 支撑变长节点的 C 布局与裸指针运算。
- `rand::rngs::StdRng`/`RngCore`/`SeedableRng` 生成跳表层高；`SystemTime` 只用于随机种子，不参与数据语义。

RustCodeGraph 的关键内部调用边包括：`Get -> findGreater/getArena/entry::getValue`；`Put -> PutWithHint`；`PutWithHint -> calculateRecomputeHeight/findSpliceForLevel/randomHeight/newNode/replace/setHeight`；`replace -> newNode/setNextAddr/arena::free`；`Delete -> DeleteWithHint`；`DeleteWithHint -> calculateRecomputeHeight/findSpliceForLevel/setNextAddr/arena::free`；`newNode -> arena::alloc/arena::grow/setArena`。

上游扩展与接线包括：`iterator.rs` 通过 `findGreater`、`findLess`、`findLast` 和 `getNext` 实现双向定位；`load_dump.rs` 通过 `Put` 恢复文件并通过 `NewIterator` 有序转储；`server/server.rs` 构造 `Arc<MemStore>`；`tikv/mvcc/db_writer.rs` 在 bundle 和 snapshot 间克隆该 `Arc`。crate 的 `lib.rs` 再导出 `lockstore::*`，根 workspace `Cargo.toml` 以 `facade_store_mockstore_unistore_lockstore` 注册该包。

## 错误处理与边界

CRUD API 不返回 `Result`：未命中的 `Get` 是 `None`，不存在 key 的 `Delete*` 是 `false`，覆盖已有 key 的 `Put*` 也是 `false`（含义是“不是新键”，不是失败）。arena 正常扩容由 `newNode` 内部处理一次。

以下前置条件靠调用者或内部不变量维持，违反时可能 panic、整数截断或产生未定义行为，而不是结构化错误：节点总大小必须能装入单个 block；`arenaBlockSize` 必须足以容纳 max-height head；key 长度不得超过 `u16::MAX`，value 长度不得超过 `u32::MAX`；传入 hint 必须源自同一 store；层号、节点指针和 `arenaAddr` 必须有效。`newNode` 在 grow 后不再次检查 `alloc` 是否仍为 `nullArenaAddr`，随后解析该地址会失败。空 key 虽可进入核心存储，但 `iterator.rs` 用空 key 表示无效位置，因此扩展公开使用场景时应明确禁止或单独修正该语义冲突。

构造随机种子时系统时间早于 Unix epoch 会通过 `unwrap_or_default` 退回 0，不会报错。锁存储本身不执行文件 I/O；加载/转储错误策略属于 `load_dump.rs`。

## 并发与资源生命周期

并发契约是一个写者、多个读者。Rust 类型层面，变更 API 使用 `&mut MemStore`，读 API 使用 `&MemStore`；文件显式 `unsafe impl Send + Sync for MemStore`。next、表高和 arena locator 均以 `SeqCst` 原子 load/store/swap 发布，但 `length`、随机源、arena 分配元数据和节点初始化不是多写安全的，因此不能绕过 `&mut` 或用内部可变性制造并发写者。

新节点先在未发布字节区完成 header/key/value 和 next 初始化，再通过前驱的原子 next 对读者可见。插入自低到高，删除自高到低；替换先完整建立新节点，再切换前驱。arena grow 创建新 locator、共享旧 `Arc<arenaBlock>` 并原子交换 `arenaPtr`；旧 locator 不立即释放，而是保存在 `retiredArenas` 直到 `MemStore::drop`，避免并发读者持有的旧 locator 引用悬空。`Drop` 最终回收当前和 retired locator。

删除与替换后的节点空间由 `arena::free` 降低 block 引用计数；空 block 等待 `reuseSafeDuration`（100ms）后才进入可写队列。`arena.rs` 明确说明这是一种降低无锁读者读到覆盖数据概率的时间窗口，而不是基于 epoch/hazard pointer 的严格证明。修改并发策略、延迟或复用条件时必须同步审查 `arena.rs`，不能只改本文件。

`lockstore_test.rs::test_mem_store_concurrent` 使用 `Arc<RwLock<Box<MemStore>>>` 串行化写并并发读取；`migration_aster_unit_test.rs::migration_memstore_supports_go_single_writer_multiple_readers` 则在构造后把只读 `Arc<MemStore>` 分享给多个线程。这两者分别证明测试中的写者序列化和并发只读行为，但不扩大源码声明的单写契约。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/lockstore/lockstore.go`。Rust 保留了 Go 的 `MemStore`/`nodeHeader`/`node`/`entry`/`Hint` 结构、16 层上限、节点字节布局、按 1/4 概率晋升的层高、hint 重算算法、插入自底向上与删除自顶向下的发布顺序，以及 `Put*`/`Delete*` 的布尔返回语义。查找中的 Rust slice `cmp` 对应 Go `bytes.Compare`。

主要语言映射差异如下：Go 的 `nil []byte` 在 Rust `Get` 中是 `None`，而空 `entry.getValue` 是空 `Vec`；Go 复用传入 `buf[:0]` 并返回该 slice，Rust 先复用 `&mut Vec` 的容量，再返回 `buf.clone()`，因此返回值拥有独立分配语义。Go 的嵌入 `*node` 被 Rust 显式 `entry.node` 和转发方法替代。Go `unsafe.Pointer`/atomic API 被 Rust 裸指针与 `Atomic*` 替代；Rust 另外保留 `retiredArenas` 并实现 `Drop`，以弥补 Go GC 会自然延长旧 locator 生命周期而 Rust 必须显式回收的差异。

Go `newNode(arena, ...)` 接收 locator 参数；Rust `newNode` 通过 `&mut self` 访问并在增长时更新当前 locator。Go 的 arena locator 由 GC 管理，Rust locator 放入 `Box` 后转成裸指针。除这些所有权适配外，主要控制流逐段对应。`lockstore_test.rs` 保留大批量 CRUD、覆盖、并发和 benchmark 工作负载；`migration_aster_unit_test.rs` 进一步以小型断言核对 Go 对齐语义。

## 扩展指南

- 新增点查或范围定位语义时，优先复用 `findGreater`/`findLess`/`findLast`，并同步审查 `iterator.rs`。不要让公开 API 返回 arena 内部借用或裸指针；沿用复制 key/value 的边界。
- 修改写入算法时，必须保持节点完全初始化后再发布、插入低层到高层、删除高层到低层，以及同一时间只有一个写者。涉及 next 布局时同步修改 `nodeLen`、`newNode`、`getKey`、`getValue` 和 Go 对照实现。
- 调整 hint 时，围绕 `calculateRecomputeHeight`、`findSpliceForLevel`、`PutWithHint`、`DeleteWithHint` 修改，并增加跨递增/递减/重复 key、表高增长和替换删除交错的测试；若允许跨 store 使用，必须先为 `Hint` 增加可验证的 store 身份。
- 增加尺寸检查或错误返回时，最可能入口是 `NewMemStore`、`newNode` 和 `MaxEntrySize`。这会改变现有 API，需要核对 `load_dump.rs` 恢复流程、server 构造和 Go 兼容性；尤其要覆盖超大 key/value、过小 block 和 grow 后仍无法分配。
- 改变 arena 生命周期或并发保证时，必须同时审查 `arena.rs` 的 `UnsafeCell`、引用计数、grow 和延迟复用，以及本文件的 `unsafe impl Send/Sync`、`setArena` 和 `Drop`。这是正确性和内存安全高风险区域。
- 测试逻辑应继续放在独立文件：核心 CRUD/并发回归加入 `lockstore_test.rs`，明确的 Go 迁移一致性断言加入 `migration_aster_unit_test.rs`；迭代器和持久化分别放在对应独立测试文件，不应内嵌到生产源文件。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/store/mockstore/unistore/lockstore` 确认目标及相邻 Go/Rust 文件；`node --file .../lockstore.rs` 分段读取 729 行和 42 个符号；`query` 核对 `MemStore`、`NewMemStore`、`PutWithHint`、`DeleteWithHint`、`findGreater`、`findLess`；`callees` 核对本文件内部调用链。图对同名 Go/Rust callers 消歧不足，因此上游接线另用精确路径搜索补证。
- 生产源码：`pkg/store/mockstore/unistore/lockstore/lockstore.rs`；直接内存依赖 `arena.rs`；扩展调用者 `iterator.rs`、`load_dump.rs`；crate 入口 `lib.rs`；生产持有者 `pkg/store/mockstore/unistore/server/server.rs` 与 `pkg/store/mockstore/unistore/tikv/mvcc/db_writer.rs`。
- 配置与 Go 对照：`pkg/store/mockstore/unistore/lockstore/Cargo.toml`、根 `Cargo.toml` 的 facade 注册、`pkg/store/mockstore/unistore/lockstore/lockstore.go`。
- 独立测试：`lockstore_test.rs` 覆盖大批量增查删、延迟复用、替换、iterator 和单写多读；`migration_aster_unit_test.rs` 覆盖 CRUD/Hint/Len/MaxEntrySize、并发只读及与 arena/load-dump 的组合语义；`load_dump_test.rs` 覆盖持久化调用面。按任务约束未运行 Cargo。
- 人工复核结论：本文能够从构造、查询、hint 写入、节点布局、arena 增长/回收、原子发布顺序解释文件为何存在与如何运行；扩展章节列出了尺寸、unsafe、并发、Go 兼容和独立测试风险。结构验证命令及退出码在任务交付前单独记录。
