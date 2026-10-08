# `pkg/util/servermemorylimit/servermemorylimit.rs`

## 文件定位

本文件是 `astersql-util-servermemorylimit` crate 的业务实现文件；对应源码为 [`servermemorylimit.rs`](./servermemorylimit.rs)。同目录的 `lib.rs` 以 `pub mod servermemorylimit; pub use servermemorylimit::*;` 对外暴露。crate 边界和直接依赖记录在 `pkg/util/servermemorylimit/Cargo.toml`：它依赖内存追踪、会话管理、SQL killer、日志、MySQL 类型以及 Datum/时间转换 crate。

它实现的是服务端内存上限控制器及最近 50 次会话 kill 历史，而不是内存统计器或 tracker 本身；全局限额、top-1 tracker、内存采样和全局仲裁均来自 `pkg/util/memory`。当前 Rust 完整实现已被 executor 的内存控制测试直接启动，但生产启动路径尚未接到这个 crate：`cmd/tidb-server/main.rs` 调用的 `dom.ServerMemoryLimitHandle()` 当前来自 `cmd/tidb-server/stubs.rs` 中只记录事件的 `domain::Handle`。同样，Rust `pkg/executor/infoschema_reader.rs` 的内存历史读取走通用 `DataRequest::MemoryUsage { history: true, ... }`，没有直接调用本文件的 `GlobalMemoryOpsHistoryManager.GetRows()`。因此应把“文件内能力已实现”和“生产主链已接线”分开判断。

## 核心职责

- `Handle::Run` 每 100 ms 唤醒一次，先驱动 `memory::HandleGlobalMemArbitratorRuntime()`，再用 `memory::ServerMemoryLimit` 调用 `killSessIfNeeded`；退出通道收到值或断开时结束。
- `killSessIfNeeded` 维护跨 tick 的 kill 状态：在实例堆占用超过限制且全局仲裁未接管时，选择 `memory::MemUsageTop1Tracker` 指向的会话；会话占用达到最小阈值后发送 `sqlkiller::ServerMemoryExceeded`。
- `MemoryMaxUsed`、`SessionKillLast`、`SessionKillTotal` 和 `IsKilling` 提供进程级观测状态；成功发起 kill 时同步更新指标并写入历史。
- `memoryOpsHistoryManager` 用固定 50 槽环形缓冲保存 kill 时的限额、当前堆占用和进程快照，`GetRows` 将其转换为 INFORMATION_SCHEMA 所需的 Datum 列序。
- 对已发信号但仍未结束的同一 SQL，逻辑每 5 秒告警一次；达到 60 秒后调用 tracker 上 killer 的 `FinishResultSet()` 强制结束结果集。

## 主要符号

- `NewServerMemoryLimitHandle(Receiver<()>) -> Box<Handle>`：创建控制器；构造后必须先调用 `Handle::SetSessionManager`，否则 `Run` 在读取 `sm` 时 panic。
- `Handle`：持有退出接收端和 `RwLock<Option<Arc<dyn sessmgr::Manager>>>`。`SetSessionManager` 注入会话查询能力，`Run` 是长生命周期轮询入口。
- `ProcessInfoProvider`：包内最小抽象，仅含 `get_process_info(id)`；对任意 `sessmgr::Manager` 有 blanket impl，使生产接口与可控单测替身复用同一核心逻辑。
- `sessionToBeKilled`：保存 `isKilling`、SQL 开始时间、连接 ID、tracker 裸指针以及 kill/日志时间。`reset` 将状态恢复为空闲值。
- `killSessIfNeeded`：核心状态机；参数 `bt` 是本轮内存上限，可能被 `issue42662_2` failpoint 改为 1。
- `MemoryMaxUsed`、`SessionKillLast`、`SessionKillTotal`、`IsKilling`：无锁原子观测量；`GlobalMemoryOpsHistoryManager` 是用 `LazyLock<Mutex<_>>` 包装的进程级历史。
- `memoryOpsHistory`：单条内部记录；`memoryOpsHistoryManager::{init, recordOne, GetRows}` 分别负责重置、环形写入和从旧到新导出。
- `go_zero_time`、`since`、`truncated`、`process_list_datum`：分别处理 Go 零时间近似值、容错时间差、Unicode 标量截断和 `ProcessListValue` 到 `Datum` 的转换。
- `init`：显式重置全局历史。Rust 不会像 Go 包 `init()` 那样自动调用它，但 `memoryOpsHistoryManager::default()` 已创建 50 个空槽，因此首次使用无需额外初始化。

