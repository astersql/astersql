# `pkg/store/pdtypes/placement.rs`

## 文件定位

本文件属于 `astersql-store-pdtypes` crate（入口见 `pkg/store/pdtypes/lib.rs`，清单见 `pkg/store/pdtypes/Cargo.toml`），定义 AsterSQL 与 PD placement rule JSON 接口共享的数据模型。它不负责解析 SQL placement policy，也不直接调用 PD；上层 `pkg/ddl/placement/lib.rs` 将这些类型再导出，随后由 `constraint.rs`、`rule.rs` 和 `bundle.rs` 完成约束解析、规则构造与规则组编排。

对应生产源码为 [`placement.rs`](placement.rs)。该文件是协议/数据边界，不是调度算法实现：PD 才负责依据这些规则把 Region peer 放置到匹配的 Store。

## 核心职责

1. 用 `PlacementRule<const HTTP: bool>` 描述一条 PD 放置规则，包括所属组、键范围、peer 角色、数量、Store 标签约束、隔离标签及运行时版本信息。
2. 通过类型别名 `Rule = PlacementRule<false>` 与 `HttpRule = PlacementRule<true>` 同时表达本地 `pkg/store/pdtypes.Rule` 和 Go `pd/client/http.Rule` 的不同 JSON 契约；差异集中在 `is_witness`。
3. 用 `PeerRoleType` 和 `LabelConstraintOp` 对已知字符串提供强类型分支，同时用 `Unknown(String)` 无损保留未来 PD 增加的字符串值。
4. 用 `LabelConstraint` 与 `RuleGroup` 表达 Store 标签过滤条件和规则组属性，并通过 serde 属性对齐 Go `encoding/json` 的字段名、零值和 `omitempty` 行为。

本文件不检查 `Count` 是否为正数、不校验键范围顺序、不解释 label 约束是否互相冲突，也不计算规则应用顺序；这些语义由构造层或 PD 负责。例如 `pkg/ddl/placement/rule.rs::RuleBuilder::BuildRules` 校验显式副本数，`constraint.rs::NewConstraint` 校验约束文本。

## 主要符号

- `PlacementRule<const HTTP: bool>`：公共泛型结构体。`GroupID`/`ID` 标识规则；`Index`/`Override` 表达排序与覆盖元数据；`StartKey`/`EndKey` 保存进程内原始字节但被 serde 完全跳过，`StartKeyHex`/`EndKeyHex` 才对应线上的 `start_key`/`end_key`；`Role`、`Count`、`LabelConstraints`、`LocationLabels` 和 `IsolationLevel` 描述放置要求；`Version` 与 `CreateTimestamp` 承载运行时元数据。
- `Rule`：`PlacementRule<false>`。对齐同目录 Go `Rule`，序列化时永远不输出 `is_witness`，反序列化时会接受并忽略任意形状的该字段。
- `HttpRule`：`PlacementRule<true>`。用于需要对齐 Go `pd/client/http.Rule` 的调用者；`is_witness` 总是按布尔值参与序列化，输入若不是布尔值则反序列化失败。`pkg/ddl/placement/lib.rs` 将它重命名再导出为 `pd::Rule`。
- `skip_witness::<HTTP>` / `deserialize_witness::<_, HTTP>`：上述两个 witness 契约的 serde 策略函数。前者决定是否省略字段，后者在本地模式用 `IgnoredAny` 丢弃输入、在 HTTP 模式严格读取 `bool`。
- `is_false`、`is_zero_i32`、`is_zero_u64`：为 `omitempty` 对齐提供的私有序列化谓词。
- `PeerRoleType`：`Voter`、`Leader`、`Follower`、`Learner` 以及 `Unknown(String)`。`Default` 是空字符串形式的 `Unknown`；`as_str`、手写 `Serialize`/`Deserialize` 在 Rust 枚举与 PD 字符串之间转换。`Voter` 等同名公共常量用于保持 Go 风格调用接口。
- `LabelConstraint`：包含 `Key`、`Op`、`Values`，仅保存约束数据，不执行匹配。
- `RuleGroup`：包含 `ID`、`Index`、`Override` 的规则组属性模型。仓库当前生产 Rust 搜索未发现除再导出外的直接消费点；DDL 自己的 `Bundle` 使用相近但不同的 `group_id/group_index/group_override/rules` JSON 结构。
- `LabelConstraintOp`：`Empty`、`In`、`NotIn`、`Exists`、`NotExists` 及 `Unknown(String)`；其手写 serde 与 `PeerRoleType` 相同地保留未知值。`In`、`NotIn`、`Exists`、`NotExists` 公共常量用于 Go 风格调用。

## 执行流程

