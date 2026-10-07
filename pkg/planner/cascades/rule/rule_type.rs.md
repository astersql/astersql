# `pkg/planner/cascades/rule/rule_type.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-rule` crate，是 Cascades 逻辑变换规则的类型目录。crate 入口 `pkg/planner/cascades/rule/lib.rs` 以私有模块 `rule_type` 装入它，再通过 `pub use rule_type::*` 对外再导出，因此其他 crate 通常以 `cascades_rule::Type`、`cascades_rule::XFJoinToApply` 等路径使用这些定义，而不是直接引用模块路径。

`pkg/planner/cascades/rule/Cargo.toml` 指明该 crate 的 Go 对照包为 `pkg/planner/cascades/rule`，并关闭自动测试发现（`autotests = false`）。本文件本身只定义规则枚举、Go 风格同名常量和名称转换，不包含 Pattern 匹配、Memo 操作或计划变换；这些行为位于同 crate 的 `rule.rs` 及具体规则 crate 中。

## 核心职责

1. `Type` 为内建 XF（transformation）规则分配稳定的 `usize` 判别值，取值从 `DefaultNone = 0` 到 `XFPullCorrPredFromAgg2 = 10`。
2. `XFMaximumRuleLength = 11` 是上界哨兵，不是可执行规则。`pkg/planner/cascades/cascades.rs` 的 `RuleMask::Test` 用它限制 `SetAll` 默认启用的 ID 范围：只有 `id < 11` 的 ID 才被视为内建槽位。
3. 文件为每个枚举变体提供同名 `pub const`，保留 Go 侧包级常量的调用形态。具体规则可把常量重命名为 `RuleType` 后交给 `NewBaseRule`。
4. `Type::String` 提供与 Go 当前实现一致的稳定跟踪名称：仅 `XFJoinToApply` 返回 `"join_to_apply"`，其他所有值都返回 `"default_none"`。

本文件描述的是规则身份和显示名称，不保证每个枚举槽位已有 Rust 规则实现或已注册进优化器。

## 主要符号

- `pub enum Type`：以 `#[repr(usize)]` 固定底层表示，并派生 `Clone`、`Copy`、`Debug`、`Eq`、`PartialEq`。它未派生 `Default`、排序、哈希或序列化能力。
- `Type::DefaultNone = 0`：未指定规则的占位值。
- `Type::XFJoinToApply = 1`：Join 到 Apply 的变换类型。当前具体规则见 `pkg/planner/cascades/rule/join/join_to_apply.rs`。
- `Type::XFDeCorrelateSimpleApply = 2`：简单 Apply 解相关类型。当前具体规则见 `pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_simple_apply.rs`。
- `Type::XFPullCorrPredFromProj` 至 `Type::XFPullCorrPredFromAgg2`（3..=10）：按 Projection、Selection、DataSource、Sort、Limit、Max1Row 与两类 Aggregation 场景预留的相关谓词上拉类型。本次直接证据中未发现这些常量被 Rust 生产代码使用，不能据此断言对应规则已实现或注册。
- `Type::XFMaximumRuleLength = 11`：枚举槽位数/掩码上界哨兵；不能作为普通规则 ID 使用。
- `DefaultNone`、`XFJoinToApply`、`XFDeCorrelateSimpleApply`、各 `XFPullCorrPredFrom*`、`XFMaximumRuleLength`：分别指向同名枚举变体的公开常量，不引入额外状态。
- `pub fn Type::String(&self) -> &'static str`：无分配地返回静态名称。命名采用 Go 风格大写方法名；crate 根的 `#![allow(non_snake_case, non_upper_case_globals)]` 允许这种兼容接口。

## 执行流程

本文件没有独立运行循环；它在规则构造、调度和跟踪路径中作为值对象参与流程：

1. 具体规则构造器选择一个本文件常量。例如 `NewJoinToApply` 将 `XFJoinToApply` 以 `RuleType` 别名传给 `NewBaseRule`；简单 Apply 解相关构造器将 `XFDeCorrelateSimpleApply` 传给其 `cascades_base`。
2. `NewBaseRule` 把 `Type` 与规则 `Pattern` 一起保存。调用 `BaseRule::String` 时，它把 `self.tp.String()` 的结果写入 `StrBufferWriter`，用于规则可读名称。
3. 具体规则的 `Rule::ID` 决定调度所用数值。简单 Apply 解相关规则明确返回 `XFDeCorrelateSimpleApply as usize`（2）；但当前 Join→Apply 规则把 `ID` 委托给 `BaseRule::ID`，而后者固定返回 0，所以不能笼统认为所有具体规则 ID 都自动等于其 `Type` 判别值。
4. 优化器规则掩码按 `usize` ID 判断是否允许执行。`RuleMask::SetAll` 后，`RuleMask::Test` 只把小于 `XFMaximumRuleLength as usize` 的 ID 视为默认内建范围；显式加入的 ID 则由掩码集合另行处理。
5. 需要跟踪名称时，`Type::String` 对值做一次匹配：值 1 得到 `join_to_apply`，其余值（包括 0、2..=10 和哨兵 11）得到 `default_none`。

