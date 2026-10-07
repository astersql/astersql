# `pkg/expression/expropt/optional.rs`

## 文件定位

本文件属于 Cargo crate `astersql-expression-expropt`（`pkg/expression/expropt/Cargo.toml`），由 `pkg/expression/expropt/lib.rs` 的私有模块 `optional` 纳入并通过 `pub use optional::*` 对外再导出。它位于表达式求值上下文与具体可选能力之间：`exprctx` 定义属性键、描述、位集合和 Provider 基础 trait，本文件负责保存 Provider、声明 Reader 所需属性，并按具体 Rust 类型取回 Provider；`current_user.rs`、`sessionvars.rs`、`infoschema.rs` 等文件在此基础上实现具体属性。

应用侧有两类直接装配者。`pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 从活动会话构造并注册全部十种属性，最后以 `IsFull()` 断言完整；`pkg/expression/exprstatic/evalctx.rs::WithOptionalProperty` 为静态求值上下文按调用方传入的 Provider 构造注册表。表达式 Reader 随后通过本文件的 `get_prop_provider` 读取能力，而不是直接依赖具体 session 实现。

## 核心职责

1. `RequireOptionalEvalProps` 统一表达式 Reader 的依赖声明，使上层可以在执行前汇总所需属性。`pkg/expression/builtin.rs` 会调用该 trait；测试断言上下文还会用声明集合审计实际访问（`pkg/expression/context.rs::assertionEvalContext::GetOptionalPropProvider`）。
2. `OptionalEvalPropContext` 将完整 `exprctx::EvalContext` 缩窄为 Reader 真正需要的接口，并通过 blanket impl 兼容所有现有求值上下文；独立测试可以只实现这个窄接口。
3. `OptionalEvalPropProviders` 提供按 `OptionalEvalPropKey.0` 索引的注册表，支持存在性检查、读取、覆盖式写入以及已注册键集合汇总。
4. `get_prop_provider<T, C>` 将类型擦除的 `dyn OptionalEvalPropProvider` 恢复为调用方要求的具体 `T`，并区分缺失、Provider 自描述键不一致、具体类型不匹配三种失败。

本文件不定义十种属性的业务值，也不执行用户、元数据、KV、SQL 等操作；这些行为由相邻 Provider/Reader 文件承担。

## 主要符号

- `pub trait RequireOptionalEvalProps`：唯一方法 `required_optional_eval_props(&self) -> OptionalEvalPropKeySet`。空集合表示不要求可选属性；具体 Reader 通常返回某个键的 `AsPropKeySet()`。
- `pub trait OptionalEvalPropContext`：要求 `get_optional_prop_provider(key)`，并提供默认返回 `None` 的 `location_name()`。后者供 `SessionVarsPropReader` 在断言模式核对上下文、会话变量与语句时区。
- `impl<T: exprctx::EvalContext + ?Sized> OptionalEvalPropContext for T`：把完整上下文的 `GetOptionalPropProvider` 和 `Location` 转发到窄接口；时区被转换为字符串。
- `pub struct OptionalEvalPropProviders`：内部字段是 `Vec<Option<Box<dyn OptionalEvalPropProvider>>>`。字段私有，调用方只能通过公开方法维护槽位。
- `new` / `Default::default`：创建恰有 `exprctx::OPT_PROPS_CNT` 个空槽位的注册表。
- `contains`：等价于 `get(key).is_some()`，因此也会执行读取时的键一致性断言。
- `get`：越界或空槽返回 `None`；非空时通过 `intest::Assert` 检查 `provider.Desc().Key() == key`，再返回借用的 trait object。
- `add`：读取 Provider 自描述键，先用 `intest::Assert`、再用普通 `assert!` 保证键小于 `OPT_PROPS_CNT`，然后将 `Box` 写入对应槽位。已有值会被替换；本方法自身不禁止重复注册。
- `prop_key_set`：遍历所有非空槽位，从空位集合开始反复调用 `OptionalEvalPropKeySet::Add`，得到当前注册键位图。
- `get_prop_provider<T, C>`：要求 `T: OptionalEvalPropProvider + Any`、`C: OptionalEvalPropContext`，返回与上下文同生命周期的 `&T`。

## 执行流程

典型会话求值路径如下：

1. `sessionexpr::NewEvalContext` 创建空 `OptionalEvalPropProviders`，把会话状态封装为十种具体 Provider，并经其私有 `set_optional_prop` 逐个加入。该包装方法在调用 `add` 前拒绝重复键。
2. 某个具体 Reader（例如 `CurrentUserPropReader::current_user`）通过自己的 `RequireOptionalEvalProps` 实现声明键，再调用 `get_prop_provider(ctx, OptPropCurrentUser)`。
3. `get_prop_provider` 先调用 `OptionalEvalPropContext::get_optional_prop_provider`。完整 `EvalContext` 走 blanket impl；测试的 `MockEvalCtx` 则直接委托给其注册表。
4. 注册表 `get` 以键的数值字段索引槽位。越界、空槽产生 `None`；非空槽在断言模式检查 Provider 自描述键与槽位一致。
5. 通用读取函数再次显式比较 `provider.Desc().Key()` 与请求键，之后调用 `as_any()` 和 `downcast_ref::<T>()`。成功则借用返回原 Provider，失败则生成带类型名和键名的错误。
6. Reader 调用具体 Provider 的业务方法或闭包，取得当前用户、会话变量、InfoSchema 等值。业务调用产生的错误由具体 Reader/Provider 继续传播，不在本文件吞掉。

静态求值路径与此相同，只是由 `exprstatic::WithOptionalProperty` 接收 Provider 列表并组装注册表；空列表和属性子集都是允许的。

## 数据与状态

键及有效宽度来自 `pkg/expression/exprctx/optional.rs`：`OptionalEvalPropKey(pub usize)` 当前定义索引 `0..9`，`OPT_PROPS_CNT` 为 10，`OptionalEvalPropKeySet(pub u64)` 是位集合。注册表在 `new` 中将长度初始化为该常量，所以合法键可直接 O(1) 索引；`prop_key_set` 是 O(`OPT_PROPS_CNT`) 的全槽扫描。

每个槽位拥有一个 `Box<dyn OptionalEvalPropProvider>`。`add` 转移所有权，`get` 和 `get_prop_provider` 只返回绑定于注册表/上下文生命周期的共享借用，不复制 Provider。覆盖同一键会立即丢弃旧 `Box`；正常会话路径通过 `set_optional_prop` 预先拒绝重复，但公共 `add` 明确保留替换语义。

注册表本身没有缓存派生位集合：每次 `prop_key_set` 都根据当前非空槽位重算，因此不会出现写入后位图失效。`location_name` 也不存储状态，只在调用时从完整上下文读取时区字符串。

## 依赖与调用关系

直接 Rust 依赖为 `std::any::Any`、`anyhow::{Result, anyhow}`，以及 crate 根再导出的 `exprctx` 与 `intest`。Cargo 中对应的关键依赖是 `anyhow = "1"`、`astersql-expression-exprctx` 和 `astersql-util-intest`；其余 expropt 依赖服务于具体 Provider，并非本文件直接使用。

上游装配与访问者包括：

- `pkg/expression/sessionexpr/sessionctx.rs::{NewEvalContext, set_optional_prop, GetOptionalPropSet, GetOptionalPropProvider}`：构造完整注册表并向表达式层暴露它。
- `pkg/expression/exprstatic/evalctx.rs::{WithOptionalProperty, NewEvalContext, GetOptionalPropSet, GetOptionalPropProvider}`：构造可为空或为子集的静态上下文，并用 `Arc` 共享注册表。
- `pkg/expression/context.rs::assertionEvalContext::GetOptionalPropProvider`：在测试断言层先检查 builtin 是否声明了 required/allowed 属性，再转发实际读取。
- `pkg/expression/builtin.rs::RequiredOptionalEvalProps`：消费 `RequireOptionalEvalProps` 声明。

下游直接调用 `get_prop_provider` 的 Reader 位于 `current_user.rs`、`sessionvars.rs`、`sessioncontext.rs`、`infoschema.rs`、`kvstore.rs`、`sqlexec.rs`、`sequence.rs`、`advisory_lock.rs`、`ddlowner.rs` 和 `priv.rs`。它们负责将类型安全的 Provider 引用转换为各自业务结果。

## 错误处理与边界

- 请求键越界：`OptionalEvalPropProviders::get` 使用 `Vec::get`，返回 `None`，随后 `get_prop_provider` 报 `optional property: '…' not exists in EvalContext`。Rust 键底层是 `usize`，因此不存在 Go 的负键分支，但仍可构造大于等于计数的键。
- 空槽：与越界相同，对注册表查询返回 `None`；Reader 得到明确的缺失属性错误。`optional_test.rs::assert_missing` 验证该错误片段。
- 写入键越界：`add` 的普通 `assert!` 无条件 panic；不能把不可信键直接交给该 API。
- 槽位与自描述键不一致：正常通过 `add` 不会形成这种状态，因为槽位就是由 `Desc().Key()` 选择且字段私有；`get` 仍保留 `intest::Assert`，通用读取函数还返回显式 `anyhow` 错误作为纵深校验。
- 类型不匹配或 `as_any()` 返回 `None`：`get_prop_provider` 返回包含 `type_name::<T>()` 和键的转换错误。后者意味着允许借用非 `'static` 状态、使用 Provider trait 默认 `as_any` 的实现不能通过这个泛型读取入口，必须由专用路径处理或实现可下转的 Provider。
- 重复键：公共 `add` 覆盖旧值而不报错；完整会话构造器的 `set_optional_prop` 额外将重复视为 panic。扩展代码不能假定所有调用方都有重复保护。
- `RequireOptionalEvalProps` 只是声明协议；遗漏声明不会由本文件阻止。测试用 `assertionEvalContext` 才在属性访问点审计 required/allowed 集合。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务，也没有内部可变性。构造和 `add` 需要 `&mut self`，读取只需 `&self`，所以同一注册表的并发修改不在 API 模型内。

