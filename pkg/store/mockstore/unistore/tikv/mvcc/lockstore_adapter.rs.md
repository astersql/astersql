# `pkg/store/mockstore/unistore/tikv/mvcc/lockstore_adapter.rs`

## 文件定位

本文件是 `astersql-store-mockstore-unistore-tikv-mvcc` crate 内的源码复用适配层，而不是一份新的锁存储实现。crate 入口 `pkg/store/mockstore/unistore/tikv/mvcc/lib.rs` 通过 `#[path = "lockstore_adapter.rs"] pub mod lockstore;` 将它公开为 `crate::lockstore`；本文件再用四个 `#[path]` 模块声明，把上两级目录 `pkg/store/mockstore/unistore/lockstore/` 中的 `arena.rs`、`iterator.rs`、`load_dump.rs` 和 `lockstore.rs` 编入当前 crate，并统一公开再导出。

因此，同一套锁存储源码既可由其原 crate 使用，也能在 MVCC crate 中以 `crate::lockstore::{MemStore, Hint, Iterator, ...}` 访问。这里的作用是解决 crate 边界和模块路径接线，真实数据结构与算法均位于上述四个被挂载文件中。

## 核心职责

1. 通过 `#[path = "../../lockstore/arena.rs"] mod arena;` 等四条声明，在 MVCC crate 的模块树下重建锁存储所需的内部模块关系。`iterator.rs` 和 `load_dump.rs` 使用 `super::lockstore::MemStore`，`lockstore.rs` 使用 `super::arena::*`，这些相对路径因本适配层的并列模块布局而成立。
2. 通过四条 `pub use ...::*` 将各模块的公开项提升到 `crate::lockstore`，让 MVCC 代码无需知道共享源码的物理目录或内部子模块划分。
3. 保持单一实现来源：本文件不复制 `MemStore` 算法，不增加包装调用，也不保存运行时状态；MVCC 看到的行为就是被挂载 Rust 文件当前实现的行为。

该文件不负责创建全局实例、不执行初始化、不实现事务语义。锁存储实例由调用方显式构造，例如 `migration_aster_unit_test.rs::db_snapshot_creates_read_view_and_shares_lock_store` 调用 `lockstore::MemStore::NewMemStore(256)`；MVCC 数据库组合则由 `db_writer.rs::DBBundle` 持有 `Arc<MemStore>`。

## 主要符号

- 私有模块 `arena`：来自 `../../lockstore/arena.rs`。其公开项经通配再导出，包括地址类型 `arenaAddr`、定位器 `arena`、块类型 `arenaBlock`、`newArenaLocator`、`newArenaAddr`、`nullArenaAddr` 和 `reuseSafeDuration`。这些名字虽然在 `crate::lockstore` 可见，但部分采用 Go 风格小写命名，主要服务于移植代码和测试。
- 私有模块 `iterator`：来自 `../../lockstore/iterator.rs`。公开核心类型是借用 `MemStore` 的 `Iterator<'a>`；`MemStore::NewIterator` 以及 `Valid`、`Key`、`Value`、`Next`、`Prev`、`Seek`、`SeekForPrev`、`SeekForExclusivePrev`、`SeekToFirst`、`SeekToLast` 由该文件的 `impl` 提供。
- 私有模块 `load_dump`：来自 `../../lockstore/load_dump.rs`。它为 `MemStore` 增加 `LoadFromFile` 与 `DumpToFile`，并公开小端辅助类型/实例 `littleEndian`、`endian`。
- 私有模块 `lockstore`：来自 `../../lockstore/lockstore.rs`。核心公开类型为 `MemStore`、`Hint`、`node`、`nodeHeader` 和 `entry`，并公开 `maxHeight`、`nodeHeaderSize`。主要入口包括 `MemStore::NewMemStore`、`Get`、`Put`、`PutWithHint`、`Delete`、`DeleteWithHint`、`Len` 与 `MaxEntrySize`。
- 四条 `pub use` 是本文件唯一的公开接口定义。它们不是调用边，而是编译期名称重导出；本文件本身没有函数、trait、常量、条件编译项或运行时代码。

## 执行流程

编译期流程如下：

