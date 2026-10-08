# `pkg/util/stmtsummary/v2/record.rs`

## 文件定位

本文件是 `astersql-util-stmtsummary-v2` crate 中“单条语句摘要”的数据与聚合层。crate 根在 [`lib.rs`](./lib.rs) 中以私有 `mod record` 装载它，再用 `pub use record::*` 对外暴露；[`Cargo.toml`](./Cargo.toml) 说明它依赖 `task-stmtsummary` 的 `StmtExecInfo` 输入、`task-stmtctx` 的语句上下文，以及 `task-execdetails` 的 Coprocessor、2PC、RU 和 TiKV 执行细节。

它位于一次 SQL 执行和摘要窗口之间：会话侧 [`scan_adapter_runtime.rs`](../../../session/runtime/scan_adapter_runtime.rs) 组装 `StmtExecInfo`，先调用 `SelectRUDetailsForStatementSummary` 选择 RU 口径，再调用 v2 摘要入口；[`stmtsummary.rs`](./stmtsummary.rs) 的 `StmtSummary::Add` 按 digest（可选附加用户）定位窗口记录，新键调用 `NewStmtRecord`，随后每次都调用 `StmtRecord::Add`。窗口驱逐时，`stmtEvicted::add` 通过 `StmtRecord::Merge` 合并成 `other`/`otherForPersist`。

## 核心职责

1. `StmtRecord` 保存一组同键语句的身份、首样本和聚合指标，包括延迟、Cop/TiKV 扫描、IA 远程段读取、2PC、内存/磁盘、重试、CPU、RU、网络流量、计划缓存和存储引擎标记。
2. `NewStmtRecord` 只初始化不随执行次数求和的身份/样本字段与 min 初值；调用者必须再调用 `Add` 才会令 `ExecCount` 增长并计入本次指标。
3. `StmtRecord::Add` 把一个 `StmtExecInfo` 累加到当前记录，维护 sum/max/min、集合和最后原因等不同聚合规则。
4. `StmtRecord::Merge` 合并已经聚合过的同键记录，主要用于窗口驱逐聚合；它不是重新执行 `Add`，而是合并两侧已有的 sum/max/min。
5. `formatSQL`、计划大小上限和二进制 discard 编码控制首样本的内存占用；`SelectRUDetailsForStatementSummary` 在进入摘要前统一 v1/v2 RU 展示口径。
6. `GenerateStmtExecInfo4Test` 和相关 setter 是独立测试夹具入口，不参与生产采集主链。

## 主要符号

- `pub struct StmtRecord`：可克隆、可 Serde 序列化的聚合记录。`Begin`/`End` 表示持久化窗口边界；身份和样本字段在构造时确定；其余字段按 sum/max/min、计数、集合或最新值聚合。IA 字段显式指定 snake_case JSON 键，IA 等待时间借助 `durationNanosSerde` 以纳秒 `i64` 序列化。
- `pub fn SelectRUDetailsForStatementSummary(raw, version, total_ru_v2, is_write)`：非 v2 或 v2 总量尚未完成时保留原始 RU；已有 v2 总量时，读语句把总量放入 RRU，写语句放入 WRU，并保留原始 RU wait duration。
- `pub fn NewStmtRecord(info) -> Box<StmtRecord>`：规范化表名，回退获取 plan digest，获取首个 SQL/计划/binding/index 样本，应用大小限制，并设置 `MinLatency`、`MinResultRows = i64::MAX`、first/last seen 等初值。
- `pub fn StmtRecord::Add(&mut self, info)`：单次执行聚合入口。它更新错误/告警、各类延迟、扫描和提交细节、资源消耗、时间范围、RU/CPU/流量以及存储引擎标志。
- `pub fn StmtRecord::Merge(&mut self, other)`：记录级合并入口。对集合取并集、sum 相加、max/min 取极值，并让非空的 plan-cache 未命中原因覆盖旧值。
- `pub fn formatSQL(sql)`：按原始 UTF-8 字节长度截断；为避免截断在码点中间，会向前回退到合法字符边界，并附加 `(len:N)`。
- `MaxEncodedPlanSizeInBytes`/`SetMaxEncodedPlanSizeInBytesForTest`、`setGlobalMaxSQLLength`/`SetGlobalMaxSQLLengthForTest`：使用 relaxed 原子读写的全局限制。前者默认 1 MiB，后者默认 32768 字节。
- `binaryPlanDiscardedEncoded`：把表示 `discarded_due_to_too_long=true` 的两字节 protobuf 负载做 raw Snappy 压缩和 Base64 编码，生成可被二进制计划消费者识别的占位值；文本计划使用常量 `[discard]`。
- `GenerateStmtExecInfo4Test` 与私有 `mockLazyInfo`：构造固定的表、索引、Cop、2PC、RU、CPU 等测试输入；应只作为测试辅助理解。