典型写入流程是：DDL placement 代码从 SQL/元模型取得 placement settings；`pkg/ddl/placement/constraint.rs::NewConstraint` 把 `+key=value` / `-key=value` 转为 `LabelConstraint`；`pkg/ddl/placement/rule.rs::NewRule` 或 `RuleBuilder::BuildRules` 填充角色、数量和约束；`pkg/ddl/placement/bundle.rs` 将规则组合成 `Bundle` 并设置表或分区的键范围；最后上层在与 PD 通信时通过 serde 生成 PD HTTP JSON。

序列化 `PlacementRule` 时，原始 `StartKey`/`EndKey` 被跳过，线协议使用十六进制字符串字段；空向量、空字符串以及指定的零值字段按各自 `skip_serializing_if` 省略。`Role` 和 `LabelConstraint.Op` 通过 `as_str` 输出已知或未知的原字符串。对 `Rule`，witness 字段无条件省略；对 `HttpRule`，该字段作为布尔值输出，即使值为 `false`。

反序列化时，结构体级 `#[serde(default = "Default::default")]` 使缺失字段回落到 Rust `Default`。角色和操作符先读取 `Option<String>`：JSON `null` 与缺失/空字符串分别落到空字符串默认值，已知文本映射到固定枚举，其他文本进入 `Unknown`。`Rule` 的 `is_witness` 输入由 `IgnoredAny` 接受后归零；`HttpRule` 则只接受布尔值。

## 数据与状态

所有模型都拥有自己的 `String`、`Vec` 和枚举值，没有借用外部缓冲区或隐藏的全局状态。派生的 `Clone`、`Debug`、`Eq`、`PartialEq` 便于构造、比较和测试；主要结构体均可 `Default` 初始化。

必须区分内存态与线协议态：`StartKey`/`EndKey` 只存在于内存，JSON 往返不会自动在它们与 `StartKeyHex`/`EndKeyHex` 之间编码或解码。调用者需要在适当的构造层维护两种表示，不能假设 serde 会同步它们。`Version` 和 `CreateTimestamp` 也只是普通字段，本文件不会递增或生成它们。

`Unknown(String)` 是兼容性状态而非错误状态：未知角色或操作符会原样保存并可再次序列化。相比把未知值降为默认枚举，这保证了代理、读取再写回等路径不会静默丢失未来 PD 字段值。

## 依赖与调用关系

直接代码依赖只有 `serde::{Serialize, Deserialize, Serializer, Deserializer}`；`Cargo.toml` 还声明了同 crate 其他模块使用的 `anyhow`、`chrono`、`kvproto`、`tikv-client` 和 `serde_json`，不能据此推断本文件直接使用这些库。`serde_json` 在独立测试中验证线协议形状。

上游生产关系经 RustCodeGraph 与源码核对如下：`pkg/ddl/placement/lib.rs::pd` 再导出全部类型，并特意把 `HttpRule` 暴露为 `pd::Rule`；`constraint.rs::{NewConstraint, NewConstraintDirect}` 构造 `LabelConstraint`；`rule.rs::{RuleBuilder, NewRule}` 构造规则；`bundle.rs::Bundle` 持有 `Vec<pd::Rule>`，并在 placement policy 转换、规则整理及键范围重置流程中使用这些字段。因此，本文件位于“SQL placement 设置 -> DDL 构造/校验 -> PD HTTP JSON”的共享模型边界。

RustCodeGraph 对目标文件报告由 20 个文件引用，并能定位到 DDL placement 的构造链；由于本文件除 serde 方法外没有业务函数，所谓下游调用主要是序列化 trait 调用和对数据字段的消费，而不是主动调用服务。`RuleGroup` 当前未在生产 Rust 中发现明确构造点，应视为已公开但接线有限的兼容模型。

## 错误处理与边界

本文件没有自定义错误类型。序列化方法将底层 `Serializer` 错误直接返回；反序列化方法返回 `D::Error`。已知的宽容边界包括：缺失字段使用默认值；角色/操作符的 JSON `null` 接受为 Go 字符串别名零值；未知字符串被保留；本地 `Rule` 忽略任意类型的 `is_witness`。

严格边界主要来自 serde 的目标类型：`HttpRule.is_witness` 的非布尔输入会失败；需要字符串、整数、列表或结构体的其他字段若类型不匹配也会失败。模型层不会拒绝空 `GroupID`/`ID`、负 `Count`、无效 hex 文本、冲突约束或未知枚举值；若新增调用者绕过 DDL 构造层，就必须自行承担这些业务校验。

JSON 往返还有一个重要边界：被 `#[serde(skip)]` 标记的原始 key 字节会丢失并恢复为默认空向量。若业务需要原始 key，必须显式从 hex 字段解码，或在反序列化后由上层补齐。

## 并发与资源生命周期

