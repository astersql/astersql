# `pkg/dxf/framework/scheduler/state_transform.rs`

## 文件定位

本文件属于 `astersql-dxf-framework-scheduler` crate，是 DXF（Distributed eXecution Framework）任务级状态迁移的纯校验模块。crate 根 `pkg/dxf/framework/scheduler/lib.rs` 通过 `pub mod state_transform` 声明模块，并用 `pub use state_transform::*` 将其两个函数公开到 crate 根。源码只依赖 `crate::interface::*` 中的 `TaskState` 与 `TASK_STATE_*` 常量，不读写任务存储，也不直接驱动调度器。

当前仓库的引用搜索表明，Rust 调用点仅位于 `pkg/dxf/framework/scheduler/scheduler_test.rs`、`pkg/dxf/framework/integrationtests/framework_ha_test.rs` 和 `pkg/dxf/framework/integrationtests/framework_rollback_test.rs`；未发现生产 Rust 调用者。因此它目前是已公开、被测试验证的状态机约束函数，而不是生产状态写入路径中的强制守卫。若要让约束覆盖实际持久化，调用方仍需在更新任务状态前显式调用它。

## 核心职责

`verify_task_state_transform(from, to) -> bool` 用白名单回答“一次任务状态变更是否合法”。它承担三项职责：允许相同状态的幂等刷新；允许预定义的单步生命周期推进；拒绝终态、未知状态或跨阶段跳转。函数只返回布尔值，不解释拒绝原因，也不执行状态更新。

允许的非幂等边如下：

| 起始状态 | 允许的目标状态 |
| --- | --- |
| `pending` | `running`、`cancelling`、`pausing`、`succeed`、`failed` |
| `running` | `succeed`、`reverting`、`failed`、`cancelling`、`pausing` |
| `reverting` | `reverted` |
| `cancelling` | `reverting` |
| `pausing` | `paused` |
| `paused` | `resuming` |
| `resuming` | `running` |

`succeed`、`failed`、`reverted` 等终态没有向外迁移；`awaiting-resolution`、`modifying` 等已在 `interface.rs` 定义、但未列入本文件白名单的状态也只能通过“状态不变”校验。

## 主要符号

- `verify_task_state_transform(from: TaskState, to: TaskState) -> bool`：蛇形命名的核心实现，公开 API。先处理 `from == to`，再按 `from` 匹配允许目标集合。
- `VerifyTaskStateTransform(from: TaskState, to: TaskState) -> bool`：Go 风格公开兼容别名，不维护第二份规则，直接委托给 `verify_task_state_transform`。
- `TaskState`：由 `crate::interface` 提供的 `&'static str` 类型别名。这里没有封闭枚举，因此任意静态字符串都可能作为输入；安全性依赖白名单的默认拒绝分支。
- `TASK_STATE_PENDING` 等常量：同样来自 `interface.rs`，把协议字符串集中为具名状态。目标文件自身不声明常量、类型、trait、`impl` 或条件编译项。

## 执行流程

1. 调用者传入旧状态 `from` 和候选新状态 `to`。
2. 若两者相等，函数立即返回 `true`。这一分支让重复写入或元数据刷新保持幂等，包括未知字符串的同态输入。
3. 否则以 `from` 为主键进入 `match`。
4. `pending` 和 `running` 使用 `matches!` 检查多个合法目标；回滚、取消、暂停和恢复中的中间态各自只接受下一状态。
5. 所有未列出的起始状态进入 `_ => false`，覆盖终态和未知状态。
6. Go 风格入口 `VerifyTaskStateTransform` 不增加分支，原样返回核心函数结果。

该流程是 O(1) 的固定分支判断，不分配集合。它验证的只是单条边，不验证一串历史状态是否连续，也不验证状态变更是否与任务步骤、错误字段或子任务状态一致。

## 数据与状态

函数没有内部可变状态。输入和输出均按值传递；`TaskState` 实际是静态字符串切片，比较采用字符串值相等语义。状态常量的真实定义位于 `pkg/dxf/framework/scheduler/interface.rs`，其中还存在本白名单未覆盖的 `TASK_STATE_AWAITING_RESOLUTION` 和 `TASK_STATE_MODIFYING`。

关键不变量是：除 `from == to` 外，只有表中明确列出的边返回 `true`。因此取消必须先 `cancelling -> reverting` 再 `reverting -> reverted`，暂停/恢复必须经过 `pausing -> paused -> resuming -> running`，不能跳过中间态。与此同时，任何未知状态到自身都会因幂等分支返回 `true`；调用者若要求拒绝未知状态，需要在进入本函数前另做状态域校验，或调整这里的幂等规则并同步 Go 语义。

## 依赖与调用关系

向下依赖只有 `crate::interface::*`。目标 crate 的 `Cargo.toml` 声明了 proto、schstatus、storage 和 dxfmetric 等直接依赖，但本文件没有直接使用这些外部 crate；它使用的是 scheduler crate 自己在 `interface.rs` 中定义的状态别名和常量。文件也没有 feature 或 `cfg` 分支，因此本逻辑不受 `Cargo.toml` 中 Windows 专属依赖组影响。

