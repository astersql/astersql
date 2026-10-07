# `pkg/executor/join/hash_table_v1.rs`

## 文件定位

本文件属于 Cargo crate `astersql-executor-join`；crate 入口 `pkg/executor/join/lib.rs` 以 `pub mod hash_table_v1` 无条件导出它，并在 `cfg(test)` 下挂接独立测试模块 `hash_table_v1_test.rs`。它位于 Hash Join v1 的 build/probe 中间层：`BuildWorkerV1::build` 把构建侧 `Chunk` 展平成行后创建并填充 `HashRowContainer`，`ProbeWorkerV1::join_probe_row` 和 `join_full_outer_probe_row` 再通过该容器定位候选构建行、取回行值并记录已匹配状态（`pkg/executor/join/hash_join_v1.rs`）。

文件的 1–725 行是由 Go 代码迁移而来的整块注释草稿，不参与编译；活跃实现从 `use crate::concurrent_map::ConcurrentMap` 开始。活跃代码没有条件编译项，也不直接使用 `pkg/executor/join/Cargo.toml` 中的外部 crate：它只依赖同 crate 的 `concurrent_map`、`joiner::Row`、`row_table_builder::Value`，以及标准库集合与原子类型。

## 核心职责

本文件把 Hash Join v1 的三个职责集中起来：

1. `HashContext::init_hash` 按指定连接键列编码每一行，产生哈希值、是否含 NULL 的标记和前 64 个键位的 NULL 位图。
2. `BaseHashTable` 及其 `UnsafeHashTable`、`ConcurrentMapHashTable` 实现维护“哈希值到一个或多个 `RowPointer`”的桶，保留重复键和哈希碰撞候选，并提供增量内存估算。
3. `HashRowContainer` 同时拥有构建侧行、普通哈希桶、NAAJ NULL 桶和 outer join 的 used 标记，向 probe 路径提供候选过滤、行取回、未匹配行枚举和简化的 spill 记账。

该文件只负责基于键的候选生成与行状态，不负责 join 类型的最终输出语义、other condition 或结果拼装；这些由 `ProbeWorkerV1` 和 `Joiner` 完成。哈希相同并不等于键相等，所以 probe 路径必须在取桶后调用 `keys_equal_cross` 做二次过滤。

## 主要符号

- `HashContext { key_indices, hash_values, has_null, null_bits }`：批量哈希的可复用结果对象。`new` 只保存键列下标；`init_hash` 清空旧结果并逐行重建。键下标越界返回 `Err(String)`。
- `RowPointer { chunk_index, row_index }`：稳定指向 `HashRowContainer::chunks` 中一行的二元坐标，同时用于 `used` 的同构索引。
- `HashNaNullBucket`：保存 `(null_bits, RowPointer)`。`put`、`len`、`is_empty` 是内部 NULL 桶的最小接口；当前匹配路径会重新读取行值判断兼容性，而不会消费保存的 `null_bits`。
- `Entry`、`NaEntry`、`EntryStore`：公开的迁移辅助类型。`EntryStore` 分别保存普通条目和 NA 条目并返回线性下标；活跃 `HashRowContainer` 和两种哈希表目前未使用它们，实际桶直接存 `RowPointer`。
- `BaseHashTable`：抽象 `put/get/len/for_each/get_and_clean_memory_delta`，并由默认 `is_empty` 补充空表判断。`get` 返回克隆出的行指针集合，不暴露内部桶。
- `UnsafeHashTable`：标准库 `HashMap<u64, Vec<RowPointer>>` 实现，`with_capacity` 使用预计行数预分配；适合单一可变所有者。
- `ConcurrentMapHashTable`：使用同 crate 的分片 `ConcurrentMap<RowPointer>` 保存链式桶。名称和内部结构对应并发 map，但本文件暴露的 `put` 仍要求 `&mut self`，`length` 也是普通 `usize`，因此当前 API 不能据此宣称支持多个线程同时调用同一个实例。
- `HashRowContainer`：主要对外对象。`new` 根据 `concurrent` 选择哈希表；`put_chunk` 建表；`get_matched_rows[_by_indices]`、`get_na_rows[_by_indices]` 探测；`row`、`mark_used`、`unmatched_rows` 管理构建行；`spill`、`close` 和字节访问器管理简化生命周期。
- `keys_equal_cross`、`na_keys_compatible`：分别实现普通等值 join 和 NULL-aware 候选兼容判断。私有 `keys_equal` 是同侧下标版本，当前活跃代码没有调用者。
- `hash_bytes`、`encode_value`、`row_size`：FNV-1a 风格哈希、带类型标签的值编码和行体积粗略估算。

