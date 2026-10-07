# `pkg/executor/batch_point_get.rs` 逻辑说明

## 文件定位

本文件属于 `astersql-executor` crate：`pkg/executor/Cargo.toml` 将库入口指定为 `lib.rs`，而 `pkg/executor/lib.rs:60` 以 `pub mod batch_point_get` 对外公开该模块。它承载批量点查的 Rust 业务模型：把一组主键句柄或唯一索引值转换为 KV 键，批量读取行，在需要时加悲观锁，最后按 `Chunk` 分批解码输出。

生产构建主链的相邻入口是 `pkg/executor/builder.rs:5090` 的 `buildBatchPointGet`：它校验临时表/缓存表、做分区裁剪、取得快照并调用 `build_batch_point_get_executor`。RustCodeGraph 能确认该入口由 `builder.rs:2381` 的 `build` 调用，但当前索引没有给出该依赖接口直接实例化本文件 `BatchPointGetExec<R>` 的边。因此，可确认两者处于同一批量点查业务链，不能仅凭现有证据断言具体运行时适配器已接到这个泛型结构体。直接、可验证的 Rust 使用者是 `pkg/executor/batch_point_get_test.rs`。

## 核心职责

`pkg/executor/batch_point_get.rs` 集中处理五类职责：

1. 用 `BatchPointGetRuntime` 抽象键编码、快照批读、句柄解码、分区判断、锁操作、行解码及统计等外部能力，使控制流与具体存储/会话类型解耦。
2. 用 `BatchPointGetExec<R>` 保存计划输入和一次执行的可变状态，并通过 `Open`、`Next`、`Close` 形成执行器生命周期。
3. 在 `initialize` 中实现唯一索引两阶段读取（索引键到句柄、行键到行值）与直接主键读取，并维持去重、顺序、分区过滤和“句柄—值”对齐。
4. 区分悲观事务的非读一致性路径与读一致性路径：前者先锁候选键再读，后者先读再只锁实际存在的键。
5. 提供缓存表快照、悲观锁值缓存和通用 `LockKeys` 的小型边界抽象。

这些职责均来自本文件的实际符号；本文件没有 SQL 解析、物理计划选择、真实 KV 客户端或具体行编码实现，它们通过构建器或 runtime trait 留在模块外。

## 主要符号

- `PointGetKey`、`PointGetValue`（`batch_point_get.rs:30-32`）：分别是键和值的字节向量别名。
- `batch_point_get_repeatable_read_failpoint`（`:35`）：触发两个与 Go 同名的 failpoint；只在唯一索引句柄收集完成后调用。
- `BatchPointGetTableInfo`（`:48`）：执行所需的表摘要，包含表 ID、分区标志、主键句柄形式、无符号主键、common handle 和读锁表标志。
- `BatchPointGetIndexInfo`（`:59`）：唯一索引摘要，记录索引 ID、是否全局索引、是否主索引。
- `BatchPointGetRuntime`（`:66`）：本文件最重要的依赖倒置边界。关联类型定义上下文、句柄、索引值、列/字段、解码器与错误；方法覆盖批读、锁、键编解码、分区、错误报告、统计和 `Chunk` 输出。
- `BatchPointGetExec<R>`（`:184`）：核心状态机。公开方法包括 `buildVirtualColumnInfo`（`:208`）、`Open`（`:215`）、`Close`（`:224`）、`Next`（`:233`）和 `initialize`（`:269`）。这些命名保留 Go 风格，模块级 `#![allow(...)]`（`:22`）允许相应命名。
- `PointGetOptions`（`:444`）与 `PointGetSnapshot`（`:449`）：描述单点读取选项及底层快照接口。
- `cacheTableSnapshot<S>`（`:465`）：缓存表快照包装；`BatchGet` 跳过不存在和空值，`Get` 转发单点读取，两者都拒绝 `return_commit_ts`。
- `MockNewCacheTableSnapShot`（`:507`）：供测试构造缓存表快照的公开辅助函数。
- `PessimisticLockRuntime`（`:512`）与 `LockKeys`（`:528`）：封装超时检查、底层锁调用，以及悲观事务成功加锁后的返回值缓存。
- `PessimisticLockCacheGetter`（`:545`）：悲观锁值的只读 getter；查不到返回 `None`，请求提交时间戳则报错。

本文件没有条件编译项、宏定义或模块级常量；唯一属性是名称风格豁免。除 `batch_point_get_repeatable_read_failpoint` 为 `pub(crate)` 外，上述业务类型和函数均为 `pub`，但泛型执行器的真实能力仍受 runtime 实现约束。

