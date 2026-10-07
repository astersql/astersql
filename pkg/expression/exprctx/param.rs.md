# `pkg/expression/exprctx/param.rs`

## 文件定位

本文件属于 `astersql-expression-exprctx` crate。crate 入口
[`pkg/expression/exprctx/lib.rs`](./lib.rs) 以私有模块 `mod param` 装配它，再用
`pub use param::*` 将本文件的公开项提升为 `exprctx` 的公共 API。它位于表达式上下文的最底层：只定义“按位置读取预处理语句参数”的协议、统一越界错误和空实现，不负责解析 MySQL 协议参数，也不拥有真实会话参数列表。

[`pkg/expression/exprctx/Cargo.toml`](./Cargo.toml) 将该目录声明为
`astersql-expression-exprctx`，入口为 `lib.rs`。本文件返回的 `types::Datum` 由入口模块从
`astersql-types` 的 `types_crate::datum::Datum` 重导出；因此本文件的直接数据依赖只有标准库格式化/错误 trait 与该 Datum 类型。

## 核心职责

- `ParamValues` 把预处理语句的绑定参数抽象成只读、0 起始的随机访问接口，使表达式层不必知道参数来自在线会话、静态快照还是测试替身。
- `ParamError` 与 `ErrParamIndexExceedParamCounts` 把“下标没有对应绑定值”收敛为单一、可比较的错误，并保留 Go 版本的错误文本。
- `EmptyParamValues` / `EMPTY_PARAM_VALUES` 提供零分配的空参数上下文，供字符串化、解释输出和没有执行期绑定值的路径显式拒绝任何索引。

该文件刻意不定义参数容器。真实实现见
[`pkg/expression/exprstatic/evalctx.rs`](../exprstatic/evalctx.rs) 的静态 `EvalContext` 和
[`pkg/expression/sessionexpr/sessionctx.rs`](../sessionexpr/sessionctx.rs) 的会话 `EvalContext`；二者从各自持有的参数序列取值。

## 主要符号

- `ERR_PARAM_INDEX_EXCEED_PARAM_COUNTS: &str`：稳定错误文本，值为
  `"Param index exceed param counts"`，同时被 `ParamError::fmt` 和测试使用。
- `ParamError::IndexExceedsParamCount`：当前唯一错误变体。类型实现
  `Clone + Copy + Debug + Eq + PartialEq`，可按值传播和断言；`Display` 总是写入上述稳定文本，且实现 `std::error::Error`。
- `ErrParamIndexExceedParamCounts: ParamError`：采用 Go 名称的常量别名，值为
  `ParamError::IndexExceedsParamCount`。实现者通常通过它构造越界结果。
- `ParamValues`：公开 trait；唯一方法
  `GetParamValue(&self, idx: usize) -> Result<types::Datum, ParamError>` 以共享借用读取下标，并按值返回一个 `Datum`。
- `EmptyParamValues`：无字段的公开单元结构体。其 `GetParamValue` 忽略下标并始终返回
  `Err(ErrParamIndexExceedParamCounts)`。
- `EMPTY_PARAM_VALUES`：`EmptyParamValues` 的静态实例。由于 `lib.rs` 全量重导出，本 crate 的调用者也能直接使用 Go 风格类型名 `EmptyParamValues` 构造同等的零大小值。

符号沿用 Go 风格大小写是迁移兼容选择；`lib.rs` 对 `non_snake_case` 和
`non_upper_case_globals` 的 crate 级允许使这些名称可直接保留。

## 执行流程

典型的预处理参数读取链如下：

1. 协议/会话层把本次执行绑定值放入会话参数序列；静态上下文则由
   `WithParamList` 把参数复制进 `EvalCtxState::param_list`。
2. 需要参数的调用点持有 `&dyn ParamValues` 或更强的 `&dyn EvalContext`；后者在
   [`pkg/expression/exprctx/context.rs`](./context.rs) 中声明为
   `WarnHandler + ParamValues`，所以天然具备参数访问能力。
3. 在线 `sessionexpr::EvalContext::GetParamValue` 从
   `SessionContext::parameter_values()` 取值；静态 `exprstatic::EvalContext::GetParamValue`
   从 `param_list` 取值。两者都使用 `get(index).cloned()`，成功返回该位置 Datum 的副本，失败返回本文件的哨兵错误。
4. [`pkg/planner/core/expression_rewriter.rs`](../../planner/core/expression_rewriter.rs)
   的 `toParamMarker` 在重写参数标记时读取当前值、推断参数类型，并在失败时把错误文本写入重写器错误槽。
5. [`pkg/expression/constant.rs`](../constant.rs) 的 `ParamMarker::GetUserVar` 在表达式真正需要参数时按保存的 `order` 再读一次，并把 `ParamError` 转换为表达式通用错误。这保留了不同 EXECUTE 绑定不同值的行为。

