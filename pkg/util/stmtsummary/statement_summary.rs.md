# `pkg/util/stmtsummary/statement_summary.rs`

## 文件定位

本文件是 `astersql-util-stmtsummary` crate 的 v1 语句摘要核心。`pkg/util/stmtsummary/lib.rs` 将其声明为私有模块并整体再导出，对外提供 `StmtExecInfo`、`StmtExecLazyInfo`、全局 `StmtSummaryByDigestMap` 及统计辅助类型。它位于语句执行完成后的观测链路上：`pkg/session/runtime/scan_adapter_runtime.rs` 组装 `StmtExecInfo` 并调用 `astersql_util_stmtsummary_v2::Add`；`pkg/util/stmtsummary/v2/stmtsummary.rs::Add` 在 v2 尚未 `Setup` 时回退到本文件的 `stmtSummaryByDigestMap::AddStatement`。聚合结果主要由 `pkg/util/stmtsummary/reader.rs` 转换为语句摘要信息模式行，也由 `pkg/server/extract_runtime.rs` 的诊断提取路径读取快照。

`pkg/util/stmtsummary/Cargo.toml` 将该目录定义为独立 crate，直接依赖 `lru`、`prometheus`、`base64`、`snap`，并通过本地 crate 依赖接入 statement context、执行细节、CPU/RU、计划编码和类型系统；workspace 根清单及 executor、session、server、infoschema 等 crate 通过路径依赖引用它。

## 核心职责

- 用 `StmtDigestKey` 将 SQL digest、schema、上一条 SQL digest、计划 digest、资源组及可选用户编码成 LRU 键；用户为空时保持旧五字段字节布局，用户非空时增加大端长度前缀以消除资源组/用户名边界碰撞。
- 用 `stmtSummaryByDigestMap` 管理有容量上限的 digest LRU、当前刷新窗口、采集开关、历史长度、SQL 截断长度、按用户分组和淘汰汇总桶 `other`。
- 将每次执行的 `StmtExecInfo` 同时并入跨窗口累计值 `cumulative` 和当前窗口 `history`，维护和/最大/最小值、集合及首次/末次时间。
- 聚合延迟、Coprocessor、TiKV 扫描、两阶段提交、内存/磁盘、结果行、计划缓存、重试、CPU、RU、网络流量及 IA 远端读段指标。
- 通过 `StmtExecLazyInfo` 延迟获取原始 SQL、编码/二进制计划、计划 digest 和绑定信息，避免无条件在执行热路径物化大对象。
- 提供 reader 使用的格式化与均值辅助函数，以及当前窗口记录数/淘汰数 Prometheus gauge。

## 主要符号

- `StmtDigestKeyPoolType` / `StmtDigestKeyPool`：以 `Mutex<Vec<StmtDigestKey>>` 复用键缓冲区；`Get` 取出或创建默认键，`Put` 清空 `hash` 后归还。
- `StmtDigestKey::{Init, Hash}`：生成复合字节键并暴露只读切片。字段顺序是 digest、schema、prev digest、plan digest、resource group、可选 length-prefixed user。
- `stmtSummaryByDigestMap`：LRU 总表及全局选项的所有者；`StmtSummaryByDigestMap: LazyLock<Mutex<_>>` 是进程级 v1 实例。
- `StmtExecInfo`：一次执行结束时的输入快照，包含 statement context、执行/提交细节、资源峰值、缓存状态、RU/CPU、网络来源及 `LazyInfo`。
- `StmtExecLazyInfo`：五个惰性访问方法的 trait；`EmptyLazyInfo` 只服务默认/测试空路径。
- `stmtSummaryByDigest`：一个复合键的固定元数据、累计统计及 `VecDeque` 历史窗口；`isInternal` 表示该条目至今是否全部由内部语句贡献。
- `stmtSummaryByDigestElement`：一个 `[beginTime, endTime)` 刷新区间及其 `stmtSummaryStats`。
- `stmtSummaryStats`：实际统计载体；`add` 接收单次执行，`merge` 合并另一份聚合值，后者用于淘汰数据等汇总场景。
- `newStmtSummaryStats` / `newStmtSummaryByDigestElement`：建立首样本与首个窗口；编码计划失败或过大时使用丢弃标记，但不丢弃整条语句统计。
- `StmtRUSummary::{Add, Merge}`、`StmtNetworkTrafficSummary::{Add, Merge}`：分别累积 RU 和 TiKV/TiFlash、同区/跨区字节数。
- `formatSQL`、`formatBackoffTypes`、`avgInt`、`avgFloat`、`avgFloat4Uint`、`avgSumFloat`、`convertEmptyToNil`：供摘要 reader 展示列使用。