## 执行流程

执行器生命周期如下：

1. 构造阶段由外部准备 `table_info`、可选 `index_info`、句柄或索引值、物理表 ID、锁选项、输出顺序、列定义和行解码器。`buildVirtualColumnInfo` 调用 runtime 预计算虚拟列索引与字段类型。
2. `Open` 只调用 `open_batch_getter(lock, read_locked_table, table_id)`；具体快照、事务内存缓冲或锁缓存如何组合由 runtime 决定。
3. 首次 `Next` 先清空输出 `Chunk`，把 `initialized` 置为 `true`，再调用 `initialize`。初始化成功且 `lock` 为真时，更新逻辑表 ID 的 delta。后续 `Next` 不再访问存储，只消费已缓存的 `values`。
4. `initialize` 先取得最大执行时间和读一致性开关。若存在非 common-handle-primary 的索引，则逐组索引值选择物理 ID、编码键、用 `BTreeSet` 去重；`keep_order` 时按键字节序排序并按需反转。随后第一次 `batch_get` 取得索引值，解码句柄；分区表为全局索引时从 value 解出分区 ID并应用单分区/分区名过滤，本地索引则从 key 解表 ID且不应用全局索引的分区名过滤。最后触发可重复读 failpoint。
5. 若不是二级索引路径而要求保序，则直接对句柄排序；整型主键是否按无符号顺序比较由 `primary_key_is_handle && primary_key_is_unsigned` 决定。
6. 对保留下来的句柄选定物理 ID，仅为正 ID 编码行键，并同步过滤句柄。非读一致性的锁读把全部行键和索引键一次性交给 `lock_keys`，之后第二次 `batch_get` 读取行值。
7. 遍历行键时，缺失行若来自非 common-handle-primary 索引且不是弱一致性读取，则调用 `report_lookup_inconsistent`；存在的值与句柄按相同顺序写入 `values` 和新句柄数组。读一致性锁读只收集实际存在的行键及对应索引键，读完后再加锁。
8. `Next` 从 `cursor` 开始逐行 `decode_row`，直到 `Chunk` 满或值耗尽；再按本批 `[start, cursor)` 填充行校验和，并计算虚拟列。耗尽后返回空 `Chunk`。
9. `Close` 汇总 runtime 统计、重置快照统计，并把 `initialized` 和 `cursor` 复位；数据向量本身不在这里清空，下次初始化会重建有效结果。

## 数据与状态

`BatchPointGetExec` 的输入状态可分为：表/索引元信息（`table_info`、`index_info`）、计划键材料（`handles`、`index_values`、`plan_physical_ids`、`single_partition_id`、`partition_names`）、锁语义（`lock`、`wait_time_ms`）、顺序语义（`keep_order`、`descending`）以及列/解码配置（`columns`、`row_decoder`、两组虚拟列元信息）。

执行期状态是 `initialized`、`values` 和 `cursor`。关键不变量是 `values[i]` 必须与 `handles[i]` 对应；`initialize` 在过滤无效分区、缺失索引、缺失行后同步重建句柄数组来保持这一点。索引路径中 `index_keys` 还必须与解出的句柄/行键保持位置对应，因为不一致报告和读一致性加锁会用同一索引访问；新增过滤或排序逻辑若只改其中一个向量会造成错锁或错误诊断。

`BTreeSet` 用于索引键去重，避免重复 `IN` 条件产生重复行；`BTreeMap` 是 runtime 批读结果及缓存实现的数据形态。`keep_order` 的二级索引路径排序的是编码后的索引键，主键路径排序的是句柄。`plan_physical_ids` 在分区索引读取后会被清空并按实际命中顺序重建。

## 依赖与调用关系

直接语言/库依赖只有 `std::cmp::Ordering`、`std::collections::{BTreeMap, BTreeSet}`、`astersql_util_chunk::Chunk` 和 `fail::eval`。`pkg/executor/Cargo.toml` 确认 crate 名为 `astersql-executor`、库入口为 `lib.rs`，并直接声明了 `astersql-util-chunk` 路径依赖与启用 `failpoints` 的 `fail = 0.5.1`。本文件不受 crate 的 `nextgen` feature 条件控制。

已验证的内部调用边包括：`Next -> initialize`，`Next -> BatchPointGetRuntime::{reset_output_chunk, decode_row, fill_row_checksum, fill_virtual_columns}`，`initialize -> BatchPointGetRuntime::{encode_unique_index_key, batch_get, decode_index_handle, global_index_partition_id, table_id_from_index_key, partition_matches, compare_handles, encode_row_key, lock_keys, report_lookup_inconsistent}`，以及 `LockKeys -> PessimisticLockRuntime::{check_max_execution_time, lock_keys, pessimistic_transaction, cache_locked_value}`。

