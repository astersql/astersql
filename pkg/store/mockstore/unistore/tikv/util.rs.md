# `pkg/store/mockstore/unistore/tikv/util.rs`

## 文件定位

本文件属于 `astersql-store-mockstore-unistore-tikv` crate；crate 根由 `pkg/store/mockstore/unistore/tikv/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`lib.rs` 通过 `pub mod util` 公开本模块。它为进程内 mock TiKV 提供三组无状态工具：Region 右边界判断、Latch 键指纹集合构造、字节缓冲区独立拷贝。

当前 Rust 生产接线主要位于 `server.rs`：`Server::with_latches` 用 `keys_to_hash_values` 为同键写操作获取和释放 Region Latch，`injected_pessimistic_deadlock` 用同一函数产生 `MvccError::Deadlock.deadlock_key_hash`。`exceed_end_key`、Mutation/user-key 转换和 `safe_copy` 已有实现与独立测试，但在当前 Rust 生产代码中未发现调用；不能据 Go 版更广的调用面宣称 Rust 已完成对应接线。

## 核心职责

1. `exceed_end_key` 解释 TiKV Region 的半开区间右边界：空 `end_key` 表示无上界，非空时 `current >= end_key` 即已到达或越过边界。
2. `fingerprint64` 在本地实现 Go `github.com/dgryski/go-farm` 的 `farm.Fingerprint64`（FarmHash NA），使 Rust 生成的 Latch/死锁键哈希与 Go 位级一致。
3. `sort_and_dedup_hash_values` 及三个键转换入口将多个键归一化为升序、无重复的 `Vec<u64>`。稳定的全局获取顺序避免不同请求以相反顺序争用多个 Latch，重复键也不会重复获取同一 Latch。
4. `safe_copy` 将借用的字节切片复制到独立所有权的 `Vec<u8>`，避免返回值继续别名调用方缓冲区。

这些函数不负责真正加锁、Region 元数据管理、MVCC 写入或错误封装；实际 Latch 生命周期由 `region.rs` 的 `RegionContext::{acquire_latches,release_latches}` 与 `server.rs` 的 `Server::with_latches` 管理。

## 主要符号

- `pub fn exceed_end_key(current: &[u8], end_key: &[u8]) -> bool`：按 Rust 字节切片的字典序比较右边界；先检查 `end_key.is_empty()`，从而保留“空值即正无穷”的约定。
- `pub fn sort_and_dedup_hash_values(mut values: Vec<u64>) -> Vec<u64>`：取得输入所有权；元素多于一个时执行 `sort_unstable` 和 `dedup`，返回升序唯一集合。空或单元素输入原样返回。
- `pub fn mutations_to_hash_values(mutations: &[Mutation]) -> Vec<u64>`：读取每个 `mvcc::Mutation.key`，求指纹后归一化。
- `pub fn keys_to_hash_values(keys: &[Vec<u8>]) -> Vec<u64>`：处理拥有型原始键列表；这是当前 `server.rs` 实际使用的入口。
- `pub fn user_keys_to_hash_values<T: AsRef<[u8]>>(keys: &[T]) -> Vec<u64>`：处理任意可借用为字节的 user key。Rust 没有暴露 Go `badger/y.Key`，故调用方需先提供其 UserKey 视图。
- `pub fn safe_copy(value: &[u8]) -> Vec<u8>`：调用 `to_vec` 分配并复制。
- `pub fn fingerprint64(value: &[u8]) -> u64`：按长度选择 `0..=16`、`17..=32`、`33..=64` 或长输入路径。
- `FARM_K0`、`FARM_K1`、`FARM_K2`：FarmHash 固定混合常量。
- `fetch64`、`fetch32`：以 little-endian 读取固定宽度块；仅由已检查长度的分支调用。
- `shift_mix`、`hash_len_16_mul`、`hash_len_0_to_16`、`hash_len_17_to_32`、`weak_hash_len_32_with_seeds`、`hash_len_33_to_64`：FarmHash 内部混合步骤，均为模块私有实现细节。

本文件没有 struct、enum、trait、impl 或条件编译项。

## 执行流程

Latch 主链如下：

1. `server.rs::Server::with_latches` 接收本次写操作的 `&[Vec<u8>]`。
2. `keys_to_hash_values` 对每个键调用 `fingerprint64`，再由 `sort_and_dedup_hash_values` 生成升序唯一哈希。
3. `RegionContext::acquire_latches(&hashes)` 按该集合获得闩锁；闭包执行 MVCC 操作；随后 `release_latches(&hashes)` 使用完全相同的集合释放闩锁。
4. `server.rs::injected_pessimistic_deadlock` 也用 `keys_to_hash_values(&[key.to_vec()])[0]` 填充注入错误的死锁哈希，保持对外错误与正常 Latch 指纹算法一致。

`fingerprint64` 对不超过 64 字节的输入采用专用定长组合；更长输入按 64 字节块迭代维护 `v/w/x/y/z` 状态，再从覆盖输入尾部的最后 64 字节完成混合。所有算术使用 `wrapping_*`，这是哈希算法规定的模 `2^64` 行为，不是溢出恢复策略。

`exceed_end_key` 与 `safe_copy` 是单步纯函数；三个 `*_to_hash_values` 入口均遵循“映射为指纹，再排序去重”的相同流程。

## 数据与状态

本文件自身没有全局可变状态。FarmHash 常量是编译期 `u64`；哈希内部状态全部位于栈上的局部变量。键、Mutation 和字节切片只读借用；只有 `sort_and_dedup_hash_values` 消耗并原地整理传入的 `Vec<u64>`，`safe_copy` 则产生新的所有权。

关键不变量是：同一字节序列必须稳定得到与 Go `farm.Fingerprint64` 相同的 `u64`；返回的哈希向量必须严格升序且无重复；空 Region 右边界绝不能阻止扫描。哈希碰撞在理论上可能发生，因此这里提供的是 Latch 分桶/串行化标识而非键的唯一身份；碰撞会增加无谓串行化，但不会把不同哈希集合恢复为原键。

复杂度方面，哈希总成本与键总字节数线性相关；对 `n` 个键的归一化另需 `O(n log n)` 排序和 `O(n)` 去重。`safe_copy` 的时间与额外空间均为 `O(value.len())`。

## 依赖与调用关系

直接 Rust 依赖只有 `crate::mvcc::Mutation` 和标准库；FarmHash 被内嵌实现，因此 `Cargo.toml` 无需声明 Rust FarmHash 第三方 crate。`Cargo.toml` 的 porting 元数据将本 crate 对应到 Go 包 `pkg/store/mockstore/unistore/tikv`；大部分内部 AsterSQL 依赖仅在 Windows target 表中声明，本文件不直接使用它们。

经 RustCodeGraph 的文件节点确认，索引记录本文件被 `server.rs`、`server_test.rs`、`util_test.rs` 和跨 crate 的 `pkg/session/runtime/scan_adapter_runtime_test.rs` 使用。源码交叉核验到的关键边为：

- `server.rs::Server::with_latches -> keys_to_hash_values -> fingerprint64 -> sort_and_dedup_hash_values`；随后哈希传入 Region Latch 的 acquire/release。
- `server.rs::injected_pessimistic_deadlock -> keys_to_hash_values`，结果写入 `MvccError::Deadlock.deadlock_key_hash`。
- `util_test.rs` 直接覆盖所有公开函数；`server_test.rs` 和 `scan_adapter_runtime_test.rs` 以死锁错误字段间接验证公开哈希入口。

RustCodeGraph 对公共短名称的首次 `explore` 混入大量同名符号，按节点 ID 的批量 callers/callees 又在 30 秒内未返回；因此上述精确调用边同时使用图的文件级 `used by` 结果和 `rg` 对目标目录的直接引用完成消歧。

## 错误处理与边界

公开函数不返回 `Result`，也不主动产生业务错误。边界行为如下：

- `exceed_end_key`：空上界永远返回 `false`；非空上界下，相等也返回 `true`，与半开区间 `[start,end)` 一致。
- 排序去重与三个转换函数：空输入得到空向量；重复键或哈希只保留一个值。
- `safe_copy`：空切片得到独立的空 `Vec`。Rust `&[u8]` 没有 Go `nil` slice 状态，因而不能保留 Go `slices.Clone(nil) == nil` 的 nil/empty 区分；本 crate API 只表达字节内容。
- `fetch32`/`fetch64` 内有固定宽度切片和 `expect`，若偏移非法会 panic；当前所有调用偏移都由 `fingerprint64` 的长度分支保证，外部无法直接调用这些私有函数。
- 长输入路径的 `last64` 与块切片依赖 `len > 64` 分支；零长到 64 字节不会进入该路径。

若修改 FarmHash 分支阈值、尾块公式、字节序或把 `wrapping_*` 换成普通算术，可能破坏 Go 兼容并影响 Latch/死锁哈希，属于兼容性风险而非普通重构。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、文件句柄或网络资源，所有函数都是可并发调用的纯计算/复制操作。它对并发正确性的贡献来自输出契约：多键哈希先排序后再交给 Latch 层，确保竞争请求采用统一顺序；获取和释放必须复用同一向量。

`Server::with_latches` 当前在执行操作后显式释放 Latch，并未在本文件中使用 RAII guard；若业务闭包 panic，正常的显式释放步骤不会执行。该生命周期风险属于调用方 `server.rs`，不能通过本工具文件的哈希逻辑解决。正常 `Result` 成功或失败都会在错误转换前执行释放。

`safe_copy` 的返回值拥有独立分配，借用在函数返回时结束，之后修改原缓冲区不会影响副本；`util_test.rs::safe_copy_has_independent_storage` 直接验证了这一点。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/tikv/util.go`，符号对应为 `exceedEndKey`/`exceed_end_key`、`sortAndDedupHashVals`/`sort_and_dedup_hash_values`、`mutationsToHashVals`/`mutations_to_hash_values`、`keysToHashVals`/`keys_to_hash_values`、`userKeysToHashVals`/`user_keys_to_hash_values`、`safeCopy`/`safe_copy`。

