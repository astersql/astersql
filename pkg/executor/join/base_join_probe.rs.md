# `pkg/executor/join/base_join_probe.rs`

## 文件定位

该文件属于 `astersql-executor-join` crate；`pkg/executor/join/Cargo.toml` 将 `lib.rs` 设为 crate 根，而 `pkg/executor/join/lib.rs` 以 `pub mod base_join_probe` 暴露本模块。它提供当前 Rust Hash Join probe（探测）实现共用的数据模型、状态机接口和按连接类型分派的工厂，具体算法分别落在 `inner_join_probe.rs`、`outer_join_probe.rs`、`semi_join_probe.rs`、`anti_semi_join_probe.rs` 与 `left_outer_semi_join_probe.rs`。

当前接线范围需要特别区分：RustCodeGraph 将本文件识别为 413 行、53 个符号，并显示文件级使用者包括 `hash_join_v1.rs`、`hash_join_v2.rs` 和测试；但精确引用搜索表明，Rust 的 `new_join_probe`/`HashJoinContext` 目前由各 probe 独立测试构造，`hash_join_v1.rs` 与 `hash_join_v2.rs` 的可执行 Rust 部分使用各自的 `ProbeWorkerV1`/`ProbeWorkerV2` 路径，没有调用本文件的工厂。因此本文件是完整的 probe 子模块公共实现和测试入口，但不能据此声称它已经接入 Rust Hash Join v2 主执行链。Go 对照 `pkg/executor/join/base_join_probe.go` 中的 `NewJoinProbe` 则明确是 Hash Join v2 的工厂。

文件没有条件编译项。测试通过 `lib.rs` 中独立的 `#[cfg(test)] mod base_join_probe_test;` 接入，测试逻辑没有内嵌在生产文件。

## 核心职责

1. `HashJoinContext::new` 接收 build 行、两侧键列下标、`Joiner`、build side 方向及输出容量，预先为不含 NULL 的 build key 建立 `BTreeMap<u64, Vec<usize>>` 哈希桶，并创建与 build 行等长的 `build_row_used` 标记。
2. `Probe` trait 统一普通/恢复 chunk 装载、分批 probe、spill 尾部行、可选 build 表扫描、完成状态与重置协议，使不同 `JoinType` 能以 `Box<dyn Probe>` 被调用。
3. `BaseJoinProbe` 保存一个 worker 当前 chunk 的序列化键、哈希、候选 build 行索引、双层游标、碰撞统计、spill 缓冲与扫描游标；具体 probe 实现复用这些状态。
4. `new_join_probe` 将 `JoinType` 与 build side 组合映射到具体实现，并拒绝当前模型无法表达的组合。
5. `serialize_key`、`encode_value`、`key_has_null`、`hash_bytes` 和 `is_key_matched` 提供本文件内部的键编码、NULL 排除、分桶及键比较规则。

`BATCH_BUILD_ROW_SIZE`、`OffsetAndLength`、`MatchedRowInfo`、`PositionAndHash` 与 `KeyMode` 保留了 Go 实现中的公共概念。其中当前 Rust 执行路径实际使用 `KeyMode`/`is_key_matched`；另外三个小结构及批大小常量在本文件内没有消费点，应视为迁移对齐的数据形状，而不是已经参与批量列构造的证据。

## 主要符号

