# `pkg/util/topsql/stmtstats/stmtstats.rs`

## 文件定位

本文件是 `astersql-util-topsql-stmtstats` crate 的语句级统计核心。crate 入口 `pkg/util/topsql/stmtstats/lib.rs` 通过私有模块 `mod stmtstats` 装载它，再以 `pub use stmtstats::*` 对外重导出公开 API；`pkg/util/topsql/stmtstats/Cargo.toml` 将 crate 归入 Go 包 `pkg/util/topsql/stmtstats` 的 Rust 移植，并声明执行详情、reporter metrics 与 TopSQL 状态三个同仓路径依赖。本文件本身没有 feature gate 或条件编译项。

它位于 SQL 执行生命周期与后台 TopSQL/TopRU 聚合器之间：`pkg/session/runtime/scan_adapter_runtime.rs` 在 `TopSQLStart` 中懒创建 `StatementStats` 并调用 `OnExecutionBegin`，在 `TopSQLFinish` 中调用 `OnExecutionFinished`；`pkg/util/topsql/stmtstats/aggregator.rs` 周期性调用 `MergeRUInto`、`Take`、`Finished` 与 `ResetRUStateOnVersionChange`，将各 session 的局部数据合并后交给 collector。`pkg/session/runtime/typed_adapter_bridge.rs` 在 session owner 析构时调用 `SetFinished`，使聚合器最终注销该实例。

当前 Rust 会话调用方会填入网络字节、RU 版本、RU v2 最终值和 TopRU 开关，但构造 begin/finish 信息时没有填 `User` 与 `RUDetails`，两者保持默认值；因此本文只把完整 RU v1 采样描述为本文件具备且由独立测试验证的能力，不声称当前 session 主链已经提供了所需明细。

## 核心职责

- 以 `SQLPlanDigest` 为键，累计语句开始次数、有效结束耗时次数、总耗时、入站/出站网络字节和 KV target 执行计数。
- 以 `RUKey`（用户、SQL digest、plan digest）为键，维护 TopRU 的 begin 计数、已完成增量缓冲和单个在途执行上下文。
- 在聚合 tick 中以“取出已完成缓冲 + 采样在途 delta”的方式避免重复统计长运行语句的 RU。
- 提供 `Take`、`MergeRUInto` 和 map/item 的合并操作，让后台聚合器以 drain 语义消费局部数据。
- 通过 `AtomicBool` 暴露 session 生命周期终止标记，并由 `CreateStatementStats` 将新实例注册进全局聚合器。
- 定义 `StatementObserver`、begin/finish 数据传输类型、digest 包装、统计项及带锁守卫，形成该 crate 对执行侧和上报侧的公共数据契约。

常规 TopSQL 与 TopRU 是两条相关但独立的状态通路：关闭 TopRU 不妨碍 begin/finish 累加常规语句统计；RU 版本切换也只清理 RU 状态，不触碰 `data`。

## 主要符号