## 执行流程

构建流程如下：

1. `BuildWorkerV1::build` 根据 build key、并发度和预计行数调用 `HashRowContainer::new`；并发度大于 1 时选 `ConcurrentMapHashTable`，否则选 `UnsafeHashTable`。
2. `put_chunk` 创建同一组 build key 的 `HashContext`，`init_hash` 对传入行逐列调用 `encode_value`，再由 `hash_bytes` 生成每行哈希。
3. 容器先为该批行建立全 false 的 `used` 向量。含任一 NULL 键的行进入 `null_bucket`；其余行以哈希值写入 `BaseHashTable`。随后累计哈希条目增量与 `row_size`，最后才把整批行压入 `chunks`。
4. `BuildWorkerV1` 读取 `memory_bytes`，由其基类判断是否越过限制；若需要 spill，则调用容器的 `spill`。

普通探测流程如下：

1. `get_matched_rows_by_indices` 先要求 probe key 数量和 build key 数量完全一致，再用 probe 侧下标对单行建哈希。
2. probe 键含 NULL 时立即返回空集合，符合普通等值连接的 NULL 不相等语义。
3. 否则从哈希桶取出所有 `RowPointer`，逐个经 `row` 取 build 行，并用 `keys_equal_cross` 消除哈希碰撞和类型编码相同但值不相等的候选。
4. `ProbeWorkerV1` 取回行值并交给 `Joiner` 处理 join 类型及附加谓词；确认匹配后调用 `mark_used`。build side outer 或 full outer 的收尾阶段通过 `unmatched_rows` 输出仍为 false 的行。

NULL-aware 探测由 `get_na_rows_by_indices` 完成：先取得普通非 NULL 桶的等值结果，再扫描全部 NULL 桶行；`na_keys_compatible` 把任一侧为 NULL 的键位视为“不约束”，只要求双方均非 NULL 的位置相等。最终 NAAJ 分类和反/半连接输出仍由 `hash_join_v1.rs` 的 `classify_naaj` 与 `Joiner` 决定。

## 数据与状态

`HashRowContainer` 中 `chunks`、`used` 具有相同的二维形状，`RowPointer` 必须同时能索引两者。`put_chunk` 在成功哈希后才创建指针和追加行；`row` 对越界指针返回 `None`，而 `mark_used` 对越界指针静默忽略。`len` 统计 `used` 中的全部构建行，包括进入 NULL 桶、未进入普通哈希表的行，因此它不同于 Go 版本 `hashRowContainer.Len()` 的“普通哈希表条目数”。

普通桶允许同一哈希保存多行。`UnsafeHashTable` 用 `Vec<RowPointer>` 保持插入顺序；`ConcurrentMapHashTable` 沿 `ConcurrentMap` 链表遍历，顺序取决于该实现。调用者不能依赖跨实现的候选顺序，只能依赖候选集合完整。

`HashContext::null_bits` 是单个 `u64`：仅当键序号小于 64 时置位；`has_null` 对任意数量的键都完整记录“本行至少有一个 NULL”。因此超过 64 个键时普通/NULL 桶分类仍正确，但位级位置证据会截断。当前 `get_na_rows_by_indices` 不读取该位图，而是按原行重算兼容性。

内存状态是估算而非分配器实测：哈希表的 delta 只按 `RowPointer` 和新 `u64` 键等固定大小累计，行大小对 `Bytes/Text` 使用内容长度、其他值统一计 8 字节，未计入 `Vec`/`HashMap` 容量、对象头和字符串额外开销。`get_and_clean_memory_delta` 通过 `AtomicI64::swap` 提供“读取并归零”的窗口增量；容器本身在每次 `put` 时直接累加返回值，并不调用该清零接口。

## 依赖与调用关系

直接上游调用边经 RustCodeGraph 查询和源码消歧后集中在 `pkg/executor/join/hash_join_v1.rs`：

- `BuildWorkerV1::build` → `HashRowContainer::new` → `HashRowContainer::put_chunk` → `HashContext::init_hash` / `BaseHashTable::put`。
- `ProbeWorkerV1::join_probe_row` → `get_matched_rows_by_indices` 或 `get_na_rows_by_indices` → `BaseHashTable::get` / `row` / 键比较；随后按 join 结果调用 `mark_used`。
- `ProbeWorkerV1::join_full_outer_probe_row` → `get_matched_rows_by_indices` → `row` / `mark_used`。
- `HashJoinV1Exec` 的 spill 状态、磁盘字节、outer 收尾和关闭路径分别读取 `already_spilled`、`disk_bytes`、`unmatched_rows` 并调用 `close`。

