# `pkg/util/dbterror/plannererrors/planner_terror.rs`

## 文件定位

本文件是 `astersql-util-dbterror-plannererrors` crate 的规划相关静态错误表，源码入口为 [`planner_terror.rs`](planner_terror.rs)。它不负责发现 SQL 错误，也不实现规划算法；它把规划器、表达式和预处理语句路径会用到的 MySQL 错误码预先构造成统一的 `terror::Error` 原型，供调用方附加参数和栈后返回。

crate 根 [`lib.rs`](lib.rs) 通过 `pub mod planner_terror` 和 `pub use planner_terror::*` 同时提供模块路径与顶层再导出。其平台启动段静态量 `PLANNERERRORS_PACKAGE_INIT` 在进程或测试主体运行前调用 `planner_terror::initialize_planner_errors()`，以模拟 Go 包级 `var` 的初始化时机。[`Cargo.toml`](Cargo.toml) 表明该 crate 直接依赖 errno、errors、parser/mysql、parser/terror 和上层 `dbterror` 包装；planner、expression、executor、session、DDL 及若干测试 crate 再以路径依赖消费它。

## 核心职责

文件承担三项紧密相关的职责：

1. 声明 98 个公开的 `LazyLock<Box<terror::Error>>` 错误原型，保持 Go [`planner_terror.go`](planner_terror.go) 的声明顺序、错误码、错误类和消息选择。
2. 用私有 `PLANNER_ERRORS` 收齐全部 98 个惰性静态量，防止某个较少使用的错误直到注册表冻结后才首次构造。
3. 由 crate 内可见的 `initialize_planner_errors()` 顺序执行 `LazyLock::force`，完成 terror 错误类到 MySQL 错误码的登记。

这是一张“错误身份与协议映射表”，而不是每次报错时重新创建错误的工厂。调用方通常引用某个原型，再调用 `GenWithStackByArgs`、`GenWithStack`、`FastGenByArgs` 或比较 `Code`/`Equal`。

## 主要符号

- 98 个 `pub static Err…: LazyLock<Box<terror::Error>>` 是公开 API。按构造类计数，93 个属于 `dbterror::ClassOptimizer`（92 个 `NewStd`，加定制消息的 `ErrAccessDenied`），1 个 `ErrTooBigPrecision` 属于 `ClassExpression`，4 个 `ErrPrepareMulti`、`ErrUnsupportedPs`、`ErrPsManyParam`、`ErrPrepareDDL` 属于 `ClassExecutor`。
- Optimizer 组覆盖绑定与名称解析（如 `ErrUnknownColumn`、`ErrAmbiguous`）、聚合与分组、窗口定义与 frame、CTE、hint、权限、临时表、只读模式、外键和存储过程等错误。这里的“Optimizer”是兼容 Go 的错误分类，不表示每个错误只会在狭义优化阶段产生。
- 若干 Rust 名称有意映射到不同名的 MySQL errno：`ErrUnknownColumn` → `mysql::ErrBadField`，`ErrStmtNotFound` → `ErrPreparedStmtNotFound`，`ErrAmbiguous` → `ErrNonUniq`，`ErrSQLInReadOnlyMode` → `ErrReadOnlyMode`，`ErrViewSelectTemporaryTable` → `ErrViewSelectTmptable`，`ErrSubqueryMoreThan1Row` → `ErrSubqueryNo1Row`。这些均与 Go 文件一致，不能按 Rust 变量名推导或替换错误码。
- `ErrMixOfGroupFuncAndFields` 对应较长的 `ErrMixOfGroupFuncAndFieldsIncompatible` errno；`ErrAccessDenied` 更特殊：码仍为 `ErrAccessDenied`，消息却显式取 `MySQLErrName[ErrAccessDeniedNoPassword]`。
- `PLANNER_ERRORS: [&LazyLock<Box<terror::Error>>; 98]` 是完整性清单，私有且固定长度。它的顺序与声明顺序一致。
- `pub(crate) fn initialize_planner_errors()` 是本文件唯一函数。它不返回值，也不处理单项错误；循环强制初始化清单中的每一项。

