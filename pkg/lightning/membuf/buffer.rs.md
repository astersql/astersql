# `pkg/lightning/membuf/buffer.rs`

## 文件定位

本文件实现 Lightning/导入链路使用的定长内存块池和块内顺序分配器，是独立 crate `astersql-lightning-membuf` 的主要实现文件。crate 入口 `pkg/lightning/membuf/lib.rs` 公开 `buffer`、`limiter` 两个模块并重新导出其 API；`pkg/lightning/membuf/Cargo.toml` 将该 crate 标记为 Go 包 `pkg/lightning/membuf` 的移植，运行时依赖只有 `log`（日志实际由相邻的 `limiter.rs` 使用），测试额外依赖 `rand`。

在应用侧，根 `pkg/lib.rs` 通过 `facade_lightning_membuf` 暴露该 crate，`pkg/ingestor/engineapi/ingest_data.rs::IngestData::NewIter` 把 `&mut membuf::Pool` 作为迭代器创建参数；`pkg/session/runtime/import_sst.rs` 创建 `NewPool(Vec::new())` 并把唯一 `Arc` 解包成可变 `Pool` 传入该接口。当前检查到的 `pkg/ingestor/globalsort/engine_api.rs::DataAdapter::NewIter` 和 `pkg/ingestor/ingestctrl/import_pipeline.rs` 中的实现尚未使用该参数，因此文件提供的是已经接入接口边界、但在这些适配器中仍预留给批量 KV 内存复用的基础设施。RustCodeGraph 将本文件标为被 6 个文件引用；直接代码检索确认生产类型边界还包括 `pkg/ingestor/engineapi`、`pkg/ingestor/ingestctrl`、`pkg/ingestor/globalsort` 和 `pkg/ingestor/simplesst` 的 Cargo 依赖或再导出。

## 核心职责

1. `Pool` 以固定大小的 `Vec<u8>` 为单位分配内存，用有界 FIFO 缓存保存已归还块，避免频繁触达底层分配器；缓存溢出才调用 `Allocator::Free`。
2. `Buffer` 从一个 `Pool` 渐进借块，并以 bump-pointer（`curBlockIdx`、`curIdx`）从块内顺序切出小对象；`Reset` 重用已经持有的块，`Destroy` 才将块归还池。
3. `WithBufferMemoryLimit` 用块数近似限制单个 `Buffer` 可持有的容量；`WithPoolMemoryLimiter` 则通过 `limiter.rs::Limiter` 限制所有已借出固定块及小对象元数据预算。
4. `AllocBytes`/`AddBytes` 提供阻塞分配，`TryAllocBytes`/`TryAddBytes` 提供配额不足时立即失败且不改变状态的分配。
5. `SliceLocation` 用三个 `i32` 记录块号、偏移和长度，替代长期保存带指针的切片；`GetSlice` 在需要时重建切片，从而对齐 Go 版本降低大量小对象 GC 扫描成本的用途。

该实现不负责 KV 编码、迭代或落盘，也不在 `Drop` 中自动归还资源；调用者必须按生命周期显式调用 `Buffer::Destroy`，最终需要释放池缓存时再调用 `Pool::Destroy`。

## 主要符号