## 执行流程

1. 调用方创建退出通道，以 `NewServerMemoryLimitHandle` 构造句柄，注入 `sessmgr::Manager`，再把 `Run` 放入后台线程。两个 executor 测试入口分别位于 `pkg/executor/test/unstabletest/memory_test.rs` 和 `pkg/executor/test/analyzetest/memorycontrol/memory_control_test.rs`。
2. `Run` 克隆一次会话管理器，创建一个跨轮次复用的 `sessionToBeKilled`，随后以 `recv_timeout(100 ms)` 同时承担 ticker 和退出检查。每次超时先运行全局内存仲裁器，再进入本文件的控制逻辑。
3. 若上一轮已发送 kill，`killSessIfNeeded` 查询同一连接。连接仍执行相同开始时间的 SQL 时，未满 5 秒直接等待；满 5 秒记录诊断；kill 已持续至少 60 秒则调用 `FinishResultSet`，否则继续等待。连接消失或 SQL 开始时间改变也视为上一轮已结束。
4. 上一轮结束后，状态与 `IsKilling` 被复位，代码尝试清理全局 top-1 tracker，调用 `runtime_gc`，并记录成功日志。需注意当前代码和 Go 对照都先 `reset()` 再用已经清空的 `sessionTracker` 做 CAS；按当前实现，此 CAS 比较的是空指针而非原 tracker。Rust 的 `runtime_gc()` 也是明确的空实现，因为没有 Go `runtime.GC` 的直接等价物。
5. 若本轮限额为 0，立即返回。否则先应用 failpoint，再采样 `ReadMemStats` 并更新峰值。若 `UsingGlobalMemArbitration()` 为真，本控制器停止选会话，由仲裁器接管。
6. 当 `heap_inuse > bt` 时读取 top-1 tracker。若会话占用小于 `ServerMemoryLimitSessMinSize`，CAS 清空 top-1 且不 kill；否则查询 `ProcessInfo`，发送 `ServerMemoryExceeded`，更新状态/指标，并通过 `recordOne` 写入历史。
7. 没有合格 tracker 时，以 5 秒间隔记录“未找到超过最小会话阈值的消费者”。找到并 kill 后，下一轮回到步骤 3 观察其退出。
8. `recordOne` 把 `ProcessInfo::ToRow(UTC)` 的快照写进当前 offset，随后模 50 前进。`GetRows` 从 offset 开始环绕扫描，跳过空时间槽，因而按仍保留记录的旧到新顺序输出 12 列。

## 数据与状态

控制状态分为三层。第一层是 `sessionToBeKilled` 的单 worker 局部状态，仅由 `Run` 所在线程可变借用；SQL 开始时间与连接 ID 联合用于防止连接复用或新语句被误判为旧 kill。第二层是进程级原子指标和 `memory::MemUsageTop1Tracker`；指标服务观测，裸指针槽服务 top-1 选择。第三层是全局历史互斥量，串行化记录与读取。

历史每条保存 `killTime`、`memoryLimit`、`memoryCurrent` 和 `ProcessInfo::ToRow` 的值快照。导出列依次为：时间、`SessionKill`、内存限制、当前内存、进程 ID、内存、磁盘、客户端、数据库、用户、SQL digest、SQL 文本。索引 `0/9/13/2/3/1/8/7` 与 `pkg/session/sessmgr/processinfo.rs` 的 `ToRow` 布局耦合，调整进程行结构时必须同步检查。

`recordOne` 构造了 SQL 文本的 256 字符截断 Datum，但随后写入历史的是原 `op`，没有把局部 `sqlInfo` 写回；这是与 Go 值拷贝写法保持一致的当前事实，不能把历史 SQL 已截断描述为已验证行为。日志 SQL 则由 `truncated(..., 100)` 按 Unicode 标量可靠截断。

## 依赖与调用关系