直接下游依赖为 `pkg/executor/join/concurrent_map.rs` 的 `ConcurrentMap`，`pkg/executor/join/joiner.rs` 的 `Row`，以及 `pkg/executor/join/row_table_builder.rs` 的 `Value`。`Value` 的变体决定编码格式；新增变体时若不同时更新 `encode_value` 和 `row_size`，本文件将无法编译或产生不一致的哈希/记账行为。

`pkg/executor/join/Cargo.toml` 声明 crate 的库入口为 `lib.rs`，且 package metadata 指向 Go 包 `pkg/executor/join`。目标文件的活跃代码不受其中 Windows-only 依赖控制；其公开模块在所有 target 上存在。

## 错误处理与边界

活跃 API 以 `Result<_, String>` 传播两类输入错误：`HashContext::init_hash` 检测键下标越界，`get_matched_rows_by_indices` 检测 build/probe 键数量不同。`get_na_rows_by_indices` 先走普通匹配，因此继承同样的数量和越界检查。这里没有 I/O，也没有自定义错误类型。

哈希冲突不是错误：桶可含多个指针，并由 `keys_equal_cross` 做值级过滤。`encode_value` 给各 `Value` 变体写入独立标签，`Bytes/Text` 还写长度，避免简单拼接产生边界歧义；整数使用小端字节，浮点使用原始 `to_bits`，因此 `-0.0` 与 `0.0`、不同 NaN 位型按位哈希可能不同，是否满足 SQL 类型/排序规则语义不能仅由本文件保证。

`row` 的非法指针返回 `None`，probe 过滤会跳过它；`mark_used` 也静默忽略非法指针。这让容器对损坏指针不 panic，但也不会报告内部不变量破坏。`unmatched_rows` 直接按 `chunks` 索引 `used`，依赖二者始终同构。

`spill` 只把估算的 `memory_bytes` 转移到 `disk_bytes` 并设置布尔标志，不序列化、释放或从磁盘恢复行；`close` 清空 `chunks`、`used`、NULL 桶和当前内存计数，但不会重建/清空内部哈希表，也不重置 `spilled` 或 `disk_bytes`。当前 `HashJoinV1Exec` 关闭后丢弃容器，因此该行为不会被当作可复用初始化；若未来复用实例，必须先补齐完整 reset 契约。

## 并发与资源生命周期

构建阶段要求 `&mut HashRowContainer`，probe 的只读查找接受 `&self`，匹配标记接受 `&mut self`。`HashRowContainer` 内持有 `Box<dyn BaseHashTable + Send>`，没有要求 `Sync`，也没有内部锁保护 `chunks`/`used`；因此当前安全模型是容器由执行器独占可变访问，而不是多个 probe 线程直接共享同一实例并并发 `mark_used`。

`ConcurrentMapHashTable` 的桶操作复用分片 map，memory delta 使用原子变量；但 `put(&mut self)` 和非原子的 `length` 仍把写入串行化在 Rust 借用规则内。Go 测试同样明确注明其 `concurrentMapHashTable` 当前不支持并行插入，所以这里的“Concurrent”应理解为底层结构/迁移对应关系，而非完整并发写契约。