## 执行流程

1. 会话完成一条语句后构造 `StmtExecInfo`；v2 门面未安装时把它传给全局 v1 map 的 `AddStatement`。
2. `AddStatement` 先检查总开关和内部查询开关，读取刷新周期与历史配置；若当前时间越过边界，则把窗口起点对齐到 `now / interval * interval` 并重置本窗淘汰计数。
3. 方法从键池借用 `StmtDigestKey`，按 `GroupByUser` 决定是否把 `sei.User` 编入键。命中 LRU 时更新条目的 `isInternal` 并累加；未命中时创建 `stmtSummaryByDigest`。
4. 条目首次使用时，`init` 调用 `newStmtSummaryStats` 记录首样本，整理 statement context 中非空表名为小写 `db.table` 列表；空 `PlanDigest` 则从惰性对象取得 Point Get 的计划 digest，同时保存绑定 SQL/digest。
5. `stmtSummaryByDigest::add` 先更新累计统计。若队尾窗口起点不早于当前起点，则直接累加队尾；否则令旧窗口 `onExpire`，新建窗口并从队首裁剪到配置长度，但无论历史长度是否为零都至少保留当前窗口。
6. `stmtSummaryStats::add` 按字段的统计语义执行求和、最大/最小、集合去重或布尔状态更新；提交细节中的原子字段按 `Relaxed` 读取，提交 backoff 列表在其内部互斥保护下读取。
7. 新条目推入已满 LRU 时，返回的被淘汰条目交给 `other.AddEvicted` 按历史窗口聚合，并增加当前窗口淘汰数；最后更新两个 gauge。
8. `reader.rs` 取得全局 map 的快照或历史元素，并使用本文件的均值、backoff 和空值转换函数构造信息模式行；诊断提取路径通过 `Summaries` 获取克隆快照。

## 数据与状态

默认配置由 `newStmtSummaryByDigestMap` 固化为：启用摘要、禁用内部 SQL、启用历史、LRU 容量 3000、刷新周期 1800 秒、历史 24 个窗口、SQL 最大 32768 字节、禁用按用户分组。注释与 Go 实现均说明这些只是编译期默认值，正常启动后由系统变量层更新。

一个键的 `cumulative` 跨全部保留窗口持续累计；`history` 仅保留最近配置数量的窗口；`other` 保存被 LRU 淘汰条目的窗口聚合。因此容量淘汰、历史裁剪与累计统计是不同生命周期。`clearHistory` 保留每个 digest 最新窗口；`SetEnabled(false)` 清空所有数据；关闭内部查询只删除仍为纯内部的条目，内部与外部混合条目因 `isInternal = old && new` 而保留；改变 `GroupByUser` 会清空映射，防止新旧键语义混合。

首样本字段（原 SQL、计划、hint、索引、字符集等）由 `newStmtSummaryStats` 固定；重复执行更新数值统计和用户集合。`minResultRows` 首次初始化为 `i64::MAX`，遇到零/负结果行时归零；计划编码失败写入 `PlanDiscardedEncoded` 并清空 hint，编码计划超过 1 MiB 写 `[discard]`，二进制计划超过上限写入可被 protobuf/snappy/base64 解码识别的 discarded sentinel。SQL 截断按 UTF-8 边界回退并追加 `(len:N)`，其中 N 是原始字节长度。

## 依赖与调用关系

上游生产路径是 `pkg/session/runtime/scan_adapter_runtime.rs` → `pkg/util/stmtsummary/v2/stmtsummary.rs::Add` →（v2 未激活时）`stmtSummaryByDigestMap::AddStatement`。v2 的配置门面还把启用、内部查询、刷新周期、历史大小、容量、SQL 长度和按用户分组设置转发到本文件。RustCodeGraph 显示本文件被 session、executor、server、distsql 等 17 个文件引用；其中 `pkg/server/extract_runtime.rs` 直接读取 `Summaries`。

