# `br/pkg/restore/internal/prealloc_table_id/alloc.rs`

## 文件定位

[`alloc.rs`](alloc.rs) 是 `astersql-br-pkg-restore-internal-prealloc-table-id` library crate 的算法实现文件；[`lib.rs`](lib.rs) 以 `#[path = "alloc.rs"] pub mod alloc` 挂载并扁平再导出全部公开项。该 crate 的 [`Cargo.toml`](Cargo.toml) 把 Go 来源标为 `br/pkg/restore/internal/prealloc_table_id`，运行时依赖只有 `sha2 = "0.10"`，其余 meta、checkpoint 与错误类型都在本文件内以最小局部模型表达。

它解决恢复前的表 ID/分区 ID 冲突：从备份表定义收集原 ID，读取目标集群的全局 ID 水位，保留仍安全的原 ID，并把冲突或异常大的 ID 映射到新预占区间。checkpoint 可保存区间和输入 ID 摘要，供重启后验证并重建相同映射。

当前接线有明确边界：RustCodeGraph 将 `NewAndPrealloc`、`ReuseCheckpoint`、`RewriteTableInfo` 的调用边定位在本文件及独立测试中；restore 主链中的 `SnapClient::AllocTableIDs` 则调用 [`snap_client/stubs.rs`](../../snap_client/stubs.rs) 的 `NewAndPreallocTableIDs` / `ReusePreallocatedTableIDs`，不是本 crate 的同名算法。因此本文件是已实现且有契约测试的独立移植 crate，但不能据此声称 snap restore 主链已经统一使用它。

## 核心职责

1. `collectTableIDs` 收集每张表的 `TableInfo.ID` 和所有 `PartitionDefinition.ID`，排序后返回，并从低于 `InsaneTableIDThreshold` 的 ID 中计算正常最大值。
2. `New` 建立“尚未执行预分配”的 `PreallocIDs`，确定 `reusable_border = max_id + 1`，保存排序 ID 的 SHA-256 摘要，并保留待分配 ID。
3. `PreallocIDs::PreallocIDs` 从 `Allocator` 读取当前全局水位，为冲突 ID 建立重写规则，并用一次 `AdvanceGlobalIDs` 预占完整半开区间 `[start, end)`。
4. `AllocID` 和 `RewriteTableInfo` 消费映射：前者改写单个 ID，后者克隆表信息并改写表 ID 与全部分区 ID，不修改输入对象。
5. `CreateCheckpoint` 持久化区间边界和输入摘要；`ReuseCheckpoint` 校验表集合与边界后，在不再次推进全局 ID 的前提下重建映射。

## 主要符号

- `InsaneTableIDThreshold: i64 = u32::MAX as i64`：正常表 ID 的上限阈值。高于或等于阈值的 ID 会被收集和哈希，但不参与 `max_id`，分配时强制重写；容量检查确保正常最大 ID 加待处理数量仍不越界。
- `Allocator`：仅要求 `GetGlobalID(&mut self)` 与 `AdvanceGlobalIDs(&mut self, usize)`。该抽象把算法与真实 meta/事务实现隔离；文件自身不持有数据库连接或事务。
- `PreallocIDs`：核心状态机。`start`/`end` 表示已预占半开区间，`reusable_border` 是新生成 ID 的起点下界，`hash` 绑定排序后的输入 ID 集合，`unalloced_ids` 区分待分配与已分配状态，`alloc_rule` 保存原 ID 到恢复侧 ID 的映射。
- `NewAndPrealloc`：便捷入口，依次调用 `New` 和实例方法 `PreallocIDs`，分别包装“创建需求失败”和“预分配失败”的上下文。
- `New`、`collectTableIDs`、`computeSortedIDsHash`：准备阶段。哈希按排序后的每个 `i64` 转成 `u64` 大端 8 字节后连续写入 SHA-256。
- `ReuseCheckpoint`：checkpoint 恢复入口，核对 `ReusableBorder` 与当前表最大 ID、核对哈希，并按旧区间重建 `alloc_rule`。
- `GetIDRange`、`AllocID`、`RewriteTableInfo`、`CreateCheckpoint`：预分配结果的查询、消费、表结构复制改写和持久化接口。
- `Error`、`errors`、`berrors`、`checkpoint`、`model`、`metautil`：为独立 crate 提供的局部替身。`Error.code` 只在 checkpoint 范围错误中保留 `BR:Common:ErrInvalidRange`；这些类型不是完整 workspace 类型。
- `compute_sorted_ids_hash_for_test`、`prealloc_ids_already_allocated_for_test`：仅在 `cfg(test)` 下暴露，用于验证摘要和不可达状态守卫，不属于生产 API。

