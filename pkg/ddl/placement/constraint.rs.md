# `pkg/ddl/placement/constraint.rs`

## 文件定位

本文件属于 `astersql-ddl-placement` crate，是 Placement Policy 到 PD placement rule 转换链中的“单条 Store 标签约束”基础层。crate 入口 `pkg/ddl/placement/lib.rs` 将本模块的公开项重新导出；上层 `pkg/ddl/placement/constraints.rs` 负责约束集合的解析、去重和冲突检测，`pkg/ddl/placement/bundle.rs` 再把约束放入 PD `Rule`/`Bundle`。因此，本文件只处理 `pd::LabelConstraint` 的值转换和两两关系，不直接提交 DDL job、不推进 schema state、不持久化元数据，也不调用 PD 网络接口。

从 DDL 执行框架看，这属于构造 placement 元数据的同步辅助逻辑，而不是独立的 job worker 或 reorg 路径。真正的 DDL 调度、版本同步和失败恢复由 `pkg/ddl` 更上层完成；本文件没有相关状态机（依据：`pkg/ddl/doc.go`、`docs/agents/ddl/README.md`、本文件全部符号）。

## 核心职责

1. `NewConstraint` 把用户侧的 `{+|-}key=value[,value...]` 文本解析成 PD 标签约束；`+` 映射为 `pd::In`，`-` 映射为 `pd::NotIn`。
2. `NewConstraintDirect` 为已经结构化的调用方提供无校验构造入口。
3. `RestoreConstraint` 把“恰好一个 value、且操作为 In/NotIn”的约束规范化还原成文本。
4. `ConstraintCompatibleWith` 将同 key 的两条约束归类为兼容、冲突或重复，为约束集合合并提供决策。
5. 阻止普通约束用 `+engine=tiflash` 强制选择 TiFlash；TiFlash 副本应由专用规则组表达（依据：`NewConstraint`、`EngineLabelKey`、`EngineLabelTiFlash`）。

## 主要符号

- `NewConstraint(label: &str) -> Result<pd::LabelConstraint, Error>`：公开解析入口。它校验最短长度、首字符、等号数量、去除 key/value 两端空白，并对 TiFlash 正向引擎约束做大小写不敏感拦截。逗号分隔后的每个 value 不会再次单独 trim。
- `NewConstraintDirect(key, op, values) -> pd::LabelConstraint`：公开直构入口，仅组装字段，不检查空 key、空 values、操作类型、冲突或 TiFlash 限制。
- `RestoreConstraint(&pd::LabelConstraint) -> Result<String, Error>`：公开单值还原入口。仅接受 `Values.len() == 1` 和 `In`/`NotIn`，输出不含额外空白的 `+key=value` 或 `-key=value`。
- `ConstraintCompatibility = u8`：兼容性结果编码；三个公开常量依次是 `ConstraintCompatible = 0`、`ConstraintIncompatible = 1`、`ConstraintDuplicated = 2`。
- `ConstraintCompatibleWith(left, right) -> ConstraintCompatibility`：公开两两比较入口。不同 key 直接兼容；同 key 时比较操作符及按索引对齐的 value。

本文件没有 struct、enum、trait、impl、宏或条件编译项。实际数据类型来自 `pdtypes::placement`，由 `lib.rs` 中的 `pd` 模块再导出。

## 执行流程

解析流程（`NewConstraint`）：

1. 少于 4 字节立即返回 `ErrInvalidConstraintFormat`，覆盖不可能形成 `+a=b` 的输入。
2. 查看第一个字节：`+` 选择 `pd::In`，`-` 选择 `pd::NotIn`，其他字符报格式错误。只有 ASCII 前缀通过后才从字节位置 1 切片，因此该切片位于合法 UTF-8 字符边界。
3. 对剩余文本按 `=` 全量分割，结果必须恰好两段；缺少等号或含多个等号都失败。
4. trim key 和整个 value 字符串，两者均不得为空。
5. 若操作为 `In`、key 精确等于 `engine`、value 忽略 ASCII 大小写等于 `tiflash`，返回 `ErrUnsupportedConstraint`。
6. value 按逗号切成 `Values`，构造 `pd::LabelConstraint`。该步骤允许多个 value，也保留各逗号子项自身的空白。

集合调用流程（`constraints.rs`）：`NewConstraints` 先 trim 每条标签，再调用 `NewConstraint`；随后 `AddConstraint` 逐个调用 `ConstraintCompatibleWith`。重复项不追加，冲突项用 `RestoreConstraint` 尝试生成错误上下文并返回 `ErrConflictingConstraints`。`RestoreConstraints` 也逐项调用 `RestoreConstraint` 后加引号、逗号拼接。

直接构造流程（`NewConstraintDirect`）：`bundle.rs` 的糖语法构造用它生成 region 的 `In` 约束，再由 `NewConstraintsDirect` 包装进规则。它有意跳过文本解析，调用方必须保证输入合法。