没有参数上下文时，字符串化/解释路径可传入 `EmptyParamValues`。若路径意外尝试求值参数标记，会立即得到相同越界错误，而不是伪造默认 Datum。

## 数据与状态

本文件自身不保存可变状态：`ParamError` 和 `EmptyParamValues` 都是不含载荷或字段的值，
`EMPTY_PARAM_VALUES` 也是只读静态零大小实例。`ParamValues::GetParamValue` 只接收
`&self`，因此协议本身只承诺观察，不允许通过该接口修改、追加或清空参数。

成功路径返回拥有所有权的 `types::Datum`。当前两个生产实现都对容器中的 Datum 调用
`cloned()`；调用者所得值与容器槽位解耦。静态上下文的 `WithParamList` 还整体接收并保存
`Vec<Datum>`，其独立测试验证构造后修改原始 `Vec` 不会改变上下文中的参数。

下标类型是 `usize`，因此 Rust API 不存在 Go `int` 可表达的负下标；所有不存在的非负下标（包括空上下文中的 0）统一归入 `IndexExceedsParamCount`。本接口不区分“参数列表为空”和“下标大于列表长度”。

## 依赖与调用关系

向下依赖：

- `std::fmt`：实现稳定的 `Display` 文本。
- `crate::types::Datum`：参数值载体；由 `lib.rs` 从 `astersql-types` 重导出。

直接实现与转发：

- `exprctx::EmptyParamValues`：固定错误实现。
- `exprstatic::EvalContext`：读取静态快照的 `EvalCtxState::param_list`。
- `sessionexpr::EvalContext<C>`：读取 `SessionContext::parameter_values()`。
- `exprctx::InnerOverrideEvalContext`、`expression::assertionEvalContext` 和
  `expression::EvalContextParamValues`：把读取原样转发给被包装上下文。
- 多个独立测试上下文实现该 trait，并以固定错误表示“不提供预处理参数”。

主要消费者：

- `exprctx::EvalContext` 以 supertrait 形式把参数访问纳入所有表达式求值上下文。
- `constant::ParamMarker::GetUserVar` 读取延迟参数；其调用者包括常量求值、类型读取、字符串化与常量折叠路径。
- `planner::core::expression_rewriter::toParamMarker` 在规划重写时读取参数并推断字段类型。
- 表达式、聚合描述和 planner handle 列的 `StringWithCtx` 接口接受
  `ParamValues`；没有实参时常使用 `EmptyParamValues`，避免这些展示路径依赖完整会话。

RustCodeGraph 对本文件的文件节点报告 19 个使用文件；其调用流明确给出
`constant.rs::GetUserVar -> util.rs::EvalContextParamValues::GetParamValue`，并识别静态/会话上下文、规划器重写与空参数测试等调用者。由于同名 `GetParamValue` 跨 Go/Rust 与多个实现高度重载，具体实现关系以上述源码位置为准。

## 错误处理与边界

- 唯一领域错误是 `ParamError::IndexExceedsParamCount`；当前没有解析错误、类型错误或底层存储错误变体。
- `Display` 不携带请求下标或参数总数，故外层转换后仍只有与 Go 相同的固定消息。若诊断需要这些数字，应由拥有容器的实现或调用点补充上下文，而不能在空实现中猜测总数。
- `EmptyParamValues::GetParamValue` 对所有 `usize` 值行为相同，没有 panic、默认值或隐式 NULL。
- 静态与会话实现使用切片安全索引 `get`，避免数组越界 panic，并统一通过
  `ErrParamIndexExceedParamCounts` 映射失败。
- `ParamMarker::GetUserVar` 和规划器 `toParamMarker` 会把类型化错误转为通用字符串错误；因此需要判等哨兵的调用点应在转换前处理 `ParamError`。
- `ParamError` 目前是单变体枚举；新增错误变体时必须同步审查 `Display`，否则不同错误会错误地显示为越界消息。

## 并发与资源生命周期

本文件没有锁、任务、通道、I/O、堆分配或析构协议。空静态实例只含零大小值，可被任意线程共享；方法仅取 `&self` 且不修改状态。

`ParamValues` 本身没有声明 `Send` 或 `Sync` supertrait，因此“所有实现均可跨线程共享”不是接口保证。是否能跨线程由具体实现及其上层 trait 对象约束决定。当前读取 API 也不提供借用 Datum 的生命周期：生产实现克隆并返回拥有值，避免返回引用与会话/快照容器生命周期耦合，代价是每次读取可能产生 Datum 内部数据的克隆成本。

参数的有效期由实现者控制：在线上下文观察当前会话执行参数，静态上下文保存构造时快照；`EmptyParamValues` 为进程期静态实例。调用者不得假设不同实现具有相同的更新可见性。

## 与 Go 版本的对应关系

Go 原型是 [`pkg/expression/exprctx/param.go`](./param.go)：

