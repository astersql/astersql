# `pkg/meta/autoid/autoid.rs`

## 文件定位

[`autoid.rs`](./autoid.rs) 是 `astersql-meta-autoid` crate 的核心实现文件。crate 入口 `pkg/meta/autoid/lib.rs` 将本模块公开并重新导出其 API；`pkg/meta/autoid/Cargo.toml` 指明该 crate 对应 Go 包 `pkg/meta/autoid`，运行时依赖只有 `thiserror`，说明具体 KV 接入被刻意隔离在本文件定义的存储 trait 之外。

在完整应用中，生产接线点之一是 `pkg/session/runtime/create_table_resources.rs`：该文件用真实元数据事务实现 `IdTransaction`、`IdStore` 和 `Requirement`，把表模型转换为本文件的 `TableInfo`，调用 `new_allocators_from_table_info`，再用表中已有的 `AutoIncID`/`AutoRandID` 对所得分配器执行 `rebase`。因此本文件位于“表资源创建/恢复 → AutoID 分配器构造 → 元数据水位事务”链路中，负责策略与状态机，不直接依赖具体 KV 类型。

同一 crate 内，`autoid_service.rs` 实现可由 `Requirement::single_point_allocator` 提供的远程单点 `AUTO_INCREMENT` 分配器，`memid.rs` 复用这里的 `Allocator`、`AllocatorType` 和批大小算法实现纯内存版本，`errors.rs` 提供统一错误类型。`pkg/ddl/split_region.rs` 则直接使用 `ShardIdFormat` 计算 `AUTO_RANDOM` 分片边界。

## 核心职责

本文件同时承担六组紧密相关的职责：

1. 定义统一分配接口 `Allocator` 及分配器集合 `Allocators`，覆盖隐藏 `_tidb_rowid`、独立 `AUTO_INCREMENT`、`AUTO_RANDOM` 和 SQL `SEQUENCE`。
2. 以 `DefaultAllocator` 实现本地批缓存：持久化层只维护全局水位，本地用 `(base, end]` 缓存减少元数据事务次数。
3. 以 `IdTransaction`、`IdStore`、`Requirement` 抽象具体存储与远程单点服务，使分配算法可由 session 层和测试内存存储复用。
4. 处理 increment/offset、signed/unsigned、rebase、强制 rebase、库表身份迁移以及序列 cache/cycle 等边界语义。
5. 提供 `AUTO_RANDOM` 参数规范化与 `ShardIdFormat` 位布局工具，以及有符号整数的保序无符号编码。
6. 提供动态预留步长和轻量运行时统计。动态步长的目标是让一批缓存约在 `DEFAULT_CONSUME_TIME`（10 秒）内消耗，并限制在 `MIN_STEP` 30,000 与 `MAX_STEP` 2,000,000 之间。

关键不变量是：普通批量分配返回 `(min, max]`，真正可用值需从大于 `min` 的第一个满足 `(id - offset) % increment == 0` 的值开始；只有持久化事务成功后，`DefaultAllocator` 才更新本地 `base/end`。`autoid_test.rs::test_rollback_alloc` 明确验证失败事务不会推进本地状态。

## 主要符号

