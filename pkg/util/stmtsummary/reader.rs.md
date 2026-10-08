# `pkg/util/stmtsummary/reader.rs`

源码链接：[`reader.rs`](reader.rs)。

## 文件定位

本文件属于 `astersql-util-stmtsummary` crate（见 `pkg/util/stmtsummary/Cargo.toml`），由 `pkg/util/stmtsummary/lib.rs` 的 `mod reader; pub use reader::*;` 对外导出。它位于语句摘要写入/聚合逻辑与信息模式表结果之间：读取全局 `StmtSummaryByDigestMap` 中按 digest 聚合的累计、当前窗口和历史窗口数据，并按照调用者请求的列顺序物化为 `Vec<Vec<types::Datum>>`。

完整应用中的对应入口目前在 Go 侧 `pkg/executor/stmtsummary.go:initSummaryRowsReader`：它创建 reader、可选设置 digest checker，再根据 cumulative/current/history 表类型选择读取方法。对 Rust 生产代码的仓库级搜索只发现 `reader.rs` 自身定义；当前 Rust 直接使用者位于同 crate 的独立测试中，因此该文件是已导出、已测试但尚未在 Rust executor 查询路径直接接线的迁移实现。

## 核心职责

1. `NewStmtSummaryReader` 绑定全局摘要 map、用户与 `PROCESS` 权限、实例地址、时区和请求列，并把每个列名预解析为 `columnValueFactory`；未知列立即 panic，防止表定义与列实现静默错位。
2. `GetStmtSummaryCumulativeRows`、`GetStmtSummaryCurrentRows`、`GetStmtSummaryHistoryRows` 分别输出跨窗口累计值、当前窗口值和受 `historySize` 限制的历史值。
3. `stmtSummaryChecker` 对 digest 做白名单过滤；`isAuthed` 对非 `PROCESS` 用户按 `stmtSummaryStats.authUsers` 做行级可见性过滤。
4. `columnValueFactoryMap` 把 summary/stats/element/reader 中的字段转换成 SQL 表列，统一处理平均值、纳秒时间、时区时间戳、空字符串、计划解码、RU、网络流量和存储类型等语义。
5. `StmtSummaryValue` 保留中间值的有符号、无符号、浮点、字符串、时间和既成 `Datum` 类型，再由 `into_datum` 完成最终编码；无效 UTF-8 计划字节会通过 `Datum::SetBytesAsString` 保持 Go string 的字节语义。

## 主要符号

- `stmtSummaryReader`：读取上下文。`user`/`hasProcessPriv` 控制权限，`columns` 与 `columnValueFactories` 一一对应，`instanceAddr` 生成集群表 `INSTANCE`，`ssMap` 指向全局或测试替换的摘要 map，`checker` 是可选 digest 白名单，`tz` 用于时间戳列。
- `NewStmtSummaryReader(...) -> stmtSummaryReader`：构造公开 reader；逐列查询 `columnValueFactoryMap()`，缺失注册时 panic。
- `GetStmtSummaryCumulativeRows` / `GetStmtSummaryCurrentRows` / `GetStmtSummaryHistoryRows`：三个公开读入口。
- `getStmtByDigestCumulativeRow`、`getStmtByDigestRow`、`getStmtByDigestHistoryRow`：选择累计数据、当前最新 element 或历史 elements。
- `getStmtByDigestElementRow`：权限校验后，对请求列依序调用工厂并转成 `Datum`。
- `getStmtEvictedOtherRow` / `getStmtEvictedOtherHistoryRow`：把被 LRU 淘汰的摘要聚合成 `other` 行；只有未设置 checker 时输出，避免 digest 精确查询混入无法归属到单一 digest 的数据。
- `stmtSummaryChecker`、`NewStmtSummaryChecker`、`isDigestValid`：包装 `set::StringSet` 并调用 `Exist`。
- `StmtSummaryValue` 与 `into_datum`：列工厂的类型联合和最终 Datum 转换。
- `columnValueFactory`：函数指针类型，参数依次为 reader、可选窗口 element、可选 digest 聚合对象和统计值。
- `columnValueFactoryMap`：列名到工厂的完整注册表；`insert_factory!`、`stat_field!`、`ru_field!`、`network_field!`、`digest_field!`、`avg_*` 宏只减少重复注册代码，不改变运行时分派模型。
- `duration_nanos`、`timestamp_value`、`system_time_seconds`、`ToI64`：饱和数值和时间转换辅助；大量 `*Str` 常量定义 summary/stats 表列名契约。

## 执行流程

构造阶段先保存上下文，然后为 `columns` 中每个 `ColumnInfo.Name.O` 查找工厂，保持请求列顺序。读取累计行时，reader 锁定 `ssMap`，遍历 `summaryMap`，先应用 checker，再检查累计统计的用户权限，最后按列工厂生成行。

读取当前行时同样遍历 map；`getStmtByDigestRow` 要求摘要已初始化、history 非空，且最新 element 的 `beginTime` 不早于 `beginTimeForCurInterval`，从而屏蔽惰性过期的上一窗口数据。普通摘要完成后，未设置 checker 的查询还尝试加入当前窗口的 evicted `other` 行。

