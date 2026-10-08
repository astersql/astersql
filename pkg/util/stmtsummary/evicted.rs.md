# `pkg/util/stmtsummary/evicted.rs`

## 文件定位

本文件属于 `astersql-util-stmtsummary` crate。crate 入口 `pkg/util/stmtsummary/lib.rs` 将它声明为私有模块 `evicted`，再公开重导出其中符号；`pkg/util/stmtsummary/Cargo.toml` 指定库入口为 `lib.rs`，且以 `package.metadata.porting.go-package = "pkg/util/stmtsummary"` 标明 Go 对照包。它位于语句摘要 LRU 的淘汰支路：`stmtSummaryByDigestMap::AddStatement` 在 `summaryMap.push` 返回旧条目时调用 `self.other.AddEvicted(...)`（`statement_summary.rs:649-654`），把无法继续按 digest 单独保存的数据按时间窗口汇总为 “other/evicted” 记录。

该文件同时提供两类读出数据：`stmtSummaryByDigestEvicted::ToEvictedCountDatum` 返回每个窗口的淘汰 digest 数，`otherSummary` 则由 `reader.rs` 的 `getStmtEvictedOtherRow` / `getStmtEvictedOtherHistoryRow` 转换成语句摘要表的聚合行。它不是 LRU 容器本身，也不决定何时淘汰；淘汰决定来自 `statement_summary.rs`。

## 核心职责

1. 以 `VecDeque` 保存按时间升序排列的淘汰窗口，队头最旧、队尾最新（`stmtSummaryByDigestEvicted::history`）。
2. 将一个被淘汰 digest 的全部历史片段合并到已有窗口，必要时按时间位置插入新窗口，并将长度裁剪到 `historySize`（`AddEvicted`）。
3. 对每个窗口同时维护淘汰 digest 个数 `count` 和跨 digest 的完整统计汇总 `otherSummary`（`addEvicted`、`addInfo`）。
4. 将窗口边界和淘汰数转成三列 `Datum`，并支持清空与选取最新若干历史窗口（`ToEvictedCountDatum`、`Clear`、`collectHistorySummaries`）。

这里的 `count` 是被淘汰的不同缓存条目次数，不是 SQL 执行次数；执行次数等指标累积在 `otherSummary.stmtSummaryStats` 中。`evicted_test.rs:test_evicted_count_detailed` 证明同一窗口连续淘汰会让 count 从 1 增到 2，而每个窗口仍只输出一行。

## 主要符号

- `stmtSummaryByDigestEvicted { history }`：全部淘汰历史的容器。`history` 是 `VecDeque<Box<stmtSummaryByDigestEvictedElement>>`，保持最旧到最新顺序。
- `stmtSummaryByDigestEvictedElement`：单一时间窗口，字段为 Unix 秒 `beginTime`/`endTime`、淘汰次数 `count`、聚合统计 `otherSummary`。
- `newStmtSummaryByDigestEvicted()`：构造空容器。
- `newStmtSummaryByDigestEvictedElement(beginTime, endTime)`：构造空窗口；令 `count = 0`，同步 `otherSummary` 的边界，以 `i64::MAX` 纳秒初始化 `minLatency`，并把 `firstSeen` 初始化到窗口结束时间。该哨兵使首次 `min` 合并得到真实最小延迟。
- `AddEvicted(evictedKey, evictedValue, historySize)`：主写入口。`evictedValue = None` 时无操作；对值中的历史从新到旧合并；始终在每次处理后从队头删除超量旧窗口。
- `matchAndAdd(...) -> i32`：若待合并区间完全包含于当前窗口，调用 `addEvicted` 并返回 `isMatch`；若其结束时间不晚于当前窗口起点，返回 `isTooOld`；其余（包括 `None`）返回 `isTooYoung`。三个返回常量分别为 `isMatch = 0`、`isTooOld = 1`、`isTooYoung = 2`。
- `addEvicted(...)`：只有 `digestKey` 存在时才增加 `count` 并调用 `addInfo`；key 为空时只允许 `AddEvicted` 建立/刷新时间窗口。
- `addInfo(addTo, addWith)`：把一个摘要窗口的统计合并进 other 桶；集合取并集，累计量求和，最大/最小量取极值，`firstSeen`/`lastSeen` 扩大时间跨度，RU 摘要调用 `Merge`，`resourceGroupName` 采用来源值覆盖。
- `ToEvictedCountDatum`：容器版本按最新到最旧输出；元素版本产生 `(SUMMARY_BEGIN_TIME, SUMMARY_END_TIME, count)` 三列。`stmtSummaryByDigestMap` 上的同名方法只是转发给 `self.other`。
- `Clear`：清空全部窗口；`collectHistorySummaries(historySize)`：保留内部升序，返回最新的至多 `historySize` 个窗口引用。
- `unix_system_time`、`mysql_timestamp`：分别完成 Unix 秒到 `SystemTime` 和 MySQL `TIMESTAMP` 所需 `types::Time` 的转换。

