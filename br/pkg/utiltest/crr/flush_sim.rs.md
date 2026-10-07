# `br/pkg/utiltest/crr/flush_sim.rs`

## 文件定位

本文说明的真实源文件是 [`flush_sim.rs`](./flush_sim.rs)。它属于 `astersql-br-pkg-utiltest-crr` library crate；crate 边界由 `br/pkg/utiltest/crr/Cargo.toml` 的 `[lib] path = "lib.rs"` 确定，`lib.rs` 通过 `pub mod flush_sim` 纳入模块并用 `pub use flush_sim::*` 再导出公开项。它不是生产日志备份实现，而是 CRR（跨区域复制）测试夹具中的日志备份 flush 模拟器：针对一个模拟 store 生成空的 log 对象和可解析的 backup metadata，再推进假 PD 中该 store 的 region checkpoint。

直接装配入口是 `harness.rs::newLocalTestHarness`：它把 `PDSim` 和带事件通知能力的 `CRRUpstreamStorage` 交给 `NewFlushSimWithTestContext`。因此 `FlushSim` 写出的对象会进入 CRR worker 的复制事件流，最终由 `TestHarness::AssertDownstreamCanRestoreTo` 使用 `RecordsUpTo` 校验下游恢复材料。该定位由 RustCodeGraph 的文件关系（`flush_sim.rs` 被 `harness.rs`、`parity_test.rs` 使用）以及上述源码调用共同确认。

## 核心职责

- `FlushSim::FlushStore` 模拟一个 store 的一次完整 flush：读取 region 快照、分配 checkpoint/flush TSO、为每个 region 生成 TS 范围与空 log 文件、写一份聚合 metadata、推进 region checkpoint，并登记 `FlushRecord`。
- `formatTaggedMetaName` 按 stream backup 的标签协议编码 `flushTS`、`storeID`、最小/最大 TS 和 flush 序号，使 `backupmetas::ParseName` 能从对象名恢复关键字段。
- `Records` 与 `RecordsUpTo` 提供已成功完成 flush 的深拷贝快照；后者只返回 `CheckpointTS <= tso` 的记录，供恢复点断言使用。
- 文件刻意只模拟对象布局、metadata wire shape、时间戳与 checkpoint 状态迁移。`buildRegionFiles` 写入的 log 内容是空字节，不模拟真实 redo entry。

## 主要符号

- `pub struct FlushSim`：持有 `Arc<PDSim>`、根种子、`Arc<dyn Storage>`、全局 flush 序号、完成记录和按 store 建立的互斥锁。公开类型可经 crate 根再导出使用，但字段均为私有。
- `struct RegionFiles`：单次 flush 的内部聚合值，包含 `DataFileGroup`、log 路径、region ID，以及跨 region 的 `min_ts`/`max_ts`；只在 `FlushStore`、`buildRegionFiles` 和 `writeBackupMeta` 之间传递。
- `NewFlushSimWithTestContext(pd, storage, tc) -> FlushSim`：公开构造器，从 `TestContext::Seed` 固化确定性随机根种子，初始化序号、记录与 store 锁表。
- `formatTaggedMetaName(...) -> String`：内部命名函数，使用 `NAME_MIN_BEGIN_TS_IN_DEFAULT_CF_TAG`、`NAME_MIN_TS_TAG`、`NAME_MAX_TS_TAG` 和 `regionIDTag`，数值均按 16 位大写十六进制编码，序号充当唯一后缀 token。
- `pickRegionTSRange(...) -> (u64, u64)`：在闭区间 `[globalCheckpoint, latestTS]` 抽取两个值并排序，保证返回 `min <= max`。
- `buildRegionFiles(...) -> Result<RegionFiles>`：逐 region 写 `v1/log/store-{storeID}/flush-{seq}-region-{id}.log` 空对象，并构造 `DataFileGroup`。对象路径放在 group 的 `Path`，嵌套 `DataFileInfo` 只携带 TS。
- `lockStore`、`nextFlushSequence`、`flushRNG`、`appendRecord`：分别负责同 store 串行化、进程内单调序号、按 `(seed, store, seq)` 派生随机流，以及按 `Sequence` 有序插入历史。
- `writeBackupMeta(...) -> Result<String>`：创建 `Metadata { StoreId, MinTs, MaxTs: checkpointTS, FileGroups }`，序列化后写到 `GetStreamBackupMetaPrefix()` 下。
- `flushRegions(...) -> Result<()>`：调用 `PDSim::flushStore` 将该 store 的各 region checkpoint 推进到本轮 `checkpointTS`。
- `pub fn FlushStore(...) -> Result<FlushRecord>`、`pub fn Records()`、`pub fn RecordsUpTo(tso)`：该类型的三个公开操作接口。

