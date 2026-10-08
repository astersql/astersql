# `pkg/util/sli/sli.rs`

## 文件定位

`sli.rs` 是 `astersql-util-sli` crate 中负责“单笔事务写入吞吐 SLI”状态机的实现文件。crate 入口 `pkg/util/sli/lib.rs` 以 `#[path = "sli.rs"] mod sli` 装载它，并只对外再导出 `TxnWriteThroughputSLI`。`pkg/util/sli/Cargo.toml` 指定 `lib.rs` 为库入口，声明唯一直接第三方依赖 `prometheus = "0.14"`，并通过 `package.metadata.porting.go-package = "pkg/util/sli"` 标出 Go 来源。

该类型目前作为 Rust 会话状态的一部分存在：`pkg/session/runtime/session.rs` 的 `SessionState::txn_write_throughput_sli` 持有一个实例，并在 `SessionState::default` 中初始化。生产 Rust 接线在 `pkg/session/runtime/scan_adapter_runtime.rs::OnFinishStatement` 中从语句执行详情提取 processed keys、write size 和 write keys，再调用本文件的累计方法。仓库搜索未发现生产 Rust 路径调用本类型的 `FinishExecuteStmt` 或 `SetInvalid`；因此“提交时上报并清零”和显式失效标记虽然已实现并由独立测试验证，但当前 Rust 应用主链只完成了部分接线，不能表述为已经与 Go 端到端等价。

## 核心职责

本文件围绕 `TxnWriteThroughputSLI` 完成四件事：

1. 在事务生命周期内累计受影响行数、写入字节数、读取/写入 key 数和有效写语句耗时。
2. 用 `IsInvalid` 排除不适合统计的事务，用 `IsSmallTxn` 将事务分为小事务和普通事务。
3. 在 `FinishExecuteStmt(..., inTxn = false)` 表示事务结束时，通过 crate 内 `metrics` 模块上报小事务耗时或事务写入吞吐，然后在正常路径执行 `Reset`。
4. 为迁移回归提供稳定的状态字符串、显式重置以及 `CheckTxnWriteThroughput` failpoint 保留状态能力。

它不负责采集底层执行详情、判断 SQL 语句类别、维护 Prometheus 注册表或决定何时提交；这些职责分别位于调用方和 `pkg/util/sli/lib.rs`。本文件也不返回业务错误，属于尽力而为的观测状态机。

## 主要符号

- `pub struct TxnWriteThroughputSLI`：唯一公开类型，`#[derive(Default)]`。六个字段均私有：`invalid: bool`、`affectRow: u64`、`writeSize/readKeys/writeKeys: isize`、`writeTime: Duration`。字段命名和顺序刻意贴近 Go 结构体。
- `pub fn FinishExecuteStmt(&mut self, cost: Duration, affectRow: u64, inTxn: bool)`：语句完成入口。仅当 `affectRow > 0` 时把语句耗时和影响行数计入；当 `inTxn == false` 时视为事务结束，必要时补计零影响行的 commit 耗时，随后上报、检查 failpoint、重置。
- `pub fn AddReadKeys(&mut self, readKeys: i64)`：累计写语句执行期间读取的 key 数，内部转换为 `isize`。
- `pub fn AddTxnWriteSize(&mut self, size: isize, keys: isize)`：累计事务写入字节数和写入 key 数。
- `fn reportMetric(&self)`：私有上报分派。无效事务不观察任何指标；小事务观察 `metrics::SmallTxnWriteDuration`，其他有效事务观察 `metrics::TxnWriteThroughput`。
- `pub fn SetInvalid(&mut self)`：永久将当前累计周期标记为无效，直到 `Reset`。
- `pub fn IsInvalid(&self) -> bool`：当显式失效、`readKeys > writeKeys`、写入大小为零或写入耗时为零时返回真。
- `const smallTxnAffectRow: u64 = 20` 与 `const smallTxnSize: isize = 1 * 1024 * 1024`：小事务的包含式上界；必须同时满足行数不超过 20、写入不超过 1 MiB。
- `pub fn IsSmallTxn(&self) -> bool`：只判断大小边界，不隐含有效性判断；因此默认空状态也会被判为“小”，调用者在上报前必须先执行 `IsInvalid`。
- `pub fn Reset(&mut self)`：将六个字段恢复默认值，开始下一笔事务的累计周期。
- `pub fn String(&self) -> String`：生成与 Go `fmt.Sprintf` 字段顺序一致的诊断文本。
- `fn format_go_duration(Duration) -> String`：私有格式化器，按纳秒、微秒、毫秒或 `h/m/s` 组合输出并裁掉小数尾零，用于贴近 Go `time.Duration.String()`。

