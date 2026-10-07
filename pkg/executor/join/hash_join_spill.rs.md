# `pkg/executor/join/hash_join_spill.rs`

源码：[`pkg/executor/join/hash_join_spill.rs`](hash_join_spill.rs)

## 文件定位

本文件属于 `astersql-executor-join` crate（见 [`Cargo.toml`](Cargo.toml) 的 `[package]` 与 `[lib]`），由 [`lib.rs`](lib.rs) 以公开模块 `hash_join_spill` 导出。它位于 Hash Join 内存跟踪器与实际分区落盘实现之间：接收“内存已超过配额”的通知，判断本轮是否值得且允许 spill，然后只把共享辅助器的状态置为 `NeedSpill`；具体的磁盘写入、分区恢复和清理由 [`hash_join_spill_helper.rs`](hash_join_spill_helper.rs) 负责。

当前 Rust 迁移状态需要特别区分：本文件的动作、阈值判断和单元测试已经存在，但生产侧 [`hash_join_v2.rs`](hash_join_v2.rs) 中创建并注册 spill action 的 `OpenSelf` 逻辑仍是注释形式的 Go 迁移稿。仓库内对 `HashJoinSpillAction` 的实际 Rust 使用目前来自 [`hash_join_spill_test.rs`](hash_join_spill_test.rs) 和 [`inner_join_spill_test.rs`](inner_join_spill_test.rs)，不能据此声称 Rust Hash Join 主链已经注册该 OOM action。

## 核心职责

- 以 `OomAction` 定义本地 OOM 动作协议：动作必须可在线程间共享（`Send + Sync`），提供优先级和执行入口。
- 由 `HashJoinSpillAction::action_impl` 串行化并发触发，等待正在进行的 spill 结束，检查超限、状态、最小数据量和允许开关，再将状态切换为 `SpillStatus::NeedSpill`。
- 在置位时通过 `HashJoinSpillHelper::set_need_spill` 保存触发瞬间的已消费字节和配额，避免真正开始 spill 前执行器继续分配内存导致诊断快照失真。
- 当仍然超限但数据不足或 spill 被禁用时，由 `OomAction::action` 转交可选的 fallback；文件本身不定义最终 OOM 策略。
- 用 `has_enough_data_to_spill` 落实 Go 版本的成本门槛：Hash Join 自身已消费内存至少达到触发 tracker 配额的 `1/20` 才值得落盘。

## 主要符号

- `SPILL_INFO: &str`：与 Go `spillInfo` 对齐的诊断文案。本文件当前只声明它，没有直接记录日志。
- `DEFAULT_SPILL_PRIORITY: i64 = 2`：`HashJoinSpillAction::priority` 的返回值，对齐 Go `memory.DefSpillPriority`。
- `OomAction`：包含 `priority(&self) -> i64` 和 `action(&self, &MemoryTracker)` 的公开 trait；`Send + Sync` 约束允许以 `Arc<dyn OomAction>` 共享。
- `HashJoinSpillAction`：动作主体。`spill_helper: Arc<HashJoinSpillHelper>` 共享状态与 tracker，`fallback: Option<Arc<dyn OomAction>>` 保存后继动作，`action_lock: Mutex<()>` 保证一次只有一个触发者执行检查及置位序列。
- `HashJoinSpillAction::new`：创建无 fallback 的动作；调用方若需要 OOM 链必须继续调用 `with_fallback`。
- `HashJoinSpillAction::with_fallback`：以 builder 形式安装 fallback，并返回更新后的自身。
- `HashJoinSpillAction::spill_helper`：只读暴露共享辅助器的 `Arc`，不转移所有权。
- `HashJoinSpillAction::trigger_fallback_action`：fallback 存在时调用其 `action`，不存在时静默返回。
- `HashJoinSpillAction::action_impl`：返回是否由本次调用成功把状态从未 spill 路径推进为 `NeedSpill`。
- `impl OomAction for HashJoinSpillAction`：固定优先级，并组合“尝试 spill”与“必要时 fallback”两条路径。
- `has_enough_data_to_spill`：比较 `hash_join_tracker.bytes_consumed()` 与 `passed_in_tracker.bytes_limit() / 20`；使用整数除法，阈值向下取整。

