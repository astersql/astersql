# `pkg/server/protocol_result_test_support.rs`

## 文件定位

本文件是 `astersql-server` crate 内部的测试辅助模块，不是生产请求路径的一部分。其父模块 `pkg/server/protocol_result.rs` 仅在 `cfg(test)` 下通过 `#[path = "protocol_result_test_support.rs"] mod test_support` 装入它，并将 `BoundaryProbe` 注入 `CanonicalResultSet` 与 `WorkerResults`。crate 根由 `pkg/server/Cargo.toml` 的 `[lib] path = "lib.rs"` 定义，`pkg/server/lib.rs` 再以私有 `mod protocol_result` 挂载父模块。

它位于会话 worker 持有的 canonical result set 与 MySQL 协议写出逻辑之间的测试观测点：不替代真实行、列元数据、游标或关闭逻辑，只在真实 `ResultSet` 边界记录事件、延时或返回测试错误。文件自身的 `BoundaryProbe` 和其方法均受 `#[cfg(test)]` 保护，因此普通构建中不存在这些类型、字段和锁。

## 核心职责

- `BoundaryProbe::before` 在结果集的 `Next`、`Finish`、`Close`、`FetchReturned` 边界记录调用顺序，供协议回归测试断言生命周期。
- 同一入口按“操作名 + 第几次匹配调用”精确注入延时，并可在延时后返回 `ConnError::Session` 包装的错误，用于区分结果迭代耗时和网络写出耗时。
- `WorkerResults::set_fault` 提供一次新的故障场景配置，同时清空上一场景的匹配计数和事件；`WorkerResults::events` 返回事件快照，避免测试直接接触内部共享状态。
- 探针只包围 `CanonicalResultSet` 的真实边界。行仍由 `ConcreteRecordSet::next_row` 获取，元数据仍由 `result_metadata` 产生，游标仍由 `WrapWithLazyCursor` 管理，因而测试覆盖的是实际协议路径而非替代桩路径。

## 主要符号

- `BoundaryProbe`（`pub(super)`）：父模块可见的测试状态。`fault` 是可选的 `(expected, at, delay, fail)`；`count` 只统计名称等于 `expected` 的调用；`events` 保存所有传入的操作名，而不只保存匹配项。`#[derive(Default)]` 产生“无故障、计数为零、空事件”的初态。
- `BoundaryProbe::before(&mut self, operation: &'static str) -> Result<(), sqlexec::GoError>`：先无条件追加事件，再检查故障。仅当操作名匹配时递增 `count`，仅在 `count == at` 时休眠；`fail == true` 时随后返回文本为 `injected {operation} failure` 的 session 错误。计数超过 `at` 后不会再次触发同一配置。
- `WorkerResults::set_fault(&mut self, operation, at, delay, fail)`（`pub(crate)`）：获得 `self.probe` 的互斥锁，替换故障配置，并把计数与事件同时复位。传入 `"none"` 等不会出现的名字可用于关闭实际注入并清空观测窗口。
- `WorkerResults::events(&self) -> Vec<String>`（`pub(crate)`）：锁定探针并克隆事件向量；返回值是读取时快照，后续边界事件不会反向修改已返回数据。

本文件没有模块级常量、trait、自由函数或条件 feature；所有行为都只在 Rust 测试配置中编译。

## 执行流程

1. 测试经 `SessionContext::result_fault_for_test` 向 session worker 发送 `SessionRequest::SetResultFault`；`pkg/server/runtime.rs` 的 worker 分支调用 `WorkerResults::set_fault`，等待应答后测试才继续。
2. `WorkerResults::register` 在 `pkg/server/protocol_result.rs` 中创建 `CanonicalResultSet`，并克隆同一个 `Arc<Mutex<BoundaryProbe>>` 放入结果集；因此配置发生在 worker 侧，随后注册或已注册的结果集都观察同一探针。
3. 普通查询写出调用 `CanonicalResultSet::Next`，游标 fetch 也最终通过 lazy cursor 到达该 `Next`。进入真实取行前先调用 `before("Next")`；如果命中失败点，真实 `next_row` 尚未执行，错误沿 `ResultSet::Next`、`WorkerResults::operate` 和协议写出路径返回。
4. 非游标结果读完后，协议路径调用 `Finish`，先经过 `before("Finish")`。正常或异常退出最终关闭结果集，`Close` 在首次实际关闭时记录 `"Close"`；`CanonicalResultSet::closed` 保证其底层关闭幂等。
5. COM_STMT_FETCH 完成一批返回后调用 `OnFetchReturned`，对应记录 `"FetchReturned"`。Go 与 Rust 协议路径都明确把该通知放在写响应耗时统计之外。
6. 测试调用 `result_events_for_test`，runtime 发送 `SessionRequest::ResultEvents`，worker 调用 `WorkerResults::events` 并返回快照，测试据此断言例如 `Next -> Next -> Finish -> Close` 或 `Next -> FetchReturned -> Close`。

## 数据与状态