本文件没有 trait、enum、条件编译项，也没有内嵌测试模块；测试由 `lib.rs` 在 `#[cfg(test)]` 下单独装载 `migration_aster_unit_test.rs`。

## 执行流程

正常累计和结束流程如下：

1. 会话初始化时创建默认实例，所有计数为零且 `invalid == false`（`pkg/session/runtime/session.rs::SessionState::default`）。
2. 语句收尾阶段，`pkg/session/runtime/scan_adapter_runtime.rs::OnFinishStatement` 从 `StmtCtx.GetExecDetails()` 读取扫描和提交详情。只有 `affected_rows > 0 && processed_keys > 0` 才调用 `AddReadKeys`；只有 `write_size > 0` 才调用 `AddTxnWriteSize`。
3. 设计上的语句完成入口是 `FinishExecuteStmt`。有影响行的写语句累计 `cost` 和 `affectRow`；事务内的零影响行语句不计时，这与 Go 回归中 SELECT 不增加写耗时的预期一致。
4. `inTxn == false` 表示当前语句结束了最后一笔事务。若该语句自身影响行数为零（典型为 commit），它的 `cost` 仍加入 `writeTime`。
5. `reportMetric` 先调用 `IsInvalid`。有效小事务上报累计秒数；有效大事务上报 `writeSize / writeTime.as_secs_f64()`，单位为字节/秒。
6. 上报之后检查 `failpoint::inject("CheckTxnWriteThroughput")`。开启时提前返回以便测试读取累计状态；关闭时调用 `Reset`，避免下一笔事务继承旧数据。

需要特别区分“文件实现流程”和“当前生产接线”：第 3 至 6 步已由 `pkg/util/sli/migration_aster_unit_test.rs` 直接验证，但当前生产 Rust 搜索只确认第 1、2 步已接入；没有证据表明会话主链会在提交点调用本类型的 `FinishExecuteStmt`。

## 数据与状态

`TxnWriteThroughputSLI` 是一段可变、事务级累计状态，不包含事务 ID；正确性依赖调用者让一个实例只追踪当前会话的一笔事务，并在事务边界触发完成/重置。

- `affectRow` 和 `writeTime` 只由 `FinishExecuteStmt` 在受影响行大于零时共同增长；commit 的零影响行耗时是唯一显式例外。
- `writeSize` 与 `writeKeys` 必须由同一提交详情共同累计；`readKeys` 仅用于判断“写 SQL 读取 key 多于写 key”的失效条件。
- `invalid` 是粘滞位：一旦 `SetInvalid`，本周期内不会自行恢复，只有 `Reset` 清除。
- `IsSmallTxn` 的两个阈值均为闭区间。测试证明 20 行和 1 MiB 仍为小事务，超过任一阈值即为大事务。
- `Default` 与 `Reset` 的结果等价：false、五个数值/时长字段归零。默认状态因写入大小和耗时为零而 `IsInvalid == true`。
- `String` 只用于观察状态，不暴露字段修改能力；`format_go_duration` 对零值输出 `0s`，并支持 ns、µs、ms、s、m、h 的组合格式。

数值类型与 Go 并非完全同构：Go 的 `int` 映射为 Rust `isize`，Go 的 `int64` read keys 在 `AddReadKeys` 中以 `as isize` 转换。当前生产调用方先用 `.max(0)` 消除负值，并只在正数时累计，但本类型公开方法自身不拒绝负的 size/keys，也不做溢出检查。

## 依赖与调用关系

上游直接证据：

- `pkg/util/sli/lib.rs`：模块装载、公开再导出，并提供本文件调用的 `failpoint`、`metrics`。
- `pkg/session/runtime/session.rs`：会话级所有权和默认初始化。
- `pkg/session/runtime/scan_adapter_runtime.rs::OnFinishStatement`：当前生产 Rust 的累计调用者，调用 `AddReadKeys` 与 `AddTxnWriteSize`。
- `pkg/util/sli/migration_aster_unit_test.rs`：直接覆盖全部关键公开 API 和两类指标分派。
- `pkg/util/mock/context.rs::GetTxnWriteThroughputSLI`：返回默认实例的 mock 边界，但没有形成真实事务生命周期。

下游直接依赖：