- `defaultPoolSize = 1024`、`defaultBlockSize = 1 << 20`：默认缓存块数和 1 MiB 默认块大小。
- `Allocator: Send + Sync`：可替换的块分配接口，包含 `Alloc(usize) -> Vec<u8>` 与 `Free(Vec<u8>)`。`stdAllocator` 以清零 `Vec` 分配，释放通过 `drop` 完成。
- `Pool`：持有 `Arc<dyn Allocator>`、`blockSize`、`Mutex<VecDeque<Vec<u8>>>`、缓存容量和可选 `Arc<Limiter>`。`WithBlockNum`、`WithBlockSize`、`WithAllocator`、`WithPoolMemoryLimiter` 是按传入顺序应用的一次性 functional options；`NewPool` 返回 `Arc<Pool>`。
- `Pool::{acquire,takeBlock,release}`：分别负责“先占配额再取块”、从 FIFO 缓存或分配器取块、向缓存归还或释放溢出块并归还配额。`Pool::{Destroy,TotalSize,NewBuffer}` 是公开生命周期与构造 API。
- `Buffer`：持有所属 `Arc<Pool>`、全部借出块、可选块数上限、当前块下标与偏移，以及小对象元数据的累计配额和本地余额。
- `WithBufferMemoryLimit`、`GetAlignedSize`、`getBlockCnt`：通过 `ceil(size / blockSize)` 把字节数转换为块数；计算沿用 Go 对 `blockSize != 0` 的调用前提。
- `AllocatedBytes::{Owned,Borrowed}`：统一表达独立大对象和固定块内借用切片。该类型的生命周期使 Rust 调用者不能在可变再次借用 `Buffer` 时继续使用旧的 `Borrowed` 值。
- `SliceLocation { bufIdx, offset, Length }`：紧凑位置句柄，仅 `Length` 公开；位置只能交回产生它且尚未失效的同一 `Buffer`。
- `Buffer::{AllocBytes,TryAllocBytes,AllocBytesWithSliceLocation,AddBytes,TryAddBytes,GetSlice}`：公开分配、复制与位置恢复 API。
- `Buffer::{allocateLocation,tryAllocLocation,addBlock,addBlockWithReservedLimiterQuota,switchToNextBlock,appendBlock}`：块内占位、非阻塞预留配额和块切换的内部主链。
- `smallObjOverheadBatch = 256 KiB`、`sizeOfSlice`、`sizeOfSliceLocation`：用于批量登记返回切片或位置句柄所需的元数据开销。

本文件唯一条件编译项是末尾的 `#[cfg(test)] #[path = "buffer_test.rs"] mod buffer_test;`，测试逻辑保存在独立文件，未与生产实现混写。

## 执行流程

创建阶段：`NewPool` 先建立默认分配器、块大小和 FIFO 缓存，再依次执行 `Option`；`Pool::NewBuffer` 初始化未分配状态（`curBlockIdx = -1`、无块数限制），应用 `BufferOption`，未预设容量时为块索引预留 128 项。

阻塞小对象分配由 `Buffer::AllocBytes` 进入 `allocateLocation`。若当前块剩余空间不足，它先检查 `blockCntLimit`，再调用 `addBlock`：Reset 后仍有后续已持有块时，`switchToNextBlock` 直接复用；否则 `Pool::acquire` 先阻塞取得一个块大小的 Limiter 配额，再由 `takeBlock` 从缓存 FIFO 头取块或调用分配器。占位成功后移动 `curIdx`、生成 `SliceLocation`，存在 Limiter 时以 `sizeOfSlice` 扣减批量元数据预算，最后用 `GetSlice` 返回块内切片。

非阻塞路径由 `TryAllocBytes` 进入 `tryAllocLocation`。函数先只计算 `need_block`、块数上限以及本次需要的新块配额和 256 KiB 元数据批次；只有 `Limiter::TryAcquire` 成功后才修改块列表、当前指针和元数据账本。已预留新块配额时使用 `addBlockWithReservedLimiterQuota -> takeBlock`，避免第二次 Acquire。配额不能立即取得时返回 `ErrCannotAcquireMemory`，块数上限则返回 `Ok(None)`。

当 `n > blockSize` 时，`AllocBytes` 和 `TryAllocBytes` 都直接创建 `AllocatedBytes::Owned(vec![0; n])`，不经过 Pool、单 Buffer 块数上限或 Limiter；相反，`AllocBytesWithSliceLocation` 必须位于池块内，所以超大请求返回 `(None, SliceLocation::default())`。`AddBytes` 和 `TryAddBytes` 只是在相应分配结果上复制输入。

`Reset` 将写指针移回首块但保留全部固定块，后续跨块时逐个复用；它只释放小对象元数据预算，不释放固定块的 Limiter 配额。`Buffer::Destroy` 释放元数据预算，将全部块逐个交给 `Pool::release`，并恢复未分配状态。`Pool::Destroy` 则一次移出缓存，在未持有缓存锁时调用外部分配器释放各块。

## 数据与状态

`Pool::blockCache` 只保存当前未被 Buffer 借用的块；`Pool::TotalSize` 因而只计算 `cache.len() * blockSize`。`Buffer::blocks` 保存其仍占有的固定块，`Buffer::TotalSize` 只计算这些固定块，不计大于块大小的 `Owned` 分配，也不计元数据预算。

`curBlockIdx = -1` 表示尚无当前块；非负时它必须指向 `blocks` 内有效项。`curIdx` 是当前块中下一次分配的起始位置。分配只有在 `curIdx + n <= currentBlockLen()` 时在当前块完成，否则切换或追加整块。`blockCntLimit = -1` 表示无限制；非负值由 `WithBufferMemoryLimit` 向上对齐产生，所以实际上限为 `blockSize * ceil(limit / blockSize)`。

