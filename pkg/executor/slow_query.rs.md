# `pkg/executor/slow_query.rs`

## 文件定位

本文件属于 `astersql-executor` crate（`pkg/executor/Cargo.toml` 的库入口为 `lib.rs`），并由 `pkg/executor/lib.rs` 以 `pub mod slow_query` 公开。它把 `INFORMATION_SCHEMA.SLOW_QUERY` 所需的慢日志读取、筛选、解析、内存记账和运行时统计抽象为泛型 Rust 实现；真正的文件系统、压缩读取、会话权限、Datum 构造、warning、取消和异步 worker 都通过 `SlowQueryRuntime` 注入。

当前仓库未找到任何 `impl SlowQueryRuntime for ...`，也没有在 Rust 生产代码中构造 `slowQueryRetriever` 的证据。因此，本文件目前提供的是公开的移植边界和可测试算法组件，而不是已经接入 Rust executor 主链的完整生产实现。Go 生产入口仍可在 `pkg/executor/builder.go:2906`（构造 `slowQueryRetriever`）和 `pkg/executor/memtable_reader.go`（调用 `retrieve`）看到。

## 核心职责

- `slowQueryRetriever<R>` 在首次 `retrieve` 时建立输出列工厂、权限/时间检查器和日志文件顺序，启动解析 worker，随后逐批接收结果。
- `parseLog` 把以 `# Time: ` 开始、以 SQL 分号行结束的文本条目转为 `R::Row`，应用时间和 PROCESS/用户权限过滤，并补齐 `INSTANCE` 与默认列。
- `slowLogReverseScanner`、`ReadLastLinesFromFile` 和行切分函数提供倒序读取与文件尾部读取基础能力。
- `slowQueryRuntimeStats`、`calculateLogSize`、`calculateDatumsSize` 和 `memConsume` 管理统计与内存核算。
- `splitByColon`、`findMatchedRightBracket`、`parseUserOrHostValue`、`parsePlan` 等函数承担慢日志字段语法和值转换的公共逻辑。

## 主要符号

- `SlowQueryRuntime`：本文件的核心依赖倒置接口。关联类型描述上下文、文件、Reader、Row、Datum、Time、Error、列工厂和取消令牌；方法覆盖日志发现/读取、列写入、时间/计划解析、warning、内存以及 worker 生命周期。所有方法都必须由实现方提供，没有“成功”的默认实现。
- `slowQueryRetriever<R>`：有状态检索器。`files`/`fileIdx`/`fileLine` 表示扫描位置，`checker` 保存权限与时间窗口，两个 factory 字段描述输出列，`stats`、`lastFetchSize` 和 `cancel` 管理资源生命周期。
- `slowLogChecker<T>`：`hasPrivilege` 在有 PROCESS 权限或日志用户等于当前用户时放行；`isTimeValid` 在未启用时间筛选时恒真，否则要求时间落入任一闭区间。
- `timeRange<T>`、`logFile<F,T>`、`offset`、`slowLogTask`、`parsedSlowLog<Row,Error>`：分别描述查询时间窗、日志文件元数据、源位置、解析输入批和解析结果。
- `ParseSlowLogBatchSize`：原子全局批大小，默认 64，以 Acquire 顺序读取；`maxReadCacheSize` 为 64 MiB；`slowLogTimeRangeInternalTolerance` 为 1 秒。
- `slowLogReverseScanner`：保存对 retriever 的可变借用和预加载块；`loadCompressedBlocks` 整体读取压缩文件并按 `# Time:` 分块，`nextBatch` 从块栈拼批。
- `slowQueryRuntimeStats`：记录文件总数/已读数、初始化/读取/解析耗时、字节数和解析并发；`Merge` 累加统计并对并发取最大值，`Tp` 当前固定返回 `1`。

## 执行流程

