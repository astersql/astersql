# `pkg/ddl/ddl_algorithm.rs` 逻辑说明

## 文件定位

`pkg/ddl/ddl_algorithm.rs` 位于 `astersql-ddl` crate 中；crate 的清单是 `pkg/ddl/Cargo.toml`，库入口 `pkg/ddl/lib.rs` 通过 `pub mod ddl_algorithm;` 公开本模块。它是 `ALTER TABLE ... ALGORITHM=...` 的算法协商层：把用户请求的算法与某类 ALTER 操作支持的算法集合进行比较，返回最终选择以及可选的不匹配信息。

该文件只做内存中的值判断，不创建或提交 DDL job，也不推进 schema state、执行 reorg/backfill、更新 schema version 或等待集群同步。当前 Rust 生产代码中没有调用 `resolve_alter_algorithm`；仓库内直接调用者只有 `pkg/ddl/ddl_algorithm_test.rs` 和 `pkg/ddl/db_integration_test.rs::test_alter_algorithm`。因此它目前是已公开且有测试的移植逻辑，但尚未像 Go 版本那样接入 ALTER 规格解析主链。

## 核心职责

- `proper_algorithm` 实现与具体 ALTER 类型无关的协商规则：默认请求直接采用默认算法；显式请求则从支持集合中选择第一个枚举序不低于请求值的算法；无候选时返回 `Default`。
- `resolve_alter_algorithm` 把 ALTER 操作粗分为 `AddConstraint` 和 `Other`，分别构造仅支持 `Inplace` 或仅支持 `Instant` 的策略，再委托给 `proper_algorithm`。
- `AlgorithmError` 保存请求值、选择值和默认值，让上层区分“原请求无法原样满足”的情况。本文件不决定它最终应成为错误还是警告。

这些职责对应 Go 文件 `pkg/ddl/ddl_algorithm.go` 中的 `getProperAlgorithm` 和 `ResolveAlterAlgorithm`，但 Rust 使用自有枚举和结构化错误数据，而不是直接使用 parser AST 类型及 `dbterror`。

## 主要符号

- `AlgorithmType`：公开枚举，依次声明 `Default`、`Copy`、`Inplace`、`Instant`，并派生 `Ord`/`PartialOrd`。`proper_algorithm` 的 `specified <= supported` 完全依赖这个声明顺序；调整变体顺序会改变协商语义。
- `AlterKind`：公开的简化操作分类。`AddConstraint` 映射到 `Inplace`，所有其余操作由 `Other` 统一映射到 `Instant`。它不是完整的 Rust parser AST 操作类型。
- `AlterAlgorithm`：公开策略值，包含有顺序的 `supported: Vec<AlgorithmType>` 和 `default_algorithm`。函数不校验列表顺序、是否重复、是否为空，也不校验默认值是否包含在列表中。
- `AlgorithmError`：公开错误载荷。`requested` 是显式请求，`selected` 是实际候选或回退的 `Default`，`default_algorithm` 用于上层生成兼容提示。
- `proper_algorithm(specified, algorithm)`：通用选择函数，返回 `(AlgorithmType, Option<AlgorithmError>)`。
- `resolve_alter_algorithm(kind, specified)`：面向当前两类 ALTER 的便捷入口，返回类型与 `proper_algorithm` 相同。

文件没有模块级常量、全局可变状态、trait、`impl`、宏或条件编译项。

## 执行流程

`resolve_alter_algorithm` 的流程如下：

1. 检查 `kind`。`AddConstraint` 的 `default_algorithm` 为 `Inplace`，`Other` 为 `Instant`。
2. 临时构造 `AlterAlgorithm { supported: vec![default_algorithm], default_algorithm }`；当前每类只支持一个候选。
3. 调用 `proper_algorithm`。
4. 若 `specified == Default`，`proper_algorithm` 立即返回策略默认值且错误为 `None`。
5. 对显式请求，按 `supported` 的迭代顺序寻找第一个满足 `specified <= supported` 的值。由于枚举顺序为 `Copy < Inplace < Instant`，这允许用更高序的可用算法替代请求，例如请求 `Copy`、仅支持 `Inplace` 时选择 `Inplace`。
6. 没有候选时选择 `Default`。只要选择值与请求值不同，就构造 `AlgorithmError`；完全相同则返回 `None`。