## 执行流程

1. 上游内存子系统应在 tracker 超限时调用 `OomAction::action`。Rust 生产接线当前尚未实现；Go 对应入口是 `HashJoinV2Exec.OpenSelf` 注册 `newHashJoinSpillAction`。
2. `action` 首先调用 `action_impl`。后者取得 `action_lock`；锁中调用 `HashJoinSpillHelper::wait_while_spilling`，若共享状态为 `InSpilling`，就在辅助器自己的条件变量上等待状态变化。
3. 等待结束后按短路顺序检查：触发 tracker 确实超限；辅助器状态仍为 `NotSpilled`；Hash Join 自身数据达到配额的 `1/20`；`can_spill()` 开关为真。
4. 四项均成立时调用 `set_need_spill(consumed, limit)`，把状态设为 `NeedSpill` 并记录触发 tracker 的消费量和配额，随后返回 `true`。`action` 立即结束，不触发 fallback。
5. 任一条件不成立时 `action_impl` 返回 `false`。`action` 再次确认 tracker 仍超限；只有“数据不足”或“禁止 spill”至少一项成立时才调用 fallback。
6. 若未成功置位的原因只是状态已经为 `NeedSpill`，而数据足够且仍允许 spill，则第二阶段条件也为假，本轮不重复置位且不 fallback，交由已经安排的 spill 路径处理。
7. 本文件不把 `NeedSpill` 推进为 `InSpilling`。辅助器中的 `spill_row_tables` / `spill_remaining_rows` 才会调用 `set_in_spilling`，结束后调用 `set_not_spilled` 并 `notify_all` 唤醒等待者。

## 数据与状态

核心状态由 `HashJoinSpillHelper` 持有，而不是复制到 action：`SpillStatus` 在 `NotSpilled`、`NeedSpill`、`InSpilling` 间变化；`can_spill_flag` 控制是否允许落盘；`memory_tracker` 统计 Hash Join 自身用量；`bytes_consumed` 和 `bytes_limit` 保存触发快照。`HashJoinSpillAction` 只保存共享 `Arc`、可选 fallback 和本动作级互斥锁。

需要区分两个 tracker：`has_enough_data_to_spill` 的第一个参数是 Hash Join 自身 tracker，第二个参数是触发 OOM action 的 tracker。阈值取第二者的配额，但比较第一者的消费量。`MemoryTracker::check_exceed` 的实际判定是配额非负且 `bytes_consumed > bytes_limit`，等于配额不算超限。

`has_enough_data_to_spill` 对配额直接做有符号整数除法。正常调用依赖有效的非负配额；负配额在 `check_exceed` 处会阻止触发，但独立调用该辅助函数时没有额外校验。配额小于 20 时阈值会变成 0，这是当前代码的精确整数语义。

## 依赖与调用关系

- 上游模块边界：[`lib.rs`](lib.rs) 公开导出本模块，并在 `cfg(test)` 下装配两个直接相关测试模块。
- Rust 当前调用者：RustCodeGraph 显示 `action_impl` 由本文件的 `action` 与 `hash_join_spill_test.rs` 的并发测试调用；`OomAction::action` 在 `hash_join_spill_test.rs` 和 `inner_join_spill_test.rs` 中被调用。未找到生产代码构造 `HashJoinSpillAction`。
- 下游状态依赖：`action_impl` 调用 `MemoryTracker::{check_exceed,bytes_consumed,bytes_limit}`，以及 `HashJoinSpillHelper::{wait_while_spilling,status,can_spill,set_need_spill}`；这些定义都在 [`hash_join_spill_helper.rs`](hash_join_spill_helper.rs)。
- fallback 边：`trigger_fallback_action` 动态分派到 `Arc<dyn OomAction>::action`，具体策略由构造者注入。
- crate 边界：本文件只使用同 crate 的 `hash_join_spill_helper` 与标准库 `Arc`、`Mutex`；`Cargo.toml` 没有为本文件引入单独的第三方依赖。该 crate 的大部分迁移依赖位于 `cfg(windows)` 目标段，但本模块自身没有条件编译项。
- Go 主链：[`hash_join_v2.go`](hash_join_v2.go) 的 `HashJoinV2Exec.OpenSelf` 在 `EnableTmpStorageOnOOM` 开启且 `partitionNumber > 1` 时创建 action，并通过 session memory tracker 的 `FallbackOldAndSetNewAction` 注册；这条生产边在 Rust 中尚未落地。

