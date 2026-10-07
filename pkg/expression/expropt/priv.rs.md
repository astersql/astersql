# `pkg/expression/expropt/priv.rs`

## 文件定位

本文件属于 `astersql-expression-expropt` crate，是表达式“可选求值属性”机制中的权限适配层。本文对应源码为 [`priv.rs`](./priv.rs)。crate 入口 `pkg/expression/expropt/lib.rs` 通过 `#[path = "priv.rs"] mod priv_provider` 装入本模块并公开再导出其符号；`pkg/expression/expropt/Cargo.toml` 表明它直接依赖 `astersql-expression-exprctx`（属性描述与 Provider trait）、`astersql-parser-mysql`（权限枚举）和 `anyhow`（Reader 取值错误）。

它不实现权限规则，也不保存用户、角色或授权表。它把权限能力抽象为 `PrivilegeChecker`，再以 `OptPropPrivilegeChecker` 这一可选属性注入求值上下文，使表达式代码不必依赖会话或权限子系统的具体类型。当前 Rust 生产调用链可在 `pkg/expression/sessionexpr/sessionctx.rs` 和 `pkg/expression/extension.rs` 中看到。

## 核心职责

1. `PrivilegeChecker` 统一静态权限与动态权限两类查询接口，并要求实现者满足 `Send + Sync`，以便经共享求值上下文安全传递。
2. `PrivilegeCheckerProvider` 封装一个按需返回 `Arc<dyn PrivilegeChecker>` 的闭包，并把自身登记为 `OptPropPrivilegeChecker` 的 Provider。
3. `PrivilegeCheckerPropReader` 声明表达式依赖的属性键，并通过公共 `get_prop_provider` 完成存在性、键一致性和具体 Provider 类型校验。
4. Reader 返回 checker 后，是否允许访问完全由 checker 实现决定；本文件不缓存结果、不解释权限位，也不把拒绝转换成 SQL 错误。

## 主要符号

- `pub trait PrivilegeChecker: Send + Sync`（`priv.rs:26`）：权限能力边界。`request_verification(db, table, column, privilege)` 查询库/表/列范围的 `mysql::PrivilegeType`；空字符串的含义由上游调用约定和具体实现解释。`request_dynamic_verification(privilege_name, grantable)` 查询按名称标识的动态权限，并把“是否要求可转授”作为显式参数。
- `pub struct PrivilegeCheckerProvider`（`priv.rs:41`）：持有 `Box<dyn Fn() -> Arc<dyn PrivilegeChecker> + Send + Sync>`。`new` 接收 `'static` 闭包；`call` 每次调用闭包，不在 Provider 内缓存 checker。
- `impl exprctx::OptionalEvalPropProvider for PrivilegeCheckerProvider`（`priv.rs:59`）：`Desc` 固定返回 `exprctx::OptPropPrivilegeChecker.Desc()`；`as_any` 暴露 `Any` 视图，供 `get_prop_provider` 安全 downcast。
- `pub struct PrivilegeCheckerPropReader`（`priv.rs:70`）：无字段的零大小 Reader，可直接以 `PrivilegeCheckerPropReader` 值使用。
- `impl RequireOptionalEvalProps for PrivilegeCheckerPropReader`（`priv.rs:72`）：`required_optional_eval_props` 只返回 `OptPropPrivilegeChecker` 对应的单键集合，供表达式汇总所需属性。
- `get_privilege_checker`（`priv.rs:80`）：接受任意 `OptionalEvalPropContext`，取出类型为 `PrivilegeCheckerProvider` 的指定属性，调用 Provider 并返回 `Arc<dyn PrivilegeChecker>`。

本文件没有模块级常量、枚举、条件编译项或内部测试模块；测试位于独立文件中。

## 执行流程

生产主链如下：