`fault` 中的操作名使用 `&'static str`，当前父模块传入的协议边界名为 `"Next"`、`"Finish"`、`"Close"`、`"FetchReturned"`。这些字符串不是生产 `Operation` 枚举的一部分，属于测试协议；拼写不一致不会报配置错误，只会永不匹配。

`count` 是当前故障配置的匹配计数，而不是全部事件数。例如配置 `("Next", 2, ...)` 时，穿插的 `FetchReturned` 会进入 `events`，但不会推进 `count`。`at == 0` 永远不会命中，因为代码先将匹配计数从零递增到一再比较。`events` 没有容量上限，生命周期由最近一次 `set_fault` 清空或由 `WorkerResults` 释放结束；它只服务于有界测试请求。

父模块的 `WorkerResults::probe` 是 `Arc<Mutex<_>>`，每个注册的 `CanonicalResultSet` 克隆该 `Arc`。因此探针是一个 session worker 的所有结果集共享状态，而不是每个结果集各自独立的状态；并行或交错使用多个结果集会把事件写入同一序列，也共同消耗匹配计数。

## 依赖与调用关系

上游配置链是 `pkg/server/conn_stmt_test.rs` → `SessionContext::result_fault_for_test`（trait 声明在 `pkg/server/conn.rs`，具体实现位于 `pkg/server/runtime.rs`）→ `SessionRequest::SetResultFault` → `WorkerResults::set_fault`。上游读取链同理通过 `SessionRequest::ResultEvents` 到达 `WorkerResults::events`。

运行时边界调用位于 `pkg/server/protocol_result.rs`：`CanonicalResultSet::Next` 调用 `before("Next")`，`Finish` 调用 `before("Finish")`，`Close` 调用 `before("Close")`，`OnFetchReturned` 调用 `before("FetchReturned")`。`WorkerResults::register` 负责把共享探针克隆到实际结果集。

本文件通过 `use super::*` 使用父模块已导入的 `sqlexec`、`ConnError`、`WorkerResults`，并直接依赖标准库的 `Duration`、`thread::sleep`、`Mutex` 和 `Vec<String>`。相应 crate 依赖由 `pkg/server/Cargo.toml` 声明，尤其是路径依赖 `astersql-util-sqlexec`；文件没有独立 Cargo target、feature 或 dev-dependency。

RustCodeGraph 将目标文件识别为 5 个符号，并确认 `BoundaryProbe` 由 `protocol_result.rs` 导入。对测试 API 的精确调用搜索显示，实际配置和事件断言集中在 `pkg/server/conn_stmt_test.rs`；索引对常见方法名的图查询有歧义，因此 runtime 桥接和测试调用以文件限定的源码搜索补证。

## 错误处理与边界

`before` 的可观察失败类型为 `sqlexec::GoError`，具体值是装箱的 `ConnError::Session`。`Next` 和 `Finish` 使用 `?` 传播该错误，最终测试可见 `ConnError::Session("injected Next failure")` 或 `ConnError::Session("injected Finish failure")`。

`Close` 与 `OnFetchReturned` 的父模块调用显式忽略 `before` 的返回值，因为对应 `ResultSet` 接口不提供错误返回通道。因此给这两个操作配置 `fail == true` 不会使协议失败；延时与事件记录仍然发生。现有回归对 `FetchReturned` 使用 `fail == false` 验证耗时排除，对 `Close` 主要验证恰好一次的生命周期事件。若未来需要验证这两个边界的错误，必须先改变接口契约，不能仅修改本探针。

`self.probe.lock().unwrap()` 在互斥锁中毒时会 panic；这是测试辅助代码的快速失败策略，没有恢复分支。`thread::sleep` 是阻塞式且在持锁期间执行，所以延时期间其他探针配置或读取会等待。错误文本直接包含静态操作名，不携带调用 ID 或结果集 ID。

其他边界包括：未知操作名静默不匹配；`at == 0` 静默不触发；一次配置只在恰好第 `at` 次命中；`set_fault` 会清掉所有旧事件，所以读取必须发生在下一次配置之前。

## 并发与资源生命周期

探针使用 `Arc<Mutex<BoundaryProbe>>`，保证 worker 结果集边界、测试配置请求和事件读取之间的数据竞争安全。锁覆盖事件追加、计数判断、休眠和可能的错误创建，因此一个边界调用从记录到注入结果是原子的；代价是故障延时会独占探针锁。

`WorkerResults` 拥有主 `Arc`，各 `CanonicalResultSet` 持有克隆。结果集关闭或 drop 后释放自己的引用；worker 状态销毁后最终释放探针。`CanonicalResultSet::Close` 的 `closed` 标志确保底层 close 与 `"Close"` 事件只在首次关闭时发生，`Drop` 再次调用 `Close` 不会重复记录。

配置和读取通过容量为 1 的同步通道往返 session worker（见 `pkg/server/runtime.rs`），因此测试调用返回时相应 `set_fault` 或 `events` 操作已经完成。尽管共享状态可跨线程安全访问，事件序列代表实际锁获取顺序；若测试主动并发驱动多个结果集，不能假设按结果集分组。

## 与 Go 版本的对应关系