该文件不创建线程、异步任务、锁、通道、事务、网络连接或文件句柄，也没有 `Drop` 定制。结构体是纯拥有型值，生命周期由普通 Rust 所有权决定；克隆会复制字符串和向量内容。

并发安全性不由本文件显式承诺或同步。由于字段均由标准拥有型数据组成，是否跨线程共享以及采用 `Arc`/锁/消息传递由调用者决定。序列化只读取值，反序列化创建新值，不维护跨调用缓存。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/pdtypes/placement.go`。字段名和 JSON tag 基本逐项对应：Go 的 `int` 在 Rust 中选为 `i32`，`uint64` 对应 `u64`，`[]byte` 对应 `Vec<u8>`，字符串别名改为可保留未知值的 Rust 枚举。Go `omitempty` 对应私有零值谓词或 `Vec::is_empty`/`String::is_empty`。

Rust `Rule` 有意保持本地 Go `pdtypes.Rule` 不含 witness JSON 字段的契约；额外的泛型底座和 `HttpRule` 用于兼容 DDL 原 Go 代码所导入的 `pd/client/http.Rule`。这是 Rust 侧的接线差异，不表示本地 Go 结构新增了 `IsWitness`。

Go 的 `PeerRoleType`/`LabelConstraintOp` 是开放字符串别名，任意新字符串都可存活；Rust 因而不能只用封闭枚举。`Unknown(String)` 保留这一前向兼容性，`Empty` 或空 `Unknown` 则表达 Go 零值。`placement_test.rs` 进一步确认 JSON `null` 被接受为 Go 字符串零值。

Go 注释说明规则应用次序为 `[GroupIndex, GroupID, Index, ID]`，但本地 `Rule` 自身没有 `GroupIndex`；组级索引由相邻的组/Bundle 表示承担。该排序规则是协议语义背景，本文件没有排序实现。

## 扩展指南

- 新增 PD JSON 字段时，先确认它属于本地 `pdtypes.Rule`、HTTP Rule，还是两者共有；若契约不同，应沿用 `HTTP` 常量泛型或拆出清晰策略，避免把 HTTP 专属字段泄漏到本地 `Rule`。
- 修改字段名、默认值或省略条件时，必须对照 Go JSON tag 与真实 PD API。尤其不要让 `StartKey`/`EndKey` 意外进入 JSON，也不要把 `Rule` 的 witness 忽略语义改成严格解析。
- 扩展角色或操作符时，保留 `Unknown(String)` 回退；新增已知 variant 后同步 `as_str` 与 `Deserialize` 的双向映射，并维持未知值 round-trip。
- 业务校验应优先放在 `pkg/ddl/placement/constraint.rs`、`constraints.rs`、`rule.rs` 或 `bundle.rs`，不要把具体 SQL/DDL 约束塞进这个通用 PD 数据模型。
- 测试必须继续放在独立文件。JSON 零值、未知值及 witness 契约应更新 `pkg/store/pdtypes/placement_test.rs` 或 `migration_aster_unit_test.rs`；DDL 构造/冲突/合并行为则同步对应的 `pkg/ddl/placement/*_test.rs`，不要把测试嵌入 `placement.rs`。
- 兼容风险集中在 JSON 线协议和 Go 零值语义；性能风险主要来自规则集合中字符串/向量的克隆与序列化。新增大字段或频繁复制路径前应检查上层 Bundle 的规模和所有权传递。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/store/pdtypes/placement.rs`；`files --filter pkg/store/pdtypes` 确认同 crate 文件；`node --file pkg/store/pdtypes/placement.rs --offset 1 --limit 500` 读取 313 行完整源码并报告 20 个引用文件；`query` 分别定位 `PlacementRule`、`PeerRoleType`、`LabelConstraint`、`RuleGroup`、`LabelConstraintOp`；聚焦 `explore` 与 `node` 核对 DDL 的 `constraint -> rule -> bundle` 构造链。
- 源码与边界：`pkg/store/pdtypes/placement.rs`、`pkg/store/pdtypes/lib.rs`、`pkg/store/pdtypes/Cargo.toml`、`pkg/ddl/placement/lib.rs`、`pkg/ddl/placement/constraint.rs`、`pkg/ddl/placement/rule.rs`、`pkg/ddl/placement/bundle.rs`。
- Go 对照：`pkg/store/pdtypes/placement.go`，用于核对字段、JSON tag、角色/操作符字符串及规则顺序说明。
- 独立测试：`pkg/store/pdtypes/placement_test.rs` 验证 `null` 的 Go 零值兼容；`pkg/store/pdtypes/migration_aster_unit_test.rs` 验证 JSON 字段形状、未知值 round-trip，以及本地 `Rule` 与 `HttpRule` 的 witness 差异。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前只执行任务指定的 11 章节结构检查，并人工复核上述结论均能回溯到列出的源码或索引结果。
