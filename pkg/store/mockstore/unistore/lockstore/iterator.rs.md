# `pkg/store/mockstore/unistore/lockstore/iterator.rs`

## 文件定位

本文件属于 `astersql-store-mockstore-unistore-lockstore` crate，crate 入口 `pkg/store/mockstore/unistore/lockstore/lib.rs` 通过 `pub mod iterator` 声明模块，并以 `pub use iterator::*` 再导出其公开项。`pkg/store/mockstore/unistore/lockstore/Cargo.toml` 将该 crate 对应到 Go 包 `pkg/store/mockstore/unistore/lockstore`；`tikv`、`server` 和可选的 `cophandler` crate 在各自 `Cargo.toml` 中依赖这个 lockstore crate。

它位于 mock UniStore 的锁存储层，不是通用 KV 迭代器：操作对象是 `lockstore.rs` 中 arena-backed 跳表 `MemStore`，职责是给该有序结构提供前向、反向和边界定位。当前 Rust 生产代码中，直接调用者是 `load_dump.rs::MemStore::DumpToFile`；迭代语义另由同 crate 的独立测试文件覆盖。不能仅凭依赖声明推断 `tikv`、`server` 或 `cophandler` 的 Rust 运行路径已经直接调用此迭代器。

## 核心职责

- `Iterator<'a>` 借用一个 `MemStore`，保存当前条目的 key/value 拷贝，而不向调用方暴露 arena 内部地址或切片。
- `MemStore::NewIterator` 创建一个尚未定位的迭代器；空 `key` 是无效位置哨兵，所以新迭代器的 `Valid()` 为 `false`。
- `Seek`、`SeekForPrev`、`SeekForExclusivePrev`、`SeekToFirst`、`SeekToLast` 将迭代器定位到有序集合中的指定边界。
- `Next` 和 `Prev` 分别移动到严格更大、严格更小的键。它们重新从跳表查找，不保存节点游标。
- `setKeyValue` 集中处理“找到节点或越界”两种结果，并复用 `Vec` 容量刷新当前位置。

这里的“有效”只由 `key` 是否为空决定（`Iterator::Valid`），因此空 key 无法表示为有效当前位置。这与 Go 实现完全相同，是调用约束而不是单独的错误返回。

## 主要符号

- `pub struct Iterator<'a>`：包含 `ls: &'a MemStore`、`key: Vec<u8>`、`val: Vec<u8>`。生命周期保证迭代器不会比被借用的 store 活得更久。三个字段当前均为 `pub`，但外部直接改写 `key` 会破坏 `Valid` 与当前位置的一致性，应通过方法使用。
- `MemStore::NewIterator(&self) -> Iterator<'_>`：仅保存 store 引用并创建两个空缓存，不自动定位。
- `Iterator::Valid(&self) -> bool`：以 `!self.key.is_empty()` 判断是否位于真实条目。
- `Iterator::Key` / `Iterator::Value`：返回迭代器自有缓存的只读切片；切片只在下一次需要可变借用迭代器的移动/定位操作前有效。
- `Iterator::Next` / `Iterator::Prev`：分别调用 `MemStore::findGreater(current_key, false)` 和 `MemStore::findLess(current_key, false)`，严格排除当前键。
- `Iterator::Seek`：调用 `findGreater(seek_key, true)`，得到第一个 `key >= seek_key` 的条目。
- `Iterator::SeekForPrev`：调用 `findLess(target, true)`，得到最后一个 `key <= target` 的条目。
- `Iterator::SeekForExclusivePrev`：调用 `findLess(target, false)`，得到最后一个 `key < target` 的条目；这是 Rust 文件相对当前 Go `iterator.go` 多出的显式 API，并由迁移测试验证。
- `Iterator::SeekToFirst` / `Iterator::SeekToLast`：前者取哨兵头节点第 0 层后继，后者调用 `MemStore::findLast`。
- 私有 `Iterator::setKeyValue(entry)`：清空并复用缓存容量，复制 `entry.key`，再经 `entry::getValue(MemStore::getArena())` 取得 value 并复制。空 `entry` 会产生空 key/value，从而使迭代器无效。

## 执行流程

1. 调用方从 `&MemStore` 调用 `NewIterator`，得到未定位状态；在读取键值前必须先调用任一 `Seek*` 方法。
2. 范围扫描通常调用 `Seek(start)`。`lockstore.rs::MemStore::findGreater` 从当前跳表最高层开始，按字节序比较 key：小于目标时向右移动，大于目标时降层；相等时由 `allowEqual` 决定立即返回还是继续寻找严格后继。
3. 反向定位调用 `SeekForPrev` 或 `SeekForExclusivePrev`。`MemStore::findLess` 同样自顶向下搜索，且明确保证不会返回头哨兵。
4. 首尾定位分别走 `getNext(head, 0)` 和 `findLast()`；所有定位结果最终进入 `setKeyValue`。
5. `setKeyValue` 把节点内容复制到迭代器缓存。若查找越过边界，`entry.node` 为空、`entry.key` 为空，`entry::getValue` 也返回空 `Vec`，于是 `Valid()` 变为 `false`。
6. `Next`/`Prev` 以当前缓存 key 作为新搜索边界并禁止相等，再重复上述查找和复制过程。典型生产链为 `DumpToFile -> NewIterator -> SeekToFirst -> Valid -> Key/Value -> Next`，按键序写出全部条目。

