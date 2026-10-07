# `pkg/ddl/placement/constraints.rs`

## 文件定位

本文件属于 `astersql-ddl-placement` crate，是 Placement Policy 中“成组标签约束”的转换与规范化层。crate 入口 `pkg/ddl/placement/lib.rs` 将本模块的公开函数再导出；输入最终落为 `pd::LabelConstraint`，供 `rule.rs` 生成 PD placement rule、供 `bundle.rs` 组合和整理 rule bundle。它处理的是 DDL 放置策略的纯内存表示，不负责创建 DDL Job、推进 schema state、持久化元数据或直接访问 PD。

`pkg/ddl/placement/Cargo.toml` 将该 crate 对应到 Go 包 `pkg/ddl/placement`，并声明 `serde_yaml`、`sha2`、`base64` 以及提供 PD 类型的 `astersql-store-pdtypes` 等依赖。DDL 层通过 `pkg/ddl/Cargo.toml` 依赖该 crate；此外 session、domain、infoschema、executor、distsql 和部分 store crate 也依赖 placement 门面，但本文件已确认的直接生产调用集中在同 crate 的 `rule.rs` 与 `bundle.rs`。

## 核心职责

1. `NewConstraints` 把多条 `{+|-}key=value` 文本逐条交给单约束解析器 `NewConstraint`，再通过 `AddConstraint` 合并为无重复、无冲突的集合。
2. `NewConstraintsFromYaml` 接收 Placement Settings 中的 YAML 数组表示；空或全空白输入按空集合处理，非数组/非法 YAML 统一映射为 `ErrInvalidConstraintsFormat`。
3. `preCheckDictConstraintStr` 处理字典约束键里的属性项。目前只支持 `#evict-leader`：原角色是 `pd::Voter` 时改为 `pd::Follower`，其他角色保持不变；未知属性报错。
4. `RestoreConstraints` 把约束集合恢复成逗号分隔、逐项带双引号的文本，供展示或错误信息使用。
5. `ConstraintsFingerPrint` 对集合及每条约束的 values 做顺序归一化，再计算 SHA-256/Base64 指纹，供 `Bundle::Tidy` 按等价约束分组。
6. `NewConstraintsDirect` 是明确绕过解析和集合校验的轻量包装器，只应用于调用者已经构造好 `pd::LabelConstraint` 的场景。

## 主要符号

- `pub fn NewConstraints(labels: Vec<String>) -> Result<Vec<pd::LabelConstraint>, Error>`：公开批量入口。保留首见顺序；重复项不追加；任一解析或兼容性检查失败即返回 `Err`。
- `pub(crate) fn preCheckDictConstraintStr(&str, pd::PeerRoleType) -> Result<(Vec<String>, pd::PeerRoleType), Error>`：仅 crate 内可见的字典键预处理器。它按逗号切分，不负责 trim 普通标签；后续 `NewConstraints`/`NewConstraint` 才完成标签解析。
- `pub fn NewConstraintsFromYaml(&[u8]) -> Result<Vec<pd::LabelConstraint>, Error>`：YAML 数组入口。反序列化目标是 `Option<Vec<String>>`，所以 YAML `null` 与空白输入都得到空集合。
- `pub fn NewConstraintsDirect(Vec<pd::LabelConstraint>) -> Vec<pd::LabelConstraint>`：原样返回所有权，不校验单项合法性、重复或冲突。
- `pub fn RestoreConstraints(&[pd::LabelConstraint]) -> Result<String, Error>`：按输入顺序调用 `RestoreConstraint`；空切片恢复为空字符串。
- `pub fn AddConstraint(&mut Vec<pd::LabelConstraint>, pd::LabelConstraint) -> Result<(), Error>`：遍历整个已有集合，依据 `ConstraintCompatibleWith` 区分 compatible、duplicated 和 incompatible。即使先发现重复，也继续检查后续元素，避免重复项掩盖冲突。
- `pub fn ConstraintsFingerPrint(&[pd::LabelConstraint]) -> String`：克隆后排序，不修改调用者集合；返回标准 Base64 编码的 SHA-256。
- `fn constraintToString(&pd::LabelConstraint) -> String`：内部规范化辅助函数，编码为 `key|operation|sorted_values`。已知操作映射为 `in`、`notIn`、`exists`、`notExists`，未知操作保留原字符串。

本文件没有类型、trait、宏或条件编译项；除 `constraintToString` 和 crate 内的 `preCheckDictConstraintStr` 外，其余函数经 `lib.rs` 再导出为 crate 公共 API。

## 执行流程

数组形式的主流程是：`bundle.rs::NewBundleFromConstraintsOptions` 或 `rule.rs::newRules` 取得 Placement Settings 字符串，调用 `NewConstraintsFromYaml`；YAML 解码得到字符串列表后，`NewConstraints` 对每项 trim、调用 `NewConstraint`，随后 `AddConstraint` 与已收集项逐一比较。成功结果进入 `NewRule`，成为 PD rule 的 `LabelConstraints`。