## 数据与状态

`Type` 是无载荷枚举，每个值仅携带判别数；`Copy` 语义使它可按值传递，不涉及堆分配或共享所有权。`#[repr(usize)]` 与显式判别值共同保证当前 Rust 进程内转换为 `usize` 时得到 0..11 的稳定编号，并与 Go 文件中从 `iota` 开始的声明顺序对应。

所有同名公开常量都是编译期值。`String` 返回 `&'static str`，字符串存于程序静态区，没有调用方负责释放的资源。本文件没有可变全局变量、缓存或注册表；规则是否启用、是否已探索等运行状态分别由优化器的规则掩码和 Memo 结构维护。

必须保持两个不变量：有效内建规则 ID 小于 `XFMaximumRuleLength`；Go/Rust 两侧枚举顺序保持一致。若在中间插入、删除或重排变体，后续数值会变化，可能改变掩码槽位和规则身份。

## 依赖与调用关系

本文件没有 `use` 项，也不调用外部 crate。它的直接依赖只有 Rust 核心语言提供的枚举、派生 trait、`usize` 和静态字符串。

已核对的上游关系如下：

- `pkg/planner/cascades/rule/lib.rs` 声明并再导出本模块的全部公开符号。
- `pkg/planner/cascades/rule/rule.rs` 的 `BaseRule` 持有 `Type`，`NewBaseRule` 接收它，`BaseRule::String` 调用 `Type::String`。
- `pkg/planner/cascades/rule/join/join_to_apply.rs` 在构造 `BaseRule` 时使用 `XFJoinToApply`。
- `pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_simple_apply.rs` 使用 `XFDeCorrelateSimpleApply` 构造规则元数据，并在 `Rule::ID` 中将其转换为 `usize`。
- `pkg/planner/cascades/cascades.rs` 的 `RuleMask::Test` 使用 `XFMaximumRuleLength` 界定默认掩码范围。
- `pkg/planner/cascades/task/task_test.rs`、`pkg/planner/cascades/cascades_test.rs` 和 `pkg/planner/cascades/rule/ruleset/rule_set_aster_unit_test.rs` 使用 `DefaultNone` 或 `Type::DefaultNone` 构建测试规则。

RustCodeGraph 已索引本文件并报告 3 个符号，但对通用名称 `Type` 的精确 `query/callers/callees` 消歧不足；因此上述未被图解析出的使用点由限定在 `pkg/planner` 下的精确 `rg` 引用搜索补证，没有把全仓同名 `Type` 结果当成本模块调用者。

## 错误处理与边界

本文件的 API 不返回 `Result` 或 `Option`，也不会主动报错。`Type::String` 通过通配分支覆盖所有枚举值，因此对任何可构造变体都返回字符串；其边界含义是“未知于当前名称表的已声明规则统一显示为 `default_none`”，而不是错误。

`XFMaximumRuleLength` 虽然能作为 `Type` 值传递并调用 `String`，但语义上只是哨兵。调用方若把它当可执行规则 ID，会落在 `RuleMask::SetAll` 的默认范围之外，因为判断是严格小于上界。

当前 `Type` 没有从任意整数安全解析的接口，正常 Rust 代码不能仅靠本文件把任意 `usize` 转换为枚举。若未来通过 FFI、反序列化或不安全转换引入非法判别值，不能依赖本文件提供校验；本文件也没有为持久化兼容定义显式协议。

名称回落存在可观测歧义：`DefaultNone`、解相关规则、谓词上拉规则与上界哨兵都得到相同字符串。该行为是 Go 对照实现的现状并由 Rust 回归测试固定，不应在没有同步评估跟踪输出兼容性的情况下擅自“补全”名称。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或 I/O 资源。`Type` 为 `Copy` 的无载荷值，公开常量不可变，`String` 返回静态借用，所以并发调用没有本地共享可变状态，也没有清理阶段。

规则值进入 `BaseRule` 后，其生命周期由具体规则对象管理；进入 `RuleMask` 的只是转换后的 `usize`。优化器调度器、Memo 和规则对象的并发/所有权策略不由本文件决定。特别是 `pkg/planner/cascades/cascades.rs` 当前调度器使用 `Rc<RefCell<...>>` 的单线程内部可变模型，这不能从 `Type` 的轻量值语义推导出跨线程安全保证。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cascades/rule/rule_type.go`。两侧都按相同顺序声明 12 个名称：0 是 `DefaultNone`，1..10 是规则类型，11 是 `XFMaximumRuleLength`。Go 以 `type Type int` 和 `iota` 递增；Rust 以 `#[repr(usize)]` 和显式数值表达相同序数，显式值降低了单个声明被移动时不易察觉的风险，但维护者仍须同步修改两侧。