定位依赖跳表搜索，通常为 `O(log n)`，最坏可退化；每次成功定位还要复制 key/value，成本为 `O(key_len + value_len)`。由于 `Next`/`Prev` 不保存节点指针，它们也不是简单的 `O(1)` 链接步进。

## 数据与状态

迭代器只有两种逻辑状态：空 `key` 表示“初始未定位或已越界”，非空 `key` 表示有效当前位置。没有独立的状态枚举、错误字段或方向字段。`val` 不参与有效性判断；空 value 是合法值，只要 key 非空。

有序关系使用 Rust 字节切片的词典序，与 `lockstore.rs` 中 `findGreater`/`findLess` 的 `cmp` 一致。当前 key/value 是拥有所有权的 `Vec<u8>`，并非 arena 视图；`setKeyValue` 的 `clear + extend_from_slice` 会尽量复用已分配容量，但遇到更长条目仍可能扩容。`entry.key` 本身已由底层节点读取为 `Vec`，value 也由 `entry::getValue` 生成 `Vec`，因此一次定位包含中间拥有型数据和迭代器缓存复制。

底层 `MemStore` 状态由 `lockstore.rs` 管理：头节点 `head`、原子高度、原子 arena 指针和跳表 next 指针。迭代器不改变这些状态，也不持有 `entry` 或裸节点指针跨方法调用。

## 依赖与调用关系

下游仅直接依赖 `super::lockstore::{MemStore, entry}`：

- `NewIterator` 扩展 `MemStore` 的公开 API。
- `Next`、`Seek` 调用 `MemStore::findGreater`。
- `Prev`、`SeekForPrev`、`SeekForExclusivePrev` 调用 `MemStore::findLess`。
- `SeekToFirst` 读取 `MemStore::head` 并调用 `getNext`；`SeekToLast` 调用 `findLast`。
- `setKeyValue` 调用 `entry::getValue` 和 `MemStore::getArena`。

RustCodeGraph 对目标文件的索引显示 14 个符号；结合精确引用检索，Rust 侧生产调用边为 `load_dump.rs::DumpToFile -> MemStore::NewIterator`，测试调用边来自 `lockstore_test.rs::test_iterator`、`lockstore_test.rs::benchmark_mem_store_iterate` 和 `migration_aster_unit_test.rs::migration_iterator_boundaries_match_go`。Go 侧调用面更广，包括 `raw_handler.go`、`cophandler/closure_exec.go`、`tikv/mvcc.go` 与 `load_dump.go`，因此这些路径是迁移/接线时应核对的对照证据，而不能写成当前 Rust 已接线事实。

## 错误处理与边界

本文件所有 API 都不返回 `Result`，查找失败或越界统一编码为无效迭代器。调用方必须在读取和继续扫描前检查 `Valid`。`Key`/`Value` 在无效状态下仍会返回空切片而不会 panic，因此跳过 `Valid` 可能把“越界”误当作空键值。

已验证的边界包括：新迭代器无效；`Seek` 可命中相等键、跳到下一个更大键、从最小键之前定位到首项、从最大键之后变为无效；`SeekForPrev` 可命中相等键、回退到更小键、从最大键之后定位到尾项；连续 `Next` 越过尾项后无效；`SeekForExclusivePrev` 严格排除相等目标。`findLess` 在目标小于等于首项且不允许相等时返回空 entry。

潜在契约边界是空 key：真实条目若使用空 key，`Valid` 仍返回 `false`。此外，公开字段允许 crate 外调用者人为制造 `key`/`val` 不匹配状态；安全扩展时不应依赖直接字段写入。

## 并发与资源生命周期

`Iterator<'a>` 借用 `&'a MemStore`，自身不拥有或释放 store/arena，因而没有 `Close`。drop 迭代器只释放其两个缓存 `Vec`；arena 与旧 locator 的回收由 `MemStore::drop` 负责。缓存拷贝避免调用者长期持有 arena 内部切片，也避免 arena locator 更新后当前位置指向旧内存。

