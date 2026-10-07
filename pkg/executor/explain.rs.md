# `pkg/executor/explain.rs`

## 文件定位

`explain.rs` 属于 `astersql-executor` crate，由 `pkg/executor/lib.rs` 的 `pub mod explain;` 对外公开。它提供 EXPLAIN 的执行期能力：普通 EXPLAIN 把已有计划渲染为行，EXPLAIN ANALYZE 先排空被分析的子执行器再渲染运行时统计，EXPLAIN FOR CONNECTION 先校验连接可见性再渲染目标会话的计划。源文件还包含 ANALYZE 期间的可选内存诊断循环。

crate 边界见 `pkg/executor/Cargo.toml`：直接使用 `astersql-errors`、`astersql-util-chunk`和 `astersql-util-execdetails`，外键触发器边界来自同 crate 的 `crate::foreign_key`。该文件没有条件编译项，`nextgen` feature 也不改变这一模块。

当前 Rust 生产代码中未搜索到 `ExplainExec`、`ExplainForConnection` 或两个 `getAnalyzeExec*` 方法的外部调用，也未见 `ExplainExec` 实现 `adapter.rs` 使用的 `ExecExecutor` trait；它们目前是公开但尚未证明已接入 Rust SQL 主链的能力。`pkg/executor/explain_test.rs` 通过 TestKit 验证 SQL 输出，但没有直接构造本文件的类型，因而不能单独作为生产接线证据。

## 核心职责

- `ExplainExec` 管理 Open/Next/Close 生命周期，惰性产生全部计划行，并按请求 chunk 容量分页返回。
- `executeAnalyzeExec` 把 ANALYZE 子执行器排空到 EOF，无论 Next 成功、返回错误或 panic 都尝试 Close，然后在条件满足时把 RU 统计登记到目标 plan id。
- `ExplainForConnection` 把存活连接查找、属主/特权检查与计划渲染的先后顺序固化。
- `MemoryDebugModeHandler` 在 ANALYZE 旁路线程中周期采样堆与 tracker 记账，根据堆规模调整采样频率，超阈值时输出 profile 和跟踪器信息。
- trait 边界把计划、子执行器、运行时与诊断实现注入本文件，使核心流程不直接依赖会话和全局单例。

## 主要符号

- `GIB: u64`：1 GiB 的字节数，仅用于内存诊断采样档位。
- `ExplainContext(Arc<dyn Any + Send + Sync>)`：可克隆的不透明上下文；`Default` 放入空元类型，具体语义由适配器约定。
- `ExplainPlan`：计划侧协议。`Analyze`决定是否执行目标计划，`TargetPlanID` 提供 RU 归属点，`RenderResult` 刷新渲染结果，`Rows` 取出字符串行。
- `ExplainForProcess` 与 `ExplainForConnectionProvider`：前者是连接 id、属主用户和 SQL 的快照；后者抽象进程查找及指定格式的计划渲染。
- `ExplainForConnection`：公开函数，先 `GetProcess`，再检查 `privileged || process.user == requesting_user`，最后才 `RenderProcessPlan`。
- `ExplainAnalyzeExecutor`：要求实现 `Open`、`Next`、`Close`、`NewCacheChunk`、`SchemaLen` 和 `ForeignKeyTrigger`；`Send` 约束允许执行器被包含在可发送的容器中，但本文件不把它移入诊断线程。
- `ExplainRuntime`：提供 chunk 上限、可选内存诊断处理器，以及 RU 快照的构建/登记接口。
- `MemoryDebugLogLevel`、`MemoryDebugField` 与 `MemoryDebugDiagnostics`：抽象结构化日志、堆采样、tracker 遍历、时间/临时目录和 profile 写入。
- `MemoryDebugControl`：用 `AtomicBool` 表示停止，用 `Mutex<()> + Condvar` 中断定时等待；`Stop` 是公开唤醒入口，`Wait` 是内部辅助。
- `ExplainExec`：拥有 `runtime`、`explain`、可选 `analyzeExec`，并以 `executed`、`rows`、`cursor` 记录单次执行状态。公开方法包括 `Open`、`Close`、`Next`、`executeAnalyzeExec`、`generateExplainInfo`、`getAnalyzeExecToExecutedNoDelay` 和 `getAnalyzeExecWithForeignKeyTrigger`。
- `MemoryDebugModeHandler`：拥有阈值、报警比例、自动 GC 开关、诊断实现、停止控制与可复用字段缓冲；方法为 `fetchCurrentMemoryUsage`、`genInfo`、`getTrackerTreeMemUseLogs` 和 `run`。
- `updateTriggerIntervalByHeapInUse`：返回采样间隔和信息日志模数；`getHeapProfile` 在临时目录的 `record` 子目录中创建 profile，写入并 `sync_all`后返回路径。