1. `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 创建 `OptionalEvalPropProviders`，用捕获会话 `Arc` 的闭包构造 `PrivilegeCheckerProvider`，并注册到固定键槽；注册完成后断言完整会话上下文的可选属性集合为满集。
2. Provider 闭包每次构造一个 `ContextPrivilegeChecker`。其实现读取当前会话的权限管理器与活动角色；没有权限管理器时按该适配器的既有语义放行，否则转发到权限管理器。
3. 表达式对象通过 `PrivilegeCheckerPropReader::required_optional_eval_props` 声明依赖。当前明确的 Rust 生产消费者是 `pkg/expression/extension.rs`：`extensionFuncClass::getFunction` 在构建期读取 checker 并检查动态权限，`extensionFuncSig::evaluate_context` 在每次求值前再次读取并检查。
4. `get_privilege_checker` 调用 `optional.rs::get_prop_provider`。只有键存在、自描述键匹配且动态类型确为 `PrivilegeCheckerProvider` 时才调用 `provider.call()`。
5. `extension.rs::checkPrivileges` 遍历扩展函数声明的动态权限，以 `grantable = false` 调用 checker；任一项被拒绝即生成访问拒绝错误。RustCodeGraph 还显示 `evaluate_context` 的调用者为 `evalString` 与 `evalInt`。

## 数据与状态

本文件自身只有两类持有状态：Provider 内的闭包，以及调用闭包后返回的 `Arc<dyn PrivilegeChecker>`。闭包可以捕获会话等外部状态，但状态所有权和更新策略由构造者负责。Reader 无字段，不保存上下文、checker 或检查结果。

属性身份由 `exprctx::OptPropPrivilegeChecker` 唯一确定。Provider 的 `Desc`、Reader 的所需键集合以及 `get_privilege_checker` 请求的键三处必须保持一致；公共注册表还会核对 Provider 自描述键。checker 使用 `Arc` 返回，因此一次读取所得对象可跨后续调用共享；Provider 是否每次返回同一对象并非本文件强制的不变量，测试中的固定对象只是合法用法之一。

## 依赖与调用关系

- 上游注册：`pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 构造并加入 `PrivilegeCheckerProvider`；`ContextPrivilegeChecker` 把无角色参数的表达式接口适配到需要活动角色的会话权限管理器接口。
- 上游消费：`pkg/expression/extension.rs::extensionFuncClass::getFunction` 与 `extensionFuncSig::evaluate_context` 调用 Reader；同文件 `checkPrivileges` 调用 `request_dynamic_verification`。当前仓库 Rust 搜索未发现其他生产文件调用 `get_privilege_checker`。
- 下游公共机制：`pkg/expression/expropt/optional.rs::get_prop_provider` 提供缺失、键不符和类型不符的校验；`OptionalEvalPropProviders` 负责定长键槽注册。
- 类型依赖：`std::sync::Arc` 管理 checker 共享所有权；`mysql::PrivilegeType` 是静态权限类型；`exprctx::{OptionalEvalPropProvider, OptionalEvalPropDesc, OptionalEvalPropKeySet}` 定义可选属性协议；`anyhow::Result` 承载 Reader 查找错误。
- crate 边界：`pkg/expression/expropt/Cargo.toml` 将 Go 对照包记录为 `pkg/expression/expropt`，且 `autotests = false`；独立测试由 `lib.rs` 的 `#[cfg(test)] #[path = ...]` 显式装配。

## 错误处理与边界

`PrivilegeChecker` 的两个方法只返回 `bool`，拒绝不是本文件层面的错误。调用者负责把 `false` 转成适当的 SQL 错误；例如 `extension.rs::checkPrivileges` 根据 SEM 开关生成不同的访问拒绝消息。

`get_privilege_checker` 的错误全部来自 `get_prop_provider`，包括属性未注册、Provider 自描述键与请求键不一致、以及无法 downcast 为 `PrivilegeCheckerProvider`。闭包自身返回 `Arc` 而非 `Result`，所以 Provider 调用阶段没有本地错误通道；若权限后端可能失败，需要先明确整体权限接口语义，不能仅在此处静默把失败映射为允许或拒绝。

输入字符串不在此文件校验或规范化。数据库、表、列的空值以及动态权限名大小写均原样转发；`grantable` 也不被改写。会话适配器的“无权限管理器则放行”位于 `sessionctx.rs::ContextPrivilegeChecker`，不是该抽象本身的默认实现。

## 并发与资源生命周期

`PrivilegeChecker` 和 Provider 闭包都要求 `Send + Sync`；返回值使用 `Arc`，因此接口允许跨线程共享 checker。这里没有 `Mutex`、`RwLock`、原子量、线程、任务、通道或显式释放逻辑，也不保证具体 checker 的内部无锁实现。