重要分支由 `pkg/ddl/ddl_algorithm_test.rs::test_find_alter_algorithm` 覆盖：添加约束的默认/`Inplace` 请求无错误，请求 `Copy` 时选择 `Inplace` 并携带错误，请求 `Instant` 时无可用候选而返回 `Default` 和错误；`Other` 默认或请求 `Instant` 无错误，请求 `Copy`/`Inplace` 时升级为 `Instant` 并携带错误。

## 数据与状态

所有输入、返回值和中间策略均为调用栈上的拥有值或借用值。`AlgorithmType` 与 `AlterKind` 是小型 `Copy` 枚举；`AlterAlgorithm` 拥有一个 `Vec`；`AlgorithmError` 是纯数据载荷。函数没有静态缓存、持久化元数据、数据库事务或跨调用状态。

核心不变量是枚举比较序必须保持 Go `ast.AlgorithmType` 的兼容顺序。另一个隐含前提是调用者按“更合适/更优先”的次序组织 `supported`，因为实现返回首个匹配项而不是计算整个集合的最大或最小值。`pkg/ddl/ddl_algorithm_test.rs::test_proper_algorithm_preserves_go_order_and_error_contract` 用 `[Instant, Copy]` 证明请求 `Inplace` 时首个合格值为 `Instant`，并验证空集合回退到 `Default`。

## 依赖与调用关系

本文件没有 `use` 导入，也不直接依赖 `pkg/ddl/Cargo.toml` 中的任何外部 crate；实现只使用 Rust 标准库的 `Vec`、迭代器、派生 trait 和 `Option`。它通过 `pkg/ddl/lib.rs` 进入 `astersql-ddl` 的公开模块树，测试模块 `ddl_algorithm_test` 则在 `#[cfg(test)]` 下独立装配，符合源文件与测试文件分离约束。

RustCodeGraph 给出的内部调用边是 `resolve_alter_algorithm → proper_algorithm`；`proper_algorithm` 构造 `AlgorithmError`。图与仓库搜索共同显示 Rust 调用者为 `pkg/ddl/ddl_algorithm_test.rs` 和 `pkg/ddl/db_integration_test.rs::test_alter_algorithm`，未发现生产调用者。

Go 主链不同：`pkg/ddl/executor.go::ResolveAlterTableSpec` 调用 `pkg/ddl/ddl_algorithm.go::ResolveAlterAlgorithm`，然后把结果写入 `spec.Algorithm`。若结果为 `AlgorithmTypeDefault`，该层返回错误；若 TiDB 选择了更优算法，则把不匹配错误追加到语句上下文作为警告并继续。这种上层策略尚不能从当前 Rust 模块的生产调用关系中得到对应保证。

## 错误处理与边界

函数不返回 `Result`，不会抛出或记录错误；不匹配通过 `Option<AlgorithmError>` 表达。需要区分两种非空错误载荷：

- `selected != Default`：找到了比用户请求枚举序更高的候选，调用者可像 Go `ResolveAlterTableSpec` 一样选择告警后继续。
- `selected == Default`：没有有效候选。Go 上层将其视为终止错误；Rust 当前没有生产上层落实该政策。

边界行为包括：空 `supported` 对任意显式请求返回 `Default` 和错误；`Default` 请求不检查支持列表，直接信任 `default_algorithm`；无序、重复或自相矛盾的策略不会被拒绝；`AlterKind::Other` 把所有非添加约束操作合并处理，无法表达操作级差异。`AlgorithmError` 也没有实现 `std::error::Error` 或格式化为 MySQL `ER_ALTER_OPERATION_NOT_SUPPORTED`，所以源码注释中的 MySQL 对应关系是语义映射，不是已经接入的协议错误。

## 并发与资源生命周期

该模块是同步、确定性的纯计算：不持锁、不启动线程或异步任务、不使用通道，不访问文件、网络、事务或 DDL 系统表。临时 `Vec` 在 `resolve_alter_algorithm` 返回时释放，返回的枚举与错误载荷完全拥有自身数据，因此没有借用跨越调用边界。

它也不管理 DDL job 的提交、owner 调度、重试、取消/回滚、delete-range GC、schema version 或 follower 同步；这些生命周期属于 DDL 执行框架，而非算法协商函数。若未来接入生产主链，应在 job 创建前完成算法解析，并由上层明确错误/警告政策，不能把本模块的结构化不匹配信息误认为已完成 DDL 执行。