上游方面，`pkg/executor/lib.rs:60` 公开模块，`pkg/executor/batch_point_get_test.rs:20-23` 导入并直接使用本文件类型。生产构建主链是 `builder.rs:2381 build -> builder.rs:5090 buildBatchPointGet -> ExecutorBuilderDependencies::build_batch_point_get_executor`；该抽象构建边与本文件具体 `BatchPointGetExec<R>` 的实例化关系在当前 RustCodeGraph 索引中未验证。不要把 RustCodeGraph 对同名 `Open`、`Next`、`Close` 的仓库级宽泛结果当作本文件调用者。

## 错误处理与边界

核心流程统一使用 runtime 的 `R::Error` 并以 `?` 原样传播：索引键编码、两次批读、索引值/分区 ID 解码、锁操作、不一致报告、行解码、校验和及虚拟列计算均可终止当前调用。`Next` 在调用 `initialize` 前先设置 `initialized = true`；因此初始化失败后，同一个实例若不先 `Close` 或由外部重置，下一次 `Next` 不会自动重试初始化，这是扩展错误恢复时必须保留或明确修改的语义。

边界行为包括：空索引键集合直接成功返回；批读缺少某个索引键时跳过；非正物理 ID 不生成行键；结果耗尽时返回成功的空输出；弱一致性允许索引命中而行缺失；common-handle 的 primary index 被视为直接句柄读取，避免重复二级索引阶段。`cacheTableSnapshot::{BatchGet, Get}` 和 `PessimisticLockCacheGetter::Get` 都明确拒绝 `return_commit_ts`，但前者返回底层关联错误，后者固定返回 `String`。

需要注意的可验证差异是：Go `cacheTableSnapshot::Get` 直接访问 `memBuffer`，Rust 包装只持有并调用一个 `PointGetSnapshot`；Go `BatchGet` 在 `memBuffer == nil` 时返回空 map，而 Rust 的空/不存在行为由 `PointGetSnapshot::get` 返回 `None` 表达。

## 并发与资源生命周期

本文件不创建线程、任务、通道或显式锁对象。`BatchPointGetExec` 的可变操作均要求 `&mut self`，类型本身没有声明可并发共享；并发安全与事务隔离由 runtime 及其上下文负责。

Rust 的 `initialized: bool` 是普通状态位，而 Go 对照使用 `uint32` 加 `atomic.CompareAndSwapUint32`。因此 Rust 版本没有 Go 版本的原子首次初始化保护；在安全 Rust 中 `Next(&mut self)` 已排除同一实例的普通并发可变调用，但如果未来通过内部可变性或跨线程包装共享执行器，不能把该布尔值当作并发同步原语。

资源生命周期为：`Open` 建立/选择 getter，第一次 `Next` 完成全部 KV 读取和可选加锁，后续 `Next` 仅分批解码内存结果，`Close` 汇总及重置统计和游标。锁的持有期不由本文件释放，而跟随外部悲观事务。`LockKeys` 只在底层锁成功后缓存返回值；Go 对照说明此时没有其他 goroutine 访问返回 map，而 Rust 通过取得 map 的所有权顺序消费，无需额外互斥。

## 与 Go 版本的对应关系

主要结构与 `pkg/executor/batch_point_get.go` 一一对应：Rust 的 `BatchPointGetExec`、`Open`、`Close`、`Next`、`initialize`、`cacheTableSnapshot`、`MockNewCacheTableSnapShot`、`LockKeys`、`PessimisticLockCacheGetter` 均保留了 Go 名称和流程。两阶段索引/行读取、重复索引键去重、升降序、无符号整型句柄排序、全局/本地索引分区 ID 来源、RR/RC 的锁时机、不一致报告、checksum/虚拟列填充等关键分支与 Go `batch_point_get.go:254-501` 对齐。

Rust 通过 trait 和摘要结构拆出 Go 里直接依赖的 session、transaction、snapshot、tablecodec、rowcodec 和 consistency reporter，因此“控制流已移植”不等于“所有 TiDB 具体设施已在本文件实现”。已观察到的差异/限制包括：