## 执行流程

正常新分配流程如下：

1. 调用者把表和分区元信息交给 `NewAndPrealloc`，或显式执行 `New` 后再调用 `PreallocIDs::PreallocIDs`。
2. `collectTableIDs` 遍历全部表/分区。正常 ID 更新 `max_id`，所有 ID（含异常大 ID）进入 `ids`；排序后检查 `max_id + ids.len() + 1` 不超过阈值。
3. `New` 设置 `reusable_border = max_id + 1`，计算排序 ID 摘要，并把 ID 列表放进 `unalloced_ids = Some(...)`。
4. `PreallocIDs` 调用 `GetGlobalID`，令 `start = current_id + 1`；若原 `reusable_border <= start`，把边界提升到 `start`。
5. 对每个原 ID：若 `id >= start && id < InsaneTableIDThreshold`，直接映射到自身；否则按遍历顺序映射到 `reusable_border + rewrite_cnt` 并递增计数。
6. 计算 `id_range = reusable_border - start + rewrite_cnt`，调用 `AdvanceGlobalIDs(id_range)` 一次性预占，随后设置 `end = start + id_range` 并清空 `unalloced_ids`。
7. `RewriteTableInfo` 克隆输入，先用 `AllocID` 改写表 ID，再逐项改写分区定义 ID；任一步失败都返回带原 ID 上下文的错误。

checkpoint 复用流程不调用 `Allocator`。`ReuseCheckpoint` 重新收集并哈希当前表集合，拒绝过小的 `ReusableBorder` 或不匹配的摘要；随后保留落在 `[legacy.Start, legacy.ReusableBorder)` 的原 ID，把小于 `Start` 或大于阈值的 ID 依次映射到 `ReusableBorder + rewrite_cnt`，并拒绝其余落在非法间隙的 ID。成功对象直接采用旧 `Start`、`End`、`Hash` 且 `unalloced_ids = None`。

空表是显式快速路径：`New` 和 `NewAndPrealloc` 返回 `start = i64::MAX, end = 0` 的空对象，不读取或推进 allocator，`Display` 输出 `ID:empty(end=0)`，`CreateCheckpoint` 返回 `None`。

## 数据与状态

`PreallocIDs` 的关键状态不变量是：

- 待分配：`unalloced_ids = Some(ids)`，`start = i64::MAX`、`end = 0`；此时 `AllocID` 必须报“not allocated yet”。
- 已分配或已复用：`unalloced_ids = None`，映射已构建；有效目标 ID 必须满足 `start <= rewrite_id < end`。
- 空输入：同样是 `unalloced_ids = None`，但 `start >= end`，因此没有 checkpoint，也没有可消费 ID。
- `reusable_border` 初始为正常最大原 ID 加一，随后至少提升到 `start`；冲突 ID 从该边界开始连续分配，避免与被保留原 ID 相撞。
- `hash` 绑定排序后的 ID 序列，所以表输入顺序不影响 `New`/`ReuseCheckpoint` 的摘要；测试辅助函数直接接收切片且不排序，因此其顺序敏感测试只验证底层编码函数。

`alloc_rule` 是 `HashMap<i64, i64>`。重复原 ID 会覆盖同一键，但 `id_range`/`rewrite_cnt` 仍按收集列表逐项计算；当前 Go 实现具有相同的 map 写入与逐项计数行为，扩展时不能擅自去重。`RewriteTableInfo` 使用 `Clone` 复制局部 `TableInfo`，所以状态变化只落在返回值，不回写传入结构。

## 依赖与调用关系

RustCodeGraph 核对到的文件内主调用边为：

- `NewAndPrealloc → New → collectTableIDs`，以及 `New → computeSortedIDsHash`；
- `NewAndPrealloc → PreallocIDs::PreallocIDs → Allocator::{GetGlobalID, AdvanceGlobalIDs}`；
- `ReuseCheckpoint → collectTableIDs/computeSortedIDsHash`；
- `RewriteTableInfo → TableInfo::Clone/AllocID`；
- `CreateCheckpoint` 生成局部 `checkpoint::PreallocIDs`；
- 测试入口 `go_rust_public_contract_matches` 调用 `compute_sorted_ids_hash_for_test` 与 `prealloc_ids_already_allocated_for_test`。