Go 版本没有 `protocol_result_test_support.rs` 的逐文件对应物；这是 Rust 移植为验证真实 canonical result-set 边界而增加的测试设施。生产语义应对照 `pkg/server/conn.go`：`writeChunks` 循环调用 `rs.Next`，读完后调用 `rs.Finish`；`writeChunksWithFetchSize` 驱动游标迭代，并在响应耗时区间之外调用可选 `FetchNotifier.OnFetchReturned`。

`Next` 故障注入的 Go 先例是 `pkg/server/conn.go` 中的 `fetchNextErr` failpoint。`pkg/server/conn_test.go` 验证 first/second `Next` 对重试与已写出数据的影响；Rust 的 `BoundaryProbe` 用可配置的 `at` 提供同类覆盖，并在 `pkg/server/conn_stmt_test.rs::multichunk_next_failure_preserves_written_rows` 验证第二次失败仍保留首个真实 chunk。

耗时语义对照 `pkg/server/conn_stmt_test.go::TestResultSetWriteSQLRespDurationIncludesFailedRowWrite` 以及 Go 写出函数中 `beginWriteSQLRespDuration` / `finishWriteSQLRespDuration` 的边界。Rust 回归 `next_and_finish_are_excluded_and_errors_preserved` 和 `cursor_iteration_is_timed_and_notification_excluded` 进一步用探针区分 `Next`/`Finish`/`FetchReturned` 时间是否计入协议写出耗时。这些是测试机制差异，不表示 Rust 生产协议新增了 Go 没有的故障功能。

## 扩展指南

- 新增可观测结果集边界时，应先在 `CanonicalResultSet` 的真实接口实现处调用 `before`，再在 `pkg/server/conn_stmt_test.rs` 增加独立回归；不要把测试逻辑内嵌回生产算法，也不要创建会绕过 canonical source 的假结果集。
- 若继续使用字符串操作名，应把配置端、调用点和断言同步更新并保持大小写一致。更大范围扩展可考虑改为测试专用枚举，以消除未知名字静默失效，但需同时调整 runtime 的 `SessionRequest::SetResultFault` 载荷。
- 若要支持重复故障、多个同时故障或按结果集隔离，应修改 `fault`/`count` 的状态模型，并补充交错结果集测试；当前单配置、worker 级共享语义不可被误认为每结果集隔离。
- 若引入异步执行，不应继续在互斥锁内使用 `thread::sleep`；需要选择与执行模型匹配的可控时钟或同步原语，并验证事件顺序与锁生命周期。
- 相关 Rust 测试应继续放在独立的 `pkg/server/conn_stmt_test.rs`，符合源码与单元测试分离要求。至少覆盖：命中次数、错误传播、已有数据保留、`Finish`/`Close` 顺序、游标 `FetchReturned` 时间边界，以及配置复位清空事件。
- 兼容风险主要是测试边界与 Go 协议写出时序漂移；性能风险只存在于测试构建和注入路径，普通构建因 `cfg(test)` 不包含探针。

## 验证依据

- 目标源码：`pkg/server/protocol_result_test_support.rs`，核对 `BoundaryProbe`、`before`、`WorkerResults::set_fault`、`WorkerResults::events` 的完整 51 行实现。
- 父模块与真实调用边：`pkg/server/protocol_result.rs` 的测试模块装入、`CanonicalResultSet::{Next, Close, Finish, OnFetchReturned}`、`WorkerResults::{register, operate}`。
- crate 与模块边界：`pkg/server/Cargo.toml` 的 package/lib/dependencies，`pkg/server/lib.rs` 的私有 `mod protocol_result`；`pkg/server` 下未发现 `doc.go`。
- 测试请求桥接：`pkg/server/conn.rs` 的 `SessionContext` 测试方法，以及 `pkg/server/runtime.rs` 的 `SetResultFault`、`ResultEvents` 请求处理和具体方法实现。
- Rust 独立回归：`pkg/server/conn_stmt_test.rs` 中 `next_and_finish_are_excluded_and_errors_preserved`、`cursor_iteration_is_timed_and_notification_excluded`、`cursor_iterator_error_is_accounted`、`multichunk_next_failure_preserves_written_rows`、`empty_results_and_successful_flush_are_accounted`。
- Go 对照：`pkg/server/conn.go` 的 `writeResultSet`、`writeChunks`、`writeChunksWithFetchSize` 与 `fetchNextErr`；`pkg/server/conn_test.go` 的 first/second Next failpoint 回归；`pkg/server/conn_stmt_test.go::TestResultSetWriteSQLRespDurationIncludesFailedRowWrite`。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；目标文件被识别为 1 个文件、5 个符号，`BoundaryProbe` 位于第 9 行并由 `protocol_result.rs` 导入。常见方法名图查询存在同名歧义，故测试 API 调用以 `rg` 的文件限定结果交叉验证。
- 本任务是纯文档分析，按计划不运行 Cargo；最终使用任务规定的 11 章节命令做结构验证，并人工复核上述事实均可回溯到列出的源码、测试或 Cargo 声明。