- Go `ParamValues.GetParamValue(idx int) (types.Datum, error)` 对应 Rust
  `GetParamValue(idx: usize) -> Result<Datum, ParamError>`。Rust 将任意 `error` 收窄为专用错误类型，并排除了负下标输入。
- Go `ErrParamIndexExceedParamCounts = errors.New(...)` 对应 Rust 的
  `ParamError::IndexExceedsParamCount`、Go 风格常量别名和独立错误文本常量；文本完全一致。
- Go `EmptyParamValues` 是保存 `&emptyParamValues{}` 的接口变量；Rust 同时公开零大小类型
  `EmptyParamValues` 和静态值 `EMPTY_PARAM_VALUES`。两者都对任意下标返回哨兵错误。
- Go 空实现失败时返回 `types.Datum{}` 与错误；Rust `Result::Err` 不携带无效 Datum，迫使调用者先处理错误，避免误用失败路径的零值。
- Go 接口方法可由指针接收者实现；Rust trait 以 `&self` 接收并由具体上下文实现。其只读意图一致，但 Rust 的线程安全不由该 trait 自动保证。

Go 静态上下文测试
[`pkg/expression/exprstatic/evalctx_test.go`](../exprstatic/evalctx_test.go) 验证参数顺序以及输入参数表重置后快照仍保留；Rust
[`pkg/expression/exprstatic/evalctx_test.rs`](../exprstatic/evalctx_test.rs) 验证同样的值顺序、输入
`Vec` 解耦，并额外断言越界失败。Go builtin 测试还验证标量与向量 GetParam 路径把越界保留为相同哨兵错误。

## 扩展指南

- 新增参数来源时，实现 `ParamValues::GetParamValue`，用安全索引并在缺值时返回
  `ErrParamIndexExceedParamCounts`；不要返回默认 Datum 或 panic。实现应明确是动态视图还是稳定快照。
- 若新增错误类别，应为 `ParamError` 增加变体，并逐变体实现 `Display`；同步检查
  `constant::ParamMarker::GetUserVar`、`planner::toParamMarker` 等字符串化转换点是否会丢失所需结构信息。
- 若希望减少 Datum 克隆，需要重新设计返回类型和生命周期，并同时评估动态会话参数、静态快照及 trait 对象调用者；不能只把某个实现改为返回引用。
- 修改参数读取语义时，至少同步独立测试
  [`pkg/expression/exprctx/migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 的空实现哨兵用例、
  [`pkg/expression/exprstatic/evalctx_test.rs`](../exprstatic/evalctx_test.rs) 的参数列表/越界用例，以及会话上下文相关测试。测试逻辑应继续与对应 Go 测试保持一致，且不要把 Rust 测试嵌入 `param.rs`。
- 新增展示用途的调用点若允许没有参数，应显式传 `EmptyParamValues`；真正执行参数标记的路径必须传实际上下文，不能用空实现掩盖接线遗漏。
- 兼容风险集中在错误文本、错误类型判等、0 起始顺序和快照/动态可见性；性能风险主要是每次成功读取的 Datum 克隆。

## 验证依据

本说明基于以下直接证据：

- 源文件：[`pkg/expression/exprctx/param.rs`](./param.rs)，核对全部常量、枚举、trait、实现和静态值。
- crate 边界：[`pkg/expression/exprctx/Cargo.toml`](./Cargo.toml) 与
  [`pkg/expression/exprctx/lib.rs`](./lib.rs)，核对 crate 名、入口、`astersql-types` 依赖、模块私有装配及公共重导出。
- 上下文与调用点：`exprctx/context.rs` 的 `EvalContext: ParamValues` 和覆盖包装转发；
  `exprstatic/evalctx.rs`、`sessionexpr/sessionctx.rs` 的生产实现；`constant.rs` 的
  `ParamMarker::GetUserVar`；`planner/core/expression_rewriter.rs` 的 `toParamMarker`。
- Go 对照：[`pkg/expression/exprctx/param.go`](./param.go)；相关测试
  `pkg/expression/exprstatic/evalctx_test.go`、`pkg/expression/builtin_other_test.go` 和
  `pkg/expression/builtin_other_vec_test.go`。
- Rust 独立测试：`pkg/expression/exprctx/migration_aster_unit_test.rs` 的
  `empty_param_values_returns_the_go_sentinel_error`，以及
  `pkg/expression/exprstatic/evalctx_test.rs` 的 `TestParamList`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和
  1,848,419 条边；对 `param.rs ParamValues EmptyParamValues GetParamValue ParamError` 的
  `explore`、对主要符号的 `query`/文件 `node`、以及 `ParamMarker GetUserVar GetParamValue`
  的精确 `explore` 用于核对使用文件、实现者和消费链。

任务为纯文档分析，按总计划不运行 Cargo。结构验收应确认文件存在，且上述固定二级标题恰好出现 11 个；人工复核重点是所有行为结论均能回溯到列出的源码、Go 对照或独立测试，而未把接口未保证的线程安全或容器语义写成既成事实。
