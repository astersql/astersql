# `pkg/planner/cascades/base/base.rs`

## 文件定位

本文件是 `astersql-planner-cascades-base` crate 的哈希/相等性契约定义，源码由 [`pkg/planner/cascades/base/lib.rs`](./lib.rs) 的 `base` 模块通过 `include!("base.rs")` 纳入，并随 `pub use base::*` 从 crate 根重新导出。crate 的边界由 [`pkg/planner/cascades/base/Cargo.toml`](./Cargo.toml) 声明；该清单没有直接依赖，`Hash64` 所接收的 `Hasher` 来自同一 `base` 模块中紧接着包含的 [`hash_equaler.rs`](./hash_equaler.rs)。

它处在 Cascades 优化器“为对象生成有损摘要，再在摘要冲突时确认语义相等”的契约层，而不是哈希算法或 Memo 容器的实现层。一个明确的生产接入点是 [`pkg/planner/core/base/plan_base.rs`](../../core/base/plan_base.rs) 中的 `LogicalPlan: Plan + cascades_base::HashEquals`：逻辑计划必须同时具备哈希与相等能力，才能满足这一上层接口。

## 核心职责

- `Hash64` 统一“把对象字段按稳定顺序写入调用方提供的可变 `Hasher`”的能力。它不返回摘要；最终摘要由 `Hasher::Sum64` 取得。这保留了 Go `Hash64(h Hasher)` 的增量写入模型。
- `Equals` 统一哈希命中后的精确比较入口。参数为 `&dyn Any`，实现者需要先检查具体类型，再比较决定对象语义的字段；类型不匹配通常应返回 `false`。
- `HashEquals` 把上述两个契约组合为一个超 trait，并通过 blanket implementation 自动覆盖所有同时实现 `Hash64 + Equals` 的类型，模拟 Go 接口的方法集隐式满足规则。

本文件只定义协议，不决定哪些字段参与哈希或相等比较，也不实现 FNV-1a、Memo 插入和冲突桶管理。具体哈希器位于 `hash_equaler.rs`；具体对象的字段语义由各实现者负责。

## 主要符号

- `pub trait Hash64`：公开哈希写入契约。唯一方法 `fn Hash64(&self, h: &mut dyn Hasher)` 接收独占可变借用，因此一次调用期间只能由当前调用者推进哈希器状态。方法名保留 Go 风格，使用 `#[allow(non_snake_case)]`。
- `pub trait Equals`：公开、类型擦除的相等性契约。`fn Equals(&self, other: &dyn Any) -> bool` 不修改任一对象，也不返回错误；实现者通常使用 `Any::downcast_ref::<Self>()` 恢复类型。
- `pub trait HashEquals: Hash64 + Equals {}`：无新增方法的组合契约。它表达的是“必须同时满足两种行为”，而不是新的比较算法。
- `impl<T> HashEquals for T where T: Hash64 + Equals + ?Sized {}`：blanket implementation。`?Sized` 允许动态大小类型也参与自动实现；新增类型不应再手写空的 `HashEquals` 实现，只需实现两个基础 trait。
- `use std::any::Any`：为 `Equals` 保留与 Go `any` 相近的运行时类型检查入口。

文件中没有常量、结构体、枚举、普通函数、条件编译分支或私有辅助实现。

## 执行流程

本文件自身没有可独立运行的控制流；其契约在优化器中的典型流程如下：

1. 调用方以 `NewHashEqualer()` 创建 `Box<dyn Hasher>`；该构造与 FNV-1a 状态实现在 `hash_equaler.rs`。
2. 对象的 `Hash64` 实现按语义字段顺序调用 `Hasher` 的基础写入方法。调用方随后用 `Sum64` 取得有损摘要。
3. Memo 或分组结构先用摘要缩小候选范围。例如 [`pkg/planner/cascades/memo/group_expr.rs`](../memo/group_expr.rs) 的 `GroupExpression::Init` 计算并缓存摘要；[`pkg/planner/cascades/memo/memo.rs`](../memo/memo.rs) 的 `find_global` 先比较 `GetHash64()`。
4. 摘要相等并不代表对象相等；`find_global` 随后调用具体 `GroupExpression::Equals`，比较算子语义与输入 Group，排除哈希冲突。