字典形式由 `rule.rs::newRulesWithDictConstraints` 解析 `{约束字符串: 副本数}`。每个键先进入 `preCheckDictConstraintStr`，过滤 `#evict-leader` 并可能覆盖 peer role，再由 `NewConstraints` 解析剩余标签，最终为每个字典项创建一条 rule。数组解析失败并不一定立即失败：`rule.rs::newRules` 会尝试字典格式，并在两者都不成立时生成更具体的映射格式错误。

公共约束与 leader/follower/learner 专属约束合并时，`bundle.rs::NewBundleFromConstraintsOptions` 多次调用 `AddConstraint`。因此角色专属约束若与公共约束冲突，会在 bundle 构建阶段带上 `LeaderConstraints conflicts with Constraints` 等上层上下文返回。

整理 bundle 时，`Bundle::Tidy` 对每条 rule 的 `LabelConstraints` 调用 `ConstraintsFingerPrint`，以指纹为 `HashMap` 键形成 `ConstraintsGroup`，再合并同约束、同角色规则。指纹只用于内存分组标识，不是持久化 ID 或安全签名。

## 数据与状态

核心数据是来自 `pdtypes::placement` 的 `pd::LabelConstraint`，包含 `Key`、`Op` 和 `Values`。本文件不定义新的持久状态；所有函数都只消费参数并返回新值，或在 `AddConstraint` 中修改调用者提供的 `Vec`。

`AddConstraint` 的不变量是：成功追加后，新项与每个既有项兼容；完全重复的新项不会改变集合；发现任一冲突时，在 push 发生前返回，因此该次调用不会修改集合。它不单独验证一条约束是否合法，这一点由 Go 注释、Rust 的 `unknown_constraint` 用例以及实现共同确认；若调用者绕过 `NewConstraint`，未知操作仍可能在“不同 key”等兼容场景中被加入。

指纹规范化分两层：每条约束的 `Values` 克隆并排序，约束列表再按规范字符串排序。因此集合顺序和 values 顺序不影响结果；原始切片及其中 values 不被改写。编码未加入条目长度前缀，当前兼容约定完全由 `constraintToString` 的分隔格式定义，修改它会改变所有既有指纹。

## 依赖与调用关系

上游直接调用证据：

- `pkg/ddl/placement/rule.rs::newRules` → `NewConstraintsFromYaml`，用于数组形式约束。
- `pkg/ddl/placement/rule.rs::newRulesWithDictConstraints` → `preCheckDictConstraintStr` → `NewConstraints`，用于字典形式约束及 `#evict-leader`。
- `pkg/ddl/placement/bundle.rs::NewBundleFromConstraintsOptions` → `NewConstraintsFromYaml`/`AddConstraint`，用于 common 与各角色约束的解析和合并。
- `pkg/ddl/placement/bundle.rs::Bundle::Tidy` → `ConstraintsFingerPrint`，用于等价约束分组。
- `bundle.rs` 的规则构造路径还调用 `NewConstraintsDirect`，将已经构造的约束直接交给 `NewRule`。

下游依赖证据：`NewConstraints` 调用 `constraint.rs::NewConstraint` 和本文件 `AddConstraint`；`AddConstraint` 调用 `ConstraintCompatibleWith`、`RestoreConstraint` 以及 `errors.rs::wrap`；`RestoreConstraints` 调用 `RestoreConstraint`；指纹路径调用 `sha2::Sha256` 与 `base64` 标准编码器。YAML 路径调用 `serde_yaml::from_slice`。

RustCodeGraph 的文件节点显示 `constraints.rs` 被 placement 相关测试以及 DDL 测试文件引用；对生产调用的精确核对以符号搜索和上述 `rule.rs`、`bundle.rs` 源码为准。索引对常见符号名的裸 `callers/callees` 查询未输出可消歧的边，因此没有把宽泛查询结果当成调用事实。

## 错误处理与边界

- 空 `labels`、空字节、全空白字节以及 YAML `null` 都成功得到空 `Vec`。非法 YAML、映射而非数组等反序列化失败统一丢弃底层 YAML 文本并返回 `ErrInvalidConstraintsFormat`。
- `NewConstraints` 对每项先 `trim`；字典预处理本身按原始逗号切分。合法性、保留标签和值的限制由 `NewConstraint` 决定。
- 未知 `#` 属性通过 `wrap(ErrUnsupportedConstraint, ...)` 返回；`#evict-leader` 只把 Voter 降为 Follower，对 Learner 等角色无影响。
- 冲突错误为 `ErrConflictingConstraints`，上下文尽量包含双方的可读恢复文本；若恢复本身失败，则退回使用恢复错误字符串，保证冲突仍可报告。
- `RestoreConstraints` 一旦遇到不可恢复的操作立即返回错误，不返回部分字符串。
- `NewConstraintsDirect` 明确没有检查，是扩展时最容易误用的边界；不可信文本必须走 `NewConstraints` 或 `NewConstraintsFromYaml`。
- `ConstraintsFingerPrint` 对空集合也生成确定的 SHA-256/Base64 字符串，而不是空串；它保证当前编码下的顺序无关性，但不承诺跨编码变更保持稳定。

## 并发与资源生命周期