- `std::time::Duration`：累计耗时、零值判断、秒值转换和纳秒级格式化。
- `crate::metrics::{SmallTxnWriteDuration, TxnWriteThroughput}`：分别接收秒数与字节/秒；底层包装位于 `lib.rs`，使用 `prometheus::Histogram`。
- `crate::failpoint::inject`：只识别 `CheckTxnWriteThroughput`，控制上报后的重置行为。

RustCodeGraph 的文件节点显示 `sli.rs` 被多个文件使用，并准确定位了 `TxnWriteThroughputSLI`、各方法和 Go 同名符号；但对方法级 callers/callees 的查询没有可靠地区分同名符号，产生了跨仓库的同名候选。因此调用边最终以索引节点结合精确 `rg` 结果核验，未把噪声边当作事实。

Go 的完整调用链比当前 Rust 接线更完整：`pkg/executor/adapter.go::ExecStmt.FinishExecuteStmt` 累计提交/扫描详情，`pkg/server/conn.go` 在请求结束处调用 SLI 的 `FinishExecuteStmt`，`pkg/executor/insert_common.go` 对 `insert|replace ... select` 调用 `SetInvalid`，`pkg/session/session.go::GetTxnWriteThroughputSLI` 提供会话访问器。

## 错误处理与边界

本文件没有 `Result`、错误返回或 panic 分支；不合法或不完整的观测通过 `IsInvalid` 静默跳过指标。关键边界为：

- `readKeys > writeKeys` 才失效，相等仍有效，除非其他条件成立。
- `writeSize == 0` 或 `writeTime == Duration::ZERO` 失效，保护吞吐公式不以零时长作除数。
- `IsInvalid` 没有拒绝负的 `writeSize/readKeys/writeKeys`；生产调用方当前过滤负执行详情，但其他调用者必须维持非负不变量。
- 普通零影响行事务内语句不计入写耗时；零影响行且 `inTxn == false` 的结束语句会计入耗时。
- 小事务阈值是 `<= 20` 行且 `<= 1 MiB`，不是严格小于。
- failpoint 在指标上报之后才阻止重置，因此重复对同一保留状态再次结束可能重复上报；它只应在串行化测试中使用。
- `Duration` 不表示负时长；累计运算和整数累计在 debug/release 溢出行为上依赖 Rust 构建配置，本文件没有饱和或 checked 运算。
- `format_go_duration` 采用整数截断生成最多 3/6/9 位小数，目标是测试兼容格式，不应当作为通用国际化时间格式 API。

## 并发与资源生命周期

`TxnWriteThroughputSLI` 本身没有锁、原子、通道、任务或异步资源；所有修改都需要 `&mut self`，Rust 借用规则保证同一实例在安全代码中不会并发可变访问。当前实例嵌在会话 `SessionState` 中，并通过会话内部的 `RefCell` 可变借用更新，因此生命周期与会话一致，累计周期与事务边界一致。

全局共享资源位于 `lib.rs` 而非本文件：failpoint 使用 `AtomicBool`，指标使用惰性初始化的 Prometheus histogram 和原子基线。独立 Rust 测试通过 `test_guard()` 的全局 `Mutex` 串行化会改动 failpoint/指标基线的用例，避免并发测试互相污染。生产扩展若引入跨线程共享实例，不能仅依赖当前结构；应由拥有者增加同步或改为消息传递，并保持每会话/每事务隔离。