## 执行流程

写入主链如下：

1. `stmtSummaryByDigestMap::AddStatement` 将新语句按 digest 写入 `LruCache`；缓存满时 `push` 返回 `(evicted_key, evicted_summary)`。
2. `AddStatement` 增加当前窗口淘汰计数，并调用 `self.other.AddEvicted(Some(key), Some(summary), historySize)`。
3. `AddEvicted` 从被淘汰摘要的最新历史片段向最旧片段遍历。若自身历史为空且允许历史，则直接建窗；否则从自身最新窗口向旧窗口扫描。
4. `matchAndAdd` 命中包含关系时就地合并；返回 `isTooYoung` 时在扫描位置之后插入；扫描到队头仍为 `isTooOld` 时前插。由此维持队头最旧、队尾最新。
5. `addEvicted` 将 `count` 加一，随后 `addInfo` 合并用户集合、延迟、coprocessor、TiKV、Insight Analytics、事务、计划缓存、内存/磁盘、重试、CPU、错误、RU 和资源组字段。
6. 每处理一个来源窗口，`AddEvicted` 都在 `history.len() > historySize` 时弹出队头，因此 `historySize = 0` 最终不保留任何窗口。

读路径分为两支：淘汰计数表调用 map 的 `ToEvictedCountDatum`，得到最新窗口优先的三列行；语句摘要 reader 直接读取 `otherSummary`，当前表只取覆盖当前时间段的队尾窗口，历史表遍历已保存窗口并通过通用列工厂生成完整摘要行（`reader.rs:253-279`）。

## 数据与状态

核心不变量是 `history` 按 `(beginTime, endTime)` 的时间位置从旧到新排列，容量不超过最近一次写入传入的 `historySize`。窗口匹配不是简单的边界相等，而是要求来源区间完全落入目标区间：`self.beginTime <= source.beginTime && source.endTime <= self.endTime`。相邻边界满足 `source.endTime <= self.beginTime` 时被归类为更旧，不会合并。

`otherSummary` 不保存某个可查询 digest 的身份字段，而是被淘汰集合的统计桶。其累计字段包括执行、延迟、coprocessor、存储、事务、内存/磁盘、重试、CPU、错误与 IA 指标；最大值字段同步相关地址，`authUsers` 取并集，`backoffTypes` 按类型累加。`resourceGroupName` 是覆盖型字段，因此一个窗口聚合多个资源组时只保留最后合入值；这是 Go 实现既有语义，不应误解为按资源组分桶。

时间以 Unix 秒保存。计数输出时 `mysql_timestamp` 使用 UTC、MySQL `TypeTimestamp` 和小数秒精度 0。构造 `SystemTime` 支持负 Unix 秒；`mysql_timestamp` 对 chrono 无法表示的秒值会触发 `expect`。

## 依赖与调用关系

