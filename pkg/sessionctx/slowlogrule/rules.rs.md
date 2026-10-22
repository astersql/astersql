# `pkg/sessionctx/slowlogrule/rules.rs`

## 文件定位

用 `GlobalSlowLogRules` 按连接 ID 保存全局配置解析后的规则桶，其中键 `-1` 按 Go 约定代表所有会话共享的规则。

## 核心职责

- 用 `SlowLogCondition` 表示“慢日志字段名 + 阈值”。
- 用 `SlowLogRule` 保存同一条规则的条件组；设计语义是组内 AND。
- 用 `SlowLogRules` 保存一个作用域内的原始规则串、去重字段集和规则列表；设计语义是规则间 OR。
- 用 `SessionSlowLogRules` 保存会话规则以及合并全局规则时需要的缓存状态。
- 用 `GlobalSlowLogRules` 按连接 ID 保存全局配置解析后的规则桶，其中键 `-1` 按 Go 约定代表所有会话共享的规则。
- 用 `NewSessionSlowLogRules` 建立确定的会话初始状态。

AND/OR 是这些容器承载的契约，而不是本文件执行的算法；真正的 Rust 求值可在 `pkg/executor/adapter_slow_log.rs` 的 `Match`/`MatchSessionVars` 中看到，但它们消费的是 `astersql_sessionctx_variable::slow_log` 中的同名模型。

## 主要符号

- `pub struct SlowLogCondition { Field: String, Threshold: Box<dyn Any> }`：`Field` 保留规则引用的字段名；`Threshold` 以类型擦除方式容纳 Go `any` 可表达的整型、浮点、布尔和字符串等值。消费者必须知道期望类型并进行 `downcast_ref`。
- `pub struct SlowLogRule { Conditions: Vec<SlowLogCondition> }`：一条规则的条件序列，派生 `Default` 后为空序列。注释规定条件间为 AND，但该类型没有自己的匹配方法。
- `pub struct SlowLogRules { RawRules, Fields, Rules }`：分别保存规范化之前/解析时保留的原始文本、去重后的字段集合和 boxed 规则列表；派生 `Default` 后三者均为空。
- `pub struct SessionSlowLogRules`：包含 boxed 会话 `SlowLogRules`、会话与全局规则字段的合并缓存 `EffectiveFields`、最近观察到的 `GlobalRawRulesHash`，以及失效标志 `NeedUpdateEffectiveFields`。
- `pub fn NewSessionSlowLogRules(Box<SlowLogRules>) -> Box<SessionSlowLogRules>`：转移输入规则集的所有权；有效字段初始化为空、全局哈希初始化为 `0`、更新标志初始化为 `true`。
- `pub struct GlobalSlowLogRules { RawRules, RawRulesHash, RulesMap }`：保存全局原始文本及其哈希，并用 `HashMap<i64, Box<SlowLogRules>>` 将连接 ID 映射到规则集；派生 `Default` 后为空。

文件没有 trait、impl、宏、条件编译分支或模块级常量。`#![allow(dead_code, non_snake_case)]` 允许迁移期未接线符号及与 Go 字段/API 同名的写法。

## 执行流程

本文件唯一可执行流程是 `NewSessionSlowLogRules`：

1. 调用方把一个 `Box<SlowLogRules>` 移交给构造器。
2. 构造器原样把该 box 放入 `SessionSlowLogRules.SlowLogRules`，不复制、不解析，也不校验其中字段。
3. 新建空 `HashSet<String>` 作为 `EffectiveFields`，表示尚未汇总会话与全局规则引用的字段。
4. 将 `GlobalRawRulesHash` 置零，并将 `NeedUpdateEffectiveFields` 置为 `true`，要求后续接线在首次求值前刷新缓存。
5. 返回 boxed 会话状态。

`migration_aster_unit_test.rs::new_session_slow_log_rules_matches_go_initialization` 还通过裸指针地址相等验证输入规则 box 未被替换。规则文本解析、哈希计算、有效字段合并和 AND/OR 匹配均不发生在本文件内。

## 数据与状态

`RawRules` 是规则原文的拥有型 `String`；本文件不会保持它与 `Rules`/`Fields` 的一致性。`Fields` 与 `EffectiveFields` 使用 `HashSet` 去重，集合迭代顺序不构成契约。`Rules`、`Conditions` 和 `RulesMap` 分别保留规则顺序、条件顺序和按连接分桶关系，但本文件没有规范化或重复项检查。

`Threshold: Box<dyn Any>` 保存动态类型和值的所有权。它没有 `Clone`、`Debug`、`PartialEq`、`Send` 或 `Sync` 约束，因此 `SlowLogCondition` 以及逐层包含它的结构不能自动获得这些能力；跨线程共享或复制不能被默认假定。迁移测试 `conditions_retain_go_any_thresholds_and_and_grouping` 以 `f64` 和 `bool` 下转验证动态类型被保留。