读取历史行时将 `historySize()` 负值收敛为零，逐 digest 读取最多该数量的 elements；未初始化或 checker 不匹配的 digest 整体跳过。随后在无 checker 时追加 evicted history。每个 element 都经过 `getStmtByDigestElementRow` 的权限检查。

列物化时，工厂从四类上下文取值：digest 级元数据（如 SQL digest、schema、表名、计划 digest）、element 级窗口边界、stats 级计数/耗时/RU/网络字段，以及 reader 级实例地址/时区。平均值根据指标语义使用 `execCount` 或 `commitCount`；duration 转纳秒，窗口与 first/last seen 转指定时区的 MySQL timestamp。`PLAN` 列调用 `plancodec::DecodePlan`，失败则记 error 日志并返回空字节串。

## 数据与状态

reader 自身除 `SetChecker` 外不修改状态；`columns` 与 `columnValueFactories` 在构造后保持位置对应。`ssMap` 类型为 `&'static Mutex<stmtSummaryByDigestMap>`，生产默认指向全局 `StmtSummaryByDigestMap`，测试通过泄漏的 `Mutex` 替换它以隔离数据。

摘要层次是 map → `stmtSummaryByDigest` → `cumulative` 与 `history`；淘汰摘要另存于 `stmtSummaryByDigestEvicted.other.history`。当前行取 history 尾部，历史行按容器迭代顺序取前 `historySize` 项。evicted 行使用默认 `stmtSummaryByDigest`，因此 digest/schema 等不可归属字段为空或 NULL，而统计字段来自 `otherSummary`。

`StmtSummaryValue::Null` 转默认 Datum；整数类型保持 signed/unsigned 差异，`Duration` 饱和转换到 `i64` 纳秒，`u64 -> i64` 的 `ToI64` 同样在 `i64::MAX` 饱和。RocksDB、写入量、受影响行和 IA 平均值中需要保留大 unsigned 范围的列使用 `f64`；相关测试覆盖大于 `i64::MAX` 的输入及分母为零语义。

## 依赖与调用关系

上游契约是信息模式语句摘要查询。Go 侧调用边为 `pkg/executor/stmtsummary.go:initSummaryRowsReader` → `NewStmtSummaryReader` → 可选 `NewStmtSummaryChecker`/`SetChecker` → 三个 `GetStmtSummary*Rows` 之一。Rust 侧 `pkg/util/stmtsummary/lib.rs` 导出本文件 API；当前非测试 Rust 代码仅使用同 crate 的写入/聚合类型，未检索到这三个读取 API 的直接生产调用者。

下游主要依赖来自本 crate 的 `statement_summary.rs` 与 `evicted.rs` 类型及辅助函数：`stmtSummaryByDigestMap`、`stmtSummaryByDigest`、`stmtSummaryByDigestElement`、`stmtSummaryByDigestEvicted`、`stmtSummaryStats`、`avgInt`、`avgFloat`、`avgFloat4Uint`、`avgSumFloat`、`convertEmptyToNil` 和 `formatBackoffTypes`。Cargo 边界明确依赖 `chrono`/`chrono-tz`、`log`，以及 auth、model、mysql、plancodec、set、types 等本地 crate；`lib.rs` 通过窄再导出模块提供这些类型。

RustCodeGraph 的 callees 证据显示 `NewStmtSummaryReader` 调用 `columnValueFactoryMap` 并实例化 reader；`GetStmtSummaryCurrentRows` 调用 `isDigestValid`、`getStmtByDigestRow` 和 `getStmtEvictedOtherRow`。独立源码搜索补充确认了 Rust 生产调用尚未接线，以及 Go executor 的真实上游入口。

## 错误处理与边界

- 未注册列被视为编程错误，构造器 panic；新增表列必须同时注册工厂。
- `ssMap.lock().expect(...)` 在 mutex poisoned 时 panic；本文件没有把锁错误转换为 SQL 错误。
- `timestamp_value` 对 chrono 无法表示的秒数 panic；正常摘要时间必须位于有效范围。
- 工厂内对语义必需的 `ssElement` 或 `ssbd` 使用 `expect`。调用路径必须保证累计行不请求窗口时间列，或保证需要 digest/element 的列拿到相应上下文；否则会 panic。
- 未初始化摘要、空 history、惰性过期当前 element、未授权用户和 checker 不匹配均静默跳过该行。
- digest checker 存在时不返回 evicted `other`，因为其统计无法证明属于白名单中的 digest。
- 空 schema/digest/table/index 等经 `convertEmptyToNil` 变成 SQL NULL；样例用户来自 `HashMap` 的任意首个 key，不承诺稳定选择。
- 计划解码错误不是整行错误：记录日志并让 `PLAN` 为空；非 UTF-8 解码结果仍保持 string Datum 的原始字节。
- 平均值的零分母行为由 crate 辅助函数决定，测试确认相关浮点平均值返回 `0.0`。

## 并发与资源生命周期