上游生产调用者是 `statement_summary.rs:stmtSummaryByDigestMap::AddStatement`，清理由同文件 `stmtSummaryByDigestMap::clearLocked -> self.other.Clear()` 触发。`stmtSummaryByDigestMap` 的全局实例是 `LazyLock<Mutex<...>>`（`statement_summary.rs:193-194`），外部共享读写先取得该锁。

下游依赖来自 crate 内部类型：`StmtDigestKey`、`stmtSummaryByDigest`、`stmtSummaryByDigestElement`、`stmtSummaryStats` 定义于 `statement_summary.rs`；`types::{Datum, Time}` 和 `mysql::TypeTimestamp` 经 `lib.rs` 从 `astersql-types`、`astersql-parser-mysql` 重导出。直接第三方依赖为 `chrono` 与 `chrono-tz`，均在 `Cargo.toml` 声明；标准库使用 `VecDeque`、`Duration`、`SystemTime`。

RustCodeGraph 将本文件标为被 `reader.rs`、`statement_summary.rs` 以及三个测试文件使用。精确查询还确认 `collectHistorySummaries` 的 reader/测试调用关系，以及 `addInfo` 被 `addEvicted`、`evicted_test.rs` 和 `go_merge_34_test.rs` 覆盖。由于图索引会把 Go/Rust 同名符号一并列出，生产边以带路径的 Rust 源码核验为准。

## 错误处理与边界

- `AddEvicted` 对空 value 直接返回；空 key 不增加 count、也不合并指标，但仍可能创建对应窗口。独立测试 `nil_key_refreshes_windows_without_incrementing_count` 和 `test_simple_stmt_summary_by_digest_evicted` 固化了该语义。
- `addEvicted` 在 key 存在而 value 缺失时会 `expect` 失败；正常调用链保证二者同时存在。直接扩展此 API 时必须维持该前置条件。
- `historySize = 0` 时所有窗口会在处理后被裁剪；空来源历史不会产生记录。
- `matchAndAdd(None)` 返回 `isTooYoung`。部分重叠但不被目标窗口完整包含的区间也归为 `isTooYoung`，可能创建新窗口；不要擅自改为按重叠合并。
- `mysql_timestamp` 对超出 chrono 表示范围的秒值 panic；生产时间来自语句摘要时钟，当前代码未返回可恢复错误。
- 所有计数和求和字段沿用整数/`Duration` 的普通加法，没有显式饱和或溢出处理；扩展高基数字段时需评估长期聚合上限。

## 并发与资源生命周期

Go 版本在淘汰容器、被淘汰 digest 和聚合元素内部持有互斥锁；Rust 版本不在本文件内嵌锁，而通过所有权约束表达单线程可变访问：`AddEvicted`、`Clear` 和 `addEvicted` 需要 `&mut self`，只读转换需要 `&self`。全局 map 由 `StmtSummaryByDigestMap: LazyLock<Mutex<stmtSummaryByDigestMap>>` 保护；需要跨线程共享局部 map 的调用者也必须在外层加锁。`evicted_test.rs:evicted_count_rows_are_safe_during_concurrent_statement_adds` 正是用 `Arc<Mutex<_>>` 同步读写。

