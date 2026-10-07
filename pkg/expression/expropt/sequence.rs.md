# `pkg/expression/expropt/sequence.rs`

## 文件定位

本文件属于 `astersql-expression-expropt` crate，是表达式“可选求值属性”（Optional Eval Prop）体系中的序列能力边界。它不实现序列的缓存、步长、循环或持久化算法，而是定义表达式层可依赖的最小序列操作接口，并把会话层提供的 `(db, name) -> operator` 工厂注册为 `OptPropSequenceOperator`。模块由 `pkg/expression/expropt/lib.rs` 的 `mod sequence; pub use sequence::*;` 纳入并公开再导出；crate 边界和依赖见 `pkg/expression/expropt/Cargo.toml`。

会话侧入口位于 `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext`：它捕获 `Arc<C>`，构造 `SequenceOperatorProvider`，并把调用转发给 `SessionContext::sequence_operator`。因此本文件位于“表达式声明能力”与“具体会话/InfoSchema/序列表实现”之间，不拥有数据库对象本身。

## 核心职责

1. `SequenceOperator` 把一个已经解析出的序列对象抽象成三项操作：读取对象 ID、取得下一值、设置序列值。
2. `SequenceOperatorProvider` 保存线程安全、具有 `'static` 生命周期的解析闭包，按数据库名和序列名创建一次操作句柄。
3. `OptionalEvalPropProvider for SequenceOperatorProvider` 把 Provider 绑定到 `exprctx::OptPropSequenceOperator`，并通过 `as_any` 支持运行时安全 downcast。
4. `SequenceOperatorPropReader` 声明表达式对该属性键的依赖，并从任意 `OptionalEvalPropContext` 中取出正确类型的 Provider。

这里刻意只负责依赖注入和转发。权限检查、名称解析、对象不存在错误、序列值规则以及会话最近值缓存都应由调用方或具体 `SequenceOperator` 实现负责；目标文件本身没有这些策略。

## 主要符号

- `pub trait SequenceOperator`：对象安全的动态分派接口。`get_sequence_id(&self) -> i64` 读取稳定对象 ID；`get_sequence_next_val(&mut self) -> anyhow::Result<i64>` 推进并返回下一值；`set_sequence_val(&mut self, new_val: i64) -> anyhow::Result<(i64, bool)>` 返回实际生效值以及“请求值已经低于当前基线”的标志。后两项需要 `&mut self`，允许具体实现维护句柄内状态。
- `pub struct SequenceOperatorProvider`：唯一字段 `provider` 是 `Box<dyn Fn(&str, &str) -> anyhow::Result<Box<dyn SequenceOperator>> + Send + Sync>`。闭包本身可被跨线程共享；每次调用返回独立的 boxed trait object。
- `SequenceOperatorProvider::new<F>`：把满足 `Fn + Send + Sync + 'static` 的闭包擦除为字段中的 trait object，不执行名称解析。
- `SequenceOperatorProvider::call`：原样传递 `db`、`name`，返回闭包结果，不包装或吞掉错误。
- `impl exprctx::OptionalEvalPropProvider for SequenceOperatorProvider`：`Desc` 返回 `OptPropSequenceOperator.Desc()`；`as_any` 返回 `Some(self)`，供 `get_prop_provider` 做具体类型检查。
- `pub struct SequenceOperatorPropReader`：无字段的零大小 Reader。
- `impl RequireOptionalEvalProps for SequenceOperatorPropReader`：返回只含 `OptPropSequenceOperator` 位的 `OptionalEvalPropKeySet`。
- `SequenceOperatorPropReader::get_sequence_operator`：以固定键调用 `get_prop_provider::<SequenceOperatorProvider, _>`，成功后调用 `provider.call(db, name)`。

文件没有模块级常量、枚举、条件编译项或内部测试；测试独立放在 `optional_test.rs` 和 `migration_aster_unit_test.rs`。