- `pub const BATCH_BUILD_ROW_SIZE: usize = 32`：对应 Go 的 `batchBuildRowSize`。当前 Rust 文件未用它驱动缓存批处理。
- `KeyMode::{OneInt64, FixedSerialized, VariableSerialized}`：选择 8 字节整数前缀比较或完整序列化字节比较。`is_key_matched` 对不足 8 字节的 `OneInt64` 输入返回 `false`，不会越界。
- `OffsetAndLength`、`MatchedRowInfo`、`PositionAndHash`：分别描述区间、build/probe 行索引对和位置/哈希对；都是公开、可复制的值类型。
- `HashJoinContext`：持有 build 行和键元数据、哈希桶、使用标记、`Joiner`、side/过滤配置与 `max_chunk_size`。`has_other_condition` 实际委托 `!joiner.is_semi_join_without_condition()`，并非直接检查一个条件表达式字段。
- `WorkerResult { rows, error }`：probe/scan 的返回容器。错误以 `Option<String>` 携带，允许返回已经产生的部分行。
- `Probe`：对象安全的行为接口；具体类型负责 `probe` 与 build 表扫描语义，公共装载/重置通常委托给 `BaseJoinProbe`。
- `BaseJoinProbe`：单 worker 的可变状态。`new` 只绑定上下文和 `worker_id`，其余缓冲与游标为空/归零。
- `BaseJoinProbe::set_chunk_for_probe`：装载入口；禁止覆盖未完成 chunk，检查 probe 键下标，编码每行键，计算哈希并在同桶中做完整键二次过滤。
- `spill_remaining_probe_chunks`：把当前行起至 chunk 尾部复制到内存中的 `spilled_chunks`，将 chunk 标为完成，再用 `mem::take` 转移全部缓冲。它不是磁盘 I/O。
- `candidate_rows`/`mark_build_rows_used`：按 probe 行取候选 build 行副本或更新 `build_row_used`。
- `ProbeFlavor`/`probe_flavor`：将七种 `JoinType` 归并为六种实现风格；左右外连接和全外连接都归入 `Outer` 分类，但工厂仍会单独拒绝 `FullOuter`。
- `new_join_probe`：创建 `InnerJoinProbe`、`OuterJoinProbe`、`SemiJoinProbe`、`AntiSemiJoinProbe` 或 `LeftOuterSemiJoinProbe`。它返回 `Result`，用错误替代 Go 工厂的 panic。
- `serialize_key`/`encode_value`/`key_has_null`/`hash_bytes`：私有辅助函数；编码覆盖 `Null`、布尔、整数、无符号整数、浮点、字节串与文本，变长值带 `u32` 小端长度前缀，哈希使用 64 位 wrapping FNV-1a 风格折叠。

## 执行流程

构建上下文时，`HashJoinContext::new` 逐行调用 `serialize_key`。含 NULL 键的行不会进入 `hash_table`，但仍保留在 `build_rows` 且有对应的 `build_row_used=false`；其余行按 `hash_bytes(key)` 追加 build 行下标，所以重复键会形成同桶多候选列表。

worker 通过 `new_join_probe` 创建：先构造 `BaseJoinProbe`，再按 `JoinType` 包装成具体 `Probe`。左右外连接计算 `outer_side_build`/`right_side_build`；semi/anti-semi 通过 `BaseSemiJoin::new` 保存 `!right_as_build_side`；左外 semi 及 anti 仅允许右侧 build，并把 `null_aware` 传给 `LeftOuterSemiJoinProbe`；full outer 直接返回错误，指向 Hash Join v1 的双 Joiner 路径。

装载 chunk 时，`set_chunk_for_probe` 先要求上一 chunk 已完成。随后替换 `probe_chunk`，清空三个派生向量，对每行执行：验证所有 probe key 下标、序列化键、计算哈希；NULL key 直接得到空候选，否则读取同哈希桶并重新序列化 build key 做字节相等过滤。完成后将两个游标归零。`set_restored_chunk_for_probe` 在当前 Rust 模型中完全复用这条路径，没有 Go 版本恢复格式中的预存哈希/序列化键和重哈希步骤。

具体 `Probe::probe` 以 `current_probe_row` 和 `current_candidate` 增量推进。例如 `InnerJoinProbe::probe` 每次最多产生 `max_chunk_size` 行，从 `matched_rows[index]` 取候选交给 `Joiner::try_to_match_inners`，按 `consumed` 推进候选游标，成功时标记 build 行；候选耗尽后调用 `finish_current_lookup_loop` 前进到下一 probe 行。外连接与各 semi 变体复用相同的已预筛候选和游标，但决定匹配、未匹配、标记列以及事后扫描的语义在各自文件中。

若需要暂停并 spill，`spill_remaining_probe_chunks` 只保存尚未开始处理的行尾 `[current_probe_row..]`，把当前 chunk 标为完成并转移缓冲。若具体 probe 要在全部 probe chunk 后输出未命中的 build 行，则通过 `need_scan_row_table`、`init_for_scan_row_table`、`scan_row_table` 和 `is_scan_row_table_done` 驱动；`scan_row_index` 是共享游标。