Provider 中的闭包与其捕获资源随 Provider 一同存活，Provider 通常归 `EvalContext` 的属性注册表所有。`call` 克隆或新建何种 checker 由闭包决定；生产会话闭包克隆会话 `Arc` 并创建轻量适配器，取得的 checker 又持有该会话 `Arc`，直到最后一个 checker 句柄释放。未见本文件形成反向引用，因此本地证据没有显示引用环。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/expropt/priv.go`。两版都包含同名概念：两方法的 `PrivilegeChecker`、绑定 `OptPropPrivilegeChecker` 的 Provider、声明单键依赖的 Reader，以及“先按类型取 Provider、再调用 Provider”的读取流程。`pkg/expression/expropt/optional_test.go` 的对应分支验证缺失时返回错误、注册后返回同一 mock checker，并验证可直接调用 Provider。

Rust 为适应所有权和线程边界做了类型层面的改写：Go Provider 是函数类型 `func() PrivilegeChecker`，Rust 是持有 boxed 闭包的结构体；Go 接口值对应 Rust 的 `Arc<dyn PrivilegeChecker>`；Go 泛型类型断言对应 Rust 的 `Any` downcast；Rust Reader 接受更窄的 `OptionalEvalPropContext`，完整 `exprctx::EvalContext` 通过 blanket impl 自动兼容。行为意图未简化：两种权限检查、固定属性键、缺失错误及按需调用均保留。

当前移植覆盖并不意味着 Go 的所有消费者都已迁移。Go 搜索显示 `pkg/expression/builtin_info.go` 多处使用此 Reader，而当前 Rust 生产搜索只确认 `pkg/expression/extension.rs` 使用它；新增或移植其他内建函数时应按对应 Go 调用点逐项接线和测试，不能据此文档宣称所有 Go 权限表达式均已接入 Rust。

## 扩展指南

- 新增权限查询维度时，先判断能否用现有 `(db, table, column, PrivilegeType)` 或动态权限接口表达。若必须改 trait，需要同步修改所有 Rust 实现（至少会话适配器和测试桩）、调用者及 Go 对照语义；这是破坏 trait 实现面的变更。
- 新增消费者时，在表达式结构的 `RequireOptionalEvalProps` 汇总中并入 Reader 的键集合，并在实际求值前调用 `get_privilege_checker`。不能只读取 checker 而漏报属性依赖，否则静态/裁剪后的求值上下文可能缺少该属性。
- 改 Provider 或 Reader 的键时，必须同步 `Desc`、`required_optional_eval_props` 和 `get_prop_provider` 的请求键，并补测错误分支；三者不一致会在注册表断言或 Reader 键校验处失败。
- 需要保持 checker 身份或复用昂贵状态时，应在构造闭包外创建 `Arc` 并在闭包内克隆；不要假设 `PrivilegeCheckerProvider::call` 自带缓存。
- 测试应继续放在独立文件，优先扩展 `pkg/expression/expropt/optional_test.rs` 或 `migration_aster_unit_test.rs`；会话注册/角色转发行为应扩展 `pkg/expression/sessionexpr/sessionctx_test.rs`，具体表达式拒绝行为应在消费者自己的独立测试中覆盖。
- 兼容风险集中在 Go/Rust 接口语义、动态权限名与 `grantable` 传递；性能风险集中在求值热路径反复构造 checker。若改变生产闭包策略，应测量而不是把缓存假设写进接口。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件节点完整显示 `priv.rs:1-90`。
- RustCodeGraph 符号查询：确认 `PrivilegeChecker`、`PrivilegeCheckerProvider`、`PrivilegeCheckerPropReader`、`get_privilege_checker` 及两种请求方法；`get_privilege_checker` 的下游边指向 `PrivilegeCheckerProvider::call`，测试调用边指向 `migration_aster_unit_test.rs`。
- RustCodeGraph 流程查询：`extension.rs::checkPrivileges` 调用 `request_dynamic_verification`，调用者为 `getFunction` 和 `evaluate_context`；`evaluate_context` 的调用者为 `evalString` 与 `evalInt`；`sessionctx.rs::NewEvalContext` 实例化 `ContextPrivilegeChecker` 并注册 Provider。
- 阅读的生产文件：`pkg/expression/expropt/priv.rs`、`lib.rs`、`optional.rs`、`Cargo.toml`、`pkg/expression/sessionexpr/sessionctx.rs`、`pkg/expression/extension.rs`。
- 阅读的 Go 对照：`pkg/expression/expropt/priv.go`、`optional_test.go`、`pkg/expression/sessionexpr/sessionctx.go`、`pkg/expression/extension.go`；并用仓库搜索核对 `pkg/expression/builtin_info.go` 的 Go 消费点。
- 阅读的独立 Rust 测试：`pkg/expression/expropt/optional_test.rs`、`migration_aster_unit_test.rs`、`pkg/expression/sessionexpr/sessionctx_test.rs`。覆盖缺失 Reader 路径、属性键集合、同一 checker 的 `Arc` 身份、静态/动态请求转发，以及会话权限启用前后的行为。
- 本任务是纯文档分析，未运行 Cargo。交付前另运行任务规定的 11 章节结构命令，并人工复核仅新增本说明文件、不改源码/Cargo/Go/总计划。
