# `pkg/expression/exprctx/optional.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-expression-exprctx`，由同目录 `lib.rs` 以 `mod optional; pub use optional::*;` 装配并公开。它位于表达式上下文的基础层：定义“某个表达式求值需要哪些会话能力”的键、描述、提供者接口和紧凑集合，但不保存具体会话对象，也不执行 SQL。具体 Provider 的注册和读取分别由 `pkg/expression/sessionexpr/sessionctx.rs`、`pkg/expression/exprstatic/evalctx.rs` 与 `pkg/expression/expropt/optional.rs` 完成。

`pkg/expression/exprctx/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/expression/exprctx`；本文件的直接 Go 对照是 `pkg/expression/exprctx/optional.go`。

## 核心职责

1. 用 `OptionalEvalPropKey` 为十类可选求值能力分配稳定的、从 0 开始的整数编号，例如当前用户、会话变量、InfoSchema、KV 存储、内部 SQL 执行器和权限检查器。
2. 用 `OptionalEvalPropKeySet(u64)` 表示能力集合，提供纯值式 `Add`、`Remove`、`Contains`、`IsEmpty` 和 `IsFull` 运算。
3. 用 `OPTIONAL_PROPERTY_DESC_LIST` 建立“键编号 -> 静态描述”的固定注册表，并由 `validateOptionalProperties` 检查位宽及下标对应关系。
4. 定义最小化的 `OptionalEvalPropProvider` trait，使具体 Provider 能声明自身对应的属性描述；`as_any` 为拥有 `'static` 状态的实现提供可选的安全类型擦除入口，同时不强迫借用型 Provider 满足 `'static`。

该文件只定义标识与契约，不决定某个表达式实际需要什么属性，也不负责缺失 Provider 的业务错误。前者由各表达式/内置函数返回的 `RequiredOptionalEvalProps` 决定，后者由 `pkg/expression/expropt/optional.rs::get_prop_provider` 处理。

## 主要符号

- `OptionalEvalPropKey(pub usize)`：公开元组结构，内部数值同时是描述数组下标和位图位号。十个关联常量及同名包级常量覆盖 `0..OPT_PROPS_CNT`。
- `OPT_PROPS_CNT` / `OptPropsCnt`：当前均为 10；前者是 Rust 风格公开常量，后者用于与 Go 导出名兼容。
- `ALL_OPT_PROPS_MASK`：低 `OPT_PROPS_CNT` 位为 1 的私有掩码，限定空集/满集判定的有效位宽。
- `OptionalEvalPropKey::AsPropKeySet`：把合法位号变为单元素集合；位号达到 `u64::BITS` 时返回空集合，避免非法移位。
- `OptionalEvalPropKey::Desc`：按 `self.0` 直接索引静态描述表并返回静态引用；调用者必须保证键在已注册范围内。
- `Display for OptionalEvalPropKey`：合法键输出描述字符串；`self.0 >= OPT_PROPS_CNT` 时输出 `UnknownOptionalEvalPropKey(n)`，不会调用会越界的 `Desc`。
- `OptionalEvalPropDesc { key, str }`：字段私有，`Key()` 公开键；字符串通过 `Display` 间接使用。
- `OptionalEvalPropProvider`：要求实现 `Desc() -> &'static OptionalEvalPropDesc`；默认 `as_any()` 返回 `None`。
- `OPTIONAL_PROPERTY_DESC_LIST`：长度在类型上固定为 `OPT_PROPS_CNT`，数组顺序必须与键编号一致。
- `validateOptionalProperties`：断言属性数不超过 64，并逐项断言 `desc.Key().0 == index`。
- `OptionalEvalPropKeySet(pub u64)`：可复制的公开位图；`Add`/`Remove`/`Contains` 对 `key.0 >= OPT_PROPS_CNT` 采用不修改/返回 false 的防御行为。

## 执行流程

典型主链如下：

1. 内置表达式声明依赖。例如 `pkg/expression/builtin_inference.rs::RequiredOptionalEvalProps` 调用 `OptPropSessionContext.AsPropKeySet()`，把会话上下文需求编码为一个位。
2. 会话求值上下文在构造阶段创建具体 Provider；`pkg/expression/sessionexpr/sessionctx.rs::set_optional_prop` 从 `prop.Desc().Key()` 取得键，拒绝重复后交给 `OptionalEvalPropProviders::add`。静态求值上下文在 `pkg/expression/exprstatic/evalctx.rs` 中使用同一种注册表。
3. `exprctx::EvalContext`（`pkg/expression/exprctx/context.rs`）通过 `GetOptionalPropSet` 和 `GetOptionalPropProvider` 暴露能力集合与按键查询接口。
4. `pkg/expression/expropt/optional.rs::OptionalEvalPropProviders::prop_key_set` 遍历已注册 Provider，并以本文件的 `Add` 合成集合；`get` 使用键的数值字段选槽，并校验 Provider 自描述键。
5. 具体 Reader 调用 `get_prop_provider` 时，先通过上下文按 `OptionalEvalPropKey` 查 Provider，再比较 `Desc().Key()`，最后借助 Provider 的 `as_any` 实现做安全向下转型。缺失、键不匹配和类型不匹配由上层分别报错。