- `SignedDuration(i64)`：保留 Go `time.Duration` 的有符号纳秒语义。`from_nanos` 构造，`Nanoseconds` 读取；负值由 finish 路径判为无效。
- `StatementObserver: Send + Sync`：定义 `OnExecutionBegin` 与 `OnExecutionFinished` 回调。`StatementStats` 的 trait 实现只转发给同名固有方法。
- `ExecBeginInfo`：可选 begin 快照，包含 `RUDetails`、`User`、`InNetworkBytes`、`RUVersion`、`TopRUEnabled`。`ExecFinishInfo` 对应 finish 快照，另含 `TotalRUV2`、`OutNetworkBytes` 和 `ExecDuration`。
- `StatementStatsInner`：锁内私有状态，包含常规 `data`、`finished_ru_buffer` 和至多一个 `exec_ctx`。至多一个上下文与 Go 注释中的 session 串行执行假设一致。
- `StatementStats`：公开线程安全容器；`inner: Mutex<_>` 串行化全部可变统计，`finished: AtomicBool` 独立表示生命周期结束。
- `OnExecutionBegin` / `OnExecutionFinished`：执行侧主要入口；前者以 begin 为准增加 `ExecCount`，后者只对非负时长增加 duration/outbound 数据。
- `GetOrCreateStatementStatsItem`：返回 `StatementStatsItemGuard`，在守卫生命周期内持有整个 `inner` 锁并通过 `Deref`/`DerefMut` 暴露目标项。与 Go 的“不负责并发控制”帮助函数不同，Rust 公共版本自身带锁。
- `Take` / `MergeRUInto`：分别 drain 常规统计与 RU 增量；前者用空 map 替换 `data`，后者还采样当前在途执行。
- `ResetRUStateOnVersionChange` / `ClearRUExecContext`：前者清空已完成 RU 缓冲并在规范化版本不一致时丢弃活跃上下文，后者只强制丢弃活跃上下文。
- `CreateStatementStats() -> Arc<StatementStats>`：创建实例并调用 `global_aggregator().register`；返回 `Arc` 供 session 与聚合器共享。
- `BinaryDigest(Vec<u8>)` 与 `SQLPlanDigest`：拥有 digest 字节的哈希键。输入切片在构造时被复制，键不借用调用方内存。
- `StatementStatsMapMerge`：为 `HashMap<SQLPlanDigest, StatementStatsItem>` 提供按键合并；同键调用 item `Merge`，异键直接移动插入。
- `StatementStatsItem`：常规聚合值，包含 `KvStatsItem`、`ExecCount`、`SumDurationNs`、`DurationCount`、`NetworkInBytes`、`NetworkOutBytes`；`Merge(None)` 是 no-op。
- `KvStatementStatsItem`：用 `Option<HashMap<String, u64>>` 保存 target 次数。构造函数初始化为 `Some(empty)`；合并时若自身为 `None` 则接管对方，否则逐 target 累加。

## 执行流程

1. session 首次执行 `TopSQLStart` 时调用 `CreateStatementStats`，实例立即注册到全局聚合器，并由 session 保存一个 `Arc`。
2. `OnExecutionBegin` 获取 `inner` 锁，以 SQL/plan digest 获取或创建统计项，无条件增加 `ExecCount`；若存在 begin 信息则增加 `NetworkInBytes`。仅当 `TopRUEnabled` 为真时，`add_ru_on_begin` 创建或替换 `exec_ctx`，规范化 RU 版本，并在该 `RUKey` 的已完成缓冲中把 RU `ExecCount` 加一。
3. 执行期间，KV 计数器可经 `add_kv_exec_count` 在相同 digest 项下按 target 累加；直接入口位于 `kv_exec_count.rs`。
4. `OnExecutionFinished` 在信息缺失时直接返回。若时长为负，取得锁后只清空 RU 上下文，不增加常规结束统计。有效时长则增加 `SumDurationNs`、`DurationCount` 与 `NetworkOutBytes`。
5. 有效 finish 且 TopRU 开启时，`add_ru_on_finish` 要求存在活跃上下文且 begin/finish 的 `RUKey` 相等。v2 使用 finish 提供的 `TotalRUV2`；其他版本读取 `RUDetails` 的 `RRU + WRU`。只有相对 `LastRUTotal` 的正 delta 才写入 `TotalRU`，并同时把本次完整执行时长计入 RU `ExecDuration`。成功匹配后无论 delta 是否为正都会清空 `exec_ctx`；键不匹配时保留当前上下文，避免旧 finish 清掉已被新 begin 替换的执行。
6. 聚合 tick 先调用 `MergeRUInto`：取走 `finished_ru_buffer`，然后对仍活跃的 v1 上下文读取当前总量，加入正 delta，并把 `LastRUTotal` 推进到当前值。这样下一 tick 只报告新增部分；v2 的在途总量固定视为零，只在 finish 结算。
7. 聚合器随后调用 `Take`，把每个实例的常规 map 取出并合并。其 `aggregate_all` 明确先 drain RU、再 drain 常规统计，保证 finished session 在注销前仍能报告尾部 RU。
8. session owner 析构时 `SetFinished`。下一轮常规 drain 发现 `Finished()` 后先从聚合器集合注销，再取走剩余常规数据。