正常资源生命周期是“会话创建默认状态 → 多个语句累计 → 事务结束上报 → Reset → 下一事务复用”。开启测试 failpoint 时生命周期暂时停在“已上报但未 Reset”，测试读取后必须关闭 failpoint并显式重置或丢弃实例。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/sli/sli.go`。Rust 保留了 Go 的结构字段、方法名、判断顺序、两个阈值、指标公式、commit 补时逻辑以及 failpoint 位于上报之后/重置之前的顺序。`migration_aster_unit_test.rs` 还复现了 Go `pkg/executor/executor_failpoint_test.go::TestTxnWriteThroughputSLI` 的关键样例：2 行/58 字节小事务、20 行与 1 MiB 包含式边界、21 行大事务、读键多于写键、显式失效、commit 后重置和 failpoint 保留状态。

已知差异和迁移状态：

- Go `int`/`int64` 对应 Rust `isize`/`i64 -> isize`，需要调用方保证范围与非负性。
- Go 使用 `time.Duration.String()`；Rust 以私有 `format_go_duration` 手工复现本任务覆盖的格式。
- Go 直接依赖全局 `pkg/metrics` 与 `github.com/pingcap/failpoint`；Rust crate 入口目前提供局部 metrics/failpoint 实现，其中注释明确将 metrics 称为供单测断言的“指标桩”。因此不能据此声称已注册到应用全局指标体系。
- Go `insert|replace ... select` 执行路径会调用 `SetInvalid`，Rust 生产代码搜索未发现对应调用。
- Go 请求/执行收尾会调用 SLI 的 `FinishExecuteStmt`；Rust 当前只找到执行详情累计接线，未找到事务结束上报接线。

所以本文件的局部算法与 Go 高度对齐，但端到端迁移尚不完整；新增说明或功能时必须分别陈述“算法已实现”和“生产主链已接线”。

## 扩展指南

扩展时应按职责选择接入点：

- 调整有效性规则：修改 `IsInvalid`，同步 `migration_aster_unit_test.rs::invalid_conditions_and_small_transaction_boundaries_match_go` 和无指标观测测试，并核对 Go `sli.go::IsInvalid`。
- 调整小事务定义：修改两个常量和 `IsSmallTxn`，补齐恰好在阈值、阈值上下各一单位的独立测试；若 Go 未同步变化，应明确兼容性偏差。
- 增加累计维度：向结构体、`Reset`、`String` 和相关 Add 方法成套加入字段，确保默认值、重置、诊断输出及 Go 对照同时更新。
- 调整指标：修改 `reportMetric` 及 `lib.rs::metrics`，验证无效、小事务、大事务各自只观察预期 histogram；确认是否需要从测试桩迁移到应用统一指标注册。
- 完成生产接线：在真实 Rust 事务结束点调用本类型的 `FinishExecuteStmt`，并在 `insert|replace ... select` 对应执行路径调用 `SetInvalid`。这属于行为修改，必须另立实现任务和回归测试，不能只靠本文档或现有直接单测宣称完成。
- 调整时长格式：扩展 `format_go_duration` 的独立测试，覆盖单位切换、复合小时/分钟和尾零裁剪；不要把测试辅助格式化器无审查地推广为通用 API。

任何 Rust 逻辑修改都应保持测试位于独立的 `migration_aster_unit_test.rs`（或新的独立 `*_test.rs`）中，不把测试嵌回 `sli.rs`。性能风险主要来自在每条语句热路径增加额外工作或锁；兼容风险主要来自阈值、耗时口径、重置时机和 Go 指标名称/单位漂移；正确性风险主要来自事务间状态泄漏、重复上报以及无效事务被错误纳入。

## 验证依据

本说明基于以下可复核证据：

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/util/sli` 定位 `lib.rs`、`sli.rs`、`migration_aster_unit_test.rs` 和 Go 对照；`node --file pkg/util/sli/sli.rs --offset 1 --limit 500` 读取了 224 行完整实现；`query` 定位 `TxnWriteThroughputSLI`、`FinishExecuteStmt`、`AddReadKeys`、`AddTxnWriteSize` 和 `format_go_duration` 的 Rust/Go 同名节点。方法级 callers/callees 输出存在同名解析噪声，故未作为单独结论依据。
- 源与 crate：`pkg/util/sli/sli.rs`、`pkg/util/sli/lib.rs`、`pkg/util/sli/Cargo.toml`。
- Rust 调用和状态：`pkg/session/runtime/session.rs`、`pkg/session/runtime/scan_adapter_runtime.rs`、`pkg/util/mock/context.rs`，以及精确符号搜索结果。
- Rust 独立测试：`pkg/util/sli/migration_aster_unit_test.rs`，覆盖累计、边界、两类指标、无效跳过、commit reset 和 failpoint。
- Go 对照与主链：`pkg/util/sli/sli.go`、`pkg/executor/adapter.go`、`pkg/executor/insert_common.go`、`pkg/server/conn.go`、`pkg/session/session.go`、`pkg/executor/executor_failpoint_test.go::TestTxnWriteThroughputSLI`。
- 人工事实复核：确认本文件为何存在（事务写 SLI 状态机）、如何运行（累计—判定—上报—重置）、如何安全扩展（保持 Go 语义、事务隔离和独立测试），并明确当前生产 Rust 只接入累计而未找到结束上报/显式失效调用。

本任务是纯文档分析，按计划不运行 Cargo。交付结构验证要求本文档存在，且上述固定二级标题恰好为 11 个。