集合运算不原地修改旧值：`Add` 和 `Remove` 接收并返回 `self` 的副本。`optional_test.rs::TestOptionalPropKeySet` 明确验证旧集合在派生新集合后保持不变。

## 数据与状态

键的数值存在三个必须同步的不变量：它是 `OPTIONAL_PROPERTY_DESC_LIST` 的下标、`OptionalEvalPropKeySet` 的位号，也是 Provider 注册表的槽位。`validateOptionalProperties` 覆盖前两个关系；`pkg/expression/expropt/optional.rs::add/get` 使用相同下标访问 Provider 槽位并检查自描述键。

`OptionalEvalPropKeySet` 的公开 `u64` 允许构造带未使用高位的值。`IsEmpty` 和 `IsFull` 总是先与 `ALL_OPT_PROPS_MASK` 相与，因此只观察低 10 个有效位；高位既不会使空集变为非空，也不会阻碍有效低位形成满集。`Contains` 同样只接受注册范围内的键。

描述表是不可变的 `static` 数组，`Desc` 与 Provider 返回的都是 `&'static OptionalEvalPropDesc`，不存在逐次分配。当前 `OptPropSequenceOperator` 的描述字符串是 `"OptPropDDLOwnerInfo"`；这与 Go 的 `optionalPropertyDescList` 原样一致，文档不将其解释为新的语义或擅自修正。

## 依赖与调用关系

本文件自身仅直接依赖标准库 `std::fmt` 和 `std::any::Any`（后者通过全限定路径使用），不直接使用 `Cargo.toml` 中的其他外部 crate。`lib.rs` 将所有符号从 crate 根重导出，所以上游通常以 `exprctx::OptionalEvalPropKeySet`、`exprctx::OptProp...` 和 `exprctx::OptionalEvalPropProvider` 访问。

RustCodeGraph 显示目标文件被 `pkg/expression/builtin.rs`、`builtin_core.rs`、`builtin_inference.rs`、`context.rs`、`context_test.rs` 等 9 个索引文件直接使用。主要跨模块关系包括：

- `pkg/expression/exprctx/context.rs::EvalContext` 使用三种核心类型定义查询契约。
- `pkg/expression/expropt/optional.rs` 持有 `Vec<Option<Box<dyn OptionalEvalPropProvider>>>`，以键为槽位索引，并用位集合汇总能力。
- `pkg/expression/sessionexpr/sessionctx.rs` 注册完整会话 Provider，最终断言 `prop_key_set().IsFull()`；该文件也按键向表达式暴露 Provider。
- `pkg/expression/exprstatic/evalctx.rs` 为可复制/应用选项的静态上下文提供相同查询接口。
- `pkg/expression/builtin_inference.rs` 等表达式实现使用 `AsPropKeySet` 声明运行时依赖。

因此本文件处在“表达式声明依赖”和“上下文供应依赖”之间，是两端共享的类型协议，而不是业务 Provider 的实现层。

## 错误处理与边界

- `Add`、`Remove` 和 `Contains` 把 `key.0 >= OPT_PROPS_CNT` 视为非法注册键：前两者原样返回集合，后者返回 `false`。它们不产生 `Result`，也不 panic。
- `AsPropKeySet` 只对达到或超过 64 的位号返回空集；对于 `10..64` 的未注册键，它仍可构造高位单键集合。公共集合操作会拒绝这些键，且空/满判断会通过掩码忽略这些位。
- `Desc` 不做边界检查，非法键会因数组索引越界而 panic；应先保证键来自公开注册常量或小于 `OPT_PROPS_CNT`。`Display` 对非法键有独立安全分支。
- `validateOptionalProperties` 是显式函数而非 Rust 自动初始化钩子；当前直接调用证据来自 `optional_test.rs::TestOptionalPropKey`。固定数组类型已静态保证描述数量，函数补充检查位宽和键/下标一致性。
- `OptionalEvalPropProvider::as_any` 默认返回 `None`。需要被 `expropt::get_prop_provider<T>` 按具体类型读取的 Provider 必须自行返回合适的 `Any` 引用；保留默认值的借用型 Provider仍能作为 trait object 使用，但不能走该具体类型向下转型路径。
- 本文件不产生业务错误；Provider 缺失、键不匹配和类型不匹配的 `anyhow::Error` 位于 `pkg/expression/expropt/optional.rs::get_prop_provider`。

## 并发与资源生命周期

键、描述和位集合本身没有锁、通道、任务或 I/O。`OptionalEvalPropKey` 与 `OptionalEvalPropKeySet` 都是 `Copy` 值，集合运算只计算并返回新的 `u64`；描述表是程序全生命周期只读的静态数据，因此这些对象没有清理顺序或资源释放要求。

`OptionalEvalPropProvider` 没有 `Send` 或 `Sync` 上界，本文件不能据此保证任意 Provider 可跨线程共享。Provider 的所有权与并发约束由持有者决定：当前 `expropt::OptionalEvalPropProviders` 用 `Box<dyn ...>` 拥有 Provider，会话构造代码在需要共享会话能力时显式使用 `Arc`。扩展时不得仅因键和位集可复制，就推断具体 Provider 线程安全。

