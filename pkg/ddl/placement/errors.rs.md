# `pkg/ddl/placement/errors.rs` 逻辑说明

## 文件定位

`errors.rs` 是 `astersql-ddl-placement` crate 的统一错误词汇表和轻量错误载体。crate 边界由 `pkg/ddl/placement/Cargo.toml` 定义，入口 `pkg/ddl/placement/lib.rs` 以私有 `mod errors` 装入本文件，再通过 `pub use errors::*` 将 `Error`、`wrap` 和所有错误消息常量公开到 crate 根。因而 crate 内部的约束、规则和 Bundle 构造代码可从 `crate::errors` 使用它们，外部依赖者也可从 `astersql_ddl_placement` 根路径取得这些公开项。

该文件位于 DDL placement 子系统，但自身不执行 DDL job、schema state 转换、元数据持久化或 PD 请求。它服务于把 Placement Settings、标签约束和 Bundle ID 转换为 PD placement rule 的纯解析/构造路径；上层是否把错误返回给 SQL/DDL 调用链由 crate 使用者决定。`pkg/ddl/placement/Cargo.toml` 的 `package.metadata.porting.go-package` 指向 `pkg/ddl/placement`，直接 Go 对照文件是 `pkg/ddl/placement/errors.go`。

## 核心职责

本文件只有三项职责：

1. 用 `Error(String)` 保存 placement 解析或构造失败的可读文本，并实现 Rust 标准错误接口。
2. 用 `wrap(kind, detail)` 统一生成 `"错误类别: 细节"` 文本，让调用点既保留稳定类别前缀，又附带输入、计数或底层解析错误。
3. 声明 15 个 `&'static str` 错误类别，覆盖单条约束、约束集合/YAML、规则副本数、Bundle ID、placement options 和映射分隔符等失败。

它不负责判断输入是否合法；判断发生在 `constraint.rs`、`constraints.rs`、`rule.rs` 和 `bundle.rs`。本文件也没有错误码、结构化字段、嵌套 source 或自动类型转换，因此“错误类别”当前是一项字符串协议。

## 主要符号

- `pub struct Error(String)`（`errors.rs:26`）：单字段、字段私有的拥有型错误。派生 `Clone`、`Debug`、`Eq`、`PartialEq`，允许复制和按完整消息比较，但外部代码不能直接读取或修改内部字符串。
- `Error::new(message: impl Into<String>) -> Error`（`errors.rs:28-33`）：接受 `String` 或 `&str` 等可转字符串值；常量类别可直接构造成无细节错误，动态消息会被拥有化。
- `impl Display for Error`（`errors.rs:35-39`）：原样写出内部字符串，不添加前缀、引号或调试信息。相关 Rust 测试依赖此输出做前缀判断。
- `impl std::error::Error for Error`（`errors.rs:41`）：接入标准错误 trait；由于未覆盖 `source()`，此类型没有可遍历的底层错误链。
- `wrap(kind: &str, detail: impl Display) -> Error`（`errors.rs:44-46`）：用 `format!("{kind}: {detail}")` 生成错误。即便 `detail` 为空，仍会产生冒号和空格；调用方目前均传入有意义细节。
- 约束类常量：`ErrInvalidConstraintFormat`、`ErrUnsupportedConstraint`、`ErrConflictingConstraints`。生产引用分别位于 `constraint.rs` 和 `constraints.rs`。
- 规则/YAML 类常量：`ErrInvalidConstraintsMapcnt`、`ErrInvalidConstraintsFormat`、`ErrInvalidSurvivalPreferenceFormat`、`ErrInvalidConstraintsReplicas`、`ErrInvalidConstraintsMappingWrongSeparator`、`ErrInvalidConstraintsMappingNoColonFound`。生产引用位于 `constraints.rs`、`rule.rs` 和 `bundle.rs`。
- Bundle/options 类常量：`ErrInvalidBundleID`、`ErrInvalidBundleIDFormat`、`ErrInvalidPlacementOptions`，生产引用位于 `bundle.rs`。
- 当前仅定义、未在 Rust placement 生产代码使用的对齐常量：`ErrLeaderReplicasMustOne`、`ErrMissingRoleField`、`ErrNoRulesToDrop`。它们存在于 Go `errors.go`，但 `rg` 未找到定义外的 Rust 引用；不能据此声称对应 Rust 校验已经接线。