两版的核心语义一致：空上界无界、非空上界按字节序比较；所有键使用 FarmHash Fingerprint64；哈希结果排序去重；复制结果不共享原缓冲区。Rust 用自包含 FarmHash NA 实现替代 Go 的 `go-farm` 依赖，并由 `util_test.rs::fingerprint_matches_go_farmhash` 的固定向量覆盖长度 `0/1/11/17/33/65/129` 的各算法路径。

接口差异包括：Go `keysToHashVals`/`userKeysToHashVals` 是 variadic，Rust 接收 slice；Go Mutation 是 kvproto 指针，Rust 使用本 crate 的拥有型 `mvcc::Mutation`；Go user-key 入口读取 `y.Key.UserKey`，Rust 泛型入口接收已经抽取、实现 `AsRef<[u8]>` 的视图；Go `safeCopy(nil)` 保留 nil，而 Rust slice API 没有 nil 状态。

调用覆盖尚不对等：Go `mvcc.go` 广泛调用 mutation/key 哈希、边界判断和安全复制，`write.go` 使用边界判断及 user-key 哈希，`mock_region.go` 使用安全复制；当前 Rust 生产引用搜索只发现 `server.rs` 使用 `keys_to_hash_values`。这是当前迁移/接线事实，不应把已存在的工具函数等同于所有 Go 调用点均已移植。

