# [`pkg/meta/autoid/memid.rs`](memid.rs)

## 文件定位

本文件属于 `astersql-meta-autoid` crate；crate 入口 `pkg/meta/autoid/lib.rs` 以 `pub mod memid` 声明该模块，并通过 `pub use memid::*` 重新导出公开 API。`pkg/meta/autoid/Cargo.toml` 将 `lib.rs` 设为库入口，运行时依赖只有 `thiserror`；本文件自身只使用标准库同步原语，以及同 crate 的 `autoid`、`errors` 模块。

它实现不访问 `IdStore`、不预留持久化区间的纯内存 AutoID 分配器。Go 版本在临时表创建路径 `pkg/table/temptable/ddl.go:newTemporaryTableFromTableInfo` 和临时表构造路径 `pkg/table/tables/tables.go` 中调用 `NewAllocatorFromTempTblInfo`。截至本次检索，Rust 的 `new_allocator_from_temp_table_info` 仅被 Rust 测试直接引用，尚未发现 Rust 生产调用者；因此“服务临时表”是与 Go 对齐的职责和预期接入点，而 Rust 当前已经实现并导出分配器能力、但生产接线尚未由调用边证实。

## 核心职责

- `new_allocator_from_temp_table_info` 从精简的 `TableInfo` 判断临时表是否需要隐式 `_tidb_rowid` 或自增 ID；两者都不需要时返回 `None`。临时表不在这里创建 `AUTO_RANDOM` 或 `SEQUENCE` 分配器。
- `InMemoryAllocator` 实现 `Allocator` trait，在进程内维护一个水位 `base`，按 `(min, max]` 契约返回新分配区间。与 `DefaultAllocator` 不同，它没有远端元数据、事务和本地缓存区间的交互。
- `alloc_signed` 与 `alloc_unsigned` 分别处理有符号和无符号水位、步长/偏移对齐以及 ID 空间耗尽检查。
- `rebase`、`force_rebase`、`base`、`next_global_auto_id` 等 trait 方法提供水位管理；序列缓存相关操作明确返回未实现错误。

## 主要符号

- `pub fn new_allocator_from_temp_table_info(table: &TableInfo) -> Option<Arc<dyn Allocator>>`：公开工厂。`has_row_id = !pk_is_handle && !is_common_handle`；若既无隐式 row ID 又无自增列则返回 `None`。创建的分配器固定使用 `AllocatorType::RowId`，有无符号语义取自 `auto_increment_unsigned`。若 `auto_increment_id > 1`，先以 `auto_increment_id - 1` 调用 `rebase`，使下一 ID 恰为元数据种子；虽然当前内存实现的 `rebase` 总是成功，工厂仍保留失败时返回 `None` 的接口分支。
- `pub struct InMemoryAllocator`：公开实现类型，包含 `Mutex<InMemoryState>`、`is_unsigned` 和 `allocator_type`。它满足 `Allocator: Send + Sync`，可装入 `Arc<dyn Allocator>`。
- `struct InMemoryState { base: i64 }`：私有可变状态。即使走无符号路径也以 `i64` 保存原始位模式，并在比较和运算时转换为 `u64`。
- `InMemoryAllocator::new(is_unsigned, allocator_type)`：从 `base = 0` 创建分配器；调用者可以显式创建 `RowId`、`AutoIncrement` 等类型，但临时表工厂只创建 `RowId`。
- `alloc_signed` / `alloc_unsigned`：私有分配核心。二者先按 `offset` 必要时抬高水位，再调用 `calc_needed_batch_size` 计算为了产生 `n` 个符合 `id ≡ offset (mod increment)` 的 ID 所需前进量，成功后返回旧水位和新水位。
- `Allocator::alloc`：`n == 0` 时直接返回 `(0, 0)`；`RowId` 或 `AutoIncrement` 类型先用 `valid_increment_and_offset` 校验参数，再加锁并分派至有符号或无符号路径。
- `Allocator::rebase`：只允许水位按对应的有符号/无符号顺序上调，忽略 `allocate_ids`；`force_rebase` 则无条件覆盖水位。
- `Allocator::alloc_seq_cache` / `rebase_seq`：返回 `AutoIdError::NotImplemented`，体现临时表分配器不支持序列。
- `Allocator::transfer`：空操作并返回成功，因为没有绑定在存储中的数据库/表身份需要迁移。
- `base` 返回当前水位；`end` 固定为 `0`，因为没有预留缓存区间；`next_global_auto_id` 按有符号或无符号位语义返回 `base + 1`；`get_type` 返回构造时保存的类型。