## 执行流程

1. 上层构造 `ExplainExec` 后调用 `Open(ctx)`。只有 `ExplainPlan::Analyze()` 为真且存在 `analyzeExec` 时，Open 才会转发给子执行器；普通 EXPLAIN 直接成功。
2. 首次 `Next(ctx, request)` 发现 `rows == None`，调用 `generateExplainInfo`。ANALYZE 模式先进入 `executeAnalyzeExec`，否则直接渲染。
3. `executeAnalyzeExec` 只在 Analyze + 有子执行器 + 未执行时真正排空。它先从 runtime 取可选 `MemoryDebugModeHandler`，若存在则以 `thread::spawn` 启动；然后先将 `executed` 设为真，再创建缓存 chunk。
4. 排空循环每轮先 `Reset`，再在 `catch_unwind` 内调用子执行器 `Next`。非空 chunk 继续，空 chunk 表示 EOF；返回错误或 panic 都转成 `execution_error` 并退出。panic payload 仅对 `&str` 和 `String` 保留原文，其他类型记为 `unknown panic`。
5. 若已启动诊断线程，主线程调用 `Stop` 并 `join`；之后总是尝试子执行器 `Close`。Next/panic 与 Close 都失败时，以 `"{execution}, {close}"` 顺序合并；有错误则立即返回，不渲染计划。
6. 在子执行器已标记执行且计划给出 target id 时，调用 `BuildRURuntimeStats`；获得快照才调用 `RegisterRURuntimeStats`。这一登记位于排空和 Close 之后。
7. `generateExplainInfo` 调用 `RenderResult`，再以 `Rows` 得到全部行。`Next` 用 `MaxChunkSize` 重置请求 chunk，每次输出 `min(Capacity, 剩余行数)` 行并推进 `cursor`，穷尽后返回空 chunk。
8. `Close` 先丢弃缓存行；如果 ANALYZE 子执行器 Open 过但从未进入排空流程，则在此转发 Close。已执行时不会再次 Close。
9. DML 的无延迟路径可通过 `getAnalyzeExecToExecutedNoDelay` 取出 schema 长度为 0 的子执行器；该方法会预先设置 `executed = true`，避免后续 `executeAnalyzeExec` 重复执行。`getAnalyzeExecWithForeignKeyTrigger` 则仅在 Analyze 模式透出子执行器的外键触发器。

## 数据与状态

`ExplainExec` 的主要不变量是：`rows == None` 表示尚未生成输出；一旦生成，后续 Next 只读同一份行缓存并单调推进 `cursor`；`executed` 阻止 ANALYZE 子执行器被再次排空。`Rows()` 按值返回 `Vec<Vec<String>>`，因此计划实现决定复制/转移成本；当前测试实现会 clone 全部行。

`ExplainContext` 本身不解析数据，只通过 `Arc` 在 Open、Next、runtime 和线程创建边界间复制句柄。RU 统计不存在 `ExplainExec` 字段中，而由 `ExplainRuntime` 在排空完成后生成快照并按 `TargetPlanID` 登记。

内存诊断的可变状态包括当前采样间隔、打印模数、循环计数和 `infoField` 复用缓冲。`TrackerTreeMemory` 返回 `BTreeMap`，保证 tracker 字段输出顺序稳定。阈值判断对百分比运算使用 `saturating_div/saturating_mul`，并把负的 `minHeapInUse` 与 `100 + alarmRatio` 截到零后再转无符号数，避免转换绕回。

## 依赖与调用关系

- 模块装配：`pkg/executor/lib.rs` 以 `pub mod explain;` 公开本文件，并在 `cfg(test)` 下注册 `explain_test.rs`、`explain_unit_test.rs` 和 `explainfor_test.rs`。
- 直接下游：`ExplainExec::Next` 调用 `generateExplainInfo`、chunk 的 `GrowAndReset/Capacity/AppendString`；`generateExplainInfo` 调用 `executeAnalyzeExec` 和 `ExplainPlan::RenderResult/Rows`；`executeAnalyzeExec` 调用子执行器 `Next/Close/NewCacheChunk`、runtime 的诊断与 RU 接口，以及 `thread::spawn`。
- 连接解释下游：`ExplainForConnection` 只依赖 provider 的 `GetProcess` 和 `RenderProcessPlan`，因而会话管理器与权限系统必须在 provider 实现中接入。
- 诊断下游：`MemoryDebugModeHandler` 的所有外部效果均通过 `MemoryDebugDiagnostics`；`getHeapProfile` 直接使用 `std::fs::File`，不创建父目录。
- 上游事实：RustCodeGraph 将 `Open/Next/Close` 等通用名称与 `pkg/executor/adapter.rs` 的执行器主链关联；但全库 Rust 精确搜索只在本文件和直接测试中找到 `ExplainExec`/`ExplainForConnection`，因此不将 `adapter.rs::openExecutor/next/handleNoDelay/prepareFKCascadeContext` 记为已证明的 Rust 调用者。它们是 Go 设计中对应能力应接入的主链位置。