小对象元数据采用两级账本：`smallObjOverhead` 记录从 Limiter 取得且尚未整体归还的批次数量，`smallObjOverheadCache` 记录其中尚未被切片头或 `SliceLocation` 消耗的余额。余额不足时一次 Acquire 256 KiB；Reset/Destroy 一次 Release `smallObjOverhead` 并把两者清零。这是配额估算，不是 Rust 元数据实际内存的所有权容器。

固定块和独立大对象均以 `vec![0; n]` 清零创建；从缓存或 Reset 复用的块不会重新清零，调用者不能假设复用区域仍为零。`SliceLocation` 的下标、偏移、长度使用 `i32`，而实际索引时转为 `usize`；文件没有运行时来源校验，合法性依赖“来自当前 Buffer 且在 Reset/Destroy 后不再使用”的调用契约。

## 依赖与调用关系

内部调用边经 RustCodeGraph 核对：

- `AllocBytes -> allocateLocation -> {currentBlockLen, addBlock}`，随后 `recordSmallObjOverhead` 和 `GetSlice`。
- `TryAllocBytes -> tryAllocLocation -> {currentBlockLen, addBlockWithReservedLimiterQuota/addBlock}`，随后 `GetSlice`。
- `AllocBytesWithSliceLocation -> allocateLocation`，成功后调用 `recordSmallObjOverhead` 和 `GetSlice`。
- `addBlock -> switchToNextBlock`；无法复用时为 `Pool::acquire -> takeBlock -> Allocator::Alloc`，再 `appendBlock`。
- `addBlockWithReservedLimiterQuota` 与上条相同，但直接调用 `takeBlock`，因为配额已由 `TryAcquire` 原子预留。
- `Buffer::Destroy -> releaseSmallObjOverhead` 和 `Pool::release -> Allocator::Free`（仅缓存溢出）；`Pool::Destroy -> Allocator::Free`。
- `AddBytes -> AllocBytes`，`TryAddBytes -> TryAllocBytes`，两者再通过 `AllocatedBytes::as_mut_slice` 复制数据。

下游直接依赖为标准库的 `Arc`、`Mutex`、`VecDeque`，以及 `super::limiter::{Limiter, ErrCannotAcquireMemory}`。上游公开入口由 `lib.rs` 重导出。生产接线的明确路径是 `pkg/session/runtime/import_sst.rs -> membuf::NewPool -> IngestData::NewIter`；接口定义在 `pkg/ingestor/engineapi/ingest_data.rs`。当前仓库检索未发现非测试 Rust 代码直接调用本文件的 `Buffer` 分配方法，且已检查的 globalsort/ingestctrl `NewIter` 实现忽略 `Pool` 参数；因此不能把 Go 中的实际 KV 缓冲使用推断为当前 Rust 已完成接线。

## 错误处理与边界

- 阻塞路径没有显式错误返回：Limiter 不足会等待；块数达到 `blockCntLimit` 返回 `None`。
- 非阻塞路径用 `Err(String)` 返回固定文本 `ErrCannotAcquireMemory`，用 `Ok(None)` 表示单 Buffer 块数限制；大对象始终 `Ok(Some(Owned))`。
- `n > blockSize` 是大对象分界，`n == blockSize` 仍走池内固定块。
- 零长度请求在尚无块时会因 `currentBlockLen() == 0` 且无需新块而形成默认负位置，公开 API 返回 `None`；已有当前块后可返回长度为零的切片。`migration_aster_unit_test.rs` 明确锁定了这一 Go 对齐边界。
- `getBlockCnt`/`GetAlignedSize` 使用 wrapping 加乘法以贴近无检查整数运算，但 `blockSize == 0` 仍会除零；调用者必须保证块大小非零。极大尺寸还可能发生回绕，不能把结果当作安全的输入验证。
- `Mutex::lock().unwrap()`、位置越界、负位置转换、`curIdx + n` 溢出或分配失败均可能 panic；本文件不把这些编程错误或资源耗尽转换成业务错误。
- `GetSlice` 不验证 location 的来源和世代。跨 Buffer、Reset 后语义已改变、Destroy 后或伪造位置都不安全（Rust 内存安全仍由边界检查保护为 panic，但数据语义可能错误）。
- `Pool::Destroy` 不设置“已销毁”标志，与 Go 关闭 channel 后不能继续安全使用的细节不同；Rust 版本排空当前缓存后对象仍可被调用。正确生命周期仍应是先销毁所有 Buffer，再销毁 Pool，且不再使用 Pool。
- `Allocator::Free` 是外部实现；代码特意在释放溢出块和批量销毁时避免持有缓存锁调用它，以降低死锁和长临界区风险。