- 系统 ID 常量与规范化函数：`SYSTEM_SCHEMA_ID_FLAG`、三个系统 schema ID、`is_mem_schema_id`；`auto_random_shard_bits_normalize` 和 `auto_random_range_bits_normalize` 处理默认值、合法区间及错误文案。
- `Context`：用 `Arc<ContextInner>`、`AtomicBool`、`Mutex<()>` 和 `Condvar` 模拟 Go `context.Context` 的取消检查及可中断等待。分配器只在需要访问存储前检查它。
- `AllocatorType`：四类水位的公开枚举；`as_str`/`Display` 保持 Go 类型名字符串。
- `Allocator`：线程安全的对象接口。`alloc`、`alloc_seq_cache`、`rebase`、`force_rebase`、`rebase_seq`、`transfer` 是状态操作，`base`、`end`、`next_global_auto_id`、`get_type` 是查询。
- `Allocators`：一张表上的分配器集合。`get` 在 `separate_auto_increment == false` 时把 `AutoIncrement` 查询回退到 `RowId`，这是旧表共享水位兼容语义；`filter` 保留该标志并生成筛选后的集合。
- `AutoIdKeyKind`、`AutoIdKey`：把库 ID、表 ID 与水位类别组合成持久化键。独立自增键还携带 `table_version`。
- `IdTransaction`、`IdStore`：事务内 `get/put/inc/copy_to` 与事务执行边界。`DefaultAllocator` 不假定后端实现，但要求 `run_in_transaction` 的成功/失败具有原子提交语义。
- `SequenceInfo`、`TableInfo`、`AllocatorOption`：从上层表模型提炼的构造输入。`AllocatorOption::CustomStep` 和 `TableInfoVersion` 决定缓存步长、键版本及是否选择单点分配器。
- `AllocatorState`：受互斥锁保护的本地可变状态，包括 `(base, end]`、当前库表身份、最近一次补充缓存时间和 step。
- `DefaultAllocator`：默认持久化分配器。不可变字段保存 store、版本、unsigned 标记、是否为用户自定义 step、类型与可选 sequence 定义；所有水位状态集中在 `state: Mutex<AllocatorState>`。
- `new_allocator`：基础工厂。只有自定义 step 为 1、表版本至少为 5、类型为 `AutoIncrement` 时才请求 `Requirement::single_point_allocator`；否则返回 `DefaultAllocator`。
- `new_allocators_from_table_info`：表级工厂。根据 handle、自增是否分离、auto-random bits 和 sequence 信息决定挂载哪些分配器。
- 算法工具：`valid_increment_and_offset`、`next_step`、`calc_needed_batch_size`、`calc_sequence_batch_size`、`seek_to_first_sequence_value`、两个 `seek_to_first_auto_id_*`、`encode_int_to_cmp_uint`/`decode_cmp_uint_to_int`。
- `ShardIdFormat`：计算增量段位数、掩码、容量并把 shard 与增量 ID 合成为最终行 ID。
- `AllocatorRuntimeStats`：记录 alloc/rebase 次数和快照/提交统计文本；`Display` 只在计数非零时输出 `auto_id_allocator: {...}`。

## 执行流程

普通表分配器的构造流程如下：

1. 上层把表元数据转换为 `TableInfo`，调用 `new_allocators_from_table_info`。
2. 工厂先生成 `CustomStep(table.auto_id_cache)` 和 `TableInfoVersion(table.version)`。无主键 handle 的表需要 `RowId`；未分离的自增列与它共享该分配器；分离自增、`AUTO_RANDOM`、`SEQUENCE` 按各自条件追加独立分配器。
3. `new_allocator` 用 `DefaultAllocator::with_options` 初始化本地空区间。表版本至少为 5 时，`AUTO_ID_CACHE=1` 只允许独立 `AutoIncrement` 走单点服务；`RowId` 会恢复全局默认 step，避免隐藏 row ID 被错误退化为逐个远程分配。
4. session 接线层对表中已保存的基值执行 `rebase(..., alloc_ids=false)`，再把 `Allocators` 交给表对象。

普通 `DefaultAllocator::alloc` 的流程是：

1. 先拒绝 `table_id == 0`，`n == 0` 则直接返回 `(0, 0)`；`RowId`/`AutoIncrement` 还会校验 increment 与 offset 均在 `[1, 65535]`。
2. 锁住 `AllocatorState`，按 `is_unsigned` 进入 `alloc_signed_locked` 或 `alloc_unsigned_locked`。若 `offset - 1` 已超过本地 base，先 rebase 到该位置，保证后续同余序列有合法起点。
3. `calc_needed_batch_size` 找到第一个满足 offset/increment 的 ID，再加上后续 `n-1` 个间隔，得到 base 实际需前进的距离；先检查 signed/unsigned 空间耗尽。
4. 若本地 `end` 足够，仅推进 `base` 并返回旧、新边界。若不足，则先按上一批耗时调用 `next_step`（自定义 step 除外），随后在 `IdStore::run_in_transaction` 内重新读取全局水位、重新计算所需批大小，并以 `max(step, needed)` 原子增加全局水位。
5. 事务成功后才更新本地 `base/end`、动态 step 和 `last_alloc_time`；然后发放当前请求。并发的同一实例由 state 锁串行化，不同实例依赖后端事务的原子 `inc` 避免重叠区间。

`rebase_signed_locked`/`rebase_unsigned_locked` 有三条路径：目标不高于 base 时无操作；目标仍在本地缓存内时只抬高 base；超过 end 时进入事务。`allocate_ids=true` 会从 `max(全局水位, required_base)` 再预留一个 step，false 只把全局水位至少推进到目标。`force_rebase` 允许水位下调并清空本地剩余缓存，但拒绝 `required_base == -1`，因为其下一个全局 ID 会是 0。`transfer` 用事务 `copy_to` 复制旧键水位，成功后才更新本地库表身份。