## 执行流程

1. 会话完成执行后，在 [`scan_adapter_runtime.rs`](../../../session/runtime/scan_adapter_runtime.rs) 中构造 `StmtExecInfo`。`SelectRUDetailsForStatementSummary` 根据 `ru_version`、`total_ru_v2` 和读写属性确定写入摘要的 RU 细节，然后把信息交给 v2 `Add`。
2. [`StmtSummary::Add`](./stmtsummary.rs) 在窗口锁内计算 digest key。已有键时复用 `Arc<Mutex<StmtRecord>>`；新键时先用 `NewStmtRecord` 保存身份和首样本，再插入 LRU，必要时触发驱逐。退出窗口锁后取得记录锁并调用 `StmtRecord::Add`。
3. `NewStmtRecord` 从逻辑计划表中跳过空表名（例如只有数据库名的 `CREATE DATABASE`），把 `db.table` 转为小写并以逗号连接。显式 plan digest 为空时从 `LazyInfo` 回退；文本/二进制计划超过全局上限时分别换成对应 discard 占位；归一化 SQL 和原始 SQL 都经 `formatSQL`。
4. `Add` 先处理执行次数、用户、成功状态和解析/编译/总延迟，再依次处理 Cop 摘要、TiKV 扫描、IA 统计、2PC 提交、计划缓存与 binding、资源/时间范围、重试与结果行、TiKV/PD 等待、CPU、网络、RU 和存储类型。
5. 当 LRU 驱逐记录时，[`stmtEvicted::add`](./stmtsummary.rs) 把记录合并进查询用的 `other`；若该记录尚未排队写日志，还会合并进持久化用的 `otherForPersist`。`newEvictedAggregateRecord` 把 `MinLatency` 初始化为极大值，使第一次 merge 的 min 规则有效。

重要分支与不变量：`ExecCount` 只在 `Add` 增长；`NewStmtRecord` 后仍是零次执行。`IsInternal` 使用逻辑与，混入任一外部执行后即为 false。结果行只在正数时累加，否则 `MinResultRows` 变为 0。没有 `ScanDetail` 或 `CommitDetail` 时对应统计保持不变。只有 IA count 大于 0 的执行才增加 `IAExecCount`。最慢 Cop process/wait 地址必须和对应最大耗时一起更新。

## 数据与状态

`StmtRecord` 的字段可按聚合语义分组：

- 身份/首样本：schema、SQL/plan digest、语句类型、规范化 SQL、表/索引、binding、字符集、首个计划、prepared、keyspace 和 resource group。构造后通常不在 `Add`/`Merge` 中改写。
- 计数与集合：`ExecCount`、错误/告警、提交/IA 次数、缓存命中/未命中、重试次数；`AuthUsers` 去重用户，`BackoffTypes` 累积每种退避次数。
- sum/max/min：延迟、扫描键/RocksDB、IA、2PC、内存/磁盘、结果行与 RU。`FirstSeen` 取最早，`LastSeen` 取最晚。
- 最新执行状态：`PlanInCache`、`PlanInBinding`、`StorageKV`、`StorageMPP` 在 `Add` 中由本次输入覆盖；`PlanCacheUnqualifiedLastReason` 保存最近一次非空原因。`Merge` 只合并其代码明确覆盖的聚合字段，不能假设所有“最新状态”字段都会被另一个记录覆盖。
- 全局配置：两个原子上限对进程内所有 v2 摘要实例生效。`StmtSummary::SetMaxSQLLength` 会同步 `GLOBAL_MAX_SQL_LENGTH`；测试通过全局互斥锁串行修改并在断言后恢复默认值。

`StmtRecord` 可被克隆用于窗口快照和驱逐日志。Serde 默认把字段改成 snake_case；`Begin`、`End`、`Digest`、`ExecCount` 和 IA 字段有显式契约。`SystemTime`、`Duration`、集合/映射的持久化格式因此也是日志兼容面，修改字段或 serde 属性需要考虑历史记录反序列化。

## 依赖与调用关系

上游调用关系（由 RustCodeGraph 文件使用关系与限定路径搜索共同确认）：

- [`scan_adapter_runtime.rs`](../../../session/runtime/scan_adapter_runtime.rs) → `SelectRUDetailsForStatementSummary` → v2 摘要全局 `Add`。
- [`stmtsummary.rs`](./stmtsummary.rs) 的 `StmtSummary::Add` → `NewStmtRecord`（仅新键）→ `StmtRecord::Add`（每次执行）。
- [`stmtsummary.rs`](./stmtsummary.rs) 的 `stmtEvicted::add` → `StmtRecord::Merge`，分别形成查询和待持久化的驱逐汇总。
- [`column.rs`](./column.rs) 消费 `StmtRecord` 字段生成 statement summary 列；`record_test.rs`、`reader_test.rs` 等测试也直接构造/读取记录。