## 执行流程

1. 临时表元信息进入 `new_allocator_from_temp_table_info`。工厂根据主键 handle 形态和自增列存在性判断是否需要分配器；不需要时立即返回 `None`。
2. 需要分配器时，以 `base = 0`、表的 unsigned 标记和 `RowId` 类型创建 `InMemoryAllocator`。若元数据带 `auto_increment_id > 1`，水位先提升到种子减一。
3. 调用方通过 `Allocator::alloc(ctx, n, increment, offset)` 请求 ID。该实现特意不检查 `ctx`，与 Go 的 `_ context.Context` 行为一致；取消的上下文仍可完成本地分配。
4. 空批次返回 `(0, 0)`。对 `RowId`/`AutoIncrement`，非法 increment 或 offset 在修改状态前返回 `InvalidIncrementAndOffset`。
5. 分配器取得状态互斥锁。有符号路径以 `i64` 比较 offset 和水位；无符号路径将两者解释为 `u64`。随后 `calc_needed_batch_size` 计算水位前进量。
6. 若前进会到达或越过对应整数空间上界（实现使用 `MAX - base <= needed`），返回 `AutoIncrementReadFailed` 且不提交新的水位；否则把 `base` 推进并返回 `(minimum, new_base)`。按照 trait 契约，可用 ID 位于这个半开区间 `(minimum, new_base]` 中并满足步长/偏移规则。
7. `rebase` 与普通分配使用同一把锁，较小的目标不会使水位后退；只有显式 `force_rebase` 能降低或任意覆盖水位。

## 数据与状态

唯一运行时可变数据是 `InMemoryState::base`。不存在独立 `end`、step 自适应值、数据库 ID、表 ID、持久化键或指标状态；因此 `end()` 的 `0` 不是当前可用区间末端，只是接口占位值。

`base` 的语义是“已经分配到的水位”，初值为 `0`。工厂将 `auto_increment_id` 解释为下一待分配值，所以写入 `auto_increment_id - 1`。有符号模式按 `i64` 排序，无符号模式把相同的 64 位内容转成 `u64` 后比较和加法；当数值超过 `i64::MAX` 时，对外的 `i64` 可能表现为负数，调用方必须结合 unsigned 属性解释其位模式。

`is_unsigned` 和 `allocator_type` 在构造后不可变。`allocator_type` 还决定是否执行 increment/offset 合法性检查；当前工厂固定为 `RowId`，所以工厂创建的实例总会执行该校验。

## 依赖与调用关系

上游边界：

- Rust 模块入口：`pkg/meta/autoid/lib.rs` 声明并重新导出本模块。
- Rust 直接调用证据：`pkg/meta/autoid/memid_test.rs` 调用工厂并直接构造 `InMemoryAllocator`；`pkg/meta/autoid/autoid_service_1_aster_unit_test.rs` 也覆盖工厂及不支持序列的行为。代码检索未找到 Rust 生产文件调用该工厂。
- Go 完整应用入口：`pkg/table/temptable/ddl.go:newTemporaryTableFromTableInfo` 与 `pkg/table/tables/tables.go` 调用 Go 的对应工厂，说明其在完整 TiDB 临时表生命周期中的位置。

下游边界：

- `crate::autoid::Allocator` 定义 `(min, max]` 分配契约及水位管理接口；`AllocatorType`、`Context`、`TableInfo` 提供类型和输入模型。
- `valid_increment_and_offset` 校验 MySQL 风格 increment/offset；`calc_needed_batch_size` 负责步长和偏移对齐，本文件不重复实现该算法。
- `crate::errors::{invalid_increment_and_offset, autoinc_read_failed}` 构造稳定的错误变体；序列方法直接构造 `AutoIdError::NotImplemented`。
- `Arc<dyn Allocator>` 提供共享 trait object 所有权，`Mutex` 串行化所有读取和修改水位的操作。