`GlobalRawRulesHash` 是缓存一致性标记而非安全哈希承诺，本文件不计算也不解释算法。`NeedUpdateEffectiveFields` 是显式失效位；构造器只负责把它设为 `true`，刷新后如何清除属于调用侧职责。

## 依赖与调用关系

直接代码依赖仅来自标准库：`std::any::Any` 提供类型擦除，`HashSet` 保存字段集合，`HashMap` 保存连接规则桶。该 crate 的 Cargo 清单没有第三方 `[dependencies]`。

模块入口是 `pkg/sessionctx/slowlogrule/lib.rs`，它重导出本文件的五个结构体和构造函数。RustCodeGraph 将 `rules.rs` 标为 7 个符号，并显示 `NewSessionSlowLogRules` 的同 crate 直接调用者为 `migration_aster_unit_test.rs::new_session_slow_log_rules_matches_go_initialization`；该测试还直接构造其余结构体。索引对同名 `SlowLogRules`/`Match` 给出了大量候选，但源码核对表明 `pkg/executor/adapter_slow_log.rs` 导入的是 `astersql_sessionctx_variable::slow_log`，不能视为本文件的下游调用。

Go 主链的直接关系更完整：`pkg/sessionctx/variable/session.go` 调用 Go `slowlogrule.NewSessionSlowLogRules` 初始化会话；`pkg/sessionctx/variable/slow_log.go` 解析文本并构造 Go `SlowLogRule`/`SlowLogRules`/`GlobalSlowLogRules`；`pkg/executor/adapter_slow_log.go` 合并字段并求值。它们说明本模型预期承担的边界，但不是当前目标 Rust 符号已接线的证据。

## 错误处理与边界

本文件没有 `Result`/`Option` 返回、显式错误类型或 panic 分支。构造器接受任何内部状态，包括空规则、空字段、字段集合与条件不一致、任意动态阈值类型以及任意 `RawRulesHash`；正确性校验必须由解析器或调用者完成。

边界语义包括：空 `SlowLogRule.Conditions` 在数学上的 AND 求值通常为真，但本文件不执行该求值；空 `SlowLogRules.Rules` 的 OR 求值通常为假，也由下游决定。`RulesMap` 的 `-1` 全局键只写在契约注释和 Go 对照中，本文件不阻止其他负键或冲突桶。动态阈值下转失败如何处理同样由消费者决定；本文件不会报告类型不匹配。

## 并发与资源生命周期

所有结构均由拥有型 `String`、集合、`Vec` 和 `Box` 组成，没有锁、原子量、通道、后台任务、文件句柄或网络资源。`Box` 和容器在所有者离开作用域时按 Rust RAII 自动释放；`NewSessionSlowLogRules` 将规则所有权移动进会话包装，不增加共享引用计数。

由于 `Box<dyn Any>` 未附加 `Send + Sync`，不能把这些规则结构视为天然可在线程间传递或共享。若未来运行时需要并发发布全局规则，应先明确阈值类型边界，再选择 `Any + Send + Sync`、类型化枚举或外层同步/快照策略，而不是仅在本文件旁增加锁。

## 与 Go 版本的对应关系

Rust 使用显式 `Box` 表达 Go 指针所有权，并保留 PascalCase 名称以方便迁移核对。

差异也必须保留在认知中：Go 的接口值、map 和指针有 nil 状态，而目标 Rust API 要求传入非空 `Box<SlowLogRules>`；Go map 是引用型，Rust 容器及 box 有单一所有权；Rust 的 `dyn Any` 默认没有线程安全约束。

## 扩展指南

若只增加规则元数据，应同步修改目标结构体、Go `rules.go` 对应字段和 `migration_aster_unit_test.rs`，并明确 `Default`、所有权、动态类型与序列化/比较需求。若增加会话缓存字段，应更新 `NewSessionSlowLogRules` 的初值测试，确保首次求值时的失效语义明确。

若要把本 crate 接入实际 Rust 主链，不能仅替换 import：需先解决它与 `pkg/sessionctx/variable/slow_log.rs` 同名类型之间的模型差异，包括 `Box<dyn Any>` 与 `Threshold`、`HashSet/HashMap` 与有序集合、boxed/optional 规则、字段命名，以及 `Send`/`Sync` 和派生 trait 能力。匹配语义应继续在独立源文件实现，测试也应放在独立测试文件中，不应内嵌到 `rules.rs`。

安全扩展至少同步以下测试面：结构与构造语义用 `pkg/sessionctx/slowlogrule/migration_aster_unit_test.rs`；若改变解析或匹配契约，再同步 `pkg/sessionctx/variable/tests/slowlog/slow_log_test.rs`、Go 同路径测试以及 executor 的独立慢日志测试。主要兼容风险是 Go/Rust 字段语义漂移，正确性风险是 `Fields` 与 `Rules` 不一致或阈值下转类型错误，性能风险是无谓复制规则文本/集合或在热路径反复动态下转。
