# `pkg/session/txninfo/summary.rs`

## 文件定位

本文件属于 `astersql-session-txninfo` crate；crate 入口 `pkg/session/txninfo/lib.rs` 公开 `summary` 与 `txn_info` 两个模块，当前文件负责“已结束事务的 SQL digest 序列摘要”，相邻的 `txn_info.rs` 提供输入类型 `TxnInfo` 与输出单元 `Datum`。它实现的是进程内、固定容量的历史 LRU，而不是持久化存储。

Rust 生产接线目前只确认到 `cmd/tidb-server/main.rs:1783-1786`：启动时把配置中的容量和最短事务时长写入全局 `Recorder`。仓库内 Rust 调用搜索没有找到生产代码调用 `Recorder.OnTrxEnd` 或 `Recorder.DumpTrxSummary`；`pkg/executor/infoschema_reader.rs:811-817` 仅把 `TRX_SUMMARY` 请求转成 `DataRequest::TransactionSummary`，没有直接调用本记录器。因此，“记录器算法已实现”和“Rust 端事务结束、信息模式读取链路已完整接通”必须分开理解。

## 核心职责

- `digest` 将一笔事务内按顺序保存的 SQL digest 字符串逐字节输入 FNV-1a 64 位算法，生成事务模式指纹。
- `TrxSummaries` 用 `HashMap<u64, ()>` 判重、用 `VecDeque<TrxSummaryEntry>` 保存最近使用顺序，并按容量淘汰最旧条目。
- `TrxHistoryRecorder` 用一个 `Mutex<TrxHistoryRecorderState>` 同步阈值与 LRU 的读取、写入、清理和缩容。
- `OnTrxEnd` 从 `TxnInfo.StartTS` 提取 TSO 物理毫秒时间，只记录持续时间不短于 `minDuration` 的事务，并保存 `TxnInfo.AllSQLDigests` 的副本。
- `DumpTrxSummary` 输出两列 `Datum`：不补零的十六进制 64 位事务指纹，以及原 SQL digest 序列的 JSON 数组文本，供事务摘要展示层消费。

## 主要符号

- `fn digest(digests: &[String]) -> u64`：从 FNV-1a offset basis `0xcbf29ce484222325` 开始，对各字符串字节连续哈希；字符串之间不插入分隔符。
- `TrxSummaryEntry { trxDigest, digests }`：一条缓存记录，保留哈希键和原始有序序列。
- `TrxSummaries { capacity, elements, cache }`：内部 LRU。`cache` 队头是最近使用项，队尾是淘汰候选；`elements` 只保存键是否存在。
- `newTrxSummaries(capacity)`：构造空缓存。
- `TrxSummaries::onTrxEnd`：命中时把已有条目移到队头；未命中时插入队头并在超容量时删除队尾及其索引。
- `TrxSummaries::dumpTrxSummary`：按从新到旧顺序生成 `Vec<Vec<Datum>>`。
- `TrxSummaries::resize`：更新容量，并立即从队尾淘汰直到满足新上限。
- `TrxSummaries::clean`：只清空 `cache`，刻意不清空 `elements`；这与 Go 当前实现及迁移测试所固定的行为一致。
- `TrxHistoryRecorderState { minDuration, summaries }`：由同一把锁保护的完整可变状态。
- `pub struct TrxHistoryRecorder`：公开线程安全门面；公开方法为 `DumpTrxSummary`、`OnTrxEnd`、`Clean`、`SetMinDuration`、`ResizeSummaries`。
- `newTrxHistoryRecorder(capacity)`：构造记录器，默认阈值为 1 秒；函数本身不公开到 crate 外。
- `pub static Recorder: LazyLock<TrxHistoryRecorder>`：进程级全局实例，初始容量为 0，预期由服务启动配置调整。

文件没有 trait、枚举、条件编译项或异步函数。公开 API 保留 Go 风格命名；缓存条目、LRU 实现和构造辅助函数均为模块私有。

## 执行流程

1. 服务启动时，`cmd/tidb-server/main.rs` 调用 `Recorder.ResizeSummaries` 和 `Recorder.SetMinDuration`，将 `TrxSummary` 配置灌入全局实例。初始容量 0 意味着在配置前提交的条目会立即被淘汰。
2. 调用方在事务结束后向 `OnTrxEnd(&TxnInfo)` 交付快照。方法用 `StartTS >> 18` 取得 TSO 物理毫秒部分，再以 Unix epoch 构造开始时间。
3. 如果开始时间晚于当前时间，`duration_since` 返回错误；如果已用时小于阈值，方法均直接返回。等于阈值时会记录。
4. 通过阈值后，方法复制 `AllSQLDigests`，调用内部 `onTrxEnd`。其 FNV 指纹若已存在，旧条目移到队头，但不会用新输入替换保存的 digest 列表；否则创建新条目。
5. 插入或缩容导致长度超过 `capacity` 时，从队尾淘汰，保持 `cache.len() <= capacity`。
6. 展示方调用 `DumpTrxSummary` 时，从队头到队尾遍历，生成 `[事务指纹, SQL digest JSON]` 两列行。当前仓库只确认迁移单测直接调用这一方法，未确认 Rust 生产展示链直接调用。

