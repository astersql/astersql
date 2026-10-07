# `pkg/lightning/verification/checksum.rs`

## 文件定位

本文件实现 Lightning/IMPORT INTO 数据导入链路使用的本地 KV 校验和，是 workspace crate `astersql-lightning-verification` 的主体。crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，未声明第三方 Rust 依赖；[`lib.rs`](lib.rs) 以私有 `mod checksum` 挂载本文件，再用 `pub use checksum::*` 暴露全部公开项。workspace 根 `Cargo.toml` 将该 crate 纳入成员，并由 importer、session、dxf/importinto 以及 Lightning backend 等 crate 通过路径依赖复用。

它不负责读取源数据、编码 SQL 行、写入 TiKV 或发起远端校验。它只维护本地摘要：每条 `keyspace || key || value` 的 CRC64-ECMA 值以 XOR 聚合，同时累计 KV 数和字节数；分组层再将记录 KV 与各索引 KV 分开统计。典型入口是 `pkg/executor/importer/chunk_process.rs::EncodedKVGroupBatch::Add`，典型终点是 `pkg/executor/importer/table_import.rs::PostProcess` 将 `MergedChecksum` 与远端结果比较。

## 核心职责

1. `KVChecksum` 对单组 KV 维护 `(checksum, bytes, kvs)` 三元组；`UpdateOne`/`Update` 从原始 KV 生成摘要，`Add`/`Sub` 合并或抵消已有摘要。
2. `KVGroupChecksum` 以 `i64` group ID 隔离数据记录与不同索引。`DataKVGroupID == -1` 表示数据组，其余 ID 按索引组处理；该约定被 `pkg/dxf/importinto/clean_up.rs` 等调用方直接使用。
3. keyspace 参与每条 KV 的 CRC 输入及字节计数。`NewKVChecksumWithKeyspace` 预计算 keyspace CRC 到 `base`，避免每次从零重复处理前缀。
4. `MakeKVChecksum`、`AddRawGroup` 和 `MergedChecksum` 在“已有三元组”与运行时对象之间转换，支持 checkpoint、分布式任务元数据及后处理聚合，而无需保留原始 KV。
5. `String`/`MarshalJSON` 与 `LogEncoder` 提供稳定的诊断表示；它们不参与校验计算。

## 主要符号

- `ECMA_REVERSED_POLYNOMIAL: u64`：反射 CRC64-ECMA 多项式 `0xc96c_5795_d787_0f42`，仅供内部 `update_crc` 使用。
- `KvPair { key, val }`：公开的拥有型 KV 数据载体；调用方传借用给更新方法。
- `update_crc(initial, bytes)`：私有 CRC 更新器。对累加器输入、输出取反并逐位应用反射多项式，因此以 `base`、key、value 分段调用等价于连续处理字节流。
- `KVChecksum { base, prefix_len, bytes, kvs, checksum }`：字段私有的单组状态。公开构造器为 `NewKVChecksum`、`NewKVChecksumWithKeyspace`、`MakeKVChecksum`、`MakeKVChecksumWithKeyspace`。
- `KVChecksum::UpdateOne`/`Update`：计算新 KV 的 CRC，并回绕增加字节数、KV 数，再 XOR 到聚合值。
- `KVChecksum::Add`/`Sub`：统计字段分别使用回绕加/减；checksum 两者都使用 XOR，因为 XOR 自逆。
- `Sum`、`SumSize`、`SumKVS`：只读导出三元组，调用方不接触内部字段。
- `LogEncoder`：本 crate 定义的最小日志编码 trait；`KVChecksum::MarshalLogObject` 写入 `cksum`、`size`、`kvs`，`KVGroupChecksum::MarshalLogObject` 以 `id=<group>` 写嵌套对象。
- `DataKVGroupID: i64 = -1`：数据记录组的哨兵 ID。
- `KVGroupChecksum { groups, keyspace }`：内部 `HashMap<i64, KVChecksum>` 加新组所需的 keyspace 副本。
- `NewKVGroupChecksumWithKeyspace`：预置数据组；索引组由 `UpdateOneIndexKV`/`getOrCreateOneGroup` 懒创建。
- `NewKVGroupChecksumForAdd`：以空 keyspace 建立聚合容器，适合只合并已有摘要的路径。
- `AddRawGroup`：将 wire/checkpoint 三元组还原为临时 `KVChecksum` 并合入指定组。
- `DataAndIndexSumSize`/`DataAndIndexSumKVS`：返回数据组数值与所有索引组数值的二元汇总。
- `GetInnerChecksums`：深克隆 map 与每个 `KVChecksum`，调用方修改返回值不会反向修改原对象。
- `MergedChecksum`：忽略分组边界，将所有组通过 `KVChecksum::Add` 合为单一摘要。