## 数据与状态

常规统计键只由 SQL digest 与 plan digest 构成，用户不参与分桶；RU 键额外包含用户。digest 是任意字节序列，空 SQL/plan digest 没有特殊分支。所有数值累计使用普通 `+=`，没有饱和或显式溢出策略；debug 构建可能因整数溢出 panic，release 行为遵循 Rust 编译配置。

`ExecCount` 在 begin 时增加，`DurationCount` 在有效 finish 时增加，两者刻意允许不同：缺失 finish、负时长以及 begin/finish 开关窗口都可能造成计数不相等。`SumDurationNs` 只接收非负 `i64` 转成的 `u64`。RU `ExecCount` 同样是 begin-based，只在 begin 时写入一次；多次 tick 不重复增加。

`finished_ru_buffer` 保存尚未被 tick 取走的已开始/已完成 RU 增量。`MergeRUInto` 使用 `mem::take` 转移整个 map，返回值与容器内后续累计互不共享。常规 `Take` 也具有相同 drain 性质。Rust map 存放拥有所有权的值，不像 Go map 存放 `*StatementStatsItem` 指针，因此 map 合并不会把源 map 中的 item 指针别名带入目标 map。

`exec_ctx` 只保存一个活跃执行：共享 RU 明细句柄、RUKey、上次水位和规范化版本。新的 TopRU begin 会防御性替换旧上下文。计数器回退（当前总量小于上次水位）产生非正 delta，不输出 RU，但采样路径仍把水位更新到较小的新总量，使后续增长可以从重置后的基线继续计算。

## 依赖与调用关系

标准库依赖为 `HashMap`、`Arc`、`Mutex`/`MutexGuard`、`AtomicBool` 及 `Deref`/`DerefMut`。通过 crate 重导出使用的直接内部依赖包括：

- `rustats.rs` 的 `RUVersion`、`RUKey`、`RUIncrementMap`、`ExecutionContext`、`SharedRUDetails` 与版本规范化函数。
- `aggregator.rs` 的 `global_aggregator`，用于创建时注册；同一文件反向消费本文件的 `Finished`、`Take`、`MergeRUInto` 与版本重置 API。
- `kv_exec_count.rs` 调用私有 `add_kv_exec_count`，把单次 SQL 内按 target 去重后的计数写入常规统计。
- `pkg/session/runtime/scan_adapter_runtime.rs` 是已检索到的 Rust 生产 begin/finish 调用方；`typed_adapter_bridge.rs` 管理实例和结束标记。
- `pkg/util/topsql/topsql.rs` 将聚合器 collector 适配到 reporter，后者继续消费 `StatementStatsMap` 和 `RUIncrementMap`。

RustCodeGraph `node --file` 确认本文件被 28 个索引文件引用，并准确列出了 56 个符号；`query` 确认 `CreateStatementStats`、`StatementStats`、`StatementStatsItem`、`MergeRUInto` 及对应 Rust 测试符号。精确 Rust symbol ID 的 `callers/callees` 查询未返回边，因此上述生产调用边由局部仓库文本检索和调用点源码复核补足，没有从 Go 调用关系反推 Rust 现状。

## 错误处理与边界

本文件没有返回自定义错误。`StatementStats.inner`、RU details 读锁一旦 poisoned，分别以 `expect("StatementStats mutex poisoned")`、`expect("RUDetails lock poisoned")` panic；guard 查找内部键失败也以“不变量被破坏”消息 panic。全局注册函数不返回失败结果。

边界行为如下：