外部依赖只有 `sha2::{Digest, Sha256}`。标准库提供 `HashMap`、格式化与错误 trait。由于 meta/checkpoint/error 均为局部替身，该 crate 无 PD、KV、事务或异步运行时依赖。

上游方面，[`lib.rs`](lib.rs) 再导出 API，并挂载 [`alloc_test.rs`](alloc_test.rs) 与 [`parity_test.rs`](parity_test.rs)。RustCodeGraph 没有给出本 crate API 的生产调用者；它同时确认 [`snap_client/client.rs`](../../snap_client/client.rs) 的 `AllocTableIDs` 调用 `snap_client/stubs.rs` 中的平行实现。因此若未来把主链切换到本 crate，需要显式完成类型适配与 Cargo 依赖接线，不能只替换函数名。

## 错误处理与边界

- `collectTableIDs` 在 `max_id + ids.len() + 1 > InsaneTableIDThreshold` 时拒绝输入，防止预留空间逼近正常 ID 上界。
- `PreallocIDs` 读取全局 ID 失败时打印错误并原样返回；`AdvanceGlobalIDs` 失败也原样传播。通过 `NewAndPrealloc` 调用时，两类错误再包装为 `failed to allocate prealloc IDs`。
- `PreallocIDs` 在待处理列表为空或已经清空时幂等返回；仅人工构造出 `start < end` 且仍有 pending IDs 的矛盾状态时返回“should only be allocated once”。
- `AllocID` 先拒绝未分配状态；map miss 按 Go map 语义得到 `0`，随后统一由区间检查返回“not in range”。
- `RewriteTableInfo(None)` 返回“table info is nil”；表或分区 ID 改写失败时分别添加具体原 ID 的上下文。
- `ReuseCheckpoint(None, ...)` 直接失败。边界过小、哈希不符或 ID 落入 `[ReusableBorder, ...]` 的非法区间时使用 `ErrInvalidRange` code，便于上层分类。
- `CreateCheckpoint` 对空/无效区间返回 `None`。与 Go 的 `p == nil` 防御不同，Rust 方法不能在空 `self` 上调用，空指针情况由类型系统排除。
- 算术使用普通 `i64` 加法和 `usize as i64` 转换；当前防线依赖 ID 规模和阈值约束。若未来允许超大集合，应评估显式 checked conversion/checked arithmetic。

## 并发与资源生命周期

本文件没有锁、线程、task、channel 或 `async`。`Allocator` 方法接收 `&mut self`，`PreallocIDs::PreallocIDs` 也接收 `&mut self`，在类型层面要求单次调用独占访问；共享与事务同步由未来的 allocator 实现负责。

资源生命周期是同步状态转换：`New` 持有 pending ID 向量；成功 `AdvanceGlobalIDs` 后才写 `end` 并将 `unalloced_ids` 置为 `None`。如果推进失败，`start`、可能提升后的 `reusable_border` 和 `alloc_rule` 已被部分写入，但 pending 向量仍保留；调用者会收到错误，不应继续用该对象改写 ID。checkpoint 只有在 `start < end` 时产生，因而不会记录尚未成功推进的区间。

真正的全局 ID 原子性、事务提交和并发冲突不在本 crate 内实现。Go 的 `TestAllocatorBound` 使用 TiDB `meta.NewMutator` 验证真实事务水位；Rust [`alloc_test.rs`](alloc_test.rs) 明确说明当前平台没有 kv/domain，只以可变内存 allocator 覆盖相同的边界算法。因此“真实集群事务并发安全”在该 Rust crate 中尚未验证。

## 与 Go 版本的对应关系

主要算法与 [`alloc.go`](alloc.go) 逐项对应：常量阈值、`Allocator` 两方法、`PreallocIDs` 字段、空输入哨兵、ID 收集排序、正常 ID 保留规则、冲突 ID 连续重写、区间计算、表/分区克隆改写、checkpoint 字段和 SHA-256 大端编码均保持一致。`Display` 对应 Go `String()`。

Rust 为独立编译引入了局部差异：