1. 调用 `slowQueryRetriever::retrieve`。若尚未初始化，依次调用 `initialize`、`create_cancel` 和 `initializeAsyncParsing`。
2. `initialize` 遍历 `output_columns`：`INSTANCE` 使用专用工厂，其余列由 `column_factory` 映射；随后根据当前用户、PROCESS 权限和时间窗口构造 `slowLogChecker`，通过 `getAllFiles` 获取并按开始时间升序排序文件，降序请求则反转。
3. `initializeAsyncParsing` 经 `parseDataForSlowLog` 调用 runtime 的 `spawn_parser`。本文件没有 worker 实现，正向/反向入口 `parseSlowLog`、`parseSlowLogReversed` 也都直接委托同一个 runtime 方法。
4. `dataForSlowLog` 先释放上一批结果的内存记账，再从 `receive_parsed` 收取一批；通道结束返回空批，解析错误直接返回，成功则估算本批行大小并重新记账。
5. 若 runtime 使用本文件的解析辅助，`getBatchLog` 会逐行读取并在每行前检查取消；`parseLog` 遇到 `# Time: ` 创建新行，遇到 `# ` 用 `splitByColon` 拆字段，遇到非 `use ` 且以 `;` 结尾的 SQL 时检查用户权限、写入 SQL/INSTANCE/默认值并产出该行。
6. 调用 `close` 时尝试关闭全部文件并保留第一个关闭错误，然后触发取消、等待 worker，并释放最后一批内存；即使文件关闭失败，取消、等待和内存释放仍会执行。

倒序辅助路径与主路径是分离的：`loadCompressedBlocks` 会把压缩文件全部物化为条目并反转；`ReadLastLinesFromFile` 则 clone 文件描述符，从 `min(end_cursor, 文件长度)` 向前读取至多 `max_cache` 字节，再规范化 CRLF 并切行，不改变调用方游标。

## 数据与状态

- 文件序列以 `logFile.start` 排序；`getNextFile` 推进 `fileIdx`，并在能取得大小时累加 `readFileNum`/`readFileSize`。`getPreviousReader` 使用 `fileIdx - 2` 定位刚越过文件之前的文件。
- `checker` 在 `initialize` 后才存在；`parseLog` 对此使用 `expect("slow log initialized")`，所以绕过初始化直接解析会 panic，这是调用顺序不变量。
- `parseLog` 只在 SQL 终止行提交 row；新的 `# Time:` 会替换尚未提交的 row，缺少起始标记的字段/SQL 不会产生结果，`use ` 行被忽略。
- `user` 由名为 `User` 的字段经 `parseUserOrHostValue` 提取；权限在 SQL 提交时检查。未知字段被忽略，但没有输出 Time 列时仍会解析 Time 并执行时间过滤。
- `lastFetchSize` 只追踪交给调用方的上一批 Row；`sendParsedSlowLogCh` 另行增加 parsed rows 的记账，但本文件并不实现通道所有权及相应释放策略，需由 runtime 与调用链保持平衡。
- `DashboardSlowLogReadBlockCnt4Test` 仅能读取，当前文件没有递增点；不能据此声称生产读取次数已完整统计。

## 依赖与调用关系

上游方面，`pkg/executor/lib.rs` 公开模块，并在测试配置中装配 `slow_query_test.rs` 与 `slow_query_sql_test.rs`。RustCodeGraph 对精确名称的查询显示 `ReadLastLinesFromFile` 被 `pkg/executor/slow_query_test.rs` 和 `pkg/executor/benchmark_test.rs` 使用；仓库文本检索未发现 `SlowQueryRuntime` 实现或 Rust 生产态 `slowQueryRetriever` 构造点。因此当前可信的 Rust 调用链是“测试/潜在 runtime → 本文件公共 API”，而非已验证的 session → executor 生产链。

下游方面，具体 I/O、时间/计划解析、Row/Datum 写入、warning、内存 tracker 和异步收发全部经 `SlowQueryRuntime` 调用；本文件直接使用标准库 `File`/`Read`/`Seek` 实现 `ReadLastLinesFromFile`，并使用 `astersql_util_memory::tracker::FormatBytes` 格式化统计。`pkg/executor/Cargo.toml` 声明了 `astersql-util-memory` 路径依赖，crate 没有为本模块设置专属 feature。