- finish 信息为 `None`：完全 no-op，也不会清理可能存在的 RU 上下文。
- finish 时长为负：不写 duration/outbound 数据，清空 RU 上下文；已有 begin 次数和入站字节保留。
- TopRU begin 关闭、finish 开启：没有上下文，finish 无法计算 RU delta；常规 finish 仍会记录。
- TopRU begin 开启、finish 关闭：finish 明确清空上下文，已在 begin 缓冲的 RU `ExecCount` 仍可被下一 tick 取走，但不再采样后续 RU。
- begin/finish RUKey 不匹配：忽略该 finish 的 RU 结算且不清掉当前上下文，防止旧语句 finish 污染新语句。
- RU details 缺失、v1 总量为零或 delta 非正：不增加 `TotalRU`，也不增加 RU `ExecDuration`。
- v2：执行期间 `current_ru_total` 返回零，最终只信任 `ExecFinishInfo.TotalRUV2`；若最终值不是正增量则不输出 RU 时长。
- `ResetRUStateOnVersionChange` 总是清空已完成 RU 缓冲；只有活跃上下文的规范化版本不同才清上下文。版本 `0` 通过 `NormalizeRUVersion` 视为默认 v1。

独立 Rust 测试覆盖上述主要业务边界，但没有覆盖锁 poison、整数溢出或全局注册失败（API 不表达失败）等异常注入。

## 并发与资源生命周期

全部常规与 RU 可变状态共享同一把 `Mutex<StatementStatsInner>`，所以 begin、finish、KV 累加、tick drain、版本重置和 `Take` 相互串行。`TestExecCountBeginBasedFinishAndTickConcurrent` 用两个线程竞争 finish 与 tick，连续 100 轮验证 RU 不丢失、不重复且 begin-based `ExecCount` 始终为一。代价是慢速 RU details 读取和 map 操作都发生在统计锁内；扩展热路径时必须评估锁持有时间。

`finished` 独立使用 `SeqCst` 原子读写，不要求取得 `inner` 锁。`SetFinished` 只是生命周期信号，并不阻止之后调用 begin/finish；真正停止聚合依赖后台下一轮观察该标记并注销，因此调用方必须在 session 不再产生统计时设置它。

`StatementStatsItemGuard` 在外部修改统计项期间一直持有整个 inner mutex。它安全地替代 Go 的无锁内部指针，但调用者不得在持有守卫时重入同一个 `StatementStats` 的其他加锁方法，否则非重入 mutex 会造成死锁。

`Arc<StatementStats>` 由 session 和全局聚合器共同持有：创建时注册增加共享所有权，session drop 设置 finished，聚合器观察后注销并释放自己的引用。文件不创建线程、任务或通道；定时聚合线程属于 `aggregator.rs`。`SharedRUDetails` 是 `Arc<RwLock<_>>`，允许执行侧持续更新、聚合侧只读采样。

## 与 Go 版本的对应关系

Rust 的 `StatementObserver`、begin/finish 信息、`StatementStats` 三组状态、RU begin/finish/tick 算法、版本切换、常规统计字段和合并规则均逐项对应 `pkg/util/topsql/stmtstats/stmtstats.go`。独立 Rust 测试名称和断言与 `stmtstats_test.go` 的合并、网络字节、TopRU 动态开关、多 tick、版本切换、键切换及并发测试保持相同意图。

主要实现差异是所有权与锁封装：Go 的 `StatementStatsMap` 保存 item 指针，Rust 保存值；Go 的 `GetOrCreateStatementStatsItem` 假定调用者已加锁，Rust 返回持锁 guard；Go begin 从 `context.Context` 提取 `*RUDetails`，Rust `ExecBeginInfo` 直接携带 `Option<SharedRUDetails>`；Go 用 `*atomic.Bool`，Rust 将 `AtomicBool` 内嵌在对象中。

Go finish 在匹配上下文后以 `defer` 清理；Rust 在正/零/负 delta 分支汇合后显式清理，匹配情况下可观察行为一致。两者对不匹配 key 都保留当前上下文。Go map `Merge` 对 nil receiver/source 是 no-op；Rust 的 `HashMap` 值不存在 nil 状态，空 map 即对应 no-op 输入。Rust `KvExecCount` 使用 `Option<HashMap<...>>` 来表达 Go map 的 nil/非 nil 差异。

当前接线差异必须单独看待：Go begin 可从 context 获取 RU details，并由调用方传用户；已核对的 Rust session 调用点只填网络、版本、v2 总量和开关。因此本文件算法与测试对齐不等于 Rust 主链已经拥有 Go 的全部 RUKey/RU v1 输入。