序列走独立流程。`alloc_seq_cache` 根据 `SequenceInfo.cache` 选择 cache size，通过 `calc_sequence_batch_size` 计算正向或反向可用跨度；触及 min/max 时，非 cycle 序列返回耗尽错误，cycle 序列重置 value 键、增加 `SequenceCycle` 轮次，再计算新批次。`rebase_seq` 按 increment 方向判断全局水位是否已满足要求，已满足返回 `(0, true)`，否则推进并返回 `(required_base, false)`。

## 数据与状态

持久化状态由 `AutoIdKey` 寻址。`RowId`、`IncrementId(version)`、`RandomId`、`SequenceValue` 彼此隔离；序列循环次数另存于 `SequenceCycle`。本文件不规定物理编码，生产实现位于 `pkg/session/runtime/create_table_resources.rs::Txn`，由它把抽象键映射到真实 meta accessor。

本地缓存状态使用 `(base, end]`：`base` 是已消费水位，`end` 是已从全局预留的末端。预留一大段允许后续分配不访问存储，也意味着进程退出后未使用的尾部可以形成合法空洞；算法保证唯一性和单调推进，不承诺连续无缺口。signed 与 unsigned 共用 `i64` 存储表示，但无符号路径在比较、加减和上限判断时显式转换为 `u64`，以覆盖完整 64 位空间。

`DEFAULT_STEP` 是进程级 `AtomicI64`，`get_step`/`set_step` 使用 `SeqCst`；每个普通分配器在构造时复制当前值到自己的 state，之后动态独立调整。`Context` 的取消位同样使用 `SeqCst`。`AllocatorRuntimeStats` 本身不含锁，其可变方法需要调用方保证独占访问；当前文件也没有把它接入 `DefaultAllocator` 的事务路径。

`ShardIdFormat` 将 ID 划为可选符号位、shard 段和 incremental 段。`new` 在有符号类型上额外扣除一位；`incremental_mask` 对 64 位增量段返回全 1（以 `i64` 表示为 -1）；`compose` 只掩码 shard，假定传入 id 已满足增量段容量约束，本函数自身不会截断或报错。

## 依赖与调用关系

上游直接证据包括：

- `pkg/session/runtime/create_table_resources.rs::rebase_ids` 调用 `new_allocators_from_table_info`，随后通过 `Allocators::get` 和 `Allocator::rebase` 恢复行 ID、自增 ID、auto-random 水位；同文件的 `Txn`/`Store`/`Requirement` 是本文件三个存储抽象的生产实现。
- `pkg/infoschema/builder.rs` 使用 `AllocatorType`、`Allocators::filter` 处理表模型更新时保留或替换相应分配器。
- `pkg/table/table.rs` 通过 `Allocators::get(AllocatorType::AutoIncrement)` 获取表执行所需分配器。
- `pkg/ddl/split_region.rs` 调用 `ShardIdFormat::new` 和 `incremental_bits_capacity`，为 `AUTO_RANDOM` 表的 region split 计算 ID 范围。
- `pkg/meta/autoid/memid.rs` 复用 `Allocator` 契约、`AllocatorType`、`Context`、`TableInfo` 与 `calc_needed_batch_size`；`pkg/meta/autoid/autoid_service.rs` 复用同一 trait 并作为单点自增实现。

下游依赖首先是 `crate::errors::{AutoIdError, Result, autoinc_read_failed, invalid_increment_and_offset}`；其次是调用方注入的 `IdStore`/`IdTransaction`。标准库依赖承担原子变量、共享所有权、互斥、条件变量、时间测量与格式化。由于存储通过 trait 注入，本文件自身不知道 TiKV、事务重试、meta key 编码或网络细节。

RustCodeGraph 索引显示本文件包含 94 个符号，并能精确定位 `new_allocators_from_table_info`、`alloc_signed_locked`、`calc_sequence_batch_size` 和 `ShardIdFormat`；本次 `callers/callees` 查询未返回边，所以上述调用关系均由相邻生产源码逐处核对，而不是由缺失的图边推断。

## 错误处理与边界

所有可能失败的公开操作使用 `crate::errors::Result`。主要错误边界为：非法 `AUTO_RANDOM` 位数返回 `InvalidAutoRandom`；表 ID 为 0 返回 `InvalidTableId`；RowId/AutoIncrement 的 increment 或 offset 越界返回专用错误；signed/unsigned 或 sequence 空间耗尽统一转为 `AutoIncrementReadFailed`；错误类型不支持的 sequence 操作返回 `NotImplemented`；取消返回 `Canceled`；存储错误由 `IdStore` 原样传播。