1. `mvcc/lib.rs` 将 `lockstore_adapter.rs` 命名为公开模块 `lockstore`。
2. 编译器按本文件的四条 `#[path]` 读取共享的锁存储源码，并把它们组织成 `crate::lockstore::{arena, iterator, load_dump, lockstore}` 四个私有子模块。
3. 子模块之间按 `super` 路径解析依赖：`lockstore` 使用 arena 分配器，`iterator` 和 `load_dump` 扩展 `lockstore::MemStore`。
4. `pub use` 将所有公开项汇聚到 `crate::lockstore`。上游可直接导入 `crate::lockstore::MemStore`，不需要访问私有子模块。

典型运行时链路由被挂载实现提供：`MemStore::NewMemStore` 创建 arena、随机高度源和最大层高的头节点；`Put`/`PutWithHint` 在跳表中定位 splice、分配或替换 arena 节点并发布 next 地址；`Get` 和 `Iterator` 按有序 next 链读取；`Delete`/`DeleteWithHint` 断链并把节点所在块交给延迟复用机制。`DumpToFile` 以迭代器顺序写入“meta + key/value 对”，而 `LoadFromFile` 按相同长度前缀格式恢复条目。

在 MVCC 主链中，`db_writer.rs` 通过 `use crate::lockstore::MemStore` 把它放入 `DBBundle<D>::LockStore` 和 `DBSnapshot<S>::LockStore`。`NewDBSnapshot` 克隆 `Arc`，使新快照与数据库束观察同一锁存储实例；适配层不介入快照创建逻辑。

## 数据与状态

本文件自身没有数据或状态。经它暴露的主要状态位于共享实现中：

- `MemStore` 是按 key 排序的 arena-backed 跳表，保存原子高度、头节点指针、当前 arena 指针、等待析构的旧 arena 定位器、随机源和逻辑条目数。
- `nodeHeader` 记录 arena 地址、层高、key 长度和 value 长度；节点的变长布局依次包含 next 地址数组、key 和 value。`arenaAddr` 的高 32 位编码块下标加一，低 32 位编码块内偏移，零值作为空地址。
- `Hint` 缓存每层的前驱/后继指针，供连续写入或删除减少重复搜索；失效时 `calculateRecomputeHeight` 会重算必要层级。
- `Iterator<'a>` 借用 `MemStore`，但把当前位置的 key/value 复制到自己的 `Vec<u8>`，避免把 arena 内部切片长期泄露给调用方。空 key 同时表示未定位或越界，因此锁存储不能用空 key 表示有效迭代位置。
- `DBBundle`/`DBSnapshot` 通过 `Arc<MemStore>` 共享锁存储；`DBBundle` 另有 `MemStoreMu: Mutex<()>`，写入互斥策略属于调用层，而非本适配文件。

## 依赖与调用关系

- 上游模块声明：`pkg/store/mockstore/unistore/tikv/mvcc/lib.rs` 第 23–24 行把本文件公开为 `lockstore`。RustCodeGraph 的文件节点确认本文件只有 1 个图符号，且没有自身函数调用边；这符合纯模块适配器的角色。
- 当前 crate 内的直接生产消费者：`pkg/store/mockstore/unistore/tikv/mvcc/db_writer.rs` 导入 `crate::lockstore::MemStore`，用于 `DBBundle`、`DBSnapshot` 和 `NewDBSnapshot`。
- 当前 crate 内的直接测试消费者：`pkg/store/mockstore/unistore/tikv/mvcc/migration_aster_unit_test.rs` 导入 `crate::{codec, lockstore}`，通过适配层构造 `MemStore` 并验证两个快照共享同一 `Arc`。
- 下游源码依赖：`lockstore.rs -> arena.rs`，`iterator.rs -> lockstore.rs`，`load_dump.rs -> lockstore.rs + iterator.rs`。`DumpToFile` 会调用 `NewIterator`，迭代并序列化所有条目。
- crate 边界：`pkg/store/mockstore/unistore/tikv/mvcc/Cargo.toml` 将库入口设为 `lib.rs`，包名为 `astersql-store-mockstore-unistore-tikv-mvcc`。被挂载实现使用的非标准库直接依赖主要是 `rand`；MVCC 其余代码还声明 `anyhow`、`astersql-errors`、`kvproto`、`protobuf` 与 `thiserror`。Cargo 没有 feature 条件控制本适配层。
- 物理共享不等于类型共享：同一 `.rs` 文件若同时编入不同 crate，会生成各自 crate 下的 Rust 类型；MVCC 必须使用本 crate 的 `crate::lockstore::MemStore`，不能假设它与独立 lockstore crate 的 `MemStore` 可直接互换。