文件没有条件编译项；测试挂载发生在 `lib.rs` 的 `#[cfg(test)]` 中。

## 执行流程

单条更新流程如下：

1. 构造器把 keyspace 的 CRC 写入 `base`，把其长度写入 `prefix_len`；无 keyspace 构造器使用两者的零值。
2. `UpdateOne` 先执行 `update_crc(base, key)`，再以结果执行 `update_crc(..., val)`，得到该条 `keyspace || key || value` 的 CRC。
3. `prefix_len + key.len() + val.len()` 以 `u64` 回绕累加到 `bytes`，`kvs` 回绕加一，单条 CRC 与 `checksum` XOR。
4. `Update` 逐条调用 `UpdateOne`，因此空切片是无操作，批量与逐条处理语义一致。

分组导入流程如下：

1. `NewKVGroupChecksumWithKeyspace` 创建 `DataKVGroupID` 数据组。
2. `pkg/executor/importer/chunk_process.rs::EncodedKVGroupBatch::Add` 用 `is_record_key` 分类：记录键调用 `UpdateOneDataKV`，索引键先解出 index ID，再调用 `UpdateOneIndexKV`。
3. 索引组首次出现时按容器保存的 keyspace 构造；数据组在构造时已存在，`UpdateOneDataKV` 直接取该组更新。
4. 并行/分片结果由上层持锁后调用 `KVGroupChecksum::Add` 汇总。例如 `pkg/dxf/importinto/encode_and_sort_operator.rs::GlobalWriterSummaries::merge_checksum` 在 `Mutex` 内合并任务摘要。
5. 后处理调用 `MergedChecksum` 得到全表三元组。`pkg/executor/importer/table_import.rs::PostProcess` 把它交给 `VerifyChecksum`；`pkg/dxf/importinto/subtask_executor.rs::buildFinalChecksum` 还会用 `Sub` 扣除冲突解决删除行的摘要。

## 数据与状态

`KVChecksum` 的核心不变量是：`checksum` 是已接收每条完整 KV CRC 的 XOR；`kvs` 是接收条数；`bytes` 是每条 `prefix_len + key.len() + val.len()` 的总和。XOR 与加法都满足交换、结合性质，因此在没有溢出语义差异时，分片合并顺序不会改变最终三元组。计数明确使用 `wrapping_add`/`wrapping_sub`，与 Go `uint64` 的模 `2^64` 运算一致；欠减不会报错，而会回绕。

`base` 和 `prefix_len` 只影响之后的 `UpdateOne`，不会随 `Add`/`Sub` 合并。因而 `MakeKVChecksum`/`AddRawGroup` 表示“已经算好的摘要”，不能在不知道原 keyspace 的情况下用于后续原始 KV 更新；需要继续更新时应使用 `MakeKVChecksumWithKeyspace`。不同 keyspace 生成的摘要虽然类型相同，但调用方必须保证不会错误混合，本文件不做一致性检查。

`KVGroupChecksum` 始终由公开构造器建立数据组；索引组按需出现。`GetInnerChecksums` 返回拥有型深克隆，`MergedChecksum` 也返回新对象，因此读取快照不会借出内部可变状态。`HashMap` 不保证遍历顺序，但 XOR、回绕加法及总和汇总对顺序不敏感；分组日志字段的输出顺序则不稳定。

## 依赖与调用关系

本文件的直接标准库依赖只有 `std::collections::HashMap` 和 `std::fmt`，CRC 实现内置在 `update_crc`，没有调用外部 CRC crate。crate 本身由 `pkg/lightning/verification/Cargo.toml` 标记 `go-package = "pkg/lightning/verification"`，且没有 feature 声明。

已核实的主要上游调用关系：