## 数据与状态

`HashJoinContext` 被值持有在每个 `BaseJoinProbe` 中，并且 `Clone` 是深拷贝语义：`Vec<Row>`、`BTreeMap`、`build_row_used` 与 `Joiner` 都随 clone 复制。因而多个 clone 出来的 probe 不共享 `build_row_used`。这不同于 Go `baseJoinProbe.ctx *HashJoinCtxV2` 的共享指针语义；扩展并发 worker 时不能假定该标记天然跨 worker 可见。

`matched_rows` 与 `probe_chunk` 一一对应，每个元素是已经经过“哈希相同 + 完整序列化键相等”过滤的 build 行下标。`hash_values` 和 `serialized_keys` 同样按 probe 物理向量顺序保存。当前实现没有 Go chunk selection vector 的逻辑/物理行下标区分。

状态不变量是：新 chunk 只能在 `current_probe_row >= probe_chunk.len()` 时装载；`current_candidate` 只属于当前 probe 行，换行时归零；`build_row_used.len() == build_rows.len()`；`matched_rows` 中下标来自上下文哈希桶。`reset_probe` 清空当前批派生数据、碰撞计数和扫描游标，但保留上下文、build 行、哈希表、`build_row_used`、worker id 以及 `spilled_chunks`。因此它不是整个 probe 实例的完全初始化。

键编码是类型值的直接拼接：定长数值没有显式类型标签，`Bool` 仅占一个字节，`Null` 也编码为单个零字节；不过含 NULL 的键在建表和 probe 候选查找前被排除。跨不同键类型或不同列分段是否可能出现相同字节序列，取决于上游保证两侧键元数据一致；本结构本身不存 `FieldType`，也不执行类型转换或排序规则处理。

## 依赖与调用关系

直接 Rust 依赖只有 `crate::joiner::{JoinType, Joiner, Row}`、`crate::row_table_builder::Value` 和标准库 `BTreeMap`。工厂还以完整路径构造同 crate 的五类 probe 和 `BaseSemiJoin`。`Cargo.toml` 显示 crate 名为 `astersql-executor-join`、根文件为 `lib.rs`；本文件使用的这些模块是 crate 内部依赖，没有由本文件直接调用 Cargo 中列出的外部 crate。

下游关系为：`HashJoinContext::new -> serialize_key/key_has_null/hash_bytes`；`BaseJoinProbe::set_chunk_for_probe -> serialize_key/hash_bytes/key_has_null/BTreeMap::get`；`serialize_key -> encode_value`；`new_join_probe -> BaseJoinProbe::new`，再分派到具体 probe 构造；具体 probe 的 `Probe` 实现再回调 `BaseJoinProbe` 的装载、spill、完成判断、游标推进、候选读取或 used 标记。

上游直接证据主要来自测试：`base_join_probe_test.rs`、`inner_join_probe_test.rs`、`outer_join_probe_test.rs`、`left_outer_join_probe_test.rs`、semi/anti 系列测试和 spill 测试均构造 `HashJoinContext` 后调用 `new_join_probe`。`lib.rs` 暴露生产模块并单独声明测试模块。RustCodeGraph 的文件级 “used by” 还列出 `hash_join_v1.rs`/`hash_join_v2.rs`，但精确符号 callers/callees 没有返回边，文本引用也没有发现这两个执行器调用工厂，所以文档不把文件级边解读为运行时主链调用。

## 错误处理与边界

`HashJoinContext::new` 假设 build 键下标有效：它在 `serialize_key` 和 `key_has_null` 中直接索引 `row[*index]`，越界会 panic。相对地，`set_chunk_for_probe` 显式检查 probe 键下标，返回 `"probe row {row_index} key index out of range"`。如果校验在 chunk 中途失败，实例已经替换 `probe_chunk` 且可能填入部分 `hash_values`/`serialized_keys`/`matched_rows`；调用者应把该 probe 视为装载失败状态并显式重置或丢弃，不能假设事务式回滚。