## 执行流程

1. `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 克隆会话 `Arc`，用闭包 `move |db, name| sequence_session.sequence_operator(db, name)` 构造 `SequenceOperatorProvider`，再注册到 `OptionalEvalPropProviders`。
2. 需要序列能力的表达式组件应通过 `SequenceOperatorPropReader::required_optional_eval_props` 声明键 `OptPropSequenceOperator`，使上下文准备阶段知道该依赖。
3. 求值时调用 `get_sequence_operator(ctx, db, name)`；Reader 先从上下文固定槽位取 Provider，再验证 Provider 自描述键和具体 Rust 类型。
4. Reader 调用 Provider；Provider 将数据库名、序列名原样交给会话闭包。会话层负责查找实际序列并返回 `Box<dyn SequenceOperator>`，或返回查找/权限等错误。
5. 调用方再根据 SQL 语义调用 `get_sequence_id`、`get_sequence_next_val` 或 `set_sequence_val`。目标文件不缓存句柄，也不替调用方记录 `LASTVAL` 状态。

当前 Rust 接线需要如实区分两条路径：`sessionctx.rs::NewEvalContext` 已注册本 Provider，测试也能通过 Reader 取用；但 RustCodeGraph 对 `get_sequence_operator` 的 callers 查询未发现生产调用者。`pkg/expression/builtin_info.rs` 中现有 `next_val`、`last_val`、`set_val` 使用独立的 `SequenceService` 接口，而非本 Reader。因而不能仅凭 Go 主链推断 Rust 序列内置函数已经通过本文件接通。

## 数据与状态

`SequenceOperatorProvider` 只拥有一个 boxed 闭包，没有数据库名、对象名或序列值缓存。数据库名与序列名采用借用的 `&str` 传入；Provider 若需长期保存它们，必须在具体闭包/对象中自行复制。返回的 `Box<dyn SequenceOperator>` 转移给调用方，其生命周期与后续可变状态由调用方持有。

`SequenceOperatorPropReader` 是零大小类型，不持有上下文或 Provider。属性注册状态存放于 `pkg/expression/expropt/optional.rs::OptionalEvalPropProviders` 的定长槽位向量中；`OptPropSequenceOperator` 是 `pkg/expression/exprctx/optional.rs` 中编号为 6 的键，对应位集合中的第 6 位。

`set_sequence_val` 返回的 `(i64, bool)` 与 Go 的 `(int64, bool, error)` 语义对齐：整数是实际结果，布尔值表示传入的新值已经低于当前 base。目标 trait 不规定低于 base 时具体实现是否保持原值、推进或执行其他策略，该行为属于具体序列实现的契约。

## 依赖与调用关系

- 上游装配：`pkg/expression/expropt/lib.rs` 声明并再导出模块；`pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 创建并注册 Provider；`SessionContext::sequence_operator` 是会话实现必须提供的窄接口。
- 属性框架：`crate::*` 带入 `RequireOptionalEvalProps`、`OptionalEvalPropContext` 和 `get_prop_provider`；`exprctx::OptPropSequenceOperator` 提供键、描述和位集合。
- 下游调用：`SequenceOperatorPropReader::get_sequence_operator -> get_prop_provider -> SequenceOperatorProvider::call -> 注入闭包 -> SessionContext::sequence_operator`。RustCodeGraph 能直接解析目标方法到 `call` 的边，以及 `NewEvalContext` 到 `SessionContext::sequence_operator` 的边；跨 trait object/闭包的其余部分由源码结构确认。
- 运行库：本文件直接使用 `anyhow::Result`；`anyhow = "1"` 由本 crate 的 `Cargo.toml` 声明。`exprctx` 通过路径依赖 `astersql-expression-exprctx` 并在 `lib.rs` 中重导出。
- 测试：`pkg/expression/expropt/optional_test.rs::verify_sequence_operator` 是最直接的 Rust 独立测试；`pkg/expression/expropt/migration_aster_unit_test.rs::registry_and_missing_reader_paths_match_go` 覆盖缺失 Provider；`pkg/expression/sessionexpr/migration_aster_unit_test.rs` 提供真实 `SessionContext` 测试适配器。Go 对照测试在 `pkg/expression/expropt/optional_test.go` 的 `OptPropSequenceOperator` 分支。