## 错误处理与边界

适配层没有返回值，也没有自己的错误路径；模块路径错误、共享源码依赖缺失或公开项冲突会在编译期失败。运行时边界来自被挂载实现：

- `Get` 未命中返回 `None`；`PutWithHint` 对新键返回 `true`、替换既有键返回 `false`；`DeleteWithHint` 未命中返回 `false`。
- arena 地址越界和某些无效内部状态通过 `panic!` 或不安全指针前提暴露，不转换为可恢复错误。条目若大于 `MaxEntrySize`，实现注释仅说明“很可能失败”；调用方应在写入前约束 key/value 总大小。
- `LoadFromFile` 对文件不存在返回 `Ok(None)`；当前 Rust 移植为了对应 Go 的命名返回值加 `defer Close` 行为，会把读取/截断错误折叠为 `Ok(None)`。这由 `lockstore/load_dump_test.rs` 和 `migration_aster_unit_test.rs::migration_load_ignores_truncated_items_like_go` 固化，扩展时不能误当成严格损坏检测。
- `DumpToFile` 以 `0600` 创建 `<目标>.tmp`，依次 flush、`sync_all`、关闭后 rename；任何 I/O 或 rename 错误以 `io::Result` 传播。失败路径可能留下临时文件，适配层没有清理策略。
- 迭代器以空 key 判定无效；若未来需要存储空 key，必须同时重设计 `Iterator::Valid` 与空 entry 表示法。

## 并发与资源生命周期

共享实现声明的契约是“单写多读”。`MemStore` 的结构发布点使用顺序一致原子操作：高度、arena 指针和节点 next 地址均原子读写；Rust 写方法要求 `&mut MemStore`，调用层通常还应提供写互斥。`unsafe impl Send/Sync` 建立在“只有一个写者、读者只读取已发布节点范围”的前提上，不能由适配层自动保证。

arena 块在节点删除后不会立即复写。当块引用计数归零且曾写满时，它进入 `pendingBlocks`，等待 `reuseSafeDuration`（100 ms）后才重新进入可写队列，以降低无锁读者读取已删除节点时遇到覆盖数据的概率。arena 增长时发布新的定位器，旧定位器记录在 `retiredArenas`，直到 `MemStore::drop` 才回收；析构还负责释放当前原始 arena 指针。

`Iterator<'a>` 的生命周期不超过其所借用的 `MemStore`，当前位置数据为复制值。`DBBundle` 与多个 `DBSnapshot` 通过 `Arc` 延长同一个 `MemStore` 的生命周期；`NewDBSnapshot` 只克隆引用，不复制锁内容。文件转储在返回前完成缓冲刷新、磁盘同步和文件关闭，随后以 rename 发布目标文件。

## 与 Go 版本的对应关系

仓库中没有与 `lockstore_adapter.rs` 同名的 Go 适配文件，因为 Go 包可直接导入 `pkg/store/mockstore/unistore/lockstore`；本文件是 Rust crate 拆分后为复用该包移植源码而新增的接线层。实际语义逐文件对应：

- `arena.rs` 对应 `arena.go`：地址编码、8 字节对齐分配、块增长与 100 ms 延迟复用一致。Rust 用 `Arc<arenaBlock>`、`Cell/UnsafeCell` 和显式 `Send/Sync` 表达 Go 指针共享及单写假设。
- `iterator.rs` 对应 `iterator.go`：定位规则一致，Rust 返回借用切片并在内部 `Vec` 中缓存副本；Go 使用复用切片。
- `load_dump.rs` 对应 `load_dump.go`：文件格式和临时文件 rename 流程一致。Rust 使用 `Option<Vec<u8>>` 表示 Go 的 `nil`，并保留 Go 中成功 Close 覆盖先前读取错误的可观察行为。
- `lockstore.rs` 对应 `lockstore.go`：均为单写多读、可复用 arena 的跳表。Rust 用 `Box`/原始指针/原子类型实现 Go 的指针布局与原子发布，并以 `Drop` 集中回收当前及退休 arena 定位器。