- `pkg/executor/importer/chunk_process.rs` 创建 `KVGroupChecksum` 并在编码批次中调用 `UpdateOneDataKV`/`UpdateOneIndexKV`。
- `pkg/executor/importer/table_import.rs`、`pkg/session/runtime/import_query.rs` 在导入完成后调用 `MergedChecksum`，再比较本地与远端 checksum、KV 数和字节数。
- `pkg/dxf/importinto/encode_and_sort_operator.rs` 将多个任务的 `KVGroupChecksum` 合并；`subtask_executor.rs::buildFinalChecksum` 通过 `AddRawGroup` 从任务元数据还原分组，并用 `Sub` 扣除已删除冲突行。
- `pkg/dxf/importinto/proto.rs::newFromKVChecksum` 和 `Checksum::ToKVChecksum` 在三元组 wire 结构与本类型之间往返。
- `pkg/lightning/backend/kv/sql2kv.rs::Pairs::ClassifyAndAppend` 对记录/索引 KV 分别更新两个 `KVChecksum`。
- `pkg/lightning/backend/tidb/tidb.rs` 用 `MakeKVChecksum` 构造已有统计并合并。

RustCodeGraph `files --filter pkg/lightning/verification` 确认该目录索引覆盖 `checksum.rs`、`checksum_test.rs`、Go 对照和 Go 测试；`node --file ...` 显示本文件被 45 个索引文件使用。精确符号查询确认了 Rust/Go 两侧的 `KVChecksum`、`KVGroupChecksum`、`NewKVChecksumWithKeyspace`、`UpdateOne` 和 `MergedChecksum`。调用边子命令在共享索引上未返回结果，因此上述具体边以调用点源码搜索和局部读取复核，没有把超时结果当作事实。

## 错误处理与边界

校验计算、构造和聚合 API 均不返回业务错误。空输入不改变状态；空 key/value 合法；空 keyspace 等价于不加前缀。计数溢出和欠减采用回绕，这是兼容行为而非异常。调用者必须保证 `Sub` 的逻辑对象确实包含在被减摘要中，也必须保证相加的摘要使用兼容 keyspace；类型系统不会验证这些前置条件。

`UpdateOneDataKV` 对预置数据组调用 `unwrap()`。经公开构造器创建时该组必然存在，且没有公开 API 能删除它，所以正常使用不触发 panic。`UpdateOneIndexKV` 不验证 key 是否真属于传入 index ID，`UpdateOneDataKV` 也不验证记录键；分类正确性属于上层编码器职责。

`KVChecksum::MarshalLogObject` 的三个 `AddUint64` 不可失败，最终总是 `Ok(())`。`KVGroupChecksum::MarshalLogObject` 会用 `?` 立即传播 `AddObject` 的首个错误，可能已经写入部分组。`MarshalJSON` 是手工格式化且不可失败，字段顺序固定为 `checksum`、`size`、`kvs`；它不是通用 serde 接口，也不做转义，因为内容只有整数和固定字段名。

## 并发与资源生命周期

两个 checksum 类型都只包含拥有型内存，没有文件句柄、网络连接、后台任务或显式析构逻辑。更新方法要求 `&mut self`，单个实例本身不提供内部同步；跨线程共享由调用方承担。例如 importer 把它放入 `Arc<Mutex<KVGroupChecksum>>`，编码线程在锁内更新或合并，结束后再取锁生成最终摘要。

keyspace 在 `KVGroupChecksum` 构造时复制为 `Vec<u8>`，因此不依赖调用者切片的生命周期。懒建组时当前实现临时克隆 keyspace，以避开同时可变借用 `groups` 与借用 `keyspace` 的冲突；克隆只发生在新 group 创建路径。每次 `GetInnerChecksums` 会克隆完整 map，适合边界快照但在高频热路径可能产生分配成本。CRC 更新逐字节、逐 bit 执行，复杂度为输入字节数的线性函数，未使用查表或 SIMD。

## 与 Go 版本的对应关系

直接对照文件是 [`checksum.go`](checksum.go)，测试对照是 [`checksum_test.go`](checksum_test.go)。Rust 保留 Go 风格公开命名和主要数据模型：同样使用 CRC64-ECMA，按 `keyspace + key + value` 计算单条 CRC，以 XOR 聚合，并维护 `bytes`/`kvs`；`DataKVGroupID`、分组懒创建、`AddRawGroup`、data/index 汇总和最终合并也一一对应。

已确认的实现差异：