## 数据与状态

核心不变量是：正常插入、命中提升和 `resize` 后，`cache` 从新到旧排序且长度不超过容量；对仍在 `cache` 中的条目，`elements` 含有其 `trxDigest`。`capacity` 可为 0，此时新条目经历插入后立刻被淘汰，最终输出为空。

`AllSQLDigests` 的顺序会原样保留，重复 digest 也不会去重；只有整个序列的 64 位哈希用于事务级判重。因为哈希输入不加入字符串边界，例如不同的字符串切分可能产生同一字节串；此外任何 64 位哈希都存在碰撞可能。碰撞会被当成同一摘要，只提升已有条目而不保存新序列。

`Clean` 有一个必须保留并谨慎对待的特殊状态：它清空队列，却保留 `elements`。因此清理前出现过的指纹之后仍会被判定为命中，但队列中找不到可移动条目，最终不会重新插入；新的指纹仍可记录。`migration_aster_unit_test.rs:88-93` 明确验证了这一行为。

## 依赖与调用关系

直接标准库依赖为 `HashMap`、`VecDeque`、`LazyLock`、`Mutex`、`Duration`、`SystemTime` 和 `UNIX_EPOCH`。crate 内依赖为 `crate::txn_info::{Datum, TxnInfo}`；外部依赖 `types::datum::NewStringDatum` 负责构造展示值，`serde_json::to_string` 负责序列化 digest 数组。`pkg/session/txninfo/Cargo.toml` 声明了 `serde_json`、`types`，以及该 crate 其他模块使用的时间、指标和解析器依赖；没有 feature 开关。

RustCodeGraph 给出的内部调用边为 `newTrxHistoryRecorder -> newTrxSummaries`、`OnTrxEnd -> TrxSummaries::onTrxEnd -> digest`、`DumpTrxSummary -> TrxSummaries::dumpTrxSummary`。测试调用者位于 `pkg/session/txninfo/migration_aster_unit_test.rs`，三个摘要测试直接构造记录器并调用公开方法。

Go 的完整生产上游可作为迁移目标证据：`pkg/session/txn.go:180,348` 在事务离开活动表时调用 `txninfo.Recorder.OnTrxEnd`；下游 `pkg/executor/infoschema_reader.go:3008` 调用 `DumpTrxSummary`；服务配置和动态配置入口调用 `ResizeSummaries`/`SetMinDuration`。这些 Go 调用边不能直接证明 Rust 已接线。

## 错误处理与边界

本文件的公开方法不返回 `Result`。互斥锁中毒时使用 `poisoned.into_inner()` 继续访问状态，而不是 panic；这使一次持锁 panic 不会永久阻断记录器，但调用者应意识到状态可能来自异常路径。

未来时间或无法从 epoch 加上物理毫秒得到有效 `SystemTime` 时不会记录：`checked_add` 溢出回退到 epoch，而 `duration_since` 对未来时间返回错误并直接跳过。`StartTS == 0` 通常会表现为从 epoch 开始的超长事务，只要容量非零就可能被记录；本文件不校验事务信息的业务有效性。

`serde_json::to_string(&Vec<String>)` 被视为不可失败，失败时用 `expect` panic。字符串 JSON 序列化在当前类型下确实没有数据相关错误路径。容量收缩和零容量均被显式支持。空 digest 序列也会获得 FNV 初始值对应的摘要，本文件没有过滤。

## 并发与资源生命周期

`Recorder` 通过 `LazyLock` 在首次访问时初始化，并存活到进程结束。所有可变字段都位于单个 `Mutex` 后，因此记录、导出、阈值更新、缩容和清理彼此串行；迁移测试用八个线程并发执行记录与导出，验证最终八条摘要均保留。

`OnTrxEnd` 在加锁前计算开始时间与已用时，持锁后执行阈值比较、克隆 `AllSQLDigests` 并更新 LRU。`DumpTrxSummary` 在持锁期间完成全部 JSON 序列化和 `Datum` 分配；摘要数量或 digest 列表很大时会延长其他操作的等待时间。当前没有后台任务、通道、文件句柄或显式关闭流程，内存上限主要由条目容量控制，但单条记录中的字符串总大小没有本地上限。