- Go 使用真实 `metautil.Table`、`model.TableInfo`、`checkpoint.PreallocIDs` 和 PingCAP errors；Rust 文件只建模此算法需要的字段与错误 code。
- Go 的 `unallocedIDs == nil` 对应 Rust `Option<Vec<i64>>::None`；非 nil 空 slice 对应 `Some(vec![])`，两者在 `PreallocIDs` 的空列表快速返回上可观察行为相同，但 `AllocID` 会区分是否仍为 `Some`。
- Go `CreateCheckpoint` 可检查 nil receiver；Rust receiver 必然有效，只检查 `start >= end`。
- Go 哈希写入理论上处理 `hash.Write` 错误并 panic；Rust `Sha256::update` 不返回错误。
- Go 在获取全局 ID 失败时使用结构化 logger；Rust 当前使用 `eprintln!`，错误本身仍原样传播。

[`alloc_test.rs`](alloc_test.rs) 移植 Go `TestAllocator` 的用例矩阵，包括分区、已占用水位、异常大 ID 和大但仍正常的 ID；但 Go `TestAllocatorBound` 的真实 kv/meta 测试仅以本地 allocator 模拟。额外的 [`parity_test.rs`](parity_test.rs) 覆盖空输入、摘要、溢出边界、错误包装、重复调用守卫、checkpoint 成功/失败复用和分区改写。

## 扩展指南

- 新增 ID 来源（例如更多嵌套元数据）时，应首先扩展 `collectTableIDs`，并同步更新哈希输入、容量检查、映射建立和 `RewriteTableInfo`；遗漏任一处会造成 checkpoint 可复用判断或实际改写不一致。
- 改变保留/重写规则时，必须同时审查 `PreallocIDs::PreallocIDs` 与 `ReuseCheckpoint`。新分配和重启复用必须为同一输入生成相同映射，并保持半开区间不变量。
- 扩展 checkpoint 字段时，要同步局部 `checkpoint::PreallocIDs`、`CreateCheckpoint`、`ReuseCheckpoint`，并考虑与 Go checkpoint 的序列化兼容；当前局部类型本身不负责落盘。
- 接入真实 restore 主链时，应替换或收敛 `snap_client/stubs.rs` 的平行实现，适配真实 `metautil/model/checkpoint` 类型和 `DbSession`，并在对应 Cargo manifest 中建立唯一依赖方向，避免两套算法继续漂移。
- 错误语义变化要保留 `ErrInvalidRange` code 与上下文包装，否则上层可能无法区分 checkpoint 不兼容与 allocator 基础设施失败。
- 测试必须放在独立文件。算法用例更新 [`alloc_test.rs`](alloc_test.rs)，Go/Rust 边界与错误契约更新 [`parity_test.rs`](parity_test.rs)；若完成生产接线，还需在 snap-client 的独立测试中证明实际调用已切换。不要把测试内嵌回 `alloc.rs`，现有两个 `cfg(test)` helper 只提供测试可达性。
- 性能上，当前复杂度主要是收集 O(n)、排序 O(n log n)、建表 O(n)，内存为 ID 向量与映射 O(n)。批量恢复规模扩大时，避免额外复制或重复排序，并用基准数据确认 HashMap 容量和摘要成本。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/restore/internal/prealloc_table_id` 确认 `alloc.rs` 有 33 个符号，并列出 Go/Rust 实现与独立测试。
- RustCodeGraph `node --file br/pkg/restore/internal/prealloc_table_id/alloc.rs --offset 1 --limit 560`：核对全部 498 行源码、公开 API、状态字段、条件编译 helper 和算法分支。
- RustCodeGraph `explore`：核对 `NewAndPrealloc → New/PreallocIDs`、`New/ReuseCheckpoint → collectTableIDs/computeSortedIDsHash`、`RewriteTableInfo → AllocID` 等调用边；未发现本 crate API 的生产调用边。
- RustCodeGraph 对 [`snap_client/client.rs`](../../snap_client/client.rs) 与 [`snap_client/stubs.rs`](../../snap_client/stubs.rs) 的节点/调用结果：确认当前 `AllocTableIDs` 使用平行的 `NewAndPreallocTableIDs` / `ReusePreallocatedTableIDs` 实现。
- crate 边界：[`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)。前者确认 library 名、Go 来源 metadata 和唯一外部依赖 `sha2`；后者确认再导出与独立测试挂载。
- Go 对照：[`alloc.go`](alloc.go) 与 [`alloc_test.go`](alloc_test.go)，用于核对生产算法、真实 meta allocator 边界测试和错误/区间语义。
- Rust 测试：[`alloc_test.rs`](alloc_test.rs) 与 [`parity_test.rs`](parity_test.rs)，用于核对纯算法用例矩阵、平台验证限制、checkpoint、哈希、错误路径和表/分区改写。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付验证仅检查固定章节结构、链接/路径和事实依据。