需要区分“契约方法”和同名固有方法：`GroupExpression::Hash64/Equals` 展示了相同的两阶段算法，但其签名是面向具体 `GroupExpression` 的固有方法；`base.rs` 的 trait 则为需要动态组合的类型提供统一边界。`LogicalPlan` 对 `HashEquals` 的超 trait 约束是本文件公开契约进入计划层的直接证据。

## 数据与状态

三个 trait 本身均不持有字段或全局状态。`Hash64` 只在借用期内修改外部 `Hasher`，摘要和临时缓存归哈希器所有；`Equals` 只读取 `self` 与类型擦除的 `other`；`HashEquals` 不增加状态。

正确实现必须维持核心不变量：若两个对象按 `Equals` 判定相等，它们写入同一初始状态哈希器后的摘要也必须相等。反向不成立，因为 `Hash64` 明确是有损摘要；因此调用方不能仅凭摘要认定语义相等。字段顺序、空值标记、集合顺序是否有意义等编码决策，必须在 `Hash64` 与 `Equals` 两侧保持一致。

`&dyn Any` 不携带所有权，比较期间不会移动或释放被比较对象；它也不会自动执行结构化比较。错误的具体类型、遗漏字段或不一致的哈希编码都只能由实现者与测试防止。

## 依赖与调用关系

下游依赖：

- `Hash64` 依赖同模块公开 trait `Hasher`（`hash_equaler.rs`），可调用 `HashBool`、`HashInt64`、`HashString`、`HashBytes` 等增量写入方法。
- `Equals` 仅依赖标准库 `std::any::Any`。
- `HashEquals` 依赖本文件的 `Hash64` 与 `Equals`。

上游使用与实现证据：

- `pkg/planner/core/base/plan_base.rs` 的 `LogicalPlan` 将 `cascades_base::HashEquals` 列为超 trait，是计划对象的公开约束。
- `pkg/planner/util/byitem.rs` 为 `ByItems` 实现 `Hash64` 与 `Equals`；`pkg/planner/util/handle_cols.rs` 为句柄列类型实现两者。
- `pkg/planner/property/physical_property.rs` 为 `SortItem` 实现两者；`pkg/expression/core_impl.rs` 的宏为表达式类型生成两种实现。
- `pkg/planner/core/scalar_subq_expression.rs` 通过 `expression::base::{Hash64, Equals}` 实现同形契约，说明仓库中还存在其他 crate 的对应接口；引用时必须确认导入来源，不能只凭同名方法判断使用的是本 crate trait。

RustCodeGraph 已索引 `base.rs` 的 6 个节点，并能定位三个 trait 及方法源码；对 `HashEquals` 的 `callers` 查询返回空结果。由于 `include!`、超 trait 和 blanket impl 的静态边未被该查询完整表达，上述调用关系又用 `rg` 对 trait 实现、导入和 `LogicalPlan` 约束做了直接源码核验。

## 错误处理与边界

所有方法均无 `Result`、无错误类型，也没有显式 panic。`Equals` 的安全边界是运行时类型检查：参考实现 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 中的 `Value::Equals` 使用 `downcast_ref::<Self>().is_some_and(...)`，不同类型返回 `false`；调用方不应对 `other` 做未经检查的类型假设。

`Hash64` 的边界由实现者保证：本 trait 不验证字段是否完整、写入次序是否稳定，也不阻止不同对象产生相同摘要。冲突是预期情况，必须继续调用 `Equals`。本文件也没有定义哈希器重置时机；复用哈希器的调用方需在对象之间按 `Hasher::Reset` 的约定清理状态。

`HashEquals` blanket impl 意味着任何同时实现两个基础 trait 的类型都会自动满足组合接口。增加更强的约束（例如 `Send`、`Sync` 或 `'static`）会改变所有上游实现的可用性，属于 crate 级兼容性变更。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络资源，也不拥有需要清理的资源。`Hash64` 的 `&mut dyn Hasher` 在类型层面串行化单次访问，但这不等价于哈希器可跨线程共享；三个 trait 都没有 `Send`/`Sync` 超 trait，因此本文件不承诺线程安全。