## 错误处理与边界

本文件的公开接口不返回 `Result`。互斥锁或条件变量因其他线程 panic 而 poisoned 时，`expect("spill action poisoned")` 或辅助器中的 `expect("spill helper poisoned")` 会继续 panic；当前策略不是恢复或转成执行错误。

fallback 是可选项：在不能或不值得 spill 的超限场景中，如果没有安装 fallback，`trigger_fallback_action` 不会产生错误或解除超限，后续责任仍在外部 OOM 链。反之，tracker 已不超限时绝不触发 fallback。已经处于 `NeedSpill` 的状态不会再次成功，也不会仅因这点触发 fallback。

本文件只发出 spill 请求，不保证实际释放内存，也不负责记录 `SPILL_INFO`、创建临时文件或处理 I/O 错误。实际落盘错误和恢复轮数边界属于 `HashJoinSpillHelper`。此外，Rust 尚无生产注册边，因此当前行为证据仅覆盖组件级动作和测试组合，不能等同于端到端 OOM 处理。

## 并发与资源生命周期

`HashJoinSpillAction` 可通过 `Arc` 被多个线程共享。`action_lock` 覆盖“等待当前 spill、重新检查条件、置 `NeedSpill`”的完整序列，因此并发触发者中最多一个能在同一 `NotSpilled` 周期返回成功；`hash_join_spill_test.rs::concurrent_actions_set_need_spill_only_once_like_go_cond_lock` 用 16 个线程验证成功数恰为 1。

这里存在两层锁：action 自己的 `Mutex<()>` 串行化 OOM 回调；helper 的状态锁与 `Condvar` 保护 spill 状态。`wait_while_spilling` 在等待时释放 helper 状态锁，但调用者仍持有 `action_lock`，避免其他 action 越过等待者重复判定。实际 spill 完成后，helper 的 `set_not_spilled` 修改状态并 `notify_all`。

`Arc<HashJoinSpillHelper>` 和 `Arc<dyn OomAction>` 管理共享生命周期，没有显式关闭逻辑。临时存储及磁盘资源的关闭由 helper 的 `close` 等路径负责；本 action 不拥有文件句柄。原子字段的 Acquire/Release 顺序也封装在 helper 中，本文件通过其方法读取开关并写入快照。

## 与 Go 版本的对应关系

[`hash_join_spill.go`](hash_join_spill.go) 是直接语义基线。Rust `HashJoinSpillAction` 对应 Go `hashJoinSpillAction`，`new` 对应 `newHashJoinSpillAction`，`priority` 对应 `GetPriority`，`action` / `action_impl` / `trigger_fallback_action` 和阈值函数也逐项对应。优先级 2、`1/20` 阈值、等待正在 spill、成功置位后不 fallback、不能或不值得 spill 时 fallback 等关键分支保持一致。

实现形态存在几处明确差异：Go 通过嵌入 `memory.BaseOOMAction` 管理 fallback，Rust 直接保存 `Option<Arc<dyn OomAction>>`；Go 使用 helper 的同一条件变量锁覆盖等待与无锁状态检查，Rust 增加 `action_lock` 串行化 action，再调用 helper 的锁封装方法；Go 分别置状态并写两个原子快照，Rust 将三步封装到 `set_need_spill`；Go 的可变 `spillChunkSize` 和状态常量位于本文件，Rust 的分块及状态定义位于 helper。本文件顶部的大段注释保留了早期 Go 形状，但有效实现以第 109 行之后的 Rust 定义为准。