## 与 Go 版本的对应关系

Rust `AlgorithmType` 对应 Go `ast.AlgorithmType`；Rust `AlterAlgorithm` 对应 Go 同名结构；`proper_algorithm` 对应 `getProperAlgorithm`；`resolve_alter_algorithm` 对应 `ResolveAlterAlgorithm`。两边都执行 `specified <= supported` 的首个匹配、无候选回退 `Default`，并对选择值不同的显式请求报告不匹配。

差异如下：

- Go 接收完整的 `*ast.AlterTableSpec` 并检查 `Tp == AlterTableAddConstraint`；Rust 只接收已归类的 `AlterKind`。
- Go 复用静态的 `instantAlgorithm`/`inplaceAlgorithm`；Rust 每次在栈上构造单元素策略和 `Vec`。
- Go 返回由 `dbterror.ErrAlterOperationNotSupported` 生成、带 MySQL 兼容消息的 `error`；Rust 返回不实现错误 trait 的 `AlgorithmError` 数据。
- Go 已由 `pkg/ddl/executor.go::ResolveAlterTableSpec` 在生产路径调用，并区分错误与警告；Rust 当前仅由测试调用。
- Go 测试 `pkg/ddl/ddl_algorithm_test.go::TestFindAlterAlgorithm` 枚举多种实际 `AlterTableSpec`。Rust 独立测试覆盖两类映射和通用选择规则，但用 `Other` 汇总具体操作，覆盖粒度较粗。

因此当前 Rust 文件准确移植了核心比较规则，但不能据此声称完整移植了 AST 接入、错误编码/消息、语句警告和各 ALTER 类型的生产行为。

## 扩展指南

- 新增算法时，先核对 Go/parser 中 `AlgorithmType` 的数值顺序，再修改 Rust 枚举；必须同步扩展 `pkg/ddl/ddl_algorithm_test.rs` 的顺序、精确匹配、升级选择和无候选用例。枚举重排是兼容性风险。
- 新增不同默认算法的 ALTER 类型时，优先扩展 `AlterKind` 与 `resolve_alter_algorithm` 的映射，并在独立测试文件添加每种请求组合；若 Rust parser/执行器已能提供完整 AST 类型，可评估消除过度简化的 `Other`。
- 若接入生产 ALTER 主链，应参照 `pkg/ddl/executor.go::ResolveAlterTableSpec`：在创建/执行 job 前写回选择结果，并明确 `selected == Default` 为错误、选择更优算法为警告的兼容行为。同时将 `AlgorithmError` 映射到现有 Rust 错误体系和 MySQL 错误码，而不是在本文件内吞掉信息。
- 若允许外部构造 `AlterAlgorithm`，应决定是否验证 `supported` 顺序、默认值归属和空集合；改变这些边界需增加 `proper_algorithm` 的独立回归测试。
- 性能方面当前单元素 `Vec` 分配可被静态切片或常量策略消除，但仅在有测量依据时调整；正确性风险主要来自排序语义和上层错误/警告分类，而非计算成本。

## 验证依据

- 源码与装配：`pkg/ddl/ddl_algorithm.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`。
- Rust 测试：`pkg/ddl/ddl_algorithm_test.rs::test_find_alter_algorithm`、`test_proper_algorithm_preserves_go_order_and_error_contract`；`pkg/ddl/db_integration_test.rs::test_alter_algorithm`。
- Go 对照：`pkg/ddl/ddl_algorithm.go::{getProperAlgorithm, ResolveAlterAlgorithm}`、`pkg/ddl/ddl_algorithm_test.go::{TestFindAlterAlgorithm, runAlterAlgorithmTestCases}`、`pkg/ddl/executor.go::ResolveAlterTableSpec` 中的调用及错误/警告分支。
- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；`node resolve_alter_algorithm` 确认其构造 `AlterAlgorithm` 并调用 `proper_algorithm`，调用者为 Rust 测试；`node proper_algorithm` 确认其构造 `AlgorithmError`，调用者为 `resolve_alter_algorithm` 和测试模块。
- 仓库搜索：对 `pkg/ddl/**/*.rs` 搜索 `resolve_alter_algorithm`/`proper_algorithm`，未发现生产调用；对 Go 文件搜索确认 `executor.go` 的生产调用。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令检查文档存在且恰有十一个固定二级标题，并人工复核没有把 Go 主链行为误写为 Rust 已接线行为。