构建行由 `chunks` 拥有，桶只保存坐标，不拥有或复制行。只有容器存活且对应 chunk 未被清除时 `RowPointer` 才有效。`close` 是显式释放入口，不实现 `Drop`；Rust 最终仍会自动释放字段，但执行器通过 `HashJoinV1Exec` 的关闭路径显式调用它以清空主要行数据。spill 当前只是统计状态，不建立文件句柄、异步任务或真实磁盘资源生命周期。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/join/hash_table_v1.go`，Go 测试是 `pkg/executor/join/hash_table_v1_test.go`。Rust 活跃实现保留了以下主干：`HashContext` 的批量哈希/NULL 分类、`BaseHashTable` 的重复键链、unsafe/concurrent 两种表、构建侧行容器、NAAJ NULL 桶、碰撞后二次键比较、内存增量读取清零，以及 spill/close 的外部接口形状。

当前 Rust 不是 Go 实现的等量复刻，重要差异必须作为扩展约束：

- Go 使用 `FieldType`、statement type context、`codec.HashChunkSelected` 和 `codec.EqualChunkRow`，覆盖 SQL 类型、字符集/排序规则、选择向量及 `ignoreNulls`；Rust 使用本地 `Value` 标签和 Rust 值相等，没有这些 SQL 语义上下文。
- Go 的 NAAJ NULL 位图是动态 `ConcurrentBitmap`，Rust 的 `u64` 只记录前 64 位；Rust 当前靠重读行值维持匹配，所以测试仅证明超过 64 键时分类不丢失，不证明位图等价。
- Go `RowContainer` 支持浅拷贝、真实 spill、磁盘读取缓冲、memory/disk tracker、failpoint 和错误返回；Rust `Vec<Vec<Row>>` 加计数器只提供内存内模拟。
- Go 的 probe API 可返回行和指针、统计 probe collision，并针对落盘行管理 `chkBuf`；Rust 返回 `RowPointer` 后由调用者取行，没有碰撞统计。
- Go 的 `Len` 只统计哈希表条目，并另以 `hashStateRows` 加 NAAJ NULL 桶；Rust 容器 `len` 统计所有构建行。
- Rust `EntryStore`/`Entry`/`NaEntry` 是公开但未接线的迁移遗留，实际存储布局不等同 Go 的分片扩容 entry arena。

Rust 测试 `hash_context_and_row_container_match_keys_collisions_nulls_and_spill`、`na_null_bucket_matches_only_equal_non_null_key_positions`、`hash_context_marks_null_keys_beyond_first_bitmap_word` 和 `unsafe_and_concurrent_hash_tables_preserve_duplicate_bucket_rows_and_delta` 验证当前 Rust 契约。Go 的 `TestHashRowContainer` 与 `TestConcurrentMapHashTableMemoryUsage` 则提供完整实现的碰撞、浅拷贝、真实 spill 和分配记账参照，不能反向当作 Rust 已具备这些能力的证据。

## 扩展指南

- 新增或修改键值类型时，同步修改 `row_table_builder::Value`、`encode_value`、`row_size`，并在 `hash_table_v1_test.rs` 加入相等值、类型边界、NULL 和碰撞用例；若目标是 SQL 等价语义，应优先接入类型/排序规则感知 codec，而不是继续扩展临时字节格式。
- 扩展 NAAJ 到依赖具体 NULL 位置的逻辑时，应将 `null_bits: u64` 替换为可覆盖任意键数的表示，并让 NULL 桶实际使用该表示；同步覆盖 64/65 位边界及多列 NULL 组合。
- 引入真正并发 build/probe 时，需要同时审查 `BaseHashTable` receiver、`ConcurrentMapHashTable::length`、`HashRowContainer::used`、trait 的 `Sync` 约束和候选顺序，不能只依赖 `ConcurrentMap` 名称。
- 实现真实 spill 时，修改入口应是 `HashRowContainer::{spill,row,close}` 及所有行所有权字段，并与 `BuildWorkerV1::build`、`HashJoinV1Exec::{IsSpillTriggered,DiskBytes}` 协调；测试必须独立放在 `hash_table_v1_test.rs` 或执行器测试中，覆盖释放、恢复、错误和清理。
- 若要复用已关闭容器，应新增明确的 reset 或让 `close` 同步清空哈希表、磁盘/布尔状态；否则旧桶指针和新 chunk 坐标可能混淆。
- 优化内存估算时，保持两种表实现与 Go tracker 定义可解释地对应，并验证 `get_and_clean_memory_delta` 首次返回正增量、第二次返回零。性能修改还应关注大量重复键、哈希碰撞、桶克隆成本和 `get_na_rows_by_indices` 对 NULL 桶的全扫描。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`query HashRowContainer/HashContext/UnsafeHashTable/ConcurrentMapHashTable` 定位到本文件、Go 对照和 Rust 测试；`node --file pkg/executor/join/hash_table_v1.rs --offset 1/992` 读取了完整活跃实现。宽泛 `explore` 因同名符号产生跨仓库噪声，最终调用边以精确符号查询后结合局部引用消歧。
- 生产源码：`pkg/executor/join/hash_table_v1.rs`（目标实现）、`pkg/executor/join/hash_join_v1.rs`（直接 build/probe/spill/outer/close 调用者）、`pkg/executor/join/lib.rs`（模块装配）、`pkg/executor/join/concurrent_map.rs`、`pkg/executor/join/joiner.rs`、`pkg/executor/join/row_table_builder.rs`（直接依赖）。
- crate 边界：`pkg/executor/join/Cargo.toml` 的 `[package]`、`[lib]`、`package.metadata.porting` 和依赖分区。
- 对照与测试：`pkg/executor/join/hash_table_v1.go`、`pkg/executor/join/hash_table_v1_test.go`、`pkg/executor/join/hash_table_v1_test.rs`。Rust 测试覆盖两种表、重复桶、NULL/NAAJ、used、spill 记账和 delta 清零；Go 测试用于识别完整迁移语义及当前差距。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文档存在且恰有 11 个固定二级章节，并人工检查没有把注释草稿或 Go 独有能力表述为活跃 Rust 能力。