兼容性流程（`ConstraintCompatibleWith`）：

1. key 不同，返回兼容。
2. key 相同时记录操作是否相同，并从左侧 `Values` 开始按索引比较；只有“右侧该索引存在且值不同”才把 `same_value` 置为 false。
3. 同操作且 `same_value` 为真时返回重复。
4. 操作相反但 `same_value` 为真，或两个操作均为 `In` 且值不同，返回冲突。
5. 其他组合返回兼容，典型例子是同 key 的两个不同 `NotIn` 值。

## 数据与状态

输入输出均为调用栈上的借用值或拥有所有权的 `String`/`Vec<String>`；函数不读写全局可变状态。`pd::LabelConstraint` 的关键字段是 `Key`、`Op`、`Values`，操作枚举除 `In`、`NotIn` 外还可能有 `Empty`、`Exists`、`NotExists` 或 `Unknown`，但文本解析只产生前两种，还原也只接受前两种。

兼容性常量使用 `u8` 与 Go 的 `byte/iota` 编码保持一致。调用方应把常量视作分类值，不依赖额外位语义。

需要特别保留的当前不变量和边界：

- `NewConstraint` 可以产生多 value 约束，`RestoreConstraint` 却只支持单 value，所以两者并非对所有合法解析结果可逆。
- 兼容性比较是方向性的：循环长度取左侧 `constraint.Values`，不会单独比较两侧长度。左侧为空或是右侧相同前缀时可能仍被判为 `same_value`。这是与 Go 版本一致的现有行为，扩展时不可在没有回归证据的情况下“顺手对称化”。
- key 比较、普通 value 比较均区分大小写；只有 TiFlash 禁止项的 value 比较忽略 ASCII 大小写。

## 依赖与调用关系

crate 边界由 `pkg/ddl/placement/Cargo.toml` 定义，crate 名为 `astersql-ddl-placement`，库入口是 `lib.rs`。与本文件直接相关的内部依赖是：

- `crate::common::{EngineLabelKey, EngineLabelTiFlash}`：TiFlash 特例常量。
- `crate::errors::{ErrInvalidConstraintFormat, ErrUnsupportedConstraint, Error, wrap}`：统一的字符串错误类型和 `kind: detail` 包装。
- `crate::pd`：由 `lib.rs` 再导出 `pdtypes::placement::*`，提供 `LabelConstraint`、`LabelConstraintOp`、`In`、`NotIn`。

RustCodeGraph 对本文件给出 5 个符号，并标记它被 `constraint_test.rs`、`constraints_test.rs` 使用。精确调用边结合源码引用核验如下：

- `constraints.rs::NewConstraints` → `NewConstraint` → `errors::wrap`/PD 值构造。
- `constraints.rs::AddConstraint` → `ConstraintCompatibleWith`；冲突时 → `RestoreConstraint`。
- `constraints.rs::RestoreConstraints` → `RestoreConstraint`。
- `bundle.rs::NewBundleFromSugarOptions` 路径 → `NewConstraintDirect`，用于 region 约束；测试中的规则辅助函数也调用该入口。
- `lib.rs` 通过 `pub use constraint::*` 将上述 API 暴露到 crate 根。

再往上的业务链由 `bundle.rs` 接收 `PlacementSettings`，构造 `pd::Rule` 与 `Bundle`，最终供 DDL placement 管理路径使用。本文件本身没有直接的跨 crate I/O。

## 错误处理与边界

`NewConstraint` 的所有语法错误都通过 `wrap(ErrInvalidConstraintFormat, label)` 返回，错误文本包含原输入；禁止 TiFlash 正向约束则使用 `ErrUnsupportedConstraint`。错误使用 `Result` 和 `?` 向集合解析层传播，没有日志、重试或降级。

`RestoreConstraint` 在 value 数量不是 1 时返回格式错误，并在详情中打印 values；操作不是 `In`/`NotIn` 时返回格式错误并打印操作。它没有额外校验空 key 或空字符串 value，因此直接构造的异常对象若恰好满足数量/操作要求，仍可能还原成不完整文本。

`NewConstraintDirect` 和 `ConstraintCompatibleWith` 不返回错误。前者把校验责任交给调用方；后者只分类，真正将冲突转成 `ErrConflictingConstraints` 的位置是 `constraints.rs::AddConstraint`。

独立 Rust 测试 `constraint_test.rs` 覆盖：空/错误格式、空 key/value、空白规范化、允许负向 TiFlash/TiFlash Compute、拒绝大小写变体的正向 TiFlash、还原的零/多 value 和未知操作，以及五种典型兼容性关系。当前没有针对“多个逗号 value 的解析与方向性长度差异”的专门单测，修改相关行为时应补充。

## 并发与资源生命周期