文件无 trait 定义、枚举、泛型类型、模块级可变状态、宏或条件编译项。`lib.rs` 上的 `#![allow(non_snake_case, non_upper_case_globals)]` 使这些与 Go 同名的常量可保持迁移命名。

## 执行流程

典型错误流如下：

1. 上游解析/构造函数接收 placement 输入。例如 `constraint.rs::NewConstraint` 解析 `{+|-}key=value`，`constraints.rs::NewConstraintsFromYaml` 解析 YAML，`rule.rs::RuleBuilder::BuildRules` 生成 PD 规则，`bundle.rs::NewBundleFromOptions`/`Bundle::ObjectID` 处理策略选项或组 ID。
2. 若只需要类别，调用点执行 `Error::new(ErrInvalidConstraintsFormat)` 一类构造；若需要诊断上下文，则执行 `wrap(ErrInvalidConstraintFormat, label)` 或传入格式化后的计数、解析错误。
3. `wrap` 先通过 `Display` 格式化细节，再把完整文本交给 `Error::new`；`Error` 随 `Result<_, Error>` 向上传播，常见传播方式是 `?` 或 `map_err`。
4. 消费者显示错误时调用 `Display`/`to_string()`，得到类别本身或 `kind: detail`。独立 Rust 测试中的 `assert_error_kind`/`assert_error_kind` 类辅助函数以 `starts_with(kind)` 判断错误类别。

具体调用边证据包括：`constraint.rs::NewConstraint` 与 `RestoreConstraint` → `wrap`；`constraints.rs::preCheckDictConstraintStr`、`AddConstraint` → `wrap`，`NewConstraintsFromYaml` → `Error::new`；`rule.rs::BuildRules`、`newRules`、`newRulesWithDictConstraints` → 两种构造方式；`bundle.rs` 的 options 构建路径与 `Bundle::ObjectID` → 两种构造方式。`errors.rs` 本身没有下游业务调用，只有 `std::fmt`、字符串转换和格式化。

## 数据与状态

`Error` 的唯一状态是一个拥有所有权的 UTF-8 `String`。创建后，本文件不提供修改器；从本文件 API 看它是不可变值。克隆会复制消息，比较会比较完整字符串，而不是仅比较类别。

15 个类别均是静态字符串切片，无运行时初始化和堆分配；只有实际构造 `Error` 时才把消息变为 `String`。`wrap` 还会为拼接后的完整消息分配一次字符串。文件不维护全局注册表、错误码映射或 locale 信息。

类别与细节之间以 ASCII `": "` 分隔。`ErrInvalidConstraintsMappingWrongSeparator` 的文本包含弯引号 `“: ”`，这是与 Go 对照文本一致的用户可见字符，修改时需警惕精确文本兼容性。

## 依赖与调用关系

直接下游依赖仅为 Rust 标准库：`std::fmt::{Display, Formatter}`、`std::error::Error`、`String` 与 `format!`。尽管 crate 的 `Cargo.toml` 声明 `serde_yaml`、`regex`、`pdtypes`、`meta-model`、`tablecodec` 等依赖，本文件不直接使用它们；它只接收其他模块把这些依赖产生的错误转成 `Display` 后的文本。

直接上游生产文件及用途：

- `pkg/ddl/placement/constraint.rs`：约束语法、操作符、空键值、TiFlash 正向约束和还原失败。
- `pkg/ddl/placement/constraints.rs`：未知属性、YAML 列表失败和约束冲突。
- `pkg/ddl/placement/rule.rs`：数组/映射 YAML 分类、映射分隔符、副本数以及规则构建失败。
- `pkg/ddl/placement/bundle.rs`：placement options、survival preference 和 Bundle ID 失败。