## 错误处理与边界

- `n == 0` 是特殊成功路径，返回 `(0, 0)`，不会读取或修改当前水位。
- `RowId`/`AutoIncrement` 的 increment 或 offset 超出 `autoid.rs` 定义的合法范围时返回 `InvalidIncrementAndOffset`；状态仍保持不变。
- 有符号空间和无符号空间耗尽分别返回带有 `signed auto ID exhausted` 或 `unsigned auto ID exhausted` 说明的 `AutoIncrementReadFailed`。边界判断使用 `<=`，因此不会把整数最大值作为成功后的新水位。
- `rebase` 不会回退水位；无符号分配器的比较必须按 `u64` 解释。`force_rebase` 没有单调性保护，调用方必须自行保证不会导致重复 ID。
- `alloc_seq_cache` 和 `rebase_seq` 必然返回 `NotImplemented`；临时表元信息不能借此获得 SEQUENCE 能力。`transfer` 则是无副作用成功。
- `alloc` 把 `u64 n` 转为 `i64` 传给批大小计算；极端的 `n > i64::MAX` 不在现有独立测试覆盖范围内，扩展或暴露新入口时应验证转换及 wrapping 算术是否仍符合 Go 语义。
- 所有锁获取均使用 `Mutex::lock().unwrap()`；若持锁线程 panic 导致锁中毒，后续调用会 panic，而不是返回 `AutoIdError`。当前函数在持锁区内没有显式 panic 分支，但这是 Rust 同步实现新增的运行时边界。
- 工厂把初始化 `rebase` 的任何错误折叠成 `None`，不保留错误细节；当前实现的该调用不会报错，但若以后改变 `rebase`，应重新评估工厂返回类型。

## 并发与资源生命周期

Go 注释称该分配器是 session-wide、通常不会并发访问；Rust 实现为了满足 `Allocator: Send + Sync`，用 `Mutex<InMemoryState>` 把 `alloc`、`rebase`、`force_rebase`、`base` 和 `next_global_auto_id` 串行化。因此共享同一个 `Arc<dyn Allocator>` 时，单次水位变更是互斥的，不会发放重叠区间。

锁只覆盖一次状态读取或修改，没有跨存储 I/O、网络等待、异步任务、通道或后台线程；资源生命周期完全跟随 `Arc`。最后一个 `Arc` 释放后，内存水位随对象销毁，不执行持久化或关闭动作。进程重启、会话/临时表对象销毁后不能依赖该水位恢复。

上下文取消不会影响生命周期：`alloc` 和 `rebase` 的 `_ctx` 参数被明确忽略，`pkg/meta/autoid/memid_test.rs:in_memory_allocator_ignores_canceled_context_like_go` 验证取消后仍可 rebase 和分配。

## 与 Go 版本的对应关系

Rust 文件逐项移植 `pkg/meta/autoid/memid.go`：工厂的建分配器条件、`AutoIncID - 1` 种子处理、`Alloc` 的空批次与参数校验、signed/unsigned 分支、溢出条件、单调 `Rebase`、无条件 `ForceRebase`、固定 `End = 0`、`NextGlobalAutoID`、空操作 `Transfer`，以及两个序列方法未实现的行为均保持一致。

主要表示差异如下：

- Go 结构体直接保存 `base int64`，依赖“session-wide 不并发访问”的生命周期约束；Rust 用 `Mutex<InMemoryState>` 显式提供线程安全，并增加锁中毒可能导致 panic 的差异。
- Go 工厂依据真实 `model.TableInfo` 的列信息判断自增列和 unsigned 标志；Rust 使用 `autoid.rs::TableInfo` 中预先抽取的布尔字段。
- Go 返回可空的 `Allocator` 接口值；Rust 返回 `Option<Arc<dyn Allocator>>`，把可空性和共享所有权显式化。
- Go 耗尽错误复用 `ErrAutoincReadFailed`；Rust使用同一错误类别 `AutoIdError::AutoIncrementReadFailed`，并附加 signed/unsigned 说明文本。
- Rust 当前缺少已证实的生产调用边；Go 的两个表层调用点仍是判断未来 Rust 临时表接线位置的直接依据。

