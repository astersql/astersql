# `pkg/sessionctx/variable/setvar_affect.rs`

源文件：[`setvar_affect.rs`](setvar_affect.rs)

## 文件定位

本文件属于 `astersql-sessionctx-variable` crate；crate 根在 [`lib.rs`](lib.rs) 中以 `pub mod setvar_affect` 公开该模块。它保存 `SET_VAR` 语句 Hint 的“已验证可临时更新”变量名白名单，并提供一个最小数据模型和批量打标函数。它不解析 Hint、不写会话变量，也不负责语句结束后的恢复；规划器真正消费的标志位定义在 [`variable.rs`](variable.rs) 的运行时 `SysVar` 上，并由 [`pkg/planner/optimize.rs`](../../planner/optimize.rs) 的 `setVarHintChecker` 检查。

[`Cargo.toml`](Cargo.toml) 将本目录定义为独立库 `astersql-sessionctx-variable`（`lib.rs` 为入口，关闭自动测试发现和 doctest）。本文件自身只使用 Rust 标准库能力，没有直接调用 crate 的外部依赖。

## 核心职责

- `HINT_UPDATABLE_VERIFIED` 是 131 个精确、小写系统变量名组成的静态切片，表示这些变量已验证可由 `SET_VAR` Hint 临时改写。
- `setHintUpdatable` 扫描传入的变量切片，仅对名称命中白名单的项把 `IsHintUpdatableVerified` 置为 `true`。
- 本文件的 `SysVar` 是只含 `Name` 与 `IsHintUpdatableVerified` 的精简类型，不是 [`variable.rs`](variable.rs) 中包含作用域、类型、默认值和钩子的完整运行时 `SysVar`。因此该函数不能直接处理运行时注册表。
- 当前 Rust 生产接线只在 [`sysvar_builtins.rs`](sysvar_builtins.rs) 注册 `tidb_enable_full_outer_join` 时直接查询 `HINT_UPDATABLE_VERIFIED`；不能据此推断 131 项均由本模块自动写入运行时注册表。其他内建变量的标志常在各自构造处直接置位。

## 主要符号

- `pub static HINT_UPDATABLE_VERIFIED: &[&str]`：进程生命周期内有效的只读白名单。采用切片而非哈希集合，成员判断是线性搜索；当前长度为 131。
- `pub struct SysVar`：本模块的精简可变记录，派生 `Clone`、`Debug`、`PartialEq` 和 `Eq`。字段沿用 Go 命名：`Name: String` 与 `IsHintUpdatableVerified: bool`。
- `SysVar::new(name: impl Into<String>) -> Self`：接收可转换为 `String` 的名称，初始化标志为 `false`。
- `pub fn setHintUpdatable(vars: &mut [SysVar])`：原地遍历切片并设置命中项。它无返回值，不分配新的集合，也不清除已有的 `true`。

文件没有 trait、枚举、宏、条件编译项或错误类型。

## 执行流程

1. 调用者准备 `&mut [setvar_affect::SysVar]`；使用 `SysVar::new` 时每项初始标志均为 `false`。
2. `setHintUpdatable` 逐项取得可变引用。
3. 对每项执行 `HINT_UPDATABLE_VERIFIED.contains(&var.Name.as_str())`，按完整字符串精确匹配。
4. 命中时将标志置为 `true`；未命中时保持原值不变。
5. 函数遍历结束后返回 `()`，结果通过原切片可见。

生产侧的后续语义在本文件之外：运行时完整 `SysVar.IsHintUpdatableVerified` 被规划器 `setVarHintChecker` 读取；变量不存在会产生 unresolved-hint 错误，变量存在但标志为 `false` 会构造 `ErrNotHintUpdatable` 警告。实际临时设置和恢复由规划器服务及 statement context 完成，不是本模块职责。

## 数据与状态

白名单是不可变静态数据，没有延迟初始化和运行时写入。`setHintUpdatable` 唯一状态变化是把调用者拥有的元素标志从任意当前值更新为 `true`；它不会把未命中元素重置为 `false`，因此调用是幂等且单调的。