文件没有自定义 struct、enum、trait、impl 或条件编译项；平台条件编译和启动段接线位于相邻 [`lib.rs`](lib.rs)。

## 执行流程

正常初始化链如下：

1. 链接该 crate 时，[`lib.rs`](lib.rs) 的 `PLANNERERRORS_PACKAGE_INIT` 被放入 Unix、macOS 或 Windows 对应的初始化段。
2. 初始化函数调用 `initialize_planner_errors()`；后者按 `PLANNER_ERRORS` 的顺序对每个 `LazyLock` 执行 `force`。
3. 每个闭包调用 `ClassOptimizer`、`ClassExpression` 或 `ClassExecutor` 的 `NewStd`。`NewStd` 从 `astersql_errno::errname::MySQLErrName` 取得标准模板，再委托 `NewStdErr`；`ErrAccessDenied` 直接选择无密码模板。
4. parser/terror 的 `ErrClass::NewStdErr` 先通过 `initError` 将类与错误码写入全局注册表，生成形如 `planner:<code>` 的 RFC code，再由 `errors::Normalize` 生成带消息模板、脱敏参数位、MySQL code 和 RFC code 的错误原型。
5. 运行期调用方解引用已经初始化的原型并生成具体错误。例如 [`pkg/planner/core/expression_rewriter.rs`](../../../planner/core/expression_rewriter.rs) 用 `ErrUnknownColumn`、`ErrInvalidGroupFuncUse` 等表达绑定失败；[`pkg/planner/core/logical_plan_builder_runtime.rs`](../../../planner/core/logical_plan_builder_runtime.rs) 用窗口、分组和不支持类错误拒绝非法计划；[`pkg/util/ranger/points.rs`](../../ranger/points.rs) 用 `ErrUnsupportedType` 记录无法转换的范围表达式。
6. 在协议边界，`terror::ToSQLError` 根据已注册的错误类和 code 还原 MySQL 数值码；未登记的类或 code 会回退 `ErrUnknown`，因此预初始化是可观察的正确性要求。

## 数据与状态

单个错误原型由 `LazyLock` 拥有，并以 `Box<terror::Error>` 固定在进程生命周期内；公开静态量只暴露共享只读访问，没有本文件级可变字段。每个原型携带至少三类协议元数据：数值 MySQL 错误码、由错误类与 code 组成的 RFC code，以及格式化消息模板（包括脱敏参数位置）。具体报错参数和调用栈由使用点生成的新错误承载，不写回原型。

真正的可变状态在下游 [`pkg/parser/terror/terror.rs`](../../../parser/terror/terror.rs)：错误构造会更新“错误类 → 已注册 code”和“RFC 前缀 → 错误类”的全局注册表。`PLANNER_ERRORS` 自身不是第二份注册表，只是保证所有惰性闭包都在允许注册的阶段执行的引用清单。其固定长度 98 与迁移测试的总数断言共同约束新增、删除或漏列条目。

## 依赖与调用关系

直接下游依赖如下：

- `std::sync::LazyLock`：提供一次性、线程安全的惰性构造与 `force`。
- `crate::errno as mysql`：提供 errno 常量与 `MySQLErrName` 消息表。
- `crate::dbterror`：提供带业务分类的 `ClassOptimizer`、`ClassExpression`、`ClassExecutor` 及 `NewStd`/`NewStdErr` 包装。
- `crate::terror`：提供 `Error` 类型和最终的注册、RFC code、SQL error 转换语义。

上游入口是 [`lib.rs`](lib.rs) 的启动构造器；公开静态量又被 crate 根再导出。已接线的代表性消费者包括：