`pkg/ddl/placement/lib.rs` 将这些错误项公开再导出。Cargo 清单搜索还显示 `pkg/ddl`、`pkg/session`、`pkg/executor`、`pkg/domain`、`pkg/infoschema`、`pkg/distsql`、部分 store/executor 子 crate 与测试 crate 依赖 `astersql-ddl-placement`；这些是 crate 级潜在消费者，不等同于每个错误常量都有跨 crate 直接引用。RustCodeGraph 的文件节点报告 `errors.rs` 被包含在广泛的 DDL 文件图中，但精确 `pub const` 查询未生成符号结果，因此常量级引用以源码 `rg` 交叉核验。

## 错误处理与边界

`Error::new` 不验证消息是否为空，也不区分类别和细节。`wrap` 接受任意 `&str` 作为 `kind`，所以调用方可以传动态错误文本；`bundle.rs` 确实存在把已有 `error.to_string()` 当作 `kind` 再包装上下文的路径。这使错误格式灵活，但也意味着类型系统不保证 kind 来自 15 个常量。

Rust 实现没有 Go `%w` 的错误身份链语义。Go 的 `fmt.Errorf("%w: ...", sentinel)` 允许 `errors.Is`/`require.ErrorIs` 沿链识别哨兵；Rust `wrap` 只保存扁平字符串，`std::error::Error::source()` 为 `None`。因此当前 Rust 测试使用 `error.to_string().starts_with(kind)`，而不是结构化匹配。调用方若依赖完整文本相等，会受到新增细节影响；若依赖前缀，也必须避免一个类别文本成为另一个类别的非预期前缀。

`serde_yaml`/整数解析等底层错误在部分路径中被格式化进细节，原错误类型随即丢失，无法 downcast。另一些路径故意只返回类别，例如 `NewConstraintsFromYaml` 把 YAML 失败统一映射为 `ErrInvalidConstraintsFormat`，不会泄露解析器诊断。两种策略由调用点决定，而非本文件自动完成。

边界验证由独立测试覆盖：`constraint_test.rs` 检查非法长度、操作符、空键值和不支持的 TiFlash；`constraints_test.rs` 检查 YAML、未知属性及冲突；`rule_test.rs` 检查数组/字典格式、错误分隔符和副本数；`bundle_test.rs` 检查 Bundle ID、placement options、约束格式和副本数。没有同名 `errors_test.rs`，也没有仅针对 `Error`/`wrap` 的独立测试。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、事务、文件句柄或网络资源。`Error` 完全拥有消息，不借用调用方数据，所以可以越过普通函数栈返回；其可否跨线程由 `String` 和零额外字段自然决定，本文件未声明特殊线程不变量。

资源生命周期只涉及字符串：`Error::new` 取得或创建拥有型 `String`，错误值被丢弃时字符串随之释放；`wrap` 的临时格式化参数在调用结束后即可释放，完整消息由返回的 `Error` 持有。常量是程序静态生命周期数据，不需要清理。并发调用不会共享可变状态，因此本文件本身没有竞争条件；性能成本主要是错误路径上的字符串分配和格式化，正常成功路径不经过这些构造。

## 与 Go 版本的对应关系

`pkg/ddl/placement/errors.go` 定义相同名称和相同文本的 15 个包级哨兵错误，Rust 常量逐项保留了错误词汇；Rust 文件还增加 `Error` 和 `wrap`，用于补足 Go `error` 接口与 `fmt.Errorf` 的承载能力。Rust `Display` 对应 Go `error.Error()` 的用户可见文本。

关键差异是身份语义：Go 常量是各自唯一的 `error` 值，生产代码用 `%w` 包装，测试可用 `require.ErrorIs`；Rust 常量只是 `&str`，包装结果是新的 `Error(String)`，只能按完整文本或约定前缀识别。`Clone + Eq` 也只代表消息值相等，不代表同一个哨兵实例。

生产接线并非逐项完全相同。Rust 已在 constraint/constraints/rule/bundle 的主要解析路径使用 12 个类别；`ErrLeaderReplicasMustOne`、`ErrMissingRoleField`、`ErrNoRulesToDrop` 在当前 Rust placement 生产文件中无定义外引用，而 Go 对照仍声明它们。文档只记录这一检索事实，不推断相关业务分支是否由其他错误或不同设计取代。