本文件没有锁、原子变量、channel、异步任务、事务、I/O 句柄或全局可变状态。除 `AddConstraint` 的独占 `&mut Vec` 外，函数只使用局部所有权和不可变借用，生命周期止于函数返回。

`ConstraintsFingerPrint` 与 `constraintToString` 会克隆约束列表或 values，换取不修改调用者数据和确定性排序；复杂度主要是约束排序及每项 values 排序，约为 `O(n log n + Σ mᵢ log mᵢ)`，并有相应临时内存。若在大批 rule 的热路径扩展该逻辑，应评估这些克隆和字符串拼接成本；当前调用点主要位于 placement 配置构建与 bundle 整理，而非逐行 SQL 数据处理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/placement/constraints.go`，主要算法和错误分类保持一致：批量解析逐项调用 `NewConstraint`/`AddConstraint`，重复跳过但继续扫描冲突，恢复格式相同，指纹均为规范字符串排序后的 SHA-256 标准 Base64。

需要注意的表示差异：Go 的空输入常返回 `nil` slice，而 Rust 统一返回空 `Vec`；Go `NewConstraintsDirect` 是 variadic，Rust 接收一个 `Vec`；Go 错误返回可携带构造到一半的 slice，Rust `Result<Vec<_>, Error>` 的错误分支不携带部分集合。调用者只依赖成功值或错误种类时语义一致，但新增调用者不得依赖 Go 的部分结果行为。

YAML 实现也有库差异：Go 使用 `yaml.UnmarshalStrict` 解码到空 slice，Rust 使用 `serde_yaml` 解码为 `Option<Vec<String>>`，并显式把全空白输入和 `null` 视为空集合。`rule.rs` 对 serde_yaml 与 Go yaml.v2 在错误分类上的差异另有预判和回退逻辑，因此不应只修改本文件来改变字典格式兼容性。

独立测试对应关系：Go 的 `pkg/ddl/placement/constraints_test.go` 覆盖构造、追加和恢复；Rust 的 `constraints_test.rs` 恢复这些表驱动语义，并增加空白 YAML、字典属性及指纹顺序无关性验证。`constraint_test.rs` 另验证 `[]` 成功和畸形 YAML 失败。

## 扩展指南

- 新增一种单标签语法或操作符时，应优先修改 `constraint.rs::NewConstraint`、`RestoreConstraint`、`ConstraintCompatibleWith`，再检查本文件 `constraintToString` 是否需要稳定的新编码；同步更新独立的 `constraint_test.rs` 与 `constraints_test.rs`，并对照 Go 文件及测试。
- 新增字典属性时，在 `preCheckDictConstraintStr` 中显式定义其角色变换及与现有属性组合的规则，并补充 Voter、Follower、Learner、未知属性和多属性测试；同时检查 `rule.rs::newRulesWithDictConstraints` 的副本数和格式错误路径。
- 改动冲突合并规则时，必须保留“重复不能提前终止后续冲突检查”的不变量，并覆盖集合不被失败调用部分修改的回归用例。
- 改动指纹格式前，应确认 `Bundle::Tidy` 分组行为和 Go 兼容性。应测试约束顺序、values 顺序、未知操作、空集合及潜在分隔歧义；这类改动可能改变规则合并结果，具有兼容性和性能风险。
- 解析外部 Placement Settings 时不要调用 `NewConstraintsDirect`。该函数只适合类型安全、调用点可审计的内部构造；新增生产调用应同时提供独立测试，而不是把测试写入源文件。
- 本文件属于纯转换层。若扩展涉及 DDL Job 持久化、schema version 或 PD 下发生命周期，应沿 DDL/placement 的真实上层调用链实现，不能在此引入全局状态或后台任务。

## 验证依据

已读取并核对：

- 目标源码：`pkg/ddl/placement/constraints.rs`（全部 151 行）。
- crate 边界：`pkg/ddl/placement/Cargo.toml`、`pkg/ddl/placement/lib.rs`、`pkg/ddl/Cargo.toml` 的 placement 依赖。
- 直接生产调用：`pkg/ddl/placement/rule.rs` 的 `newRules`/`newRulesWithDictConstraints`，`pkg/ddl/placement/bundle.rs` 的 `NewBundleFromConstraintsOptions`/`Bundle::Tidy`。
- Go 对照：`pkg/ddl/placement/constraints.go`。
- 独立测试：`pkg/ddl/placement/constraints_test.rs`、`pkg/ddl/placement/constraint_test.rs`、`pkg/ddl/placement/constraints_test.go`；另通过搜索确认 `bundle_1_aster_unit_test.rs` 对去重和冲突有集成式覆盖。
- DDL 包契约：`pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`；本文件不承担其中描述的 Job/owner/schema-sync 生命周期。

RustCodeGraph 证据包括：`status` 确认索引含 7032 个 Rust 文件且目标目录已索引；`files --filter pkg/ddl/placement` 列出 Rust/Go 源与独立测试；`node --file` 读取目标、Go 对照、模块入口、调用点和测试；`query` 确认本文件的公开符号。随后用 `rg` 对精确符号补充调用边消歧。任务为纯文档分析，按计划未运行 Cargo。