上一 chunk 尚未完成时装载新 chunk返回 `"previous chunk is not probed yet"`。`base_join_probe_test.rs::set_chunk_rejects_replacing_unfinished_probe_chunk` 以 `max_chunk_size=1` 和两个重复 build key 证明第一次 `probe` 只消费部分候选、新装载被拒绝，继续 `probe` 后才完成。

NULL join key 不进入哈希候选，符合普通等值连接的 NULL 不相等基础规则；null-aware anti 语义由具体 probe 处理，不能仅从本文件的空候选推导完整 SQL 三值逻辑。哈希碰撞会由完整序列化键二次比较过滤，因此 `matched_rows` 本身不含纯哈希碰撞项；具体 probe 的碰撞统计含义还受它的 `Joiner` 消费方式影响。

`new_join_probe` 对左外 semi/anti-left-outer-semi 的左侧 build 返回错误，对 full outer 返回错误。`probe_flavor` 虽把 full outer 分类为 `Outer`，它不是可构造性检查。`is_key_matched(OneInt64, ...)` 只比较双方前 8 字节；测试 `one_int64_key_requires_a_complete_value` 覆盖短输入为 false，但未验证 8 字节后的尾部处理。未知 `Value` 变体不是问题，因为 `encode_value` 对当前 enum 做穷尽匹配；若以后新增变体，编译器会要求同步处理。

## 并发与资源生命周期

本文件不创建线程、锁、原子变量、通道、任务、文件或事务。`worker_id` 只是身份数据；`BTreeMap`、行向量和所有游标都由 `&mut self` 串行修改。`Probe: dyn` 没有 `Send`/`Sync` 约束，不能据此保证跨线程传递安全。

内存生命周期由所有权管理：`set_chunk_for_probe` 接管传入 `Vec<Row>`；候选只存 build 下标，避免为每个候选复制整行；`candidate_rows` 才克隆实际行。`spill_remaining_probe_chunks` 会复制未处理尾行后用 `mem::take` 把 spill 缓冲所有权交给调用者，因此返回后内部 `spilled_chunks` 为空。名称中的 spill 在此仅是内存交接，真正磁盘分区生命周期属于 `hash_join_spill.rs`/`hash_join_spill_helper.rs` 或执行器自身 helper。

`reset_probe` 可复用已分配实例，但它通过 `clear` 释放元素而通常保留向量容量；上下文和 build 表继续存活。`HashJoinContext`/`BaseJoinProbe` 的派生 `Clone` 会复制大量行与索引，且 used 状态不共享，既有性能成本也有并发语义风险。

## 与 Go 版本的对应关系

Rust 名称总体一一对应：`KeyMode` 对应 `keyMode`，`BATCH_BUILD_ROW_SIZE` 对应 `batchBuildRowSize`，`Probe` 对应 `ProbeV2`，`BaseJoinProbe` 对应 `baseJoinProbe`，`new_join_probe` 对应 `NewJoinProbe`，`is_key_matched` 对应 `isKeyMatched`。连接类型到具体 probe 的映射和左外 semi 只支持右 build 的约束与 Go 保持一致；Rust 用 `Result` 报错，Go 对不支持组合 panic。

当前 Rust 是面向 `Vec<Row>` 的安全简化实现，并非 Go 内存布局的等价替换。Go `baseJoinProbe` 处理 `chunk.Chunk` 的 selection vector、分区哈希表、tagged pointer 链、预过滤向量、列裁剪、other-condition 临时 chunk、SQL killer、真实 spill helper、恢复 chunk 的预存哈希/键和批量行列转换；Rust 上下文只保存行、键下标、普通哈希桶和 `Joiner`，恢复路径等同普通装载，spill 只返回内存行块，也没有 SQL 中断检查。

Go `isKeyMatched(OneInt64)` 通过 unsafe 指针读取 8 字节，Rust 使用安全切片并为短输入返回 false。Go 的 fixed/variable 模式从 row table 元数据定位 key；Rust 函数直接接收两段字节。Go `ResetProbe` 主要重建可能影响 GC 的缓存，Rust `reset_probe` 清理当前批和统计游标，语义范围不同。

最重要的迁移差异是接线：Go 注释和 `hash_join_v2.go` 调用关系将 `NewJoinProbe` 放在 v2 worker 主链；当前 Rust `hash_join_v2.rs` 自带 `ProbeWorkerV2`/`HashTableContext` 模型，没有调用本工厂。本文件及具体 probe 的测试证明局部行为，但不证明 Rust v2 主执行器使用了这套状态机。