取消语义刻意不是“进入 API 就失败”。`alloc` 的参数验证、零数量返回、本地缓存发放和无需存储的 rebase 可以在已取消 context 下完成；只有即将进入存储事务的路径调用 `Context::check`。`autoid_test.rs::test_context_cancellation_matches_go_control_flow` 固化了这一与 Go 一致的顺序。

算术大量使用 `wrapping_*`，用来复现 Go 固定宽度整数行为；空间耗尽判断在真正推进前完成。`test_go_wrapping_boundaries` 覆盖 `i64::MAX` 后 next ID 回绕、64 位 mask 及边界寻值。调用纯算法函数时仍有前置条件：例如 increment 不应为 0，普通公开 `alloc` 只为 RowId/AutoIncrement 强制验证；`AutoRandom` 与 sequence 的参数应由上层 DDL/模型校验保证。

锁中毒通过 `unwrap()` 处理，线程 panic 会使后续访问继续 panic，而不会映射为 `AutoIdError`。`Context::wait` 也对 condvar 锁结果使用 `unwrap()`。此外，`force_rebase` 允许主动下调全局水位，若外部在仍有旧缓存的其他分配器并发使用时调用，可能破坏唯一性；它应只用于具备全局协调的管理场景。

## 并发与资源生命周期

`DefaultAllocator` 实现 `Send + Sync` 所需的 `Allocator` 契约。单实例所有水位操作由同一 `Mutex<AllocatorState>` 保护，`alloc`、rebase、查询 base/end、transfer 会互斥；锁跨越存储事务持有，因此同实例不会同时补充缓存，但慢事务也会阻塞该实例的本地分配与查询。`autoid_test.rs::test_issue_40584` 验证并发 alloc/base 不死锁或崩溃。

不同分配器实例不共享本地锁，唯一性依赖 `IdStore::run_in_transaction` 对水位读取与 `inc` 的原子化。`autoid_test.rs::test_concurrent_alloc` 让 10 个线程使用共享 store、各自分配器并验证全局不重复；`seq_autoid_test.rs::test_concurrent_alloc_sequence` 对序列做同类验证。内存测试 store 使用 scratch map，只有 operation 成功才合并，模拟回滚边界。

`Arc<dyn IdStore>` 让 store 至少与分配器同寿命；`Allocators` 和返回的 trait object 也使用 `Arc`，可安全克隆到表及执行上下文。`Context` 克隆共享取消状态，`cancel` 设置原子位后持有 signal 锁并 `notify_all`，唤醒所有在 `wait` 中的线程。文件内没有后台线程、异步 task 或显式关闭流程；资源随最后一个 `Arc` 释放。

## 与 Go 版本的对应关系

Rust 文件以 `pkg/meta/autoid/autoid.go` 为直接语义基准。主要一一对应关系为：Go `Allocator`/`Allocators` ↔ Rust 同名 trait/struct，Go `allocator` ↔ `DefaultAllocator`，`NewAllocator` ↔ `new_allocator`，`NewAllocatorsFromTblInfo` ↔ `new_allocators_from_table_info`，`alloc4Signed/alloc4Unsigned` ↔ 两个 `alloc_*_locked`，`rebase4Signed/rebase4Unsigned` ↔ 两个 `rebase_*_locked`，以及批大小、寻值、保序编码、`ShardIDFormat` 等纯算法。

Rust 为解除 TiDB Go 类型耦合，引入了精简 `TableInfo`/`SequenceInfo` 和 `AutoIdKey`，并以 `IdStore`/`IdTransaction` 替代 Go 中直接使用 `kv.RunInNewTxn`、`meta.AutoIDAccessor` 的方式；具体映射下沉到 session 接线层。Go `context.Context` 被替换成最小 `Context`。这些是边界适配，不改变核心分配规则。

当前 Rust 并非 Go 文件所有观测能力的完整等价实现。Go 的分配事务包含 internal-source 标记、tracing、metrics、snapshot/commit detail 收集和事务统计注入；Rust `DefaultAllocator` 当前没有接入这些设施。Rust `AllocatorRuntimeStats` 只保存字符串和计数，`merge` 仅用非空字符串覆盖且不会合并计数，而 Go 版本会 clone/merge 结构化 runtime stats。文档或后续功能不能据此声称 Rust 已具备 Go 的完整可观测性。