- [`pkg/planner/core/expression_rewriter.rs`](../../../planner/core/expression_rewriter.rs)：列解析、聚合、分组、精度、系统变量权限等表达式重写错误。
- [`pkg/planner/core/logical_plan_builder_runtime.rs`](../../../planner/core/logical_plan_builder_runtime.rs)：窗口 frame、窗口继承、分组、表权限及不支持语法的计划构建检查。
- [`pkg/planner/optimize.rs`](../../../planner/optimize.rs)：只读模式、hint 名称和权限错误。
- [`pkg/session/runtime.rs`](../../../session/runtime.rs) 与 [`pkg/session/hint_runtime.rs`](../../../session/hint_runtime.rs)：会话层只读模式和不可更新 hint。
- [`pkg/executor/stmtsummary.rs`](../../../executor/stmtsummary.rs)：statement summary 不支持路径。
- [`pkg/util/ranger/points.rs`](../../ranger/points.rs) 与 [`pkg/util/hint/hint_processor.rs`](../../hint/hint_processor.rs)：范围构造类型错误和 hint 冲突警告。

RustCodeGraph 能定位本文件和 `initialize_planner_errors`，但当前索引将文件报告为“used by 0 files”，且没有解析出 `LazyLock::force` 或公开静态量的有效调用边；以上接线因此使用 crate 根与实际 Rust 引用交叉核验，而没有把图的缺边误写成“未使用”。

## 错误处理与边界

本文件只定义错误，不吞掉、重试或降级业务失败。调用点决定填充哪些格式参数以及是否附栈；因此扩展时必须让消息模板的占位符与调用参数保持一致。

注册时机是最重要的边界。parser/terror 的 `RegisterFinish()` 以原子标志冻结注册；冻结后再通过 `NewStd`/`NewStdErr` 构造未初始化原型会打印 backtrace 并 panic。`PLANNERERRORS_PACKAGE_INIT` 与 `initialize_planner_errors()` 就是为避免“冷门错误首次访问过晚”而存在。反过来，`LazyLock::force` 的闭包如果引用不存在的标准消息码，`MySQLErrName` 索引也会失败，而不是静默制造无模板错误。

协议转换也有明确失败语义：`ToSQLError` 找不到 RFC 类或未登记 code 时回退 `ErrUnknown` 并记录日志。独立测试据此要求转换结果既不是 `ErrUnknown`，又等于原型自身的 `Code()`。

`ErrAccessDenied` 是兼容性边界而非笔误：系统无法在此处判断用户是否带密码登录，所以沿用 Go 的无密码消息模板。修改为默认 `ErrAccessDenied` 模板会改变客户端可见文本，即使数值码不变。

## 并发与资源生命周期

`LazyLock` 保证每个静态错误闭包最多执行一次，成功构造后在整个进程生命周期内存活；多个线程随后只读共享该 `terror::Error`。初始化循环本身没有线程、异步任务、通道、文件句柄、网络连接或显式清理动作。

需要区分“惰性量本身线程安全”和“登记阶段允许并发”两个问题。相邻 [`pkg/util/dbterror/terror.rs`](../terror.rs) 保留了 `NewStd` 通常用于全局初始化、不可任意并发调用的 Go 约束；底层注册表虽用锁和原子量保护，但冻结后注册必然 panic。启动段在正常服务并发开始前集中 `force`，既确定了生命周期顺序，也避免运行期首次访问与 `RegisterFinish` 竞态。资源没有析构需求，静态 `Box` 有意随进程终止释放。

## 与 Go 版本的对应关系

Rust [`planner_terror.rs`](planner_terror.rs) 逐项迁移 Go [`planner_terror.go`](planner_terror.go) 的单个 `var` 块。两边均有 98 个错误，声明次序、93/1/4 的 Optimizer/Expression/Executor 分类以及上述异名 errno 映射保持一致。Go 在包初始化时立即调用构造函数；Rust 不能在普通 `static` 初始化器中执行这些非 const 操作，因此采用 `LazyLock`，再由启动构造器集中强制初始化来恢复 Go 的时序语义。