## 扩展指南

- 新增 `JoinType` 时，同时更新 `probe_flavor`、`new_join_probe`、`Joiner` 分支和独立 probe 测试；分类成功不等于工厂可构造，两个 match 必须分别审查。
- 修改键编码时应从 `encode_value`/`serialize_key` 入手，并同步保证 build/probe 编码完全一致。需增加独立测试覆盖多列边界、不同变长值、浮点特殊值、类型组合和 NULL；若要达到 Go 等价，还需引入字段类型、排序规则与规范化证据，不能只调整哈希函数。
- 修改 chunk 状态机时应保持“未完成 chunk 不可覆盖”和双层游标可暂停恢复。同步 `base_join_probe_test.rs`，并覆盖装载中途出错后的状态策略、空 chunk、输出容量为零和 spill 后重用。
- 增强 spill/restore 时不要在本文件伪造磁盘语义；应与 `hash_join_spill_helper.rs`、`hash_join_v2.rs` 的分区生命周期统一，并测试多轮 spill、恢复键重哈希、错误传播和资源清理。
- 引入多 worker 共用 build used 标记前，应重新设计 `HashJoinContext` 所有权（例如明确的共享同步结构），并检查 outer/semi 的事后扫描；简单 clone 会产生彼此独立的标记。
- 若要把本工厂接入 `HashJoinV2Exec`，需要明确取代还是适配现有 `ProbeWorkerV2`/`HashTableContext`，并以端到端执行器测试证明接线。仅让 `new_join_probe` 编译或增加局部测试不足以证明主链迁移。
- 测试继续放在独立文件。公共状态/键规则放 `base_join_probe_test.rs`；具体连接输出放对应 `*_probe_test.rs`；spill 行为放 `outer_join_spill_test.rs` 等既有测试面，不把测试加入生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件且目标在索引中；`files --filter pkg/executor/join/base_join_probe.rs` 定位目标；`node --file ... --offset 1 --limit 500` 读取全部 413 行并列出文件级使用者；`query BaseJoinProbe`、`query new_join_probe`、`query set_chunk_for_probe`、`query HashJoinContext` 核对主要符号；精确 `callers/callees` 未返回可用符号边，此限制已在调用关系结论中保留。
- 生产源码：`pkg/executor/join/base_join_probe.rs`；直接具体实现 `inner_join_probe.rs`、`outer_join_probe.rs`、`base_semi_join.rs`、`semi_join_probe.rs`、`anti_semi_join_probe.rs`、`left_outer_semi_join_probe.rs`；执行器接线核验 `hash_join_v1.rs`、`hash_join_v2.rs`；模块入口 `lib.rs`。
- crate 边界：`pkg/executor/join/Cargo.toml` 的 `[package]`、`[lib] path = "lib.rs"` 与本地依赖声明。
- Rust 测试：重点读取 `pkg/executor/join/base_join_probe_test.rs`；并通过精确引用核对 `inner_join_probe_test.rs`、`outer_join_probe_test.rs`、`left_outer_join_probe_test.rs`、`right_outer_join_probe_test.rs`、semi/anti 系列及 `outer_join_spill_test.rs` 对工厂和上下文的使用。
- Go 对照：`pkg/executor/join/base_join_probe.go` 的 `ProbeV2`、`baseJoinProbe`、`SetChunkForProbe`、`SetRestoredChunkForProbe`、`SpillRemainingProbeChunks`、`ResetProbe`、`isKeyMatched`、`NewJoinProbe`；`pkg/executor/join/hash_join_v2.go` 的 worker 调用点；`pkg/executor/join/inner_join_probe_test.go` 的工厂与 probe 循环。
- 人工复核结论：本文件存在是为了统一具体 probe 的公共状态与工厂；当前 Rust 运行方式是先预建 build 哈希桶、按 chunk 预筛候选，再由具体 probe 以可暂停游标分批输出；安全扩展必须同时维护键编码、工厂分派、状态机不变量和独立测试，并区分局部 probe 行为与 v2 主链是否已经接线。