测试对应关系明确：`autoid_test.rs` 标注移植自 `autoid_test.go`，覆盖 signed/unsigned、并发、回滚、动态 step、计算边界和取消控制流；`seq_autoid_test.rs` 对应 `seq_autoid_test.go`，覆盖正负 increment、cache/cycle 和并发序列。Rust 还增加了 `row_id_ignores_auto_id_cache_one_since_table_info_v5`、Go wrapping 边界和纯 helper 对照等聚焦回归。

## 扩展指南

新增分配器类型时，应同步检查 `AllocatorType` 的字符串、`AutoIdKeyKind`、`DefaultAllocator::key_for`、`new_allocators_from_table_info` 的挂载条件、上层表模型转换、`Allocators::get` 的兼容回退以及所有穷举 match；持久化键的兼容性必须先在生产 `IdTransaction` 实现中确认。测试应放在独立 `*_test.rs` 文件，不能内嵌进本源文件，并与对应 Go 测试意图保持一致。

修改普通分配算法时，优先保持 `(min, max]` 契约和“事务成功后才更新本地状态”不变量。至少同步 `autoid_test.rs` 的 signed/unsigned、回滚、并发与边界用例；若触及 increment/offset，再覆盖第一个合法值与跨 cache refill 重算。修改 sequence 时同步 `seq_autoid_test.rs`，同时验证正负 increment、cache 末批不足、非 cycle 耗尽、cycle round 和并发唯一性。

修改缓存 step 或 `AUTO_ID_CACHE=1` 路由时，要同时审查 `with_options` 与 `new_allocator`：表版本 5 后 RowId 不得误走单点服务，独立 AutoIncrement 才可通过 `Requirement` 获取远程实现。性能风险主要是过小 step 增加元数据事务，过大 step 增加跳号范围；锁跨事务也意味着新增慢操作会扩大串行阻塞。

修改 `ShardIdFormat` 或 AUTO_RANDOM 规范化时，应同时核对 `pkg/ddl/split_region.rs` 和字段类型上游校验，测试 0/64 位、signed/unsigned、最大 shard bits 与容量溢出边界。`compose` 当前不验证 id 是否越过 incremental mask；若要新增验证，属于 API/错误语义变化，需与 Go 行为和所有调用者共同设计，不能只在 Rust 中静默截断。

若要补齐 Go 可观测性，应在存储抽象与调用上下文层设计结构化统计传递，不能仅扩展 `AllocatorRuntimeStats` 字符串字段；同时要明确重试事务下 alloc/rebase 计数、快照与 commit detail 的合并时机。

## 验证依据

- RustCodeGraph：`status` 确认项目索引含 7,032 个 Rust 文件，`files --filter pkg/meta/autoid` 确认目标及对照文件已索引；`node --file pkg/meta/autoid/autoid.rs` 完整读取 1,290 行源码；`query` 精确定位 `new_allocators_from_table_info`、`alloc_signed_locked`、`calc_sequence_batch_size` 和 Rust `ShardIdFormat`。针对前三个符号执行了 `callers`/`callees`，未得到可用调用边，因此没有把空结果当作“无调用者”。
- 源与 crate 边界：`pkg/meta/autoid/autoid.rs`、`pkg/meta/autoid/Cargo.toml`、`pkg/meta/autoid/lib.rs`、`pkg/meta/autoid/errors.rs`。
- 直接生产接线：`pkg/session/runtime/create_table_resources.rs`（事务/store/requirement 实现、表级工厂调用与 rebase）、`pkg/meta/autoid/autoid_service.rs`、`pkg/meta/autoid/memid.rs`、`pkg/infoschema/builder.rs`、`pkg/table/table.rs`、`pkg/ddl/split_region.rs`。
- Go 对照：`pkg/meta/autoid/autoid.go`，重点核对 allocator/rebase、表级工厂、sequence、寻值/编码、shard 格式与 runtime stats。
- 独立测试：`pkg/meta/autoid/autoid_test.rs`、`pkg/meta/autoid/seq_autoid_test.rs`；并对照 `pkg/meta/autoid/autoid_test.go`、`pkg/meta/autoid/seq_autoid_test.go`、`pkg/meta/autoid/bench_test.go` 的测试范围。未运行 Cargo，符合本纯文档任务约束。
- 人工复核结论：文件存在是为了把四类 AutoID 的策略、缓存与数学规则从具体 KV 后端中抽离；运行时由表资源创建路径注入事务存储并构造/恢复分配器；安全扩展必须维持水位事务原子性、`(min, max]` 契约、signed/unsigned 固定宽度语义，并在独立 Rust 测试及 Go 对照中同步验证。