## 执行流程

`FlushStore` 的顺序是其主要行为契约：

1. 通过 `lockStore(storeID)` 获取并持有 store 专属锁；同一 store 的两次 flush 不会交错，不同 store 可以并行进入主体。
2. 调用 `PDSim::RegionSnapshotsOnStore` 取得当前 region 快照；查询失败直接传播，空集合返回 `store ... has no regions to flush`。
3. 连续调用两次 `PDSim::AllocTSO`，先得到 `checkpointTS`，再得到 `flushTS`；生成 region 范围时的 `latestTS` 明确取前者。随后读取 `GlobalCheckpoint`。
4. 通过 `nextFlushSequence` 取得全局唯一递增序号，并由 `flushRNG` 基于根种子、store 和序号创建本轮确定性随机流。
5. `buildRegionFiles` 为每个快照抽取有序 TS 范围、写空 log 对象，并汇总 `DataFileGroup`、路径、region ID 和全局最小/最大 TS。任一写入失败即停止。
6. `writeBackupMeta` 将所有 group 聚合为一份 metadata。文件名中的最大 TS 来自 region 范围聚合值，而 metadata 的 `MaxTs` 是 `checkpointTS`，两者职责不同。
7. `flushRegions` 推进假 PD 中该 store 的 region checkpoint；只有成功后才构造并登记 `FlushRecord`。
8. `appendRecord` 按 `Sequence` 插入共享历史，返回值再经 `clone_record` 深拷贝，避免调用方与内部记录共享路径或 ID 容器。

注释保留了 Go 的四个 failpoint 边界（开始、写 metadata 前后、推进 region 后），但 Rust 当前均为空操作，不具备注入能力。

## 数据与状态

`seq` 是整个 `FlushSim` 实例共享的序号，不按 store 分区。它既参与 log/meta 路径，也参与 RNG component 名；因此相同初始布局、种子和相同 flush 调用次序可复现结果，而改变跨 store 调度次序会改变序号及随机序列。`records` 保存的仅是已经走完 metadata 写入和 PD flush 的操作，按 `Sequence` 排序；`RecordsUpTo` 的筛选条件是 checkpoint，而不是 flush TS。

`RegionFiles::min_ts` 从 `u64::MAX` 开始、`max_ts` 从 0 开始，但空 region 已在构建前拒绝，因此成功路径不会把哨兵值写进 metadata。对每个 region，`DataFileGroup::{MinTs, MaxTs}` 与唯一的 `DataFileInfo::{MinTs, MaxTs}` 相同；测试确认 group `Path` 指向 log，而 `DataFileInfo.Path` 保持空值，以匹配下游“优先 group.Path、再回退到 DataFilesInfo.Path”的读取约定。

TS 抽样依赖正常时间线不变量 `globalCheckpoint <= checkpointTS`。底层 `DeterministicRNG::Uint64InRange` 在 `lower >= upper` 时直接返回 `lower`，所以相等边界是合法退化区间；若调用环境破坏前述不变量，本文件不会主动报错，而会取全局 checkpoint 值。

## 依赖与调用关系

上游调用关系：

- `harness.rs::newLocalTestHarness` 构造 `FlushSim`，并把它暴露为 `TestHarness::FlushSim`。
- `parity_test.rs::go_rust_public_contract_matches` 直接构造模拟器并调用 `FlushStore`，还覆盖不存在 region 的 store。
- `harness.rs::TestHarness::AssertDownstreamCanRestoreTo` 调用 `RecordsUpTo`，再解析 metadata 文件名和内容，验证其引用的 log 在下游可读。

下游依赖关系：