上游方面，RustCodeGraph 将目标识别为 26 个符号的文件；精确源码检索确认真实控制器由 executor 的两个内存测试构造并在独立线程运行。`pkg/domain/Cargo.toml` 声明了此 crate 依赖，但 `pkg/domain/**/*.rs` 尚无符号使用；Rust server 主流程当前调用 `cmd/tidb-server/stubs.rs` 的同名门面。因此目前没有证据证明本文件的 `Handle::Run` 已进入 Rust 生产启动链。

下游方面，`Handle::Run` 调用内存仲裁、读取全局限额并进入 `killSessIfNeeded`；后者依赖 `memory::{ReadMemStats, MemUsageTop1Tracker, ServerMemoryLimitSessMinSize, UsingGlobalMemArbitration}`、`sessmgr::Manager::GetProcessInfo`、tracker 的 `SQLKiller`、后台日志和原子工具。历史路径依赖 `ProcessInfo::ToRow`、`chrono/chrono-tz` 以及 parser/types crate 的 MySQL 时间和 Datum 构造函数。

Go 生产消费者还包括 `pkg/executor/infoschema_reader.go`：它读取四个观测指标和 `GlobalMemoryOpsHistoryManager.GetRows()`，形成 `MEMORY_USAGE` 与 `MEMORY_USAGE_OPS_HISTORY`。Rust `infoschema_reader.rs` 目前只把请求转给通用数据源，不能据此认定它消费了本文件的 Rust 全局量。

## 错误处理与边界

本文件没有返回 `Result` 的恢复路径。会话管理器未注入、`RwLock`/`Mutex` 中毒、活跃 `ProcessInfo` 缺少 `MemTracker`、tracker 缺少 `Killer`，以及不满足裸指针有效性假设时，都可能 panic 或触发未定义行为。调用方必须保证：top-1 槽只保存仍存活的会话根 tracker；只要对应 `ProcessInfo` 仍描述该 SQL，tracker 及其 killer 就继续有效。

关闭语义包括：限额为 0 时不采样、不更新峰值也不 kill；启用全局仲裁时仍会采样并更新峰值，但不走会话 kill；低于会话最小阈值会清空 top-1 槽；超过限制但查不到 `ProcessInfo` 时不会发送信号。时间倒退或无效时间差由 `since` 当作 0，避免错误触发超时分支。退出通道断开和收到显式退出值等价，都会终止 worker。

历史导出假设 `infos` 非空且 `ProcessInfo::ToRow` 至少包含 14 项；默认值和 `init` 均保证容量为 50，但若未来增加允许任意容量的构造器，`GetRows` 的取模和固定索引需要新的校验。当前代码不传播日志失败，也不对指标峰值更新做 compare-and-swap；单 worker 是维持“先 Load 后 Store”可接受的隐含前提。

## 并发与资源生命周期

`Handle` 的会话管理器通过 `RwLock` 注入，`Run` 启动时只克隆一次 `Arc`，之后替换 `sm` 不会影响已经运行的 worker。退出接收端归句柄所有；worker 结束时局部 `sessionToBeKilled` 随之销毁，但它不拥有 `sessionTracker` 指针指向的对象。

进程级计数和 top-1 指针使用原子操作，top-1 的 CAS 采用 `SeqCst`。历史由外层 `Mutex<memoryOpsHistoryManager>` 保护，和 Go 将 mutex 内嵌在 manager 的布局不同，但公开调用都必须先拿外层锁。测试使用额外的 `TEST_LOCK` 串行化全局指标，防止并行测试互相污染。

资源释放上，Rust tracker 的所有权在会话/`Arc` 一侧；本文件仅借用裸指针并发送信号。`FinishResultSet` 是 60 秒兜底，而 `runtime_gc` 不回收任何资源。安全扩展时不能让裸指针跨越其所有者生命周期，也不能假设发出 kill 后内存会由本文件同步释放。

## 与 Go 版本的对应关系

主要结构与 `pkg/util/servermemorylimit/servermemorylimit.go` 一一对应：相同的全局指标、100 ms tick、`sessionToBeKilled` 状态、5 秒日志节流、60 秒强制结束、最小会话阈值、top-1 kill、50 槽历史和输出列序。`pkg/util/servermemorylimit/servermemorylimit_test.go` 的环形覆盖测试也被独立移植到 `servermemorylimit_test.rs`。