Provider 的生命周期由槽位中的 `Box` 管理，注册表销毁或覆盖槽位时释放。返回的 `&dyn Provider` / `&T` 不能活过上下文。具体 Provider 可以自行通过 `Arc`、`Mutex` 或原子量管理共享状态，例如 `optional_test.rs` 的咨询锁桩使用 `Mutex<Vec<String>>`、DDL Owner 桩使用 `AtomicBool`；这些同步保证属于具体 Provider，不属于注册表。

`exprstatic::EvalCtxState` 通过 `Arc<OptionalEvalPropProviders>` 在克隆上下文间共享只读注册表。需要注意基础 `OptionalEvalPropProvider` trait 本身没有全局 `Send + Sync` 约束；是否能够跨线程共享取决于实际 Provider 和承载上下文的类型约束，不能仅凭本文件宣称线程安全。

## 与 Go 版本的对应关系

Go 对照为 `pkg/expression/expropt/optional.go`，测试为 `optional_test.go`。核心语义对应如下：

- Go `RequireOptionalEvalProps.RequiredOptionalEvalProps` 对应 Rust trait 的 snake_case 方法。
- Go `[exprctx.OptPropsCnt]exprctx.OptionalEvalPropProvider` 是编译期定长数组；Rust 用创建时填满 `None` 的 `Vec` 模拟固定槽数，长度不向外暴露或改变。
- Go `Contains`、`Get`、`Add`、`PropKeySet` 分别对应 Rust 同名 snake_case 方法。两边都按描述键定位，并从所有非空 Provider 汇总位集合。
- Go `Get` 明确检查负数和上界；Rust `usize` 消除了负数表示，仅用 `Vec::get` 处理上界。
- Go `Add` 在 `intest.AssertFunc` 中按每个键断言具体 Provider 类型；Rust `add` 只校验键范围，具体类型在 `get_prop_provider` 的 `Any::downcast_ref` 阶段校验。因此错误暴露时机不同，但正常 Reader 路径仍保持键/类型契约。
- Go 泛型读取先在断言模式检查泛型零值描述键，再进行运行时类型断言；Rust 无相同零值约束，改为取得 Provider 后比较实际描述键，并安全下转。
- 两边对缺失属性和类型转换失败保留不同错误分支。Rust 另外显式返回“Provider 键与请求键不一致”错误。