- `PDSim::{RegionSnapshotsOnStore, AllocTSO, GlobalCheckpoint, flushStore}` 提供 region 布局、时钟和 checkpoint 状态迁移；其中 `pd_sim.rs::flushStore` 最终调用 fakecluster 的 `ApplyCheckpointToStore`，并桥接 `Context` 取消状态。
- `Storage::WriteFile` 是所有对象写入边界；harness 注入 `CRRUpstreamStorage` 时，写入还会发出复制事件。
- `astersql-br-pkg-stream` 提供 metadata 前缀和 protobuf-like `Metadata`/`DataFileGroup`/`DataFileInfo`；`astersql-br-pkg-stream-backupmetas` 提供命名标签常量。
- `types.rs` 提供 `DeterministicRNG`、`FlushRecord`、`RegionState`、`TestContext` 与 `regionIDTag`。

RustCodeGraph 能解析文件级使用关系和 `NewFlushSimWithTestContext -> newLocalTestHarness` 等边；其索引把 Rust impl 方法记为 `function`，对 `FlushStore`/`RecordsUpTo` 的独立 `callers` 查询未返回边，因此调用点另以精确 `rg` 和对应源码段交叉核验。

## 错误处理与边界

- region 查询错误原样通过 `?` 返回；空 store 返回显式 `Error`，不会产生序号、文件或记录。
- log 写入、metadata 序列化、metadata 写入和 PD flush 错误均增加操作上下文（对象路径或 store ID）。没有事务或回滚：中途失败可能留下已写的部分 log；PD flush 失败则 metadata 也可能已经存在，但本轮不会加入 `records`。
- `nextFlushSequence` 在文件写入前分配，失败后不会回收，所以历史记录和后续文件名可以出现序号空洞；这不破坏单调性。
- `std::sync::Mutex::lock().unwrap()` 和取消桥线程的 `join().unwrap()` 遇到锁中毒或线程 panic 会 panic，而不是转成 `Result`。正常的存储、PD 和输入边界使用 `Result`。
- 文件名与路径由数值字段组成，没有用户输入路径穿越面；`storeID as i64` 对大于 `i64::MAX` 的值采用 Rust 转型语义，当前测试布局使用小正数，未验证超大 store ID 的 wire 兼容性。
- 当前 failpoint 仅为注释，不能用来验证四个中断窗口；扩展故障注入时不能把这些注释当作已实现功能。

## 并发与资源生命周期

`FlushSim` 通过 `Arc` 共享 PD 和 Storage，自身的可变状态均在互斥锁后。`mu` 保护 store 锁的创建过程，`stores` 保存 `storeID -> Arc<Mutex<()>>`；获得 Arc 后，`FlushStore` 持有 store 锁直到返回，因此同一 store 的 region 快照、对象写入、PD 推进和记录提交构成串行序列。不同 store 可并行执行，只会在 `seq`、`records` 及短暂的锁表访问处竞争。

锁顺序固定为：创建/查找 store 锁时先 `mu` 后 `stores`；随后两者均释放，才获取 store 专属锁。`nextFlushSequence` 和 `appendRecord` 分别短暂获取各自锁，没有反向嵌套，因此当前实现没有明显的锁顺序环。序号可能先于另一并发 flush 完成，故 `appendRecord` 不能简单 push，而按 `Sequence` 插入以稳定 `Records` 的创建顺序。