资源生命周期完全由调用方持有：对象以共享借用 `&self` 参与哈希或比较，`other` 也是只在调用期间有效的共享借用，哈希器则由调用方创建、重置和释放。`Box<dyn Hasher>` 等堆分配来自 `hash_equaler.rs` 的构造器，不是本文件的职责。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/planner/cascades/base/base.go`](./base.go)：声明顺序和组合关系均为 `Hash64`、`Equals`、`HashEquals`。

- Go `Hash64(h Hasher)` 对应 Rust `Hash64(&self, h: &mut dyn Hasher)`。Rust 显式表达接收者共享借用和哈希器独占可变借用；两者都把状态写入外部哈希器而非直接返回 `u64`。
- Go `Equals(any) bool` 对应 Rust `Equals(&self, &dyn Any) -> bool`。Go 实现常用类型断言；Rust 实现使用 `downcast_ref`。Rust 参数是借用，因此不复制或转移被比较值。
- Go `HashEquals` 通过嵌入两个接口组成。Rust 用超 trait 表达，并以 blanket impl 恢复“方法集满足后自动实现”的效果。
- Go 接口可被任意具有匹配方法集的类型隐式满足；Rust 仍要求显式实现 `Hash64` 与 `Equals`，只有组合 trait 的实现是自动的。

测试语义并非逐项一一相同：[`base_test.go`](./base_test.go) 用两个 benchmark 比较泛型接口断言与 `any` 类型断言的开销；[`base_test.rs`](./base_test.rs) 将其改成确定性单元测试，验证同类型比较成功及类型擦除后错误类型返回 `false`，不保留性能基准。`migration_aster_unit_test.rs` 另用 `Value` 验证本文件真实 `Equals` trait 的同值、异值和异类型分支。

## 扩展指南

为新优化器对象接入该契约时：

1. 在对象自己的生产文件中分别实现 `Hash64` 和 `Equals`，不要手写空的 `HashEquals` 实现。
2. 列出决定语义相等的全部字段；`Hash64` 与 `Equals` 必须覆盖相同语义。对可空字段先编码 `NilFlag`/`NotNilFlag`，对序列同时考虑长度和顺序，具体基础编码复用 `Hasher` 方法。
3. `Equals` 先对 `&dyn Any` 执行目标类型 downcast；类型不匹配返回 `false`，再逐字段比较。不要依赖哈希值代替精确比较。
4. 将测试放在独立 Rust 测试文件中。契约级边界可扩展 `pkg/planner/cascades/base/base_test.rs` 或 `migration_aster_unit_test.rs`；具体实现应在其同目录独立测试中覆盖“相等对象同哈希”“强制/已知哈希冲突仍不相等”“异类型返回 false”以及 nil、空集合、顺序等边界。

若修改 trait 签名或超 trait，需要同步审查 `LogicalPlan`、所有 `impl Hash64/Equals`、trait object 使用点和 crate 再导出。兼容性风险主要是破坏下游实现或对象安全；正确性风险是哈希/相等字段漂移；性能风险来自在 `Equals` 中引入昂贵比较或在热路径 `Hash64` 中分配。仅增加对象实现时通常无需修改本文件。

## 验证依据

- RustCodeGraph `status`：索引可用，共 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录与 `base.rs` 均已收录。
- RustCodeGraph `files --filter pkg/planner/cascades/base`：确认目标生产文件、crate 入口、Go 对照和独立 Rust/Go 测试均在索引中。
- RustCodeGraph `node --file`：完整读取 `base.rs`（45 行）、`base_test.rs`（108 行）、`base.go`（37 行）、`base_test.go`（79 行）、`lib.rs`、`hash_equaler.rs`，并读取 `plan_base.rs`、`group_expr.rs`、`memo.rs` 与 `migration_aster_unit_test.rs` 的直接相关片段。
- RustCodeGraph `query`：精确确认 `Hash64`、`Equals`、`HashEquals` 位于 `base.rs`，`Hasher` 位于 `hash_equaler.rs`；`callers HashEquals` 返回空，故未把图查询缺边误写成“无调用者”。
- Cargo 与源码搜索：读取 `pkg/planner/cascades/base/Cargo.toml`，并用 `rg` 核验 `LogicalPlan: ... HashEquals`、直接 trait 实现与导入位置，补足 `include!`/trait 边。
- Go 与测试对照：读取 `base.go`、`base_test.go`、`base_test.rs`、`migration_aster_unit_test.rs`，确认接口形状、类型不匹配行为和 benchmark 到确定性测试的迁移差异。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅执行任务指定的 11 章节结构检查并人工复核链接、符号和边界陈述。