`pkg/meta/autoid/memid_test.rs:test_in_memory_alloc` 对齐 Go 的 `pkg/meta/autoid/memid_test.go:TestInMemoryAlloc`，覆盖连续分配、批量、increment、offset、rebase、不回退、有符号/无符号耗尽和元数据种子。Rust 额外测试取消上下文被忽略，以锁定 Go 形参不使用的控制流语义。

## 扩展指南

- 若要把 Rust 临时表生产路径接入本实现，应在对应表构造边界调用 `new_allocator_from_temp_table_info`，并验证“不需要 row ID 且无自增列时为 `None`”及 `auto_increment_id` 种子语义；不要绕过工厂自行推导主键 handle 条件。
- 若修改分配算法，应同时核对 `alloc`、`alloc_signed`、`alloc_unsigned` 与共享的 `calc_needed_batch_size`，保持 `(min, max]`、offset 对齐、signed/unsigned 位解释和耗尽边界一致。相应回归应放在独立的 `pkg/meta/autoid/memid_test.rs`，并与 `memid_test.go` 保持同等覆盖，不要把测试内嵌进生产文件。
- 若新增 `AllocatorType` 行为，先决定是否应执行 increment/offset 校验；临时表仍不应被悄然赋予 `AUTO_RANDOM` 或 `SEQUENCE` 能力。若真正支持序列，必须成对实现 `alloc_seq_cache`、`rebase_seq` 及缓存/轮次状态，而不能仅把错误改成成功桩。
- 若改变并发模型，必须保留每次分配和 rebase 的原子性，评估 `Mutex` 中毒处理及高并发锁竞争；Go 的“通常无并发”假设不能替代 Rust `Send + Sync` trait 契约。
- 若改变错误或工厂失败传播，需同步 `pkg/meta/autoid/errors.rs` 以及调用方 API；尤其应考虑把初始化 rebase 的错误从 `Option` 中可观察地传出。
- 性能上本实现每个查询/修改方法都获取同一互斥锁，但无 I/O；优化时优先以临时表会话实际访问模式和竞争测量为依据，不要用无锁读破坏水位一致性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/meta/autoid` 确认目标、模块、Go 对照和独立测试均已索引；`node --file pkg/meta/autoid/memid.rs` 核对了 194 行完整实现；针对 `new_allocator_from_temp_table_info`、`alloc`、`alloc_signed`、`alloc_unsigned`、`rebase` 等符号的 `explore/query/callers/callees` 结果用于核对内部调用和直接引用。图结果显示工厂进入 `InMemoryAllocator::new` 与 `rebase`，`alloc` 分派到 signed/unsigned 核心，并只发现 Rust 测试直接调用工厂。
- Rust 源与 crate 契约：`pkg/meta/autoid/autoid.rs` 的 `Context`、`AllocatorType`、`Allocator`、`TableInfo`、`valid_increment_and_offset`、`calc_needed_batch_size`；`pkg/meta/autoid/errors.rs` 的 `AutoIdError` 与错误构造器；`pkg/meta/autoid/lib.rs` 的模块声明/再导出；`pkg/meta/autoid/Cargo.toml` 的 crate 边界和依赖。
- Go 对照与真实入口：`pkg/meta/autoid/memid.go`、`pkg/table/temptable/ddl.go`、`pkg/table/tables/tables.go`。
- 独立测试：`pkg/meta/autoid/memid_test.rs`、`pkg/meta/autoid/memid_test.go`；另以 `pkg/meta/autoid/autoid_service_1_aster_unit_test.rs` 中的内存分配器断言核对序列不支持边界。未运行 Cargo，符合本纯文档任务要求。
- 人工复核结论：该文件存在是为了在无需持久化 AutoID 水位的临时表场景提供 trait 兼容分配器；运行时通过单锁水位、共享批大小算法和显式 signed/unsigned 边界完成分配；安全扩展需要同步独立 Rust/Go 测试并保留 trait、溢出、单调 rebase 与不支持序列的契约。