## 扩展指南

- 新增常规指标时，应同时修改 `StatementStatsItem`、begin 或 finish 的写入点、`StatementStatsItem::Merge`，并检查 reporter 数据模型是否消费该字段；同步扩展独立的 `stmtstats_test.rs` 与 Go 对照测试意图，不能只增加字段而漏掉聚合。
- 新增 RU 维度时，应修改 `RUKey`（位于 `rustats.rs`）、begin/finish 构键及 reporter RU 数据模型；键基数增长会影响 `aggregator.rs` 的 `MAX_RU_KEYS_PER_AGGREGATE` 上限和丢弃指标。
- 改动 RU 采样时必须保持三条不变量：执行次数只在 begin 计一次；每次采样后推进水位；finish 只增加尚未采样的正 delta。至少同步覆盖长语句跨 tick、计数器重置、v1/v2、开关切换和 finish/tick 并发测试。
- 若补齐 Rust session 的 `User`/`RUDetails` 接线，应在真实 session/RU 上下文所属模块做最小桥接，不在本文件伪造用户或资源明细；增加调用方独立测试证明 v1 主链可采样且不同用户不串桶。
- 若拆锁优化性能，需防止 `Take` 与增量写入丢数据，并保持“先 RU 后常规统计”的 finished-session drain 顺序。不要在持有 `StatementStatsItemGuard` 时调用同对象其他加锁方法。
- 若改变负时长、缺失 finish 或 key 不匹配行为，应先对齐 Go 版本并补回归测试；这些分支决定活跃 RU 上下文是否被清理，错误修改会造成跨语句污染或尾部 RU 丢失。
- 测试继续放在同目录独立文件 `pkg/util/topsql/stmtstats/stmtstats_test.rs`；不要把测试模块内嵌到生产文件。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/topsql/stmtstats` 列出本 crate 的 21 个 Go/Rust 文件。
- 目标源码：RustCodeGraph `node --file pkg/util/topsql/stmtstats/stmtstats.rs --offset 1 --limit 420` 与 `--offset 421 --limit 180` 覆盖全部 530 行；确认 56 个符号、公开 API、内部状态及分支。
- 符号查询：`query CreateStatementStats`、`query StatementStatsItem`、`query MergeRUInto`、`query OnExecutionBegin` 确认 Rust/Go 对应定义和测试符号；精确 `callers/callees` 未返回边，故调用方改由下述源码与局部 `rg` 交叉验证。
- crate 边界：`pkg/util/topsql/stmtstats/Cargo.toml`、`pkg/util/topsql/stmtstats/lib.rs`；核对 crate 名、路径依赖、模块装载和公开重导出。
- Rust 生产调用证据：`pkg/session/runtime/scan_adapter_runtime.rs` 的 `TopSQLStart`/`TopSQLFinish`，`pkg/session/runtime/typed_adapter_bridge.rs` 的 `Drop`，`pkg/util/topsql/stmtstats/aggregator.rs` 的 `aggregate_all`/两条 drain 路径，以及 `kv_exec_count.rs` 的 `add_kv_exec_count` 调用。
- RU 类型证据：`pkg/util/topsql/stmtstats/rustats.rs`；核对版本规范化、共享 details、RUKey、ExecutionContext 与增量 map 的定义。
- Rust 独立测试：`pkg/util/topsql/stmtstats/stmtstats_test.rs`；核对 map/item 合并、注册/finished、Take、网络字节、v1/v2、版本切换、开关矩阵、长语句多 tick、key 切换、计数器回退和 finish/tick 并发语义。
- Go 对照：`pkg/util/topsql/stmtstats/stmtstats.go`、`pkg/util/topsql/stmtstats/stmtstats_test.go`；核对接口、锁与状态布局、context 提取 RU details、分支语义及对应测试清单。
- 本任务仅新增文档，按总计划不运行 Cargo。交付验证使用任务规定的 11 章节结构命令，并人工复核文档能回答文件定位、执行过程、安全扩展点及当前接线限制。