## 并发与资源生命周期

`Pool` 可在线程间共享：分配器要求 `Send + Sync`，缓存由 `Mutex<VecDeque<_>>` 保护，Limiter 自身也通过同步原语协调配额。缓存语义是 FIFO：`release` 在尾部 `push_back`，`takeBlock` 从头部 `pop_front`。锁只覆盖缓存状态变更，不覆盖底层 Alloc/Free 或 Limiter 等待。

`Buffer` 是顺序可变分配器，所有分配与生命周期 API 都需要 `&mut self`；它不是供多个线程同时写入的共享对象。返回的 `Borrowed(&mut [u8])` 受 Rust 借用规则约束，下一次可变调用前必须结束该借用。`SliceLocation` 则没有借用生命周期编码，调用者必须人工遵守位置有效期。

Limiter 统计的是“借给 Buffer 的固定块”，不是 Pool 缓存：`acquire` 借出时扣减，`release` 归还池时恢复，即使块仍驻留缓存。Reset 保留块，所以不释放这部分配额；Destroy 才释放。大对象明确绕过限制。元数据配额按 256 KiB 批量获取，因此首次很小的分配也可能因需要“一个块 + 一个元数据批次”而阻塞或使 Try 路径失败。

资源释放没有 RAII `Drop` 兜底。推荐顺序是：停止使用所有切片/位置，调用 `Buffer::Destroy` 归还块，再在所有 Buffer 都结束后调用 `Pool::Destroy` 释放缓存。若遗漏 Buffer::Destroy，Pool 的 Arc 仍由 Buffer 持有且块留在 Buffer 内；若只遗漏 Pool::Destroy，缓存 Vec 最终随 Pool Drop 释放，但自定义 Allocator 的 `Free` 回调不会按 API 约定被显式调用。

## 与 Go 版本的对应关系

直接对照 `pkg/lightning/membuf/buffer.go`，Rust 保留了默认值、functional options、块数向上对齐、Pool/Buffer 的 Reset/Destroy/TotalSize、阻塞与 Try 分配、256 KiB 元数据批次以及 SliceLocation 三字段模型。

主要实现映射如下：Go 的有界 `chan []byte` 对应 `Mutex<VecDeque<Vec<u8>>> + blockCacheCapacity`，仍为非阻塞取块和 FIFO 复用；Go 的 `curBlock []byte` 在 Rust 中由 `curBlockIdx` 每次索引 `blocks`，避免保存自引用可变切片；Go 统一返回 `[]byte`，Rust 用 `AllocatedBytes::Owned/Borrowed` 区分大对象所有权；Go 的 `nil` 对应 Rust `Option::None`，Go 的 `ErrCannotAcquireMemory` 对应 Rust 固定错误字符串。

Rust 的 `TryAllocBytes` 将 Go `tryAllocBytes` 的状态机拆为 `tryAllocLocation` 后再借用切片，关键不变量仍是 TryAcquire 失败前不修改块、指针或元数据账本。`buffer_test.rs::test_pool_mem_limit` 和 `migration_aster_unit_test.rs::migration_try_allocation_is_nonblocking_and_failure_is_atomic` 均验证这一点。

需要注意的差异：Go Pool::Destroy 关闭 channel，Rust 只排空 VecDeque；Go 切片可在 Buffer 后续方法调用期间被保存，Rust `Borrowed` 受可变借用约束；Go `GetSlice` 返回普通切片，Rust 返回绑定到本次 `&mut self` 借用的切片；Go 的运行时 GC 是 SliceLocation 优化的主要背景，而 Rust 没有扫描式 GC，但紧凑句柄仍减少元数据大小并保留跨语言布局意图。Rust `buffer_test.rs` 还显式验证 FIFO 块复用，而 Go channel 的 FIFO 行为是实现机制本身。

## 扩展指南