`MemStore` 的读路径通过原子高度、next 地址和 arena 指针支持单写多读设计，旧 arena locator 会保留到 store drop。但在安全 Rust 借用模型下，活跃迭代器持有共享 `&MemStore`，同一 store 不能同时取得执行 `Put`/`Delete` 所需的 `&mut MemStore`；若上层通过锁共享，则锁的持有策略仍由上层负责。迭代器方法需要 `&mut self`，所以同一个迭代器不能被多个线程同时推进。文件中没有任务、通道、文件句柄或显式同步原语。

## 与 Go 版本的对应关系

Rust `Iterator`、`NewIterator`、`Valid`、`Key`、`Value`、`Next`、`Prev`、`Seek`、`SeekForPrev`、`SeekToFirst`、`SeekToLast` 和私有 `setKeyValue` 逐项对应同目录 `iterator.go`。关键布尔参数也一致：前向 seek 允许相等，next 禁止相等；反向 seek 允许相等，prev 禁止相等。两端都以空 key 代表无效，并通过重用已有缓冲区复制 key/value。

表达方式上的差异是：Go 保存 `*MemStore` 并返回可变 `[]byte`，Rust 以生命周期约束的 `&MemStore` 保存借用，`Key`/`Value` 只返回 `&[u8]`；Go 的 `append(it.key[:0], ...)` 对应 Rust 的 `clear + extend_from_slice`。Rust 的 `entry::getValue` 返回拥有型 `Vec`，所以实现细节上比 Go 的 arena 切片多一层临时复制。

当前 Go `iterator.go` 没有 `SeekForExclusivePrev`，Rust 版本补充了这一严格小于定位方法；`migration_iterator_boundaries_match_go` 将其作为明确迁移边界验证，而原 Go `TestIterator` 与对应 Rust `test_iterator` 都因上游 #26235 标记为跳过/ignore。除这个附加 API 外，核心迭代逻辑保持 Go 语义。

## 扩展指南

- 新增定位语义时，优先复用 `findGreater`/`findLess` 的 `allowEqual` 契约，并让所有结果经过 `setKeyValue`，避免有效性哨兵和缓存更新分叉。
- 若要把 `Next`/`Prev` 优化成保存节点指针的常数时间步进，必须同时审查 arena grow、删除后的延迟复用、旧 locator 生命周期和并发读者安全；这不是本文件内的局部改动。
- 若要支持空 key，应先把有效性从“key 是否为空”改为独立状态，并同步 Go 兼容决策、所有 `Valid` 调用者和边界测试。
- 若收紧封装并将 `ls`/`key`/`val` 改为私有，需要先迁移 `load_dump.rs::DumpToFile` 对 `it.key`/`it.val` 的直接读取，改用 `Key`/`Value`。
- 行为变更应更新独立测试文件，不能把测试嵌入 `iterator.rs`：常规定位覆盖位于 `lockstore_test.rs`，迁移差异与 `SeekForExclusivePrev` 覆盖位于 `migration_aster_unit_test.rs`；同时核对 Go 的 `iterator.go` 与 `lockstore_test.go`。
- 性能改动应关注每步跳表查找和双重拥有型复制，并保留 `lockstore_test.rs::benchmark_mem_store_iterate` 对应的全表遍历工作负载。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/store/mockstore/unistore/lockstore` 确认 Rust/Go 源与独立测试；`node --file .../iterator.rs` 读取 14 个目标符号；对 `lockstore.rs`、`lockstore_test.rs`、`migration_aster_unit_test.rs` 和 `load_dump.rs` 的节点读取核验了查找算法、调用边、测试和生产使用。宽泛 `explore` 未可靠消歧 `Iterator`/`NewIterator`，因此没有把其噪声结果作为调用关系结论。
- Rust 源：`pkg/store/mockstore/unistore/lockstore/iterator.rs`；直接依赖实现 `lockstore.rs` 中的 `entry::getValue`、`MemStore::getNext`、`findGreater`、`findLess`、`getArena` 以及 arena/原子生命周期代码。
- crate 边界：`pkg/store/mockstore/unistore/lockstore/Cargo.toml`、`lib.rs`，以及 `tikv/Cargo.toml`、`server/Cargo.toml`、`cophandler/Cargo.toml` 的依赖声明。
- Rust 调用与测试：`load_dump.rs::DumpToFile`；`lockstore_test.rs::test_iterator`（`#[ignore]`）和 `benchmark_mem_store_iterate`；`migration_aster_unit_test.rs::migration_iterator_boundaries_match_go`（非 ignore，覆盖主要边界及严格前驱）。
- Go 对照：`pkg/store/mockstore/unistore/lockstore/iterator.go`、`lockstore_test.go::TestIterator`、`BenchmarkMemStoreIterate`，并通过引用检索确认 `raw_handler.go`、`cophandler/closure_exec.go`、`tikv/mvcc.go` 和 `load_dump.go` 的调用面。
- 本任务仅做静态文档分析，按计划不运行 Cargo；最终以固定 11 章节结构命令和人工事实复核作为交付验证。