匹配区分大小写，且不做 trim、别名解析或规范化。例如测试中的 `tidb_allow_mpp` 命中，而 `not_hint_updatable` 与 `tidb_paging_size_bytes` 不命中。名称规范化应由上游完成，或新增行为时显式定义，不能默认为本函数已有能力。

Rust 白名单与 [`setvar_affect.go`](setvar_affect.go) 的 Go map 均为 131 项；按名称排序比较时集合完全一致。Go 源码还以注释明确排除了在 planner preprocess 阶段使用、行为尚需修正的 `tidb_read_staleness`，Rust 切片同样未包含它。

## 依赖与调用关系

- 模块入口：[`lib.rs`](lib.rs) 的 `pub mod setvar_affect`。
- 直接生产消费者：[`sysvar_builtins.rs`](sysvar_builtins.rs) 的内建变量注册逻辑用 `HINT_UPDATABLE_VERIFIED.contains(&vardef::TiDBEnableFullOuterJoin)` 设置完整运行时变量的标志。
- 直接测试调用者：[`error_1_aster_unit_test.rs`](error_1_aster_unit_test.rs) 构造本模块的三个精简 `SysVar`，调用 `setHintUpdatable` 并检查一项命中、两项不命中。
- 下游运行时消费者：[`pkg/planner/optimize.rs`](../../planner/optimize.rs) 的 `setVarHintChecker` 通过 `GetSysVar` 获取完整运行时变量并读取同名标志；这是一条“共享字段语义”的下游关系，不是对本文件函数的直接调用。
- Go 对照接线：[`variable.go`](variable.go) 在初始化 `defaultSysVars` 时调用 Go `setHintUpdatable`，这是 Go 版本的批量生产路径。当前 Rust 没有对应的完整表批量调用，因为本文件定义的是独立精简类型。

RustCodeGraph 的文件级索引报告本文件被 `sysvar_builtins.rs` 使用；精确 `query` 同时定位到 Go/Rust 两个 `setHintUpdatable`。`callers`/`callees` 精确查询在本次分析中超时且没有返回边，因此直接调用关系以上述索引文件关系和源码引用搜索为准。

## 错误处理与边界

本文件没有 `Result`、`Option` 解包、panic 或 I/O，因此没有本地错误传播。空切片直接完成；重复名称逐项处理；重复调用不改变已经为 `true` 的结果。

主要边界来自精确名称匹配：大小写不同、前后空白、别名和未列入白名单的新变量均不命中。线性 `contains` 的复杂度是每个输入元素扫描最多 131 个名称，即约为 `O(vars.len() * whitelist.len())`；对白名单和注册表的当前规模通常很小，但若未来显著扩张，应重新评估集合表示。

“列入白名单”只表达已经验证可通过 Hint 更新，不替代完整变量的作用域检查、值校验、设置钩子和恢复机制。新增名称却未在运行时 `SysVar` 上形成相同标志，或只有标志却缺少正确的会话写入/恢复行为，都会造成接线不完整。

## 并发与资源生命周期

静态白名单只读，可被多线程安全共享；本文件没有锁、原子变量、任务、通道、事务或外部资源。`setHintUpdatable` 要求独占的可变切片借用，因此同一切片的并发写入由 Rust 借用规则阻止。

函数不保存输入引用，借用只持续到调用返回。`SysVar` 拥有名称字符串，克隆会复制该字符串和布尔值；没有需显式关闭或回收的资源。语句级变量旧值的保存与恢复生命周期位于 planner/statement-context 层，本文件不参与管理。

## 与 Go 版本的对应关系

[`setvar_affect.go`](setvar_affect.go) 使用 `map[string]struct{}` 保存相同的 131 个名称，Rust 用 `&[&str]` 表示；前者平均常数时间查找，后者线性查找。两端 `setHintUpdatable` 都逐项原地把命中变量的 `IsHintUpdatableVerified` 设为 `true`，不处理未命中项，也不返回错误。