## 错误处理与边界

`ExplainForConnection` 对不存在的 id 返回 `Unknown thread id: <id>`；非特权请求且用户不匹配时返回 access-denied 错误，并且绝不调用渲染器。本文件使用通用 `errors::New(String)`，没有构造 Go 版本的类型化 planner error，上层若要保持 MySQL/TiDB 错误码还需适配。

ANALYZE 流程将 panic 转为普通错误，仍保证 Close。Next 错误优先，Close 错误追加在后；任一错误都会阻止 `RenderResult`。诊断线程的 `join` 结果被忽略，因此诊断线程自身的 panic 不会直接变为 SQL 错误；但 `MemoryDebugControl::Wait` 遇到 poisoned mutex/condvar 会 `expect` panic。

`getHeapProfile` 不负责创建 `<temp>/record` 目录，目录不存在、文件名非法、写入或同步失败都转为 `SharedError`。在 `run` 中，生成周期内字段/profile 失败会结束循环并记录错误，不向 `ExplainExec` 返回。

## 并发与资源生命周期

`ExplainExec` 自身依赖 `&mut self` 串行推进，没有支持并发 Next 的内部同步。唯一显式后台任务是 `executeAnalyzeExec` 中的内存诊断线程：handler 按值移入新线程，主线程保留其 `Arc<MemoryDebugControl>`；子执行器排空后先发布 Release 停止标记并通知 condvar，再 join，因此正常路径不会让诊断线程泄漏到语句之后。`Wait` 以 Acquire 读取标记，并用 `wait_timeout_while` 同时处理超时与提前唤醒。

子执行器的资源规则是：Open 后若从未调用 Next，外层 `Close` 负责关闭；一旦进入 `executeAnalyzeExec`，它在所有 Next 结果后负责唯一次 Close，并用 `executed` 防止外层重复关闭。`getAnalyzeExecToExecutedNoDelay` 提前标记已执行后把可变借用交给调用方，调用方必须承担实际执行和关闭责任；当前 Rust 生产调用方未找到，这项生命周期合同尚未由接线测试证明。

## 与 Go 版本的对应关系

Rust `ExplainExec::{Open,Close,Next,executeAnalyzeExec,generateExplainInfo,getAnalyzeExecToExecutedNoDelay,getAnalyzeExecWithForeignKeyTrigger}` 对应 `pkg/executor/explain.go` 的同名类型/方法。共同语义包括：仅 Analyze 转发 Open；首次 Next 惰性生成行并按 chunk 分页；排空直到空 chunk；执行后渲染；Next 和 Close 错误合并；空 schema DML 可走 no-delay 路径；外键触发器可向上层透出。`pkg/executor/explain_unit_test.rs` 还专门固定了 Go 测试中的 Next panic 后仍 Close 契约。

两者的重要差异是：

- Go `ExplainExec` 内嵌 `BaseExecutor` 并持有具体 `core.Explain`/`exec.Executor`；Rust 通过 `ExplainPlan`、`ExplainAnalyzeExecutor`、`ExplainRuntime` 和 `ExplainContext` 抽象这些能力，目前未发现完整生产适配器。
- Go RU 路径区分 RU 版本，并在 `format = "ru"` 时展平物理计划、合并写入和 operator 统计；Rust 本文件只负责在 Close 后调用 runtime 构建/登记单个 `RURuntimeStats`，完整计算和版本策略必须由 runtime 实现承担。
- Go 用 context cancel + `WaitGroup` 管理诊断 goroutine；Rust 用 atomic + condvar + thread join。Go 直接读全局配置、runtime heap、memory tracker 和 logger，Rust 收敛在 `MemoryDebugDiagnostics` trait。
- Go 的 `getHeapProfile` 使用 pprof 并显式 Close 文件；Rust 由 diagnostics 写入后 `sync_all`，再依赖 RAII 关闭。两者都假定 `record` 父目录已准备。
- Rust 源文件额外定义 `ExplainForConnection` 的轻量 provider 边界；Go 的完整行为主要由 planner/session/executor 接线体现，`pkg/executor/explainfor_test.go` 覆盖真实进程、权限、live runtime stats 和 plan cache，范围远大于 Rust provider 单元测试。