- Go 使用标准库 `hash/crc64` 表，Rust 用 `update_crc` 实现同一反射算法；Rust 固定样例测试得到与 Go 期望相同的 `4_850_203_904_608_948_940`。
- Go 的批量 `Update` 在一个循环中积累局部变量，Rust 复用 `UpdateOne`；结果相同，Rust 更直接但每条都会写回对象字段。
- Go 构造器返回指针，Rust 返回拥有型值并通过可变借用更新。
- Go `MarshalJSON` 返回 `([]byte, error)`，Rust 返回不可失败的 `Vec<u8>`；Go 直接实现 zap 接口，Rust 通过本地 `LogEncoder` trait 隔离日志后端。
- Go `GetInnerChecksums` 显式逐项复制指针指向的值，Rust `HashMap::clone` 产生同等的深值克隆。
- Go 注释规定 `NewKVGroupChecksumForAdd` 不应用于原始 KV 更新；Rust 构造器实际等价于空 keyspace 的普通分组对象，并未在类型或运行时禁止更新。安全扩展仍应遵守 Go 的用途约束，避免把“聚合已有摘要”与“从原始 KV 计算”混在同一实例中。
- Go 的无符号整数天然回绕；Rust 显式使用 wrapping 运算，使 debug/release 构建一致。

## 扩展指南

- 修改 CRC 算法、keyspace 拼接或字节计数时，应从 `update_crc`、`KVChecksum::UpdateOne` 和四个构造器一起审查；这些属于远端兼容协议，任何变化都必须与 Go `checksum.go`、TiDB/TiKV 远端校验口径及固定向量同步验证。
- 新增摘要字段时，应同步 `KVChecksum`、`Add`/`Sub`、访问器、`String`/`MarshalJSON`、两个 `MarshalLogObject` 路径，以及 `pkg/dxf/importinto/proto.rs` 等 wire/checkpoint 转换。字段加入 XOR 还是加减聚合必须明确其代数性质。
- 修改 group ID 或分类语义时，应同步 `DataKVGroupID`、`UpdateOneDataKV`、`UpdateOneIndexKV`、data/index 汇总方法，以及调用方 `chunk_process.rs`、`clean_up.rs` 中的分类逻辑。
- 若要并行化内部计算，不应直接给类型加入隐式锁；现有契约是值类型加 `&mut self`，同步边界由 importer 决定。优先保持分片本地累加、边界处 `Add` 合并。
- 性能优化可考虑 CRC 查表和减少新组创建时的 keyspace 克隆，但必须以 Go 固定向量、keyspace 用例和分片合并等价性为基准，不能只验证空 keyspace。
- 测试应继续放在独立的 `pkg/lightning/verification/checksum_test.rs`，不要内嵌到源文件。修改跨 crate 接线时还应补对应调用方的独立测试，例如 importer 的 `chunk_process_testkit_test.rs`、dxf/importinto 的 `encode_and_sort_operator_test.rs` 或 backend/kv 的 `sql2kv_test.rs`。

## 验证依据

- Rust 源码：`pkg/lightning/verification/checksum.rs`，完整 291 行；重点符号为 `update_crc`、`KVChecksum::UpdateOne`、`KVChecksum::{Add,Sub}`、`KVGroupChecksum::{Add,AddRawGroup,MergedChecksum}`。
- crate/模块：`pkg/lightning/verification/Cargo.toml`、`pkg/lightning/verification/lib.rs`；workspace 与直接路径依赖由根 `Cargo.toml` 及各调用 crate 的 `Cargo.toml` 核验。
- Go 对照：`pkg/lightning/verification/checksum.go`；Go 测试：`pkg/lightning/verification/checksum_test.go`。
- Rust 独立测试：`pkg/lightning/verification/checksum_test.rs`，覆盖固定 CRC、空批次、重复输入 XOR 自逆、JSON/Display、日志字段与错误传播、分组/合并、深克隆、keyspace、Add/Sub 以及 `u64` 回绕。
- 直接调用证据：`pkg/executor/importer/chunk_process.rs`、`pkg/executor/importer/table_import.rs`、`pkg/session/runtime/import_query.rs`、`pkg/dxf/importinto/subtask_executor.rs`、`pkg/dxf/importinto/encode_and_sort_operator.rs`、`pkg/dxf/importinto/proto.rs`、`pkg/lightning/backend/kv/sql2kv.rs`。
- RustCodeGraph：`status` 报告索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/lightning/verification` 返回 5 个相关文件；`node --file pkg/lightning/verification/checksum.rs --offset 1 --limit 500` 返回完整源码并报告 45 个使用文件；`query` 找到 Rust/Go 对应符号。`explore` 与 `callers/callees` 在本次共享索引环境中超时无输出，调用边改由精确 `rg` 与调用点源码读取验证。
- 人工一致性检查：文档区分了源码内纯计算责任与上层 IO/并发责任，并明确记录 keyspace、回绕、分组、日志顺序和调用者前置条件；未把未成功返回的图查询描述为已验证调用边。