Go 对照调用链为 `pkg/executor/builder.go` 构造 retriever、`pkg/executor/memtable_reader.go::Next` 调 `retrieve`，再进入 `initialize`/`initializeAsyncParsing`、正向或反向批读取、`parseLog` 与结果通道。该链仅用于解释设计来源，不能替代 Rust 接线证据。

## 错误处理与边界

- 大部分 runtime 操作通过 `Result` 立即传播；`close` 是例外，它继续清理所有资源并最终返回首个文件关闭错误。
- `dataForSlowLog` 把 `parsedSlowLog.err` 转为调用错误；收到 `None` 表示解析结束而不是错误。
- `getBatchLog` 每次迭代检查取消，但 `parseLog` 自身当前不检查取消；`loadCompressedBlocks` 发现取消时仅停止读取并返回已有块，不返回取消错误。
- 字节行使用 `String::from_utf8_lossy`，非法 UTF-8 会被替换而不是报错；`splitByColon` 对不匹配括号或字段/值数量不等返回两个空向量。
- `parsePlan` 解码失败时回退到剥离包装后的原字符串；`slowLogTimeWithTolerance` 在时间加减溢出时回退原值。
- `getFileStartTime`/`getFileEndTime` 最多考察约 128 行，找不到时间时调用 `parse_time("")` 让 runtime 决定错误语义。
- `ReadLastLinesFromFile` 拒绝负 cursor，cursor 超过文件长度时截到 EOF；它读取的是末尾至多 `max_cache` 字节，不保证从完整日志条目或完整首行开始。

## 并发与资源生命周期

`ParseSlowLogBatchSize` 可跨线程调整，并以 Acquire load 读取。实际并发、channel 和 worker 由 runtime 实现：retriever 只负责启动、接收、取消和等待。`retrieve` 仅以 `initialized` 做一次性门控，没有内部锁，因而应由单一执行器顺序调用，不能假设同一实例可并发 `retrieve`/`close`。

文件句柄由 runtime 列举并存入 `files`，`close` 对每个句柄调用 `close_file`。取消令牌仅在首次 `retrieve` 初始化成功后创建；`close` 通过 `take` 保证最多取消一次，并在释放最后一批内存前等待 worker。反向扫描器持有 retriever 的独占可变借用，Rust 类型系统阻止扫描期间同时修改同一 retriever。

内存核算有两层：文本/结果大小由 `calculateLogSize`、`calculateDatumsSize` 饱和累加，实际 tracker 更新由 runtime 执行；`dataForSlowLog` 维持跨 fetch 的上一批生命周期。runtime 若在 worker 侧调用 `sendParsedSlowLogCh`，必须设计对应释放点，否则可能重复记账。

## 与 Go 版本的对应关系

结构和命名基本对应 `pkg/executor/slow_query.go`：retriever/checker/file/offset/stats、字段拆分、文件起止时间探测、正反向扫描、runtime stats 与尾读函数均能找到同名来源。Rust 独立测试 `slow_query_test.rs` 明确以 Go `TestSplitByColon` 用例矩阵校验空值、嵌套方/花括号和畸形括号，并校验真实文件尾读。

但当前 Rust 版本不是 Go 完整语义的等价生产接线，重要差异包括：

- Go `parseSlowLogByBatchGetterWithLimit` 串行解析并严格截断到 limit；Rust 同名方法忽略 `_limit`，仅启动 runtime worker。
- Go 正向与反向解析分别驱动 reader/scanner，并控制 goroutine、任务通道和并发度；Rust 两个入口都只调用 `spawn_parser`，方向和并发策略完全交给 runtime。
- Go `parseLog` 处理 Prev_stmt、User@Host、Cop_backoff、Warnings、Session_connect_attrs、DB 等特殊字段，逐行检查 context、恢复 panic、记录 warning 与 parse 耗时；Rust 实现采用通用冒号拆分，未复刻这些专门分支、panic 恢复和逐行取消。
- Go `getAllFiles` 扫描轮转文件、对不可用文件追加 warning，并依据文件起止时间（压缩文件用下一文件起点）提前剪枝；Rust `getAllFiles` 只对 runtime 返回的列表排序，发现和剪枝职责留给 runtime。
- Go 时间区间重叠计算应用 ±1 秒容差；Rust `slowLogMayOverlapTimeRangeWithTolerance` 当前只做无容差闭区间相交判断，容差函数独立存在但未在该判断中调用。
- Go 尾读按扩大窗口寻找行边界；Rust `ReadLastLinesFromFile` 是固定字节尾片段。二者在 `max_cache` 截断发生于行中间时不应被视为完全等价。