`optional_test.go::TestOptionalEvalPropProviders` 与 Rust `optional_test.rs::TestOptionalEvalPropProviders` 都按键顺序覆盖十种属性，验证注册前缺失、注册后可读、Reader 声明键一致以及最终集合为满集。Rust 测试按独立文件组织，符合生产逻辑与测试逻辑分离要求。

## 扩展指南

新增可选属性不能只改本文件。至少应同步：

1. 在 `pkg/expression/exprctx/optional.rs` 增加键、描述项并调整 `OPT_PROPS_CNT`，同时核对位集合仍不超过 64 位；Go 的 `pkg/expression/exprctx/optional.go` 也必须保持编号与描述顺序一致。
2. 在 expropt 独立源文件定义具体 Provider/Reader；Provider 若要通过 `get_prop_provider` 读取，必须正确实现 `Desc` 和返回 `Some(self)` 的 `as_any`。Reader 应实现 `RequireOptionalEvalProps`。
3. 在 `pkg/expression/expropt/lib.rs` 接入并导出模块，在 `sessionexpr::NewEvalContext` 注册会话 Provider；若静态上下文也需要该能力，为其调用 `WithOptionalProperty`。
4. 扩展 `pkg/expression/expropt/optional_test.rs` 和 Go 的 `optional_test.go`，覆盖缺失、成功值、键集合、类型/业务错误传播；测试必须继续放在独立测试文件，不嵌入 `optional.rs`。
5. 若希望禁止覆盖，应在所有装配入口统一做重复检查，而不是只依赖会话层的 `set_optional_prop`。若更改 Provider trait 的 `Send + Sync` 或借用能力，应评估静态上下文的 `Arc` 共享和 `Any` 的 `'static` 限制。