## 扩展指南

- 新增输出格式时，优先在 `ExplainPlan::RenderResult` 的实现中扩展，不要在 `ExplainExec::Next` 中插入格式分支；Next 应继续只处理惰性生成和 chunk 分页。同步扩展 `pkg/executor/explain_test.rs` 的 SQL/编码边界用例。
- 接入新的 ANALYZE 执行器时，完整实现 `ExplainAnalyzeExecutor` 六个方法，并保持 Open→多次 Next→唯一 Close 、空 chunk 表示 EOF 的约定。在 `pkg/executor/explain_unit_test.rs` 增加成功、Next 错误、panic、Close 错误、未 Next 直接 Close 和重复 Next 用例，不要把测试嵌入生产文件。
- 完善 Rust 主链接线时，需在构建器/执行器适配层证明 `ExplainExec` 与 `ExecExecutor` 的 Open/Next/Close、no-delay DML 与外键触发器语义，并为这条生产调用链增加独立测试。不应仅依赖同名方法或 TestKit 输出推断已接线。
- 扩展 RU 时，保持“子执行器 Close 完成后再快照”的时序，并在 runtime 实现和独立测试中明确 pending 计数排空、RU 版本、重复渲染与 stale stats 清理策略。Go `pkg/executor/explain_unit_test.go` 的 RU 用例是对照依据，不代表 Rust 已自动获得这些行为。
- 扩展 EXPLAIN FOR CONNECTION 时，在 `ExplainForConnectionProvider` 实现中对接存活进程快照和类型化权限错误，保持“先查找、再鉴权、后渲染”。在 `pkg/executor/explainfor_test.rs` 同步覆盖未知连接、非属主、特权用户、空计划与缓存计划。
- 修改内存诊断时，保持停止可唤醒、线程可 join、超阈值运算不溢出，并在独立的 `*_test.rs` 中用伪 diagnostics 验证 30/40 GiB 边界、profile 失败、负配置和 Stop 竞态。写 profile 的生产适配器还应明确由谁创建 `record` 目录和清理旧文件。

## 验证依据

- Rust 源码：`pkg/executor/explain.rs` 全文 581 行，通过 RustCodeGraph `node --file` 分段阅读；主要符号用 `query ExplainExec`、`query executeAnalyzeExec`、`query ExplainForConnection`、`query MemoryDebugModeHandler`、`query getAnalyzeExecToExecutedNoDelay` 和 `query getAnalyzeExecWithForeignKeyTrigger` 核对。
- 调用图：运行 RustCodeGraph `explore "pkg/executor/explain.rs ExplainExec ExplainAnalyzeExec execution flow callers callees"` 和针对 `adapter.rs` 的 explore；图确认本文件内 `Next → generateExplainInfo → executeAnalyzeExec`、`executeAnalyzeExec → analyze Next/Close` 等边，但精确 `callers/callees` 命令未返回详细记录。随后用全库 Rust 精确搜索核对，未发现测试之外的本类型调用或 trait 接线。
- crate/模块：`pkg/executor/Cargo.toml` 确认 `astersql-executor` 与直接 crate 依赖；`pkg/executor/lib.rs` 确认公开模块和三份独立测试的注册。目标包下不存在 `doc.go`。
- Rust 测试：`pkg/executor/explain_unit_test.rs` 证明 Open/Next/Close 次数、chunk 分页、单次渲染、错误顺序与 panic 后 Close；`pkg/executor/explainfor_test.rs` 证明属主/特权/未知连接且拒绝时不渲染；`pkg/executor/explain_test.rs` 证明统计估算输出与 TiDB_JSON 嵌套/转义，但它对本文件没有直接类型引用。
- Go 对照：`pkg/executor/explain.go` 核对主生命周期、RU 注册、no-delay DML、外键与内存诊断；`pkg/executor/explain_unit_test.go` 核对 panic/Close 及 RU 边界；`pkg/executor/explainfor_test.go` 作为完整 EXPLAIN FOR CONNECTION 会话行为的对照。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `test -f` + `rg -c` 命令检查固定章节，并人工检查只新增本文档且未修改 `plan.md`。