- 修改块缓存策略时，应集中改 `takeBlock`、`release`、`Destroy`，保持“缓存锁外调用 Alloc/Free”、FIFO 复用和 Limiter 在借出/归还边界记账；同步扩展 `buffer_test.rs::test_buffer_pool` 与 `test_pool_reuses_cached_blocks_in_fifo_order`。
- 修改分配算法或上限语义时，应改 `allocateLocation`、`tryAllocLocation`、`switchToNextBlock`，保证阻塞与非阻塞路径对块数限制和 Reset 复用的判断一致；同步 `test_buffer_mem_limit` 及 Go `TestBufferMemLimit`。
- 增加新的 Try API 时，所有可能失败的配额必须先汇总并一次 `TryAcquire`，成功后才修改状态；回归测试必须断言失败后的 `blocks`、`curBlockIdx`、`curIdx`、`smallObjOverhead`、`smallObjOverheadCache` 均未变化。
- 调整小对象元数据模型时，应同时检查 `sizeOfSlice`、`sizeOfSliceLocation`、`recordSmallObjOverhead`、`releaseSmallObjOverhead`，并评估首次小分配额外 256 KiB 配额对兼容性和吞吐的影响。
- 若要让 `SliceLocation` 跨 Reset 或跨 Buffer 使用，不能只放宽 `GetSlice`；必须引入 Buffer 身份/世代校验并处理 `i32` 范围，否则会读取已覆盖数据或 panic。
- 若把当前预留的 `IngestData::NewIter` Pool 参数真正用于 Rust KV 批处理，需要在使用方显式设计 Buffer Destroy/迭代器 Close/ReleaseBuf 的配对，且在独立测试文件中验证取消、错误和正常结束均归还配额。不要把测试写入 `buffer.rs`。
- 与 Go 行为对齐的修改应同时审阅 `buffer.go`、`buffer_test.go` 和 Rust 的 `buffer_test.rs`、`migration_aster_unit_test.rs`；若有意产生语义差异，应在 API 注释和对齐测试中明确记录。
- 性能敏感点包括缓存锁竞争、每块清零分配、256 KiB 配额批处理、块索引容量和 SliceLocation 重建；不要以通过测试为由把固定块路径简化为逐对象 Vec 分配。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 文件、307,296 节点、1,848,419 条边；`files --filter pkg/lightning/membuf` 确认 `buffer.rs` 有 44 个符号；`node --file pkg/lightning/membuf/buffer.rs --offset 1 --limit 500` 与尾段查询覆盖全部 507 行；精确 `query` 定位了 Rust/Go 的 `NewPool`、`NewBuffer`、`AllocBytes`、`TryAllocBytes`、`allocateLocation`。图查询核对了本文件内部主调用边，文件关系报告本文件被 6 个文件使用。由于 CLI 对带路径的 callers 查询出现同名扩展噪声，上游生产接线另以精确路径代码检索复核，未据噪声输出推断调用者。
- 源码与 crate 边界：`pkg/lightning/membuf/buffer.rs`、`pkg/lightning/membuf/lib.rs`、`pkg/lightning/membuf/Cargo.toml`、根 `Cargo.toml` 与 `pkg/lib.rs`。
- 相邻依赖：`pkg/lightning/membuf/limiter.rs`，确认 Acquire/TryAcquire/Release 的阻塞、FIFO 和错误常量语义。
- Go 对照：`pkg/lightning/membuf/buffer.go`、`pkg/lightning/membuf/buffer_test.go`；Bazel 目标由 `pkg/lightning/membuf/BUILD.bazel` 核对。
- Rust 独立测试：`pkg/lightning/membuf/buffer_test.rs` 覆盖块复用与溢出释放、FIFO、Limiter 阻塞与 Try 失败原子性、位置隔离、块数上限、对齐和并发获取；`pkg/lightning/membuf/migration_aster_unit_test.rs` 补充 Go 对齐的零长分配、Reset 覆盖位置、非阻塞失败与 FIFO Limiter 行为。
- 应用接线：`pkg/session/runtime/import_sst.rs`、`pkg/ingestor/engineapi/ingest_data.rs`、`pkg/ingestor/globalsort/engine_api.rs`、`pkg/ingestor/ingestctrl/import_pipeline.rs`、`pkg/ingestor/ingestctrl/local.rs` 及相关 Cargo manifests。
- 本任务是纯文档分析，按计划未运行 Cargo。结构验收另以任务指定命令确认目标文档存在且恰好具有上述 11 个固定二级标题。