所有函数都是同步、无锁、无异步任务、无 channel、无事务、无文件句柄和无网络资源的纯计算函数。返回对象拥有自己的字符串和向量；借用仅在函数调用期间有效。错误路径不会留下部分写入或外部副作用。

时间复杂度方面，解析和还原与输入长度线性相关；兼容性比较最多扫描左侧 `Values`。空间分配主要来自解析时构造 `Vec<&str>`、复制 key/value，以及还原时格式化字符串。当前 `split('=').collect::<Vec<_>>()` 会为分段向量分配内存，但相对 Placement DDL 的低频配置路径通常不是主要成本；若优化实现，必须保持“多个等号非法”的行为。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ddl/placement/constraint.go`，独立测试为 `constraint_test.go`。Rust 四个函数与 Go 的 `NewConstraint`、`NewConstraintDirect`、`RestoreConstraint`、`ConstraintCompatibleWith` 一一对应，三个结果常量也保持 `0/1/2` 顺序。

已核对的等价点包括：最短长度 4、只接受 `+/-`、要求恰好一个等号、trim key/value、拒绝 `+engine=tiflash` 且忽略 value 大小写、按逗号切值、还原仅接受单 value 与 In/NotIn，以及兼容性循环只遍历左侧 values。Rust 的 `impl Into<String>`/`Vec<String>` 替代 Go 的 string/variadic 参数；Go 用 `%w` 保留哨兵错误链，Rust 当前用字符串 `Error` 与前缀匹配表达错误类别，这是错误类型机制上的差异，但现有 Rust 测试明确按错误文本前缀验证。

Rust `constraint_test.rs` 基本复刻 Go 表驱动用例；其中 YAML 两例实际测试 `constraints.rs::NewConstraintsFromYaml`，其余分别覆盖本文件三个有行为的入口。Rust 另有 `bundle_1_aster_unit_test.rs` 对解析、还原、TiFlash 拒绝和集合冲突做贯通验证。

## 扩展指南

- 扩展文本语法时，主要修改 `NewConstraint`，同时评估 `RestoreConstraint` 是否仍能规范化回写；应同步更新 `constraint_test.rs` 和 Go 对照语义，避免 Rust 单边漂移。
- 支持新的 `LabelConstraintOp` 时，必须同时定义输入语法、还原格式、兼容性矩阵，并检查 `constraints.rs::constraintToString` 的指纹编码，否则集合去重、错误展示和指纹可能不一致。
- 改变多 value 行为时，应新增独立测试覆盖逗号项空白、空项、左右长度不同、前缀相同以及参数顺序互换。特别注意当前比较方向性来自 Go，不能仅凭直觉修正。
- 调整 TiFlash 规则时，应同时检查 `common.rs` 的引擎常量和 `bundle.rs` 的 TiFlash/普通规则组职责，避免普通 Placement 规则与专用 TiFlash 规则重叠。
- 若新增仅供测试的逻辑，遵守仓库约定放在独立 `*_test.rs` 中，不把测试模块内嵌进本文件；`lib.rs` 已用 `#[cfg(test)] mod constraint_test` 接线。
- 若改动进入运行时代码，需保留文件顶部 PingCAP Apache License 和 AsterSQL 处理标记，并按仓库要求更新对应测试、执行 `cargo fmt --all`；本次任务仅新增文档，没有修改 Rust。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件；`files --filter pkg/ddl/placement` 确认 Rust/Go 对照文件、测试及模块入口均已索引；`query` 确认四个 Rust 入口和对应 Go 符号；`node --file` 读取并核对 `constraint.rs`、`constraints.rs`、`bundle.rs`、`errors.rs`、`common.rs`、`lib.rs` 及 Rust/Go 测试。`callers/callees` 对精确符号未返回边，因此调用关系又用限定 Rust 文件的符号引用搜索核验，没有把空结果解释为“无人调用”。
- 源码：`pkg/ddl/placement/constraint.rs`（5 个图符号、全部 139 行）。
- 直接调用方：`pkg/ddl/placement/constraints.rs`、`pkg/ddl/placement/bundle.rs`。
- crate 与模块：`pkg/ddl/placement/Cargo.toml`、`pkg/ddl/placement/lib.rs`。
- 错误与常量：`pkg/ddl/placement/errors.rs`、`pkg/ddl/placement/common.rs`。
- Go 对照：`pkg/ddl/placement/constraint.go`、`pkg/ddl/placement/constraint_test.go`。
- Rust 独立测试：`pkg/ddl/placement/constraint_test.rs`；补充贯通证据来自 `bundle_1_aster_unit_test.rs` 和 `constraints_test.rs`。
- DDL 边界：`pkg/ddl/doc.go`、`docs/agents/ddl/README.md`。它们用于确认本文件不承担 job 生命周期；具体结论仍以源码和测试为准。
- 按任务要求不运行 Cargo。交付前执行固定 11 章节结构检查，并人工复核本文能回答文件存在原因、运行路径、边界与安全扩展点。