- Go `Open` 明确组合事务 mem-buffer、悲观锁缓存、表缓存和 snapshot，Rust 将其压入 `open_batch_getter`。
- Go `Close` 展开合并 scan/read-pool/CPU/index-usage 统计，Rust 只调用两个统计钩子。
- Go 用 context deadline 实现最大执行时间，Rust只把毫秒数传给 `batch_get`。
- Go failpoint 使用上下文通道协调并发步骤，Rust函数仅顺序执行两个 `fail::eval`，没有在本文件中实现相同的通道协调。
- Go 在多分区保序处有断言/TODO；Rust 没有对应断言，调用者仍需保证句柄或索引键排序后物理 ID 的位置关系正确。
- Go `PessimisticLockCacheGetter` 从事务上下文动态读取，Rust结构体持有独立的 `BTreeMap` 快照。

测试证据也不等量：`pkg/executor/batch_point_get_test.rs` 当前只覆盖锁缓存 getter 的正常/拒绝 commit-ts 行为，以及本地索引不应用全局分区名过滤；Go 的 `pkg/executor/batch_point_get_test.go` 还覆盖 RR/RC 下主键与唯一键的并发锁行为、缓存快照和临时表点查。

## 扩展指南

新增键选择、分区或顺序语义时，优先修改 `BatchPointGetExec::initialize`，同时维护 `handles`、`row_keys`、`index_keys`、`plan_physical_ids` 的相同排序与过滤关系。若能力属于存储、会话或编码设施，应扩展 `BatchPointGetRuntime` 及真实适配器，而不是把具体依赖硬编码进控制流。修改输出列逻辑应接入 `buildVirtualColumnInfo` 或 `Next` 的解码后阶段，确保 checksum 的 `[start, cursor)` 范围仍对应本批行。

涉及锁语义时要分别验证 `pessimistic_read_consistency() == false/true`：前者必须覆盖存在和不存在候选键并在读取前锁，后者只能在读取后锁实际存在键。修改 `LockKeys` 时还需保持“先检查最大执行时间、锁成功后仅在悲观事务缓存返回值”的顺序。缓存 getter 若新增选项，应同步审查三个拒绝 `return_commit_ts` 的入口。

测试应放在独立的 `pkg/executor/batch_point_get_test.rs`，不要内嵌回生产文件。至少同步覆盖：重复索引值去重、升降序及无符号句柄排序、全局/本地索引分区过滤、无效物理 ID、索引行不一致与弱一致性、RR/RC 锁键集合、初始化失败后的状态、Chunk 分页及虚拟列。需要保持 Go 行为时还应对照 `pkg/executor/batch_point_get_test.go` 和 `pkg/executor/executor_failpoint_test.go` 的并发场景。兼容性风险主要是错锁/漏锁和输出顺序变化；性能风险主要是额外克隆、全量排序、两次 `batch_get` 以及 `BTreeMap/BTreeSet` 的对数开销。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标源码可由 `node --file pkg/executor/batch_point_get.rs` 完整读取（560 行）。
- RustCodeGraph 源码/符号查询：读取 `pkg/executor/batch_point_get.rs:1-560`；`query BatchPointGetExec --kind struct` 同时定位 Go/Rust 类型；`query cacheTableSnapshot --kind struct` 定位两端对应实现；`query LockKeys --kind function` 定位 Rust `batch_point_get.rs:528` 与 Go `batch_point_get.go:505`。
- RustCodeGraph 调用链：`node pkg/executor/builder.rs::buildBatchPointGet` 确认 `builder.rs:2381 build` 调用 `builder.rs:5090 buildBatchPointGet`，后者调用 `ExecutorBuilderDependencies::build_batch_point_get_executor`。精确查询 `BatchPointGetExec::Next/initialize` 只命中 Go method，未产生本文件 Rust 泛型方法的 callers/callees，故本文没有虚构直接生产调用边。
- crate/模块：读取 `pkg/executor/Cargo.toml` 与 `pkg/executor/lib.rs:60,527-528`，确认 crate 边界、`Chunk`/failpoint 依赖、公开模块及独立测试模块。
- Go 对照：通过 RustCodeGraph 完整读取 `pkg/executor/batch_point_get.go:1-591`，核对结构、生命周期、两阶段读取、锁语义、缓存与差异。
- 测试：通过 RustCodeGraph 完整读取 `pkg/executor/batch_point_get_test.rs:1-212` 和 `pkg/executor/batch_point_get_test.go:1-233`；前者直接验证本地索引分区过滤及悲观锁缓存 getter，后者提供锁并发、缓存快照与临时表行为证据。`pkg/executor` 下不存在 `doc.go`，因此没有额外包契约可读取。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的结构命令确认目标存在且恰有 11 个固定二级章节，并人工复核没有把索引缺失的接线写成已验证事实。