## 错误处理与边界

`get_sequence_operator` 可能在两个阶段失败。第一阶段是 `get_prop_provider`：属性未注册、Provider 描述键不匹配、或具体类型无法 downcast，都会返回带上下文的 `anyhow::Error`；缺失属性的稳定消息包含 `not exists in EvalContext`。第二阶段是 Provider 闭包自身失败，例如序列名不存在或上层拒绝访问；`call` 和 Reader 都原样透传该错误。

目标文件不捕获 panic。非法属性键、重复/错误注册等注册表不变量由 `optional.rs` 的断言维护；闭包或具体 Operator 的锁中毒、内部断言等 panic 也不会在此转换为 `Result`。目标文件同样不校验空数据库名、空序列名、`new_val` 范围、序列耗尽或溢出，这些边界必须由会话解析层和具体 Operator 处理。

Provider 返回 `Box<dyn SequenceOperator>` 而不是 `Option`，因此“找不到对象”必须表示为 `Err`，不能用空句柄表示。`get_sequence_id` 本身不返回 `Result`，具体实现若无法稳定取得 ID，需要在创建 Operator 时提前失败，或调整公共契约并同步所有实现。

## 并发与资源生命周期

Provider 闭包要求 `Send + Sync + 'static`，所以注册表可以持有捕获 `Arc` 等共享状态的长期闭包，并允许从多个线程并发调用 Provider。`SequenceOperator` trait 本身没有 `Send` 或 `Sync` 约束：创建出的单个句柄不承诺能在线程间移动或共享，且修改操作要求 `&mut self`。扩展时不要把 Provider 的线程安全错误等同于 Operator 的线程安全。

`NewEvalContext` 捕获会话 `Arc`，Provider 存活期间会延长会话对象生命周期；目标文件没有显式关闭、取消或析构协议。每次 `call` 的 Operator 归调用者独占，离开作用域后按 Rust RAII 释放。若具体实现持有事务、锁或远程资源，应在其自身类型中实现明确的释放策略，不能依赖本 Reader。