Rust reader 的三个公开读取方法在取得 `ssMap` mutex 后，持锁遍历并完成权限过滤和全部列物化，guard 在方法返回时释放；因此结果是一致的 map 视图，但昂贵列（特别是计划解码）会延长全局锁持有时间。reader 不创建线程、任务、通道、事务或 I/O 资源，返回行拥有自己的 Datum 数据。

这与 Go 锁粒度不同：Go 先在全局 map 锁下复制 values/beginTime/other 引用并解锁，再使用每个 digest、element 或 evicted 对象自身的锁读取。Rust 数据结构由外层 `Mutex` 统一保护，当前实现没有对应的内部锁。扩展时必须保持“读取期间不可并发突变”的不变量，并评估新工厂是否会显著增加锁内 CPU 时间或产生阻塞调用；工厂不应执行网络/磁盘 I/O。

测试辅助 `reader_for`/相关用例通过 `Box::leak` 获得 `'static` map，仅用于测试进程生命周期；生产 reader 使用真正的全局静态 map。

## 与 Go 版本的对应关系

Rust 文件逐项移植 `pkg/util/stmtsummary/reader.go`：reader/checker 结构、三种读取模式、权限与 digest 过滤、evicted `other` 规则、列名常量和列工厂语义均有直接对应。Go 的 `any` 工厂返回值在 Rust 中由 `StmtSummaryValue` 显式建模；Go 的 `types.NewDatum` 由 `into_datum` 替代；Go `time.Location` 对应 `chrono_tz::Tz`。

数值对应关系保留了 Go 的关键分母：执行期指标按 `execCount`，事务提交指标按 `commitCount`；unsigned 累计量需要浮点列时先转 `f64`。Go string 可含任意字节，Rust 通过 `From<Vec<u8>>` 在 UTF-8 失败时构造 bytes-as-string Datum。计划解码失败两侧都记录错误并返回空字符串/字节，而不丢弃整行。

已确认的实现差异是锁模型：Go 使用全局快照加对象级锁，Rust持有全局 mutex 完成读取。另一个迁移状态差异是 Go executor 已直接消费 reader，而 Rust executor 尚未检索到对应读取调用。两点都应视为当前事实，不应把 Go 的生产接线或细锁粒度假定为 Rust 已具备。

## 扩展指南

新增/改名语句摘要列时，应同步修改列名常量和 `columnValueFactoryMap`，确认所需上下文是 reader、element、digest 还是 stats，并确保 cumulative/current/history 各路径不会向工厂传入不允许的 `None`。若字段来自聚合数据，还应先在 `statement_summary.rs` 的统计累积/合并路径保持 Go 同名逻辑，再在独立测试文件中覆盖；不要把测试内嵌进 `reader.rs`。

选择平均值分母前应核对 Go 工厂：执行观测通常使用 `execCount`，仅提交时产生的指标使用 `commitCount`。涉及 `u64` 大值时避免先压到 `i64`；时间字段要明确单位并沿用时区转换。新增可能失败的转换时，应决定是列级降级（类似 `PLAN`）、跳过行还是传播错误，并与 Go 行为保持一致。

测试应优先扩展 `pkg/util/stmtsummary/reader_test.rs`（列工厂类型/字节边界）或 `pkg/util/stmtsummary/statement_summary_test.rs`（当前/历史/淘汰/权限的端到端聚合）；同时对照 `statement_summary_test.go`。若将 Rust reader 接入 executor，应另补 executor 层针对 cumulative/current/history 表选择、digest 下推和列顺序的测试，并关注全局锁内计划解码的性能风险。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 文件；`files --filter pkg/util/stmtsummary` 确认目标、Go 对照和测试均被索引；`node --file pkg/util/stmtsummary/reader.rs --offset 1/421/921` 阅读完整 1,332 行；`query NewStmtSummaryReader`、`callees NewStmtSummaryReader`、`query/callees GetStmtSummaryCurrentRows` 核对构造与当前行调用链。`explore` 和部分 `callers` 查询在 30 秒内无输出，因此上游接线用精确 `rg` 补证。
- 源码与边界：`pkg/util/stmtsummary/reader.rs`、`pkg/util/stmtsummary/lib.rs`、`pkg/util/stmtsummary/Cargo.toml`、`pkg/util/stmtsummary/reader.go`、`pkg/executor/stmtsummary.go`。
- Rust 独立测试：`pkg/util/stmtsummary/reader_test.rs` 覆盖大 unsigned 浮点指标、执行次数分母、零分母和非 UTF-8 plan bytes；`pkg/util/stmtsummary/statement_summary_test.rs` 覆盖当前/历史行、上一窗口 evicted 排除、权限、时间列和 IA 指标；`pkg/util/stmtsummary/evicted_1_aster_unit_test.rs` 也直接构造 reader 验证淘汰聚合。
- Go 对照测试：`pkg/util/stmtsummary/statement_summary_test.go` 的 reader 构造、`TestToDatum`、权限/current/history/evicted 与列工厂用例提供原语义证据。
- 本任务是纯文档分析，依任务计划不运行 Cargo；交付结构以任务指定的 11 标题检查，并人工复核“为何存在、如何运行、如何安全扩展”。