向上由 `lib.rs` 公开模块并再导出符号。RustCodeGraph 将该文件标为被三个测试文件使用；仓库文本搜索没有发现生产 Rust 调用。`scheduler_test.rs` 验证代表性的合法边、幂等边和非法跳转；两个 integrationtests 文件分别把该约束用于 owner 故障切换和取消回滚路径的断言。Go 同路径实现由 scheduler 包导出，但非测试 Go 源码中同样未发现调用。

## 错误处理与边界

本 API 不返回 `Result`，非法迁移统一表现为 `false`，不会产生错误文本、日志或副作用。调用者必须决定是忽略、返回业务错误、重试还是中止持久化。

边界行为包括：相同状态总是允许；`pending` 可以直接成功或失败；`running` 可以直接失败，也可以进入取消、暂停或回滚；终态只能保持自身；未知起始状态到不同目标被拒绝；未知状态到自身被允许。`awaiting-resolution` 与 `modifying` 目前没有非幂等出边，这与 `interface.rs` 已定义这些状态的事实之间应被视为明确的当前限制，而不能推断为完整支持。

## 并发与资源生命周期

函数是无锁、无 I/O、无堆分配的纯函数，不创建线程、任务、通道、事务或资源句柄，可被多个线程并发调用。它只做检查，不提供“检查后更新”的原子性：若生产调用方先校验再写存储，仍须由存储事务、比较并交换或等价并发控制保证旧状态在写入前未变化。本文件也不管理任务或子任务生命周期；实际持久化、调度 tick 和故障恢复属于 scheduler 的其他模块。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/framework/scheduler/state_transform.go`。两版具有相同的幂等前置判断和相同的非幂等迁移集合。Go 用 `map[proto.TaskState][]proto.TaskState` 加 `slices.Contains` 查表，Rust 用固定 `match`/`matches!`，因此 Rust 避免了每次调用构造映射与切片，但语义保持一致。Go 对未知键取得空切片而返回 `false`；Rust 由默认分支返回 `false`。

Go 明列 `succeed`、`failed`、`reverted` 为空出边，Rust 将它们与其他未匹配状态合并到 `_ => false`。Go 注释说明当前没有 `revert_failed`，Rust 也未定义或允许该边。`pkg/dxf/framework/scheduler/scheduler_test.go::TestVerifyTaskStateTransform` 是 Go 回归基准；Rust 的 `scheduler_test.rs::test_verify_task_state_transform` 覆盖了相同核心意图，并额外列举取消、暂停和恢复链中的多条合法边。

## 扩展指南

新增任务状态或迁移时，应先确认 Go 的协议和 `state_transform.go` 是否同步变化，再修改 `interface.rs` 中的状态定义与本文件的 `match` 白名单。不要只在兼容别名中加逻辑；所有规则应集中于 `verify_task_state_transform`。若新增状态需要进入或退出现有链，需逐项判断终态性、是否允许幂等刷新、能否从 `pending`/`running` 直达，以及失败、取消和恢复时的路径。

测试逻辑应继续放在独立文件，而不是嵌入生产源文件。至少同步扩展 `pkg/dxf/framework/scheduler/scheduler_test.rs::test_verify_task_state_transform`，并对照 `scheduler_test.go::TestVerifyTaskStateTransform`；若改变故障切换或回滚链，还应更新 `framework_ha_test.rs::owner_failover_state_edges_remain_valid` 或 `framework_rollback_test.rs::cancellation_rolls_through_reverting_before_reverted`。兼容风险主要是放宽非法跳转或收紧现有幂等行为；并发风险不在纯函数内部，而在调用方是否把验证和写入置于同一原子边界。固定匹配的性能成本可忽略，但若改为动态规则表，应评估初始化、同步和热路径开销。

## 验证依据

- 源码：`pkg/dxf/framework/scheduler/state_transform.rs`，确认两个公开函数、完整白名单和默认拒绝分支。
- 类型与常量：`pkg/dxf/framework/scheduler/interface.rs`，确认 `TaskState = &'static str`、全部任务状态常量以及本文件未覆盖的状态。
- crate 边界：`pkg/dxf/framework/scheduler/Cargo.toml` 与 `pkg/dxf/framework/scheduler/lib.rs`，确认 crate 名、模块声明、公开再导出和依赖边界。
- Go 对照：`pkg/dxf/framework/scheduler/state_transform.go` 与 `pkg/dxf/framework/scheduler/scheduler_test.go::TestVerifyTaskStateTransform`，确认迁移表和测试意图。
- Rust 测试：`pkg/dxf/framework/scheduler/scheduler_test.rs::test_verify_task_state_transform`、`pkg/dxf/framework/integrationtests/framework_ha_test.rs::owner_failover_state_edges_remain_valid`、`pkg/dxf/framework/integrationtests/framework_rollback_test.rs::cancellation_rolls_through_reverting_before_reverted`。
- RustCodeGraph：`status` 显示索引包含目标仓库；`explore` 确认 `VerifyTaskStateTransform -> verify_task_state_transform`；目标文件节点列出上述三个直接使用文件。精确 `callers` 查询长时间无输出后被中止，因此调用范围另以 `rg` 全仓搜索交叉核验。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定十一章节结构检查和人工事实复核作为验证。