trait 返回静态描述，但 Provider 对象自身不必是 `'static`。`optional_test.rs::TestOptionalPropProviderAllowsBorrowedState` 构造借用会话字符串的 Provider，证明默认 `as_any == None` 保留了借用生命周期；若新增无条件 `'static`、`Any`、`Send` 或 `Sync` 约束，会改变这一契约。

## 与 Go 版本的对应关系

Rust 的十个键及其顺序、`OptPropsCnt`、低位掩码、描述表内容、值式集合运算都逐项对应 `pkg/expression/exprctx/optional.go`。Rust 测试 `optional_test.rs` 复刻 Go 测试 `optional_test.go` 的三组核心意图：单键/多键增删，未使用高位不参与空满判断，以及键、单键集合与描述表下标一一对应。

实现差异如下：

- Go 以 `init()` 在包初始化时检查位宽、描述切片长度和下标；Rust 用定长数组在类型层保证长度，并提供 `validateOptionalProperties()` 检查其余不变量，但不会自动执行。
- Go 的非法键防护包含 `intest.Assert` 后的返回分支；Rust 的 `Add`/`Remove`/`Contains` 只保留确定性的防御返回，没有测试模式断言。
- Go 的 `AsPropKeySet` 直接移位；Rust 在位号不小于 64 时返回空集以避免移位溢出。
- Go Provider 接口只有 `Desc`；Rust 增加可选 `as_any`，供上层以安全 downcast 替代 Go 类型断言，同时用默认 `None` 保持借用型实现可用。
- Go 的 `OptionalEvalPropKey` 是有符号 `int`，Rust 使用 `usize`，不存在负键；两边对正常注册键的行为一致。

## 扩展指南

新增可选属性时，应把它追加到现有顺序末尾，避免改变已存在位号，并同步修改：`OptionalEvalPropKey` 的关联常量、包级别名、`OPT_PROPS_CNT`、`OPTIONAL_PROPERTY_DESC_LIST`、Go 的 `optional.go`、对应具体 Provider/Reader、会话与静态上下文的注册逻辑，以及独立测试 `pkg/expression/exprctx/optional_test.rs` 和 `optional_test.go`。若会话上下文承诺提供全部能力，还要保持 `sessionctx.rs` 的 `IsFull` 断言成立。

新增 Provider 若要通过 `expropt::get_prop_provider<T>` 读取，应实现 `as_any` 并返回自身的 `&dyn Any`；若 Provider 借用了非静态状态，则可保留默认 `None`，但读取方必须只依赖 trait 方法。不要为方便 downcast 而给整个 trait 增加 `'static` 限制。

安全修改时需重点回归：键/描述/槽位的三方顺序一致性、`OPT_PROPS_CNT <= 64`、高位掩码语义、重复注册和缺失 Provider 行为，以及 Go/Rust 描述文本兼容性。键数量接近 64 时，`1_u64 << OPT_PROPS_CNT` 的掩码表达式也需要重新设计；当前实现仅适用于计数严格小于 64，虽然校验函数写的是“小于等于 64”。性能上应继续保持位运算和静态描述的零分配路径。

Rust 单元测试必须继续放在同目录独立文件 `optional_test.rs`，不要内嵌进生产源文件。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 7,032 个 Rust 文件；目标目录查询列出 `optional.rs`、`optional_test.rs`、`context.rs`、`lib.rs` 及对应 Go 文件。
- RustCodeGraph `node --file pkg/expression/exprctx/optional.rs`：核对了目标文件全部 216 行及 42 个索引符号。
- RustCodeGraph `query`：核对 `OptionalEvalPropKey`、`OptionalEvalPropKeySet`、`OptionalEvalPropProvider`、`validateOptionalProperties`、`GetOptionalPropProvider`、`set_optional_prop`、`get_optional_prop_provider` 与 `RequiredOptionalEvalProps` 的定义位置和使用面。
- RustCodeGraph 源码节点：读取了 `pkg/expression/exprctx/context.rs`、`pkg/expression/expropt/optional.rs`、`pkg/expression/sessionexpr/sessionctx.rs`、`pkg/expression/exprstatic/evalctx.rs`、`pkg/expression/builtin_inference.rs` 的直接接线片段。
- crate 边界：读取 `pkg/expression/exprctx/Cargo.toml` 与 `lib.rs`，确认包名、Go 包映射、模块装配和公开重导出。
- Go 对照：读取 `pkg/expression/exprctx/optional.go` 全文，核对键顺序、描述表、初始化校验和位集算法。
- 独立测试：读取 `pkg/expression/exprctx/optional_test.rs` 与 `optional_test.go` 全文，核对值语义、高位掩码、描述指针一致性及 Rust 借用型 Provider 边界。
- 本任务是纯文档分析，按计划不运行 Cargo；最终仅执行任务指定的 11 章节结构验证，并人工复核没有把上层错误处理或 Provider 实现误写为本文件职责。