## 与 Go 版本的对应关系

Rust 基本逐项对应 `pkg/session/txninfo/summary.go`：同样使用 FNV-1a 64 位哈希、同样按事务 digest 序列判重、同样维护最近使用顺序、默认 1 秒阈值、全局实例默认容量 0，并输出十六进制指纹及 JSON 数组。Rust 用 `HashMap + VecDeque` 替代 Go 的 `map + container/list`，用 `StartTS >> 18` 直接对应 Go `oracle.ExtractPhysical`。

两版都保留 `Clean` 只重建/清空链表而不清空判重 map 的可观察语义；Rust 测试明确固定了“旧指纹清理后不能重新进入缓存”。Rust 还把阈值与摘要集合放在同一 `Mutex` 状态中，且 `Clean` 也持锁；Go 的 `Clean` 当前没有加锁。Rust 对未来开始时间显式跳过，而 Go 的 `now.Sub(future)` 得到负时长，在非负阈值下同样会跳过。

最大的迁移差异是接线：Go 已有事务结束写入、信息模式读取、启动配置及动态配置调用；Rust 当前直接证据只覆盖启动配置和单元测试。`pkg/infoschema/test/clustertablestest/tables_test.rs:776-807` 名为 `test_tidb_trx_summary` 的 Rust 测试仅自行拼接 JSON 并检查 digest 顺序，没有驱动本记录器或真实 SQL 端到端链路。

## 扩展指南

- 若修改摘要身份算法或 JSON/列格式，应同时修改 `digest`、`dumpTrxSummary` 与 `pkg/session/txninfo/migration_aster_unit_test.rs` 中的 `fnv64a` 和精确行断言，并评估与 Go `TRX_SUMMARY` 输出的兼容性。
- 若改变 LRU、容量或 `Clean` 语义，应扩展独立测试 `summaries_match_go_lru_digest_json_and_resize_behavior` 和 `recorder_matches_go_zero_capacity_and_concurrent_access`；尤其不要无意中把“保留 elements”改成普通全清空，除非同步决定 Go 兼容行为。
- 若改变时间过滤，应修改 `OnTrxEnd` 并扩展 `recorder_applies_go_physical_timestamp_duration_threshold`，覆盖小于、等于阈值、未来时间、零 `StartTS` 与 TSO 边界。
- 若补齐生产接线，入口应分别落在 Rust 事务结束路径和 `DataRequest::TransactionSummary` 的数据提供路径，并新增独立的端到端测试；测试逻辑不得内嵌进本生产文件。
- 若增大默认容量、允许更长 digest 历史或在导出时做昂贵转换，应评估单锁竞争、持锁 JSON 序列化开销及无单条大小上限带来的内存风险。

## 验证依据

- Rust 源与模块：`pkg/session/txninfo/summary.rs`、`pkg/session/txninfo/lib.rs`、`pkg/session/txninfo/txn_info.rs`。
- crate 边界：`pkg/session/txninfo/Cargo.toml`，package 为 `astersql-session-txninfo`，`lib.rs` 为 crate 根。
- Go 对照：`pkg/session/txninfo/summary.go`、`pkg/session/txninfo/txn_info.go`、`pkg/session/txn.go:180,348`、`pkg/executor/infoschema_reader.go:3008`、`cmd/tidb-server/main.go:1215-1216`。
- Rust 接线：`cmd/tidb-server/main.rs:1783-1786`、`pkg/executor/infoschema_reader.rs:811-817`；全仓 `rg` 未发现 Rust 生产代码对 `Recorder.OnTrxEnd` 或 `Recorder.DumpTrxSummary` 的调用。
- 独立 Rust 测试：`pkg/session/txninfo/migration_aster_unit_test.rs:64-162` 覆盖 LRU 命中提升、JSON、缩容、特殊 Clean 语义、零容量、并发访问、持续时间阈值与未来时间；`pkg/infoschema/test/clustertablestest/tables_test.rs:776-807` 仅覆盖期望 JSON 形状，不是记录器端到端测试。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；查询确认 `summary.rs` 含 17 个符号，并得到 `newTrxHistoryRecorder -> newTrxSummaries`、`OnTrxEnd -> onTrxEnd -> digest` 等调用边。对公开方法的 callers 查询未返回 Rust 生产调用者，随后用精确 `rg` 复核接线边界。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅执行任务指定的 11 章节结构验证并人工复核事实与范围。