最大的迁移差异是接线：Go `hash_join_v2.go` 已在 `OpenSelf` 注册该动作；Rust `hash_join_v2.rs` 只保留对应注释稿。因此该组件的局部语义已对齐，完整应用主链尚未对齐。

## 扩展指南

- 修改触发门槛时，集中调整 `has_enough_data_to_spill`，并同步扩展 [`hash_join_spill_test.rs`](hash_join_spill_test.rs) 中阈值以下、恰好达到阈值、低配额整数截断和负配额的边界断言；同时核对 Go 是否仍为 `1/20`，避免无意分叉。
- 增加新状态或改变 `NeedSpill` 生命周期时，需要联合修改 helper 的 `SpillStatus`、`wait_while_spilling`、`set_need_spill`、开始/结束 spill 的通知路径，而不能只改本文件；同步测试必须保持在独立 `*_test.rs` 文件中。
- 接入 Rust 生产主链时，应在 `hash_join_v2.rs` 的真实初始化/关闭生命周期中构造并注册 action，保留 `EnableTmpStorageOnOOM` 与分区数条件、旧 action fallback 链以及关闭时的所有权关系；不能把现有注释稿当作已执行代码。
- 改动 fallback 协议时要维持 `Send + Sync` 和对象安全，明确多个 action 的优先级排序及 fallback 是否可能重入；相关并发测试应继续验证一次状态周期只成功置位一次。
- 若增加日志，应使用 `SPILL_INFO` 与 helper 保存的触发快照，而不是日志打印时的即时 tracker 值；热路径上避免在 action 锁内引入磁盘 I/O 或长耗时工作。
- 性能风险主要来自 OOM 高频触发下的串行锁、条件变量等待和重复原子读取；兼容风险主要来自门槛、优先级、fallback 条件或 Go/Rust 状态机差异。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被识别为含 17 个符号的 Rust 文件。
- RustCodeGraph `node --file pkg/executor/join/hash_join_spill.rs`：核对了 208 行有效源码、全部模块级常量、trait、结构体、实现和函数；文件没有条件编译项。
- RustCodeGraph `node action_impl`、`node has_enough_data_to_spill`、`node HashJoinSpillHelper`、`node wait_while_spilling`、`node set_need_spill`、`node can_spill`、`node check_exceed`、`node set_not_spilled` 与 `node set_in_spilling`：核对关键调用边、锁/条件变量、阈值、状态转换和快照写入。
- 读取 [`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)：确认 crate 名称、模块公开性、测试装配、依赖边界和无目标文件专属 feature。
- 读取 [`hash_join_spill.go`](hash_join_spill.go) 与 [`hash_join_v2.go`](hash_join_v2.go)：核对 Go 动作逻辑、优先级、阈值、fallback 和生产注册条件。
- 读取 [`hash_join_spill_test.rs`](hash_join_spill_test.rs)：核对优先级 2、4/5 字节阈值分支、fallback 调用以及 16 线程只成功置位一次。读取 [`inner_join_spill_test.rs`](inner_join_spill_test.rs) 的 `spill_action_transitions_need_spill_and_falls_back_when_spill_disabled`：核对允许 spill 时进入 `NeedSpill`、禁用时 fallback。
- 精确搜索 `HashJoinSpillAction`、`OomAction` 和 `has_enough_data_to_spill`：除测试外未发现 Rust 生产构造者；[`hash_join_v2.rs`](hash_join_v2.rs) 的对应注册逻辑仍位于注释稿。
- 本任务是只增说明文档的分析任务，按计划不运行 Cargo；交付时以任务指定的 11 章节结构命令、链接与差异人工复核为验证。