Go 的 `ErrAccessDenied = ClassOptimizer.NewStdErr(ErrAccessDenied, MySQLErrName[ErrAccessDeniedNoPassword])` 在 Rust 中保持同样的“码与消息来源不同”行为。Go 的 `ErrPartitionNoTemporary` 注释及预处理语句四项的 Executor 分类也被保留。

测试对应关系：Go [`errors_test.go`](errors_test.go) 的 `TestError` 遍历一组代表错误，断言 `ToSQLError` 不返回 unknown 且数值码等于原错误；Rust [`errors_test.rs`](errors_test.rs) 保持同一测试意图。Rust 额外的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 覆盖全部 98 项的类别前缀、SQL code、总数、`ErrAccessDenied` 模板差异，以及注册表冻结后仍能访问预初始化尾项的行为。

## 扩展指南

新增或调整规划错误时应保持局部且成套修改：

1. 先在 Go [`planner_terror.go`](planner_terror.go) 和 errno/消息定义中确认真实的错误类、数值码与模板，不按变量名猜映射。
2. 在本文件相同逻辑分组中新增 `pub static LazyLock`；标准模板用 `NewStd`，只有 Go 明确指定不同模板时才用 `NewStdErr`。
3. 将新静态量按声明顺序加入 `PLANNER_ERRORS`，同步数组长度。漏加可能只在冻结后首次访问时 panic。
4. 在独立的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 对应类别数组中加入该项并更新总数；若它应参与 Go 的转换回归集合，也同步独立的 [`errors_test.rs`](errors_test.rs)。不要把测试嵌入生产源文件。
5. 若增加新的下游 crate 使用点，更新该 crate 的 Cargo 路径依赖并复用 crate 根再导出；不要复制错误原型。

兼容性风险主要是错误码、RFC 类、客户端消息文本和参数脱敏位置漂移；正确性风险是新项漏进初始化清单；性能风险较低，仅增加启动时一次性构造与常驻错误原型的成本。改变启动段或注册冻结顺序属于 crate 级生命周期变更，不能当作单个错误表编辑处理。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/dbterror/plannererrors` 找到目标 Rust/Go/Cargo/测试上下文；`node --file ... --offset ...` 覆盖目标文件 350 行；`query initialize_planner_errors --kind function` 定位第 346 行。`callers` 无有效输出、`callees` 报告无边，已明确作为索引限制处理。
- 生产源码：[`planner_terror.rs`](planner_terror.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`pkg/util/dbterror/terror.rs`](../terror.rs)、[`pkg/parser/terror/terror.rs`](../../../parser/terror/terror.rs)。其中静态声明计数为 98，构造调用计数为 Optimizer `NewStd` 92、Optimizer `NewStdErr` 1、Expression 1、Executor 4。
- Go 对照与独立测试：[`planner_terror.go`](planner_terror.go)、[`errors_test.go`](errors_test.go)、[`errors_test.rs`](errors_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。目标目录未发现 `doc.go`，因此没有额外的包契约文件可读。
- 调用点抽查：[`pkg/planner/core/expression_rewriter.rs`](../../../planner/core/expression_rewriter.rs)、[`pkg/planner/core/logical_plan_builder_runtime.rs`](../../../planner/core/logical_plan_builder_runtime.rs)、[`pkg/planner/optimize.rs`](../../../planner/optimize.rs)、[`pkg/util/ranger/points.rs`](../../ranger/points.rs)、[`pkg/session/runtime.rs`](../../../session/runtime.rs)、[`pkg/executor/stmtsummary.rs`](../../../executor/stmtsummary.rs)。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付前以任务指定命令验证文档存在且恰有 11 个固定二级标题，并人工复核仅新增本说明文件、未修改 Rust/Go/Cargo 或总计划。