这些差异应作为迁移状态记录，而不是在文档中推断已由不存在的 runtime 自动补齐。

## 扩展指南

- 接入生产链时，先实现一个真实 `SlowQueryRuntime`，明确 Row/Datum 类型、session warning、gzip reader、时间列时区、计划解码、权限来源和 channel 协议，再在 memtable builder/reader 中构造并关闭 retriever；同时新增独立 `*_test.rs`，不要把测试放回本源文件。
- 若补齐 limit 或倒序语义，优先修改 `parseSlowLogByBatchGetterWithLimit`、`parseSlowLogReversed` 和 `slowLogReverseScanner`，以 Go 对应函数及 `TestSlowQueryRetrieverReversedScanWithLimit`、`TestSlowQueryRetrieverReversedScanWithTimeJitter` 为行为基线。
- 若增强字段解析，修改 `parseLog`/`setColumnValue`/`splitByColon`，同步扩展 `pkg/executor/slow_query_test.rs`；需要覆盖 User@Host、特殊长字段、畸形输入、warning 与无 Time 输出列时的时间筛选。
- 若调整文件发现/时间剪枝，修改 runtime 的 `list_log_files` 契约及 `getAllFiles`，同步验证压缩/非压缩轮转文件、1 秒容差、损坏文件 warning 和取消行为。
- 若修改尾读，保持调用方游标不变、负 cursor/EOF/max-cache 边界和 CRLF 语义，并同步 `slow_query_test.rs` 与 `benchmark_test.rs` 中的真实文件用例。
- 性能风险主要在压缩文件整体物化、批大小、Row 内存双重记账和大文件尾读；兼容风险主要在字段语法、时区/时间容差、权限过滤、limit 与日志轮转顺序。

## 验证依据

- Rust 源与装配：`pkg/executor/slow_query.rs`（966 行）、`pkg/executor/lib.rs:230,443-448`、`pkg/executor/Cargo.toml`（`astersql-executor`、`lib.rs`、`astersql-util-memory`）。
- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点、1,848,419 边；对 `slowQueryRetriever`、`parseSlowLog`、`parseLog`、`ReadLastLinesFromFile` 的 query/explore/callers/callees 查询用于核对符号和调用边。宽泛同名查询存在 Go/Rust及其他模块歧义，因此结论同时用文件限定结果复核。
- Rust 测试：`pkg/executor/slow_query_test.rs` 覆盖字段拆分、用户提取、嵌套括号和真实尾读；`pkg/executor/slow_query_sql_test.rs` 覆盖行切分、大小计算、统计合并，并提供更宽的 SQL 行为证据；`pkg/executor/benchmark_test.rs` 覆盖真实尾读边界和一个 ignore 的 10 MiB 长行案例。
- Go 对照：`pkg/executor/slow_query.go`、`pkg/executor/slow_query_test.go`，以及生产构造/调用点 `pkg/executor/builder.go:2906`、`pkg/executor/memtable_reader.go`。
- 接线限制：`rg 'impl(.*)SlowQueryRuntime|SlowQueryRuntime for|slowQueryRetriever<' --glob '*.rs' pkg/executor` 只命中本文件的泛型定义/impl，没有 runtime 实现；因此本文没有把 Go 的生产调用关系误报为 Rust 已接线。
- 本任务是纯文档分析，按任务要求不运行 Cargo。结构验证应确认目标文件存在且恰有本文固定的 11 个二级标题。