主要下游依赖：

- `task-stmtsummary::{StmtExecInfo, StmtExecLazyInfo}` 提供执行快照与延迟加载的 SQL/计划/binding。
- `task-stmtctx::{NewStmtCtx, TableEntry}` 提供表、索引、告警、影响行数和 TiKV/TiFlash 标志。
- `task-execdetails` 提供 `CopTasksSummary`、`ExecDetails`、`CommitDetails`、`ScanDetail`、`RUDetails` 和 `LoadTiKVExecDetails`。
- `serde` 保持日志 JSON 契约；`snap` 与 `base64` 生成二进制计划 discard 占位。
- 标准库 `AtomicU32`/`AtomicUsize` 用于全局限制，`HashSet`/`HashMap` 用于用户和退避类型聚合。

RustCodeGraph 的 `status` 显示索引覆盖本文件及同目录 Rust/Go 文件；其文件关系报告本文件被会话适配器、`column.rs` 和测试使用。由于 `Add`/`Merge` 是仓库内高频重名，通用 explore 产生歧义，精确 callers/callees 未返回边，因此上述具体边又用限定到 `pkg/util/stmtsummary/v2` 与会话适配器的符号搜索复核。

## 错误处理与边界

- 生产聚合接口不返回 `Result`；可选的 Cop、scan、commit 和 RU 细节以 `Option` 分支安全跳过。
- `durationFromNanos` 把非正纳秒值收敛为零，避免负计数转换为超大 `Duration`。IA JSON 反序列化则明确拒绝负等待时间。
- `formatSQL` 的限制单位是 UTF-8 字节，不是字符数；超长时保留合法前缀并记录原始字节长度。上限为 0 时得到空前缀加长度后缀。
- 文本计划只有在 `len() > limit` 时替换为 `[discard]`；等于上限仍保留。二进制计划使用协议兼容占位，不能随意换成普通文本。
- `IndexNames` 和 commit detail 内部状态需要加锁；当前实现对 poisoned 标准锁使用 `expect`，即锁中毒会 panic。二进制占位的固定两字节压缩也以 `expect` 表达不可失败假设。
- 所有数值累加使用普通 Rust 算术，没有饱和或 checked 保护；极端长期累计可能溢出整数或 `Duration`，当前实现与既有聚合路径一样依赖现实窗口规模。
- `Merge` 假设双方属于相同摘要键。它不会验证 digest/schema 等身份是否一致，也不会重新合并所有首样本/最新状态字段；错误地跨键调用会得到语义混杂的记录。

## 并发与资源生命周期

`StmtRecord` 本身不实现内部同步；正常生产路径由 [`stmtsummary.rs`](./stmtsummary.rs) 的 `Arc<parking_lot::Mutex<StmtRecord>>` 保护。窗口 LRU 另有互斥锁，`StmtSummary::Add` 在窗口锁内完成定位/插入和可能的驱逐，随后释放窗口锁再锁定单条记录，缩短全局临界区。驱逐合并发生在持有窗口锁时，传入的是已锁定后取得的记录值。

读取输入时仍要尊重其内部并发结构：索引名使用标准互斥锁克隆；`ResolveLockTime`、`PrewriteRegionNum`、TiKV 等待/流量与存储标志使用 relaxed 原子读取；commit backoff 列表在 `commit.Mu.Lock()` 保护下读取。这里的 relaxed 足以取得独立统计快照，但不提供跨字段一致性事务。

两个全局大小限制也使用 relaxed 原子操作，更新会被后续构造观察到，但并不追溯修改已存在记录。测试修改全局限制时必须持有 `SQL_LENGTH_TEST_LOCK` 并恢复默认值，避免并行测试污染。`StmtRecord` 不持有线程、通道或外部资源；后台轮转/驱逐线程和关闭生命周期属于 [`stmtsummary.rs`](./stmtsummary.rs)。

## 与 Go 版本的对应关系

直接对照文件是 [`record.go`](./record.go)，基准测试是 [`record_test.go`](./record_test.go)。Rust 保留了 Go 的核心两阶段协议：“`NewStmtRecord` 保存基本信息，然后必须 `Add` 才计入统计”；表名小写与空表跳过、plan digest 回退、计划超限占位、sum/max/min、Cop 最慢地址、2PC 锁内退避、RU/CPU/流量和驱逐 `Merge` 均按相同结构移植。

实现形态上的差异包括：