Go 的 `func (tp *Type) String() string` 与 Rust 的 `Type::String(&self) -> &'static str` 分支语义相同：只特化 `XFJoinToApply`，其余返回 `default_none`。差异在于 Go 返回普通 `string` 且方法接收指针，Rust 返回静态字符串切片且共享借用；对当前固定字面量而言没有所有权开销差异所导致的行为变化。

Rust 额外提供每个变体的同名包级 `pub const`，以模拟 Go 包常量的使用体验；Rust 枚举变体本身也可用 `Type::...` 访问。Rust 还派生了可复制、调试和相等比较能力。两侧的 `BaseRule` 都保存规则类型并用它生成名称，但 ID 接线仍由 `Rule` 实现决定：两侧基础实现当前都固定返回 0。

## 扩展指南

新增内建规则类型时，应在 `XFMaximumRuleLength` 之前追加变体并显式分配下一个连续值，同时同步 `pkg/planner/cascades/rule/rule_type.go` 的声明顺序。不要重用或改变已有编号；随后上移 `XFMaximumRuleLength`，并添加对应同名 `pub const`，否则 Go 风格导入路径不完整。

若新规则需要可读跟踪名，应明确决定是否同步扩展 Go 与 Rust 的 `String` 分支。改变现有回落名称会影响规则跟踪和测试预期，至少要更新同目录独立测试 `pkg/planner/cascades/rule/binder_test.rs`；新增/调整掩码边界时还应更新 `pkg/planner/cascades/cascades_test.rs`。

具体规则实现必须在自己的 crate/文件中实现 `Rule` 并明确返回正确 ID。不要假设把 `Type` 传入 `NewBaseRule` 就会自动让 `BaseRule::ID` 返回该判别值：当前基础实现固定为 0。可参考简单 Apply 解相关规则显式返回 `CascadesRuleType as usize` 的接线，并为该具体规则增加独立 `*_test.rs`，不要把测试嵌入生产源文件。

扩展前还应检查规则注册点、Pattern、规则集过滤和 `RuleMask` 行为。仅增加枚举槽位不会让规则进入优化器，也不证明相应变换已实现。兼容性风险主要是 ID 重排和跟踪名变化；性能风险主要来自上界扩大后的规则探索数量，而非本文件的常量或字符串匹配本身。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库，报告 11,467 个文件；`files --filter pkg/planner/cascades/rule` 确认目标、Go 对照、规则实现和测试均在索引内。
- RustCodeGraph `node --file pkg/planner/cascades/rule/rule_type.rs`：核对完整 77 行源码、12 个枚举值、12 个同名常量及 `String` 分支。
- RustCodeGraph 源码节点：`pkg/planner/cascades/rule/lib.rs`、`rule.rs`、`join/join_to_apply.rs`、`apply/decorrelateapply/xf_decorrelate_simple_apply.rs`、`pkg/planner/cascades/cascades.rs`，分别证明再导出、`BaseRule` 名称调用、具体规则构造/ID 和掩码上界关系。
- RustCodeGraph Go 节点：`pkg/planner/cascades/rule/rule_type.go`、`rule.go`，核对枚举顺序、名称回落和基础规则行为。
- 独立 Rust 测试：`pkg/planner/cascades/rule/binder_test.rs::rule_type_and_base_rule_keep_go_defaults` 验证名称与基础 ID；`pkg/planner/cascades/cascades_test.rs::cascades_context_rule_mask_respects_go_bitset_length` 验证严格上界；`pkg/planner/cascades/rule/join/join_to_apply_aster_unit_test.rs::join_to_apply_keeps_go_pattern_and_todo_result` 验证具体规则当前 ID/Pattern；`pkg/planner/cascades/rule/ruleset/rule_set_aster_unit_test.rs` 证明规则过滤消费 `usize` ID。
- Cargo 边界：`pkg/planner/cascades/rule/Cargo.toml` 定义 `astersql-planner-cascades-rule`、`lib.rs` 入口、直接依赖和 Go 包映射；`pkg/planner/cascades/Cargo.toml` 证明上层 Cascades crate 以路径依赖消费 `cascades-rule`。
- RustCodeGraph 对本文件通用符号的调用图未能可靠消歧；为此运行限定目录和精确标识符的 `rg` 搜索补齐调用证据。未运行 Cargo，符合本纯文档任务约束。