## 扩展指南

- 新增键来源时，优先复用 `fingerprint64` 加 `sort_and_dedup_hash_values`，或新增语义明确的薄转换入口；不要在调用方另选哈希算法或跳过排序去重。
- 修改 FarmHash 时必须同步扩充 `util_test.rs::fingerprint_matches_go_farmhash`，尤其覆盖长度分界 `0/4/8/16/17/32/33/64/65`、非整 64 字节尾部和超长输入，并用 Go `farm.Fingerprint64` 生成独立期望值。
- 修改 Region 边界语义时同步更新 `util_test.rs::end_key_comparison_keeps_unbounded_ranges_open`，并检查 Rust 中真正执行扫描的模块；不要因本函数存在就假设所有扫描已调用它。
- 修改哈希集合构造时同步更新 `util_test.rs::hashes_are_sorted_deduplicated_for_keys_and_mutations`，并验证 `server.rs::Server::with_latches` 的获取/释放集合仍完全一致以及死锁错误哈希保持兼容。
- 修改复制 API 时同步更新独立测试 `util_test.rs::safe_copy_has_independent_storage`，明确空输入与所有权语义；Rust 单元测试应继续放在独立 `util_test.rs`，不要内嵌到生产文件。
- 性能优化应保留位级输出；可重点基准长键哈希、批量排序和重复键比例。切换哈希实现会改变并发分桶与协议可观察的死锁字段，必须视为兼容性变更。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/store/mockstore/unistore/tikv/util.rs` 确认目标文件含 20 个符号；`node --file ... --offset 1 --limit 400` 读取了完整 266 行并给出四个使用文件；精确 `query --json` 定位了各公开函数及其文件节点 ID。
- 生产源码：`pkg/store/mockstore/unistore/tikv/util.rs`（全部公开入口与 FarmHash 私有实现）、`lib.rs`（模块公开）、`server.rs`（Latch 与死锁哈希调用）、`region.rs`（Latch 获取/释放）、`mvcc.rs`（`Mutation` 定义）。
- crate 配置：`pkg/store/mockstore/unistore/tikv/Cargo.toml`（crate 名、lib 入口、Go 包映射及依赖边界）。
- Go 对照：`pkg/store/mockstore/unistore/tikv/util.go`；调用面核验了同目录 `mvcc.go`、`write.go`、`mock_region.go`、`server.go` 的直接引用。
- 测试：`pkg/store/mockstore/unistore/tikv/util_test.rs`、`server_test.rs`、`pkg/session/runtime/scan_adapter_runtime_test.rs`；另读 `pkg/store/mockstore/unistore/tikv/util_test.go` 核对 Go 的边界、排序去重与复制语义。
- 本任务只新增说明文档，未运行 Cargo。结构由任务指定的 11 章节命令验证；人工复核区分了已实现、已接线与仅测试覆盖的结论。