兼容风险集中在键编号、描述数组顺序和位图宽度；改变它们会影响声明集合与所有上下文装配。性能上当前键读取为 O(1)，集合汇总为 O(N) 且 N=10；增加属性通常影响很小，但频繁调用 `prop_key_set` 时仍应避免引入额外分配或改成无界扫描。

## 验证依据

- RustCodeGraph 索引状态：项目共索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/expropt` 确认目标及相邻实现、Go 对照和独立测试均在图中。
- RustCodeGraph 源码节点：`pkg/expression/expropt/optional.rs`（完整 157 行）、`pkg/expression/exprctx/optional.rs`、`pkg/expression/sessionexpr/sessionctx.rs`、`pkg/expression/exprstatic/evalctx.rs`、`pkg/expression/context.rs`、`pkg/expression/expropt/current_user.rs`、`pkg/expression/expropt/sessionvars.rs`。
- RustCodeGraph/文本调用证据：具体 Reader 从十个相邻属性文件调用 `get_prop_provider`；会话和静态上下文调用 `new`、`add`、`get`、`prop_key_set`；builtin 和断言上下文消费属性声明。
- crate 与模块证据：`pkg/expression/expropt/Cargo.toml`、`pkg/expression/expropt/lib.rs`。
- Go 语义证据：`pkg/expression/expropt/optional.go`、`pkg/expression/expropt/optional_test.go`。
- Rust 测试证据：`pkg/expression/expropt/optional_test.rs` 的 `TestOptionalEvalPropProviders`、`assert_before_add`、`assert_after_add` 及十种 `verify_*`；另参考 `migration_aster_unit_test.rs` 的注册表使用。依任务约束未运行 Cargo。
- 人工复核结论：文档覆盖了文件存在原因、注册与读取流程、真实上下游、三类读取错误、覆盖/越界边界、资源生命周期、Go 差异和安全扩展位置；未把测试桩行为描述为生产能力。