每个窗口由 `Box` 独占并存于 `VecDeque`；弹出、清空或 map 生命周期结束时由 Rust 自动释放，无后台任务、通道或显式关闭动作。`collectHistorySummaries` 返回借用，生命周期不能超过容器借用；写操作需要独占借用，因而无法在这些引用存活时修改队列。`ToEvictedCountDatum` 立即复制为拥有所有权的 Datum 行，不向调用者泄露内部引用。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/util/stmtsummary/evicted.go`：Go 的 `list.List` 映射为 `VecDeque`，`PushFront`/`InsertAfter`/移除 Front 分别映射为 `push_front`/`insert(index + 1)`/`pop_front`；`AddEvicted` 都从来源最新窗口向旧窗口遍历，并按目标窗口从新到旧匹配。`collectHistorySummaries` 两边都选择最新 N 项后按升序返回；Rust 用 `skip(len.saturating_sub(N))`，Go 从 Back 收集后 `slices.Reverse`。

聚合规则与 Go `addInfo` 保持一致，包括 IA 指标、RU `Merge` 和资源组覆盖。`go_merge_34_test.rs` 专门验证最新窗口选择与 IA 求和/最大值。Rust 将 Go 的 nil 指针表达为 `Option<&...>`，但没有“nil receiver”概念；Go `matchAndAdd` 对 `seElement == nil` 返回 `isTooYoung` 的分支在 Rust 中由类型系统排除。

主要实现差异是同步方式：Go 在结构内部加锁并在计数读出时先复制快照再解锁，Rust 依赖外层 map 互斥锁和借用规则。另一个细节是 Rust 的元素级 `toEvictedCountDatum` 内联时间转换，而 Go 抽出 `evictedCountToDatum`；输出列语义相同。文档只陈述当前 Rust 接线，不据此声称所有 Go 并发调用模式都可无修改迁移。

## 扩展指南

新增一个可聚合统计字段时，先在 `statement_summary.rs::stmtSummaryStats` 定义并明确它属于求和、最大/最小、集合并集还是覆盖语义，再同步更新本文件 `addInfo`；至少扩展 `evicted_test.rs:test_add_info`，若来自 Go 增量还应补相应独立移植测试。遗漏这里会导致活跃 digest 数据正确、但淘汰后的 other 行丢字段。

改变窗口匹配或容量策略时，应修改 `AddEvicted` / `matchAndAdd`，并同步覆盖：同窗合并、乱序插入、相邻边界、部分重叠、多个来源窗口、`historySize` 为 0 和缩容只保留最新窗口。测试应继续放在独立的 `evicted_test.rs` 或迁移测试文件，不能内嵌到生产源文件。

新增淘汰计数列时需要同时检查元素与容器的 Datum 转换、`stmtSummaryByDigestMap` 转发接口以及上层执行/信息模式表的列契约。改变并发模型时必须审计全局 `StmtSummaryByDigestMap` 的外层锁和所有局部 map 调用点；不要只在本结构内局部加锁造成重复锁或不一致快照。时间转换变化需与 Go 的 `time.Unix`、MySQL TIMESTAMP 时区/精度行为共同验证。

## 验证依据

- 目标实现：`pkg/util/stmtsummary/evicted.rs`，RustCodeGraph `node --file` 完整读取 1-372 行；主要符号查询覆盖 `stmtSummaryByDigestEvicted`、`AddEvicted`、`matchAndAdd`、`ToEvictedCountDatum`、`collectHistorySummaries`、`addInfo`。
- 调用图：RustCodeGraph `callers`/`callees`（用 `--file pkg/util/stmtsummary/evicted.rs` 消歧）确认本文件内部调用及 reader/测试关系；`rg` 补充确认索引未解析出的 Rust 生产边 `statement_summary.rs:653`。
- crate 边界：`pkg/util/stmtsummary/Cargo.toml`、`pkg/util/stmtsummary/lib.rs`；本目录没有 `doc.go`。
- Rust 生产上下文：`pkg/util/stmtsummary/statement_summary.rs:193-194, 577-671`，`pkg/util/stmtsummary/reader.rs:232-279`。
- Go 对照：`pkg/util/stmtsummary/evicted.go:28-430`，核对容器、锁、窗口算法、Datum 顺序和全部统计合并规则。
- 独立 Rust 测试：`pkg/util/stmtsummary/evicted_test.rs`（nil、排序、裁剪、Datum、全字段合并、外层锁并发），`pkg/util/stmtsummary/evicted_1_aster_unit_test.rs`（迁移边界），`pkg/util/stmtsummary/go_merge_34_test.rs`（最新窗口与 IA 指标）。Go 测试入口由 RustCodeGraph caller 结果确认在 `pkg/util/stmtsummary/evicted_test.go`。
- 本任务仅新增说明文档，按计划不运行 Cargo；最终以固定 11 章节结构命令和人工事实复核验收。