- Go 的可变全局 `MaxEncodedPlanSizeInBytes` 和 `GlobalStmtSummary.MaxSQLLength()` 在 Rust 中变为原子全局值，并由 `StmtSummary::SetMaxSQLLength` 显式同步。
- Go `formatSQL` 直接按字节切片；Rust 为保持有效 `String`，在多字节 UTF-8 边界向前回退。这是安全性差异，但长度后缀仍记录原始字节数。
- Go 复用 `plancodec` 的两种 discard 常量；Rust 文本占位为本地常量，二进制占位按同一 protobuf/Snappy/Base64 语义即时生成。
- Go 通过嵌入的 `StmtRUSummary` 和 `StmtNetworkTrafficSummary` 聚合；Rust 展开为具体字段并逐项更新。
- `SelectRUDetailsForStatementSummary` 是 Rust 会话接线使用的适配函数：v2 总 RU 完成时按读写拆到 RRU/WRU，未完成时保留原始值。其语义由 Rust 独立测试与会话调用点验证，不能仅从 `record.go` 推导。
- Rust 额外用 `durationNanosSerde` 固定 IA 等待时间的 JSON 纳秒表示，并拒绝负值；`record_test.rs` 验证字段名不会退化成 Serde 自动拆分出的错误形式。

## 扩展指南

新增一个执行统计字段时，至少检查四处：`StmtRecord` 与 `Default`、`Add` 的单次输入映射、`Merge` 的记录级规则、[`column.rs`](./column.rs) 或日志序列化消费者。先明确它属于 sum、max、min、集合、首次样本还是最新值；尤其不能只加字段和 `Add` 而漏掉驱逐时的 `Merge`。若字段来自 `StmtExecInfo`，还应检查会话组装位置和 `task-stmtsummary` 的定义，不应在本文件虚构缺失输入。

修改计划/SQL 截断时，要保持编码计划可解码、二进制 discard 协议兼容、UTF-8 有效和全局测试隔离。修改 JSON 字段名、时间单位或可选字段时，要同时评估已持久化日志的向后兼容。修改 RU 口径时需同步检查会话侧 `SelectRUDetailsForStatementSummary` 调用以及读写语句在 v1、v2 已完成、v2 未完成三种分支。

测试应继续放在独立文件，不嵌入 `record.rs`：Go 对照回归在 [`record_test.go`](./record_test.go)，主要 Rust 移植回归在 [`record_test.rs`](./record_test.rs)，更完整的 Cop/2PC/流量/截断/驱逐覆盖在 [`record_2_aster_unit_test.rs`](./record_2_aster_unit_test.rs)。字段若暴露为表列，还应同步 [`column_test.rs`](./column_test.rs)；若影响持久化读取，则同步 reader/logger 相关独立测试。

兼容与性能风险集中在：高频 `Add` 路径增加锁或分配；漏合并造成驱逐统计偏小；错误的 max/min 初值；跨窗口 JSON 格式漂移；首样本无限增长；以及在持有窗口/commit 锁时做昂贵工作。扩展应优先复用现有宏和一次性首样本策略，并用不同大小、空 `Option`、多用户和多记录 merge 夹具验证。

## 验证依据

- RustCodeGraph：`status`（索引 11467 文件，目标目录 Rust/Go 文件均被索引）、`files --filter pkg/util/stmtsummary/v2`、目标文件 `node --file` 全文、`record_test.rs`/`record_2_aster_unit_test.rs`/`stmtsummary.rs`/会话适配器的定点 `node --file`；`query NewStmtRecord` 和 `query SelectRUDetailsForStatementSummary` 确认 Rust/Go 精确符号。通用 callers/callees 对重名方法未给出可用边，因此调用边另以限定路径搜索确认。
- 生产源码：[`record.rs`](./record.rs)、[`stmtsummary.rs`](./stmtsummary.rs)、[`lib.rs`](./lib.rs)、[`scan_adapter_runtime.rs`](../../../session/runtime/scan_adapter_runtime.rs)。
- crate 与依赖：[`Cargo.toml`](./Cargo.toml)，确认 crate 名、lib 入口、`autotests = false`、测试装配和 base64/serde/snap/三个内部 task crate 依赖。
- Go 对照：[`record.go`](./record.go) 与 [`record_test.go`](./record_test.go)，核对构造/Add/Merge、空表、SQL 截断、IA/RU/CPU 和日志字段语义。
- Rust 独立测试：[`record_test.rs`](./record_test.rs) 覆盖 JSON 键、空表、IA、核心聚合、RU 版本；[`record_2_aster_unit_test.rs`](./record_2_aster_unit_test.rs) 覆盖完整 Cop/2PC/流量、计划与 SQL 上限以及 LRU 驱逐；[`tests/table_test.rs`](./tests/table_test.rs) 还从表读取路径使用 RU 选择函数。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收以任务指定命令检查目标文件存在且恰有 11 个固定二级标题，并人工复核只新增本说明、未修改 Rust/Go/Cargo/`plan.md`。