文件本身不创建后台任务、通道或显式 close 资源；Storage 和 PDSim 的生命周期由调用方持有的 `Arc` 管理。harness 负责关闭本地存储并删除临时目录。`PDSim::flushStore` 内部会临时创建取消桥线程并在返回前 join，这一同步成本属于下游调用生命周期。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/utiltest/crr/flush_sim.go`。Rust 保留了 Go 的类型划分、公开 API 名称、两次 TSO 分配顺序、按 store 加锁、随机 component 名、对象路径、tagged metadata 名、wire 字段、先写文件后推进 PD、成功后登记记录，以及按 checkpoint 筛选历史的语义。

实现机制上的差异主要是语言所有权与同步表达：Go 用一个 `mu` 同时保护 `seq`、`records` 和 `stores`，Rust 将其拆成多个 `Mutex`，并把 store 锁放在 `Arc` 中；Go 返回指针构造结果，Rust 返回拥有所有权的 `FlushSim`。Go protobuf 使用指针 slice，Rust stub 使用值集合并在写 metadata 时 clone。两边都会返回记录副本，Rust 显式调用 `FlushRecord::clone_record`。

当前最重要的迁移差异是 failpoint：Go 实际调用 `failpoint.InjectCall`，Rust 只保留对齐位置的注释。此外，Go `flushRegions` 忽略 `pd.flushStore` 返回的 region 列表，Rust 同样只关心成功或失败。`parity_test.rs` 已验证 metadata 的 `StoreId`、group 数量、group `Path` 以及嵌套 info 空路径等关键 wire shape。

## 扩展指南

- 若增加真实 log 载荷或多个 data file，优先修改 `buildRegionFiles`，同时保持 `DataFileGroup` 与下游 `extractDataFilePaths` 的路径约定；在独立测试文件 `br/pkg/utiltest/crr/parity_test.rs` 中补充 metadata 解析和复制后内容断言，不要把测试嵌入本源文件。
- 若调整 metadata 命名，需同时核对 `formatTaggedMetaName`、`astersql-br-pkg-stream-backupmetas::ParseName` 和 `harness.rs::AssertDownstreamCanRestoreTo`；兼容风险是旧对象无法被枚举或解析。
- 若实现 Rust failpoint，应接入现有四个注释边界，并增加独立测试验证每个失败窗口的孤儿文件、PD checkpoint 和 `records` 状态；不能改变 Go 对齐的操作顺序来简化测试。
- 若改变并发模型，必须保持同 store 串行不变量及全局 sequence 的唯一、单调、有序可见性。扩大 store 锁作用域会降低并行度，缩小作用域则可能让同一 store 的 metadata 与 checkpoint 交错。
- 若增加回滚或幂等重试，应明确处理已写 log、已写 metadata、PD 已推进这三个阶段；这会改变目前允许孤儿对象和序号空洞的行为，需与 Go 实现同步决策。
- 若改变 TS 范围规则，应同时检查 `types.rs::DeterministicRNG::Uint64InRange`、`PDSim` 的全局 checkpoint 单调性和文件名标签含义，避免 metadata 内外的最大 TS 语义混淆。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件，其中 Rust 7,032 个；目标文件已索引，显示 16 个符号，并显示文件被 `harness.rs`、`parity_test.rs` 和同名无关概念所在文件引用。针对目标精确读取了 `flush_sim.rs` 全部 342 行。
- RustCodeGraph/源码核验：`NewFlushSimWithTestContext` 由 `harness.rs::newLocalTestHarness` 调用；`FlushStore` 下游依次涉及 `PDSim::RegionSnapshotsOnStore`、`AllocTSO`、`GlobalCheckpoint`、`Storage::WriteFile`、metadata `Marshal` 和 `PDSim::flushStore`；`RecordsUpTo` 由 `TestHarness::AssertDownstreamCanRestoreTo` 使用。
- 读取的 Rust 文件：`br/pkg/utiltest/crr/flush_sim.rs`、`lib.rs`、`harness.rs`、`types.rs`、`pd_sim.rs`、`parity_test.rs`。测试证据位于独立的 `parity_test.rs`：正常 flush、对象存在、metadata wire shape、空 store 错误，以及 flush→拉取→复制→上传 checkpoint→恢复断言。
- crate/依赖证据：`br/pkg/utiltest/crr/Cargo.toml` 将本目录声明为 library，并直接依赖 stream、stream/backupmetas、streamhelper、streamhelper/config、fakecluster、`rand`、`serde` 与 `serde_json`；本文件直接使用其中的 stream、backupmetas 和 crate 内模块。
- Go 对照：完整读取 `br/pkg/utiltest/crr/flush_sim.go`，逐项核对构造、锁、TS/RNG、路径、metadata、checkpoint、记录与错误分支；Rust failpoint 仅保留注释的差异已明确记录。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前仅执行任务指定的 11 节结构验证，并人工检查文档未把空 log、failpoint 或未覆盖边界描述成已实现能力。