`lockstore/lockstore_test.go` 与 `lockstore/lockstore_test.rs` 保持 CRUD、替换、hint、迭代和并发工作负载对应；Go 因 #26235 跳过不稳定迭代器测试，Rust 对应测试也标记 `#[ignore]`。额外的 Rust `lockstore/migration_aster_unit_test.rs` 用较小、确定性的用例覆盖 arena、边界定位、dump/load 和单写多读语义。

## 扩展指南

- 若新增锁存储行为，应修改 `pkg/store/mockstore/unistore/lockstore/` 下对应的独立 Rust 实现文件，而不是在适配层复制一份逻辑。公开方法只要定义为 `pub`，通常会被现有通配再导出自动暴露；若新建第五个源码模块，则需在本文件增加匹配的 `#[path] mod ...` 和明确的再导出。
- 修改模块布局前，必须同时验证独立 lockstore crate 与本 MVCC crate 的 `super::arena`/`super::lockstore` 相对路径；共享源码必须能在两棵模块树中成立。
- 扩展 `MemStore` CRUD/hint/arena 时同步更新独立测试 `lockstore/lockstore_test.rs` 或 `lockstore/migration_aster_unit_test.rs`，并对照同目录 Go 文件及 `lockstore_test.go`。不要把测试内嵌进本生产适配文件。
- 扩展迭代器时覆盖空表、首尾、相等/非相等 Seek、越界和空 key 约束；扩展持久化格式时覆盖旧文件兼容、截断项、临时文件失败与原子替换语义。
- 改动 `unsafe` 内存布局、原子发布或 arena 回收时，应重点评估节点对齐、旧定位器生命周期、读者悬空指针和 100 ms 复用窗口；这些属于正确性与数据损坏风险，而非单纯性能优化。
- 若 MVCC 需要不同于独立 lockstore crate 的行为，应先明确是否仍能共享源码。通过条件编译或适配 trait 隔离差异通常比在本文件静默覆盖符号更可审计。

## 验证依据

- 目标与模块入口：`pkg/store/mockstore/unistore/tikv/mvcc/lockstore_adapter.rs`、`pkg/store/mockstore/unistore/tikv/mvcc/lib.rs`。
- crate 声明：`pkg/store/mockstore/unistore/tikv/mvcc/Cargo.toml`，确认包名、`lib.rs` 入口、依赖与无 feature 条件。
- 被挂载 Rust 实现：`pkg/store/mockstore/unistore/lockstore/arena.rs`、`iterator.rs`、`load_dump.rs`、`lockstore.rs`。
- MVCC 直接入口与测试：`pkg/store/mockstore/unistore/tikv/mvcc/db_writer.rs`、`pkg/store/mockstore/unistore/tikv/mvcc/migration_aster_unit_test.rs`。
- 独立 Rust 测试：`pkg/store/mockstore/unistore/lockstore/lockstore_test.rs`、`load_dump_test.rs`、`migration_aster_unit_test.rs`；它们覆盖 CRUD、替换、hint、迭代边界、dump/load、截断文件与并发读取。
- Go 对照：`pkg/store/mockstore/unistore/lockstore/arena.go`、`iterator.go`、`load_dump.go`、`lockstore.go`、`lockstore_test.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点与 1,848,419 条边；`files --filter .../lockstore_adapter.rs` 与文件 `node` 显示本适配文件共 22 行、1 个符号且无自身调用者；`node` 核对了 `mvcc/lib.rs` 的模块挂载、四个共享实现及 `db_writer.rs` 的消费关系。对常见名 `MemStore`/`NewMemStore` 的 `query` 同时找到 Go 与 Rust 定义，确认对应位置；一次无文件限定的 `callers NewMemStore` 查询因同名图规模过大而中止，调用关系改由精确文件节点和仓库引用核验，未据此推断额外调用者。
- 本任务按计划仅做静态事实与文档结构验证，不运行 Cargo；运行时代码未发生变化。