关键迁移差异是类型和接线：Go 函数接收完整的 `[]*SysVar`，并由 [`variable.go`](variable.go) 对 `defaultSysVars` 批量调用；Rust 函数接收本模块自定义的精简 `[SysVar]`，没有对 [`variable.rs`](variable.rs) 完整 `SysVar` 的通用批量应用。Rust 当前仅复用常量为一个生产变量打标，同时在其他注册代码中直接设置若干变量。因此白名单内容已经对齐，但不能宣称 Go 的统一初始化机制已完整移植。

相关 Go 测试包括 [`sysvar_test.go`](sysvar_test.go) 对 `tidb_dml_max_execution_time`、`tidb_max_keys_read` 的真值及 `tidb_paging_size_bytes` 的假值检查，以及 [`tests/variable_test.go`](tests/variable_test.go) 对短路表达式变量的检查。Rust 运行时对应断言分散在 [`sysvar_test.rs`](sysvar_test.rs)、[`sysvar_builtins_test.rs`](sysvar_builtins_test.rs) 和 [`tests/session_test.rs`](tests/session_test.rs)；本文件函数的直接边界测试位于 `error_1_aster_unit_test.rs`。

## 扩展指南

新增或移除可由 `SET_VAR` 更新的变量时，应先验证变量在规划阶段临时设置后，执行语义正确且语句结束能恢复旧值；随后同步修改 Go map 与 Rust `HINT_UPDATABLE_VERIFIED`，并比较两端集合。若 Rust 运行时仍采用逐变量注册，还必须在对应 [`sysvar_builtins.rs`](sysvar_builtins.rs) 构造路径设置完整 `SysVar.IsHintUpdatableVerified`，不能只改本文件的列表。

测试至少应覆盖：新变量命中、相近但不应支持的变量不命中、运行时 `GetSysVar` 返回的完整变量标志、有效/无效值的 Hint 设置，以及语句结束后的恢复行为。纯粹针对本模块的单元逻辑应继续放在独立测试文件中，不要把测试嵌入 `setvar_affect.rs`；可扩展现有 [`error_1_aster_unit_test.rs`](error_1_aster_unit_test.rs)，运行时行为则扩展 `sysvar_builtins_test.rs` 或 planner 的独立测试。

兼容风险包括 Go/Rust 白名单漂移、名称大小写或别名处理不一致、只打标而缺少设置/恢复钩子；性能风险主要是列表持续增长后的线性查找。若把精简类型改为完整运行时类型，需谨慎处理模块依赖和现有公开 API，避免形成循环依赖或保留两套互不一致的 `SysVar`。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7,032 个 Rust 文件和 4,415 个 Go 文件；`files --filter pkg/sessionctx/variable` 定位目标、Go 对照及测试；`node --file pkg/sessionctx/variable/setvar_affect.rs --offset 1 --limit 240` 读取完整 182 行源码并报告文件消费者；`query SetVarHintRestore`/`query setHintUpdatable` 区分相关符号；对精确函数的 `callers`/`callees` 查询超时，无边结果未被当作证据。
- 源码与接线：[`setvar_affect.rs`](setvar_affect.rs)、[`lib.rs`](lib.rs)、[`sysvar_builtins.rs`](sysvar_builtins.rs)、[`variable.rs`](variable.rs)、[`pkg/planner/optimize.rs`](../../planner/optimize.rs)。本包路径下不存在 `doc.go`，因此没有可补充读取的包级 Go 文档。
- crate 边界：[`Cargo.toml`](Cargo.toml) 的 package、lib、dependencies 与 `package.metadata.porting`。
- Go 对照：[`setvar_affect.go`](setvar_affect.go)、[`variable.go`](variable.go)；通过抽取并排序名称确认 Go/Rust 均为 131 项且无集合差异。
- 测试证据：[`error_1_aster_unit_test.rs`](error_1_aster_unit_test.rs)、[`sysvar_test.rs`](sysvar_test.rs)、[`sysvar_builtins_test.rs`](sysvar_builtins_test.rs)、[`tests/session_test.rs`](tests/session_test.rs)、[`sysvar_test.go`](sysvar_test.go)、[`tests/variable_test.go`](tests/variable_test.go)。本任务仅写文档，按计划未运行 Cargo 或代码测试。