测试 `optional_test.rs::TestSequenceOperator` 使用 `Arc<Mutex<i64>>` 演示跨新建句柄共享底层序列状态，但这只是测试实现的选择，并非本 trait 强制的并发模型。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/expression/expropt/sequence.go`。Rust `SequenceOperator` 的三个方法逐项对应 Go `GetSequenceID`、`GetSequenceNextVal`、`SetSequenceVal`；Rust 将 Go 的尾部 `error` 映射为 `anyhow::Result`，并把 `SetSequenceVal` 的两个成功返回值放入元组。

Go 用命名函数类型 `type SequenceOperatorProvider func(db, name string) ...` 同时充当 Provider 和可调用值；Rust 因 trait 实现与类型擦除需要，改为持有 boxed `Fn` 的结构体，并提供 `new`/`call`。Rust 额外实现 `as_any`，用于替代 Go 的运行时类型断言并返回可诊断错误。两端 Reader 都声明相同属性键，并执行“取 Provider后再传入 db/name”的流程。

Go 的生产会话适配器 `pkg/expression/sessionexpr/sessionctx.go::sequenceOperatorProp` 使用 `util.GetSequenceByName` 解析对象，并由 `sequenceOperator` 转发到 `SequenceTable`。Rust 对应会话 trait 和 Provider 注册已存在，但目标文件不包含 Go 中的具体 `SequenceTable` 适配器；目前只能从 `SessionContext::sequence_operator` 的实现获得它。并且 Rust 现有 `builtin_info.rs` 序列函数走 `SequenceService`，所以生产主链是否最终统一到本接口仍是未接线/未验证边界。

`pkg/expression/expropt/optional_test.go` 与 Rust `optional_test.rs::verify_sequence_operator` 对齐验证：`db1/name1` 参数不变、无 Provider 时失败、Provider 可返回同一语义的 Operator，以及 Provider 错误经直接调用和 Reader 调用均不被改写。

## 扩展指南

- 新增 Operator 能力时，先修改 `SequenceOperator`，同步所有生产/测试实现以及 Go 对照接口；不要为让调用方编译而提供无行为的默认实现。评估对象安全性以及是否仍需要 `&mut self`。
- 修改解析输入时，优先扩展 `SequenceOperatorProvider::call` 和 `SessionContext::sequence_operator` 的共同契约，并同步 `NewEvalContext` 闭包；数据库名/序列名的大小写、当前库补全和权限边界应只由一个明确层级负责。
- 改变属性键时，必须同步 `exprctx::OptPropSequenceOperator`、描述表顺序、`required_optional_eval_props`、注册表测试以及 Go 常量顺序；键位变化会影响所有属性集合。
- 若要把 Rust `NEXTVAL/LASTVAL/SETVAL` 接到此 Provider，应先明确 `SequenceService` 与 `SequenceOperator` 的职责映射，尤其是权限校验和 `SessionInfo::sequence_state` 更新，避免重复推进序列或改变 `LASTVAL` 语义。
- 测试应继续放在独立文件。至少扩展 `pkg/expression/expropt/optional_test.rs::verify_sequence_operator`，并在涉及会话装配时同步 `pkg/expression/sessionexpr/migration_aster_unit_test.rs`；Go 语义变更还应核对 `pkg/expression/expropt/optional_test.go`。关注错误原样透传、低于 base 的布尔语义、并发闭包捕获和 Operator 非 `Send` 边界。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/expropt` 确认目标 Rust/Go 文件及独立测试均已索引。
- 源码与符号：`rustcodegraph node --file pkg/expression/expropt/sequence.rs` 完整读取 88 行；`query SequenceOperator`、`query SequenceOperatorProvider`、`query SequenceOperatorPropReader` 核对公开 trait、结构体、构造/调用方法和 Reader。
- 调用图：`callees get_sequence_operator` 得到 `get_sequence_operator -> call`；`callers get_sequence_operator` 未返回生产调用者；`node sequence_operator` 显示 `SessionContext::sequence_operator <- NewEvalContext`。
- 装配与公共框架：已核对 `pkg/expression/expropt/lib.rs`、`pkg/expression/expropt/optional.rs`、`pkg/expression/exprctx/optional.rs`、`pkg/expression/sessionexpr/sessionctx.rs` 和 `pkg/expression/expropt/Cargo.toml`。
- Go 对照：已核对 `pkg/expression/expropt/sequence.go`、`pkg/expression/expropt/optional_test.go`、`pkg/expression/exprctx/optional.go` 和 `pkg/expression/sessionexpr/sessionctx.go`。
- Rust 测试证据：已核对 `pkg/expression/expropt/optional_test.rs::verify_sequence_operator`、`pkg/expression/expropt/migration_aster_unit_test.rs::registry_and_missing_reader_paths_match_go`，以及 `pkg/expression/sessionexpr/migration_aster_unit_test.rs` 的 `SessionContext::sequence_operator` 测试实现。
- 应用链边界：`rustcodegraph node next_val/last_val/set_val` 表明 `pkg/expression/builtin_info.rs` 当前依赖 `SequenceService`；这支持“本 Provider 已注册但未证明被这些生产内核消费”的限定结论。
- 本任务是纯文档分析，按计划未运行 Cargo；交付前仅执行任务指定的 11 章节结构验证，并人工复核没有把未解析的调用边写成已支持行为。