下游依赖包括：`lru::LruCache` 提供 O(1) 容量淘汰；`task_stmtctx::StatementContext` 提供告警数、影响行、表/索引和存储引擎标志；`task_execdetails` 提供 Cop/Scan/Commit、TiKV 原子执行细节与 RU；`ppcpuusage` 提供 TiDB/TiKV CPU；`plancodec_dependency` 提供计划丢弃标记；`snap`、`base64` 构造超大二进制计划占位；`crate::stmtSummaryByDigestEvicted` 接收淘汰条目。`pkg/util/stmtsummary/reader.rs` 是主要下游消费者，读取聚合字段并调用格式化/均值辅助函数。

## 错误处理与边界

配置 setter 使用 `GoResult<()> = Result<(), String>` 对齐 Go 的 error 形态。当前只有 `SetMaxStmtCount(0)` 明确报错；它会先存入配置原子值再返回容量错误，以保持 Go 的可观察顺序。刷新周期、历史大小和 SQL 长度假定已由系统变量层校验：零刷新周期会参与除法，负 SQL 长度会在 `usize::try_from` 处 panic，因此这些 API 不应绕过上层校验直接接收非法值。

锁中毒均通过带上下文的 `expect` 终止，而不是静默忽略。`SystemTime` 早于 epoch 时 `unix_now` 返回 0；TiKV 纳秒原子计数为负时 `atomic_duration` 按 0 处理。计划编码错误是可降级错误：摘要继续保留，只丢弃计划展示内容。缺少 Cop、Scan、Commit、RU 或网络明细时，对应维度不累加。`avg*` 在分母非正时返回 0，空 backoff map/空字符串分别映射为 `None`。

LRU resize 与插入淘汰语义不同：现有 Rust 测试明确要求 `SetMaxStmtCount` 缩容不增加 `currentWindowEvictedCount`，只有后续插入触发的逐出进入 `other`。复合键没有给各旧字段加长度边界，这是与 Go 旧布局兼容的既定行为；仅新增的 user 字段带长度前缀。

## 并发与资源生命周期

全局 `StmtSummaryByDigestMap` 由一个外层 `Mutex` 串行化读取和修改；map 的修改方法要求 `&mut self`，因此 Rust 版本不在摘要条目和窗口上再叠加内部锁。配置仍使用 `AtomicBool/AtomicI32/AtomicI64/AtomicU32`，统一采用 `SeqCst` 读写，与可从配置门面并发观察的状态相匹配。执行细节里的计数器使用其来源类型的原子与互斥锁读取。

`StmtDigestKeyPool` 自身使用独立 mutex；所有 `AddStatement` 正常分支（已有条目、新条目、初始化失败）都会归还借出的键。插入 LRU 时实际存入 `hash.to_vec()` 的所有权副本，因此归还并清空池中键不会改变缓存键。`Summaries` 和 `collectHistorySummaries` 返回克隆快照，调用者不持有 map 内部借用；代价是诊断读取时复制较大的统计结构。`LazyLock` 保证全局 map 与 gauge 只初始化一次。