测试意图总体保持一致：Go 的 `constraint_test.go`、`constraints_test.go`、`rule_test.go`、`bundle_test.go` 提供原始边界表；Rust 同目录对应 `*_test.rs` 文件用相同类别文本和大量对照用例验证迁移结果。Rust 测试与源文件通过 `lib.rs` 的 `#[cfg(test)] mod ...` 分离，符合仓库“测试逻辑不内嵌生产源文件”的约束。

## 扩展指南

新增或调整 placement 错误时，应按以下顺序处理：

1. 先确认失败属于现有类别还是需要新类别。若只是补充输入上下文，优先在 `constraint.rs`、`constraints.rs`、`rule.rs` 或 `bundle.rs` 的判定点调用 `wrap`，不要在 `errors.rs` 加入业务判断。
2. 若新增类别，需在 `errors.rs` 添加稳定文本，并检查 Go `errors.go` 是否有对应哨兵或语义差异；若目标是逐提交 Go 对齐，只纳入该提交增量所需的类别和最小接线。
3. 在最接近行为的独立 Rust 测试文件补用例：单约束用 `constraint_test.rs`，集合/冲突用 `constraints_test.rs`，规则/YAML/副本数用 `rule_test.rs`，Bundle/options/ID 用 `bundle_test.rs`。同时核对对应 Go 测试的边界和错误身份意图。
4. 保持类别位于消息开头，除非同步改造所有 `starts_with(kind)` 断言和消费者。修改现有文本属于兼容风险：SQL 错误展示、日志、测试或外部字符串匹配都可能受影响。
5. 若要获得类似 Go `errors.Is` 的结构化识别，应设计枚举 kind 或保存 source，而不是继续强化字符串解析；这会改变公开 `Error` 形态、相等语义和调用点，需单独评估跨 crate 兼容性。
6. `ErrLeaderReplicasMustOne`、`ErrMissingRoleField`、`ErrNoRulesToDrop` 若未来接线，应先定位真实 Rust 行为入口和 Go 测试，不要仅因常量存在就添加无依据分支。

性能上，错误路径增加细节会增加格式化/分配成本，但通常不影响成功热路径；正确性风险主要是错误分类顺序、丢失底层诊断或破坏类别前缀；兼容风险主要是公开常量名称与精确消息变化。

## 验证依据

- RustCodeGraph：`status` 确认项目索引包含 11,467 个文件，`files --filter pkg/ddl/placement` 确认 Go/Rust 源与独立测试；`node --file pkg/ddl/placement/errors.rs --offset 1 --limit 240` 完整读取 81 行目标文件，并报告该文件在模块图中的使用关系。对 `ErrInvalidConstraintFormat`、`ErrInvalidBundleID` 的精确 constant 查询无结果，因此没有把图缺失误写成“无调用者”。
- 目标与边界：完整读取 `pkg/ddl/placement/errors.rs`、`pkg/ddl/placement/Cargo.toml`、`pkg/ddl/placement/lib.rs`；读取最近包契约 `pkg/ddl/doc.go` 和 DDL 入口说明 `docs/agents/ddl/README.md`，确认本文件不参与 job/状态机执行。
- 直接生产证据：核对 `pkg/ddl/placement/constraint.rs`、`constraints.rs`、`rule.rs`、`bundle.rs` 中全部 `Err*` 和 `wrap`/`Error::new` 引用；用定义外搜索确认三个当前未接线常量。
- Go 对照：读取 `pkg/ddl/placement/errors.go`，并核对 `constraint.go`、`constraints.go`、`rule.go`、`bundle.go` 的 `%w` 包装位置。
- 测试证据：读取 Rust `constraint_test.rs`、`constraints_test.rs`、`rule_test.rs`、`bundle_test.rs`、`bundle_1_aster_unit_test.rs` 的错误断言，并对照 Go `constraint_test.go`、`constraints_test.go`、`rule_test.go`、`bundle_test.go`；测试证明当前 Rust 类别判定采用消息前缀，Go 采用哨兵身份。
- Cargo 使用面：搜索各 `Cargo.toml` 对 `astersql-ddl-placement` 的依赖，确认该 crate 被 DDL、session、executor、domain、infoschema、distsql、store 等组件引用；这只证明 crate 边界，不夸大为每个错误项的直接调用边。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构验证，并人工复查只有本说明文档与任务文件删除属于本会话范围。