重要差异如下：Go 使用 ticker 和 channel `select`，Rust 用 `Receiver::recv_timeout`；Go 的 `atomic.Value` 被 `RwLock<Option<Arc<dyn Manager>>>` 取代；Go manager 内嵌 mutex，Rust 在全局对象外包 mutex；Go `runtime.GC()` 有实际效果，Rust `runtime_gc()` 是 no-op；Go 包 `init()` 自动运行，Rust 的同名函数只是普通公开函数；Go 生产 domain 和 infoschema 直接接线，Rust 当前主链尚未直接消费本实现。

`migration_aster_unit_test.rs` 补充了 Go 原测试没有直接覆盖的 kill 路径：验证超限发送信号并记录历史、进程消失后复位、低于最小阈值清空 tracker、60 秒后调用 `FinishResultSet`。这些测试证明局部状态机行为，不等同于证明生产启动接线或真实操作系统内存条件下的完整行为。

## 扩展指南

- 修改轮询、选择或 kill 状态机时，入口是 `Handle::Run` 和 `killSessIfNeeded`；同步扩展独立的 `migration_aster_unit_test.rs`，覆盖限额为 0、全局仲裁、会话复用、日志节流和错误组件等边界。不要把测试写回生产源文件。
- 修改历史字段、容量或列序时，联合修改 `memoryOpsHistory`、`recordOne`、`GetRows`，并同步 `servermemorylimit_test.rs`、`migration_aster_unit_test.rs` 以及 Go 对照/infoschema schema 消费契约。特别检查 `ProcessInfo::ToRow` 索引和 SQL 文本截断是否真正写回。
- 把控制器接入生产 Rust 主链时，应替换 `cmd/tidb-server/stubs.rs` 的同名 handle，而不是仅增加 Cargo 依赖；还要定义退出 sender 的持有者、worker join/关停顺序以及真实 server 到 `sessmgr::Manager` 的适配。
- 若让 Rust INFORMATION_SCHEMA 直接展示这些全局量，需要明确 `DataRequest` 数据源与本 crate 的单一事实来源，避免生产进程出现两套互不一致的历史/指标。
- 修改裸指针生命周期或成功清理路径前，应专门审查 `reset` 后 CAS 的现有顺序，并增加断言 top-1 槽最终清空的回归测试；不能只依靠现有“状态已复位”断言。
- 性能方面保持轮询热路径无大范围锁：会话查询、日志字段构造、历史锁只应在越限或诊断分支发生；更高采样频率会放大内存统计成本。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件且目标路径已索引；`files --filter pkg/util/servermemorylimit` 返回 `lib.rs`、目标文件、两个 Rust 测试和 Go 对照文件；`node --file ... --offset 1/360` 读取了目标文件完整 476 行；`query serverMemoryLimit` 定位了构造器、`Run`、`killSessIfNeeded`、历史方法及 Go 对照符号。`callers` 查询在本地长时间无结果后中止，因此调用方结论改由精确源码检索验证，不宣称来自调用图。
- 已读实现与边界：`pkg/util/servermemorylimit/servermemorylimit.rs`、`lib.rs`、`Cargo.toml`，以及 `pkg/session/sessmgr/processinfo.rs` 的 `Manager`/`ProcessInfo::ToRow` 接口。
- 已读对照与测试：`pkg/util/servermemorylimit/servermemorylimit.go`、`servermemorylimit_test.go`、`servermemorylimit_test.rs`、`migration_aster_unit_test.rs`。
- 已核对接线：`cmd/tidb-server/main.rs`、`cmd/tidb-server/stubs.rs`、`pkg/domain/Cargo.toml`、`pkg/executor/infoschema_reader.rs`、`pkg/executor/infoschema_reader.go`，以及两个 executor 内存控制 Rust 测试中的真实 handle 构造点。
- 本任务按计划是纯文档分析，未运行 Cargo。结构验证应确认本文件存在且恰有 11 个规定的二级标题；人工复核重点是实现能力、测试能力与生产接线状态没有混写。