Go 版本在 map、digest 条目和窗口上使用分层 mutex；Rust 版本依靠全局外锁覆盖完整更新流程，避免跨线程暴露中间状态，但也意味着慢速统计累加会延长全局临界区。新增字段应避免在 `add` 中执行阻塞 I/O 或昂贵解码。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/stmtsummary/statement_summary.go`。Rust 保留了 Go 的主要类型与方法命名、编译期默认值、LRU/other 模型、窗口对齐、至少保留当前窗口、首样本策略、计划失败降级、统计聚合、RU/网络辅助类型和按用户分组切换时清空数据的语义。`StmtDigestKeyPoolType` 对应 `sync.Pool`，`LruCache` 对应 `kvcache.SimpleLRUCache`，`VecDeque` 对应 `container/list`。

可见实现差异包括：Rust 把全局实例包在 `LazyLock<Mutex<_>>`，Go 的 mutex 嵌入 map 并在条目/窗口再加锁；Rust 用 `nowForTest` 代替 Go 的 failpoint 时间注入；Rust 的 `format_sql_with_limit` 会回退到合法 UTF-8 边界，而 Go 直接按字节切片；Rust 的 `formatBackoffTypes` 在计数相同的情况下按名称排序，使输出确定。Rust `newStmtSummaryStats` 返回 `Option`，当前路径总是 `Some`，为初始化失败传播保留接口。Go 对照仍是行为基准，修改聚合字段时应同时检查两边的 add/merge/reader 列语义，而不能只让 Rust 测试通过。

## 扩展指南

新增一个执行统计维度时，通常需要同步修改 `StmtExecInfo` 输入字段、`stmtSummaryStats` 存储字段、`Default`、`newStmtSummaryStats` 的首样本初始化、`stmtSummaryStats::{add, merge}`，以及 `reader.rs` 的列工厂；若属于 RU 或网络流量，则修改对应 summary 的 `Add`/`Merge`。新增键维度应修改 `StmtDigestKey::Init` 和 `AddStatement` 的取值，并认真处理旧键兼容、无歧义编码及配置切换时清空旧数据的问题。

修改窗口策略应集中在 `AddStatement`、`stmtSummaryByDigest::add`、`stmtSummaryByDigestElement::{add,onExpire}` 和 `collectHistorySummaries`，并同步检查 `evicted.rs` 的窗口匹配。修改容量或清理行为要同时维护 `other` 和两个 gauge。惰性字段应优先扩展 `StmtExecLazyInfo`，但必须评估其所有实现者，尤其是 session adapter、v2 record 和测试桩。

测试必须放在独立文件而非本源文件。优先扩展 `pkg/util/stmtsummary/statement_summary_test.rs`（Go 对齐的主行为与并发/reader 覆盖）和 `statement_summary_2_aster_unit_test.rs`（迁移补充边界），并对照 `statement_summary_test.go`。关键回归应覆盖同键多次执行、跨窗口、历史 0/缩容、LRU 淘汰、纯内部与混合条目、按用户分组切换、计划错误/超限、UTF-8 截断、零分母、缺失执行细节及 add/merge 一致性。性能风险主要来自扩大每条摘要、增加克隆量或延长全局 mutex 临界区。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件且目标文件已索引；`files --filter pkg/util/stmtsummary` 确认模块文件集合；`explore "StatementSummaryByDigestMap statement_summary.rs pkg/util/stmtsummary"` 核对主要符号、测试和 reader/v2 调用关系；`node --file pkg/util/stmtsummary/statement_summary.rs` 分段读取 1–1487 行；`callees AddStatement` 核对 `Get/Put/Init/Hash`、选项读取、`add`、指标更新和时钟依赖。
- crate/入口：`pkg/util/stmtsummary/Cargo.toml`、`pkg/util/stmtsummary/lib.rs`、workspace 根 `Cargo.toml` 及引用该 crate 的 session/executor/server/infoschema 清单。
- 上下游源码：`pkg/session/runtime/scan_adapter_runtime.rs`、`pkg/util/stmtsummary/v2/stmtsummary.rs`、`pkg/util/stmtsummary/reader.rs`、`pkg/server/extract_runtime.rs`。
- Go 对照：`pkg/util/stmtsummary/statement_summary.go`，重点核对 `StmtDigestKey.Init`、`newStmtSummaryByDigestMap`、`AddStatement`、配置 setter、`stmtSummaryByDigest::{init,add}`、`newStmtSummaryStats`、窗口更新、统计 add、格式化/均值及 RU/网络汇总。
- Rust 独立测试：`pkg/util/stmtsummary/statement_summary_test.rs` 覆盖历史清理/缩容、淘汰窗口、并发开关、内部条目、Point Get、权限、按用户分组、IA 统计等；`pkg/util/stmtsummary/statement_summary_2_aster_unit_test.rs` 覆盖键边界、完整聚合、窗口裁剪、容量、计划错误、SQL 长度、RU/网络辅助。Go 测试基准为 `pkg/util/stmtsummary/statement_summary_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅运行任务规定的 11 章节结构验证，并人工复核本文未把 v2 激活路径误写成 v1 直接路径。
