# `pkg/ddl/placement/rule.rs`

## 文件定位

`rule.rs` 属于 `astersql-ddl-placement` crate，是 SQL Placement Settings 到 PD placement rule 之间的规则构建层。crate 入口 `pkg/ddl/placement/lib.rs` 将本模块公开再导出，并把 `pdtypes::placement::HttpRule` 别名为这里使用的 `pd::Rule`；因此本文件返回的是可继续装入 `Bundle`、最终提交给 PD 的规则对象，而不是 DDL job、schema state 或持久化记录。

直接上游是 `pkg/ddl/placement/bundle.rs::NewBundleFromConstraintsOptions`：它从 `model::PlacementSettings` 读取通用、Follower、Learner 等约束，配置 `RuleBuilder`，再把返回的 `Vec<Box<pd::Rule>>` 拆箱并加入 `Bundle`。更外层的 `NewBundleFromOptions` 会调用 `Bundle::Tidy` 规范化规则；表/分区 DDL 再使用这些 Bundle。本文件自身不参与 DDL owner 调度、schema 版本推进或回填。

`pkg/ddl/placement/Cargo.toml` 声明 crate 名为 `astersql-ddl-placement`，`lib.rs` 为库入口；本文件直接使用标准库 `HashMap`/`LazyLock`、`regex`、`serde_yaml`，并通过 crate 内再导出的 `pd` 类型连接 `astersql-store-pdtypes`。

## 核心职责

1. 以链式 builder 收集副本角色、显式副本数、是否跳过总数校验及 YAML 约束字符串（`RuleBuilder`）。
2. 区分两类输入：数组形式表示一组共同约束并生成一条规则；字典形式把每个“约束字符串 → 副本数”条目展开成一条规则（`newRules`、`newRulesWithDictConstraints`）。
3. 将角色、副本数和已解析的 `LabelConstraint` 封装成 `pd::Rule`，其余 PD 字段保持默认值（`NewRule`）。
4. 维持 Go 版本的错误分类语义，尤其提前识别 YAML 映射中 `key:1` 这种冒号后缺少空格的输入（`wrongSeparatorRegexp`、`is_complete_mapping_with_wrong_separator`、`getYamlMapFormatError`）。
5. 对字典键中的特殊属性执行预处理；当前 `#evict-leader` 可把 Voter 规则改成 Follower，实际逻辑位于 `constraints.rs::preCheckDictConstraintStr`。

它不负责给规则填写 ID、GroupID、key range、Index 或 Override；这些字段由 `bundle.rs` 的 `Tidy`、`Reset`、`RebuildForRange` 等后续步骤处理。

## 主要符号

- `attributePrefix: &str = "#"`、`attributeEvictLeader: &str = "evict-leader"`：由 `constraints.rs::preCheckDictConstraintStr` 使用的属性协议常量。它们是公开常量，但属性解释不在本文件内完成。
- `RuleBuilder`：公开的可克隆、可调试、具默认值的构建器。四个字段均为私有：`role`、`replicasNum`、`skipCheckReplicasConsistent`、`constraintStr`。
- `NewRuleBuilder() -> RuleBuilder`：返回全默认构建器；调用者通常随后连续调用 setter。
- `RuleBuilder::{SetRole, SetReplicasNum, SetSkipCheckReplicasConsistent, SetConstraintStr}`：均以 `&mut self` 修改状态并返回同一可变引用，支持链式配置。
- `RuleBuilder::BuildRulesWithDictConstraintsOnly`：公开构建入口，只接受字典语义；它不使用 `replicasNum`，直接委托 `newRulesWithDictConstraints`。
- `RuleBuilder::BuildRules`：公开通用入口，先调用 `newRules`，再按需验证显式 `replicasNum` 与所有结果规则的 `Count` 总和一致。
- `NewRule(role, replicas, constraints)`：公开的最小规则构造器，将 `u64 replicas` 转成 `i32 Count`，设置 `Role` 和 `LabelConstraints`，其余字段取 `Default`。
- `wrongSeparatorRegexp: LazyLock<Regex>`：进程内惰性初始化一次的常量正则；正则编译失败会触发 `expect`，但模式为源码常量。
- `is_complete_mapping_with_wrong_separator`：私有的完整映射预检，只在文本首尾为 `{...}` 且命中缺空格模式时报告分隔符错误，避免普通数组内容被误判。
- `getYamlMapFormatError`：私有的错误细分器；无冒号返回 `ErrInvalidConstraintsMappingNoColonFound`，命中错误分隔符返回 `ErrInvalidConstraintsMappingWrongSeparator`，否则不覆盖更具体的副本数错误。
- `newRules`、`newRulesWithDictConstraints`：私有解析主流程，分别承担数组优先分派和字典展开。

本文件没有 trait、条件编译项或异步函数。

## 执行流程

通用路径从 `BuildRules` 开始：

1. `newRules` 先对完整 `{...}` 文本执行错误分隔符预检。
2. 调用 `constraints.rs::NewConstraintsFromYaml` 尝试把输入解析为 YAML 字符串数组；空白输入在该函数中视为空约束集合。
3. 数组解析成功时：若 `replicas > 0`，调用 `NewRule` 生成唯一规则；若 `replicas == 0` 且约束字符串非空，返回 `ErrInvalidConstraintsReplicas`；若副本数和字符串都为空，返回空规则列表。
4. 数组解析失败时，先用 `serde_yaml` 验证输入能否成为 `HashMap<String, i32>`。若也失败，将数组错误和映射错误一起包装为 `ErrInvalidConstraintsFormat`；若成功，则转入字典路径。
5. `newRulesWithDictConstraints` 再次执行错误分隔符预检并解析映射。它先遍历全部条目验证 `count > 0`，确保不会在发现晚到的非法计数前生成部分规则。
6. 对每个合法条目调用 `preCheckDictConstraintStr`，移除属性并可能覆盖角色；随后调用 `NewConstraints` 解析、去重并检查标签冲突，最后用 `NewRule` 生成规则。
7. 返回 builder 后，`BuildRules` 在未设置跳过开关时汇总所有 `Count`。只有显式 `replicasNum != 0` 时才要求总数相等；不相等返回 `ErrInvalidConstraintsReplicas`。

字典专用入口 `BuildRulesWithDictConstraintsOnly` 从第 5 步开始，因此不会接受数组路径，也不会执行 builder 层的总副本数一致性校验。`bundle.rs` 用它解析通用 `Constraints` 的映射形式；Follower/Learner 约束则使用完整 `BuildRules` 路径。

## 数据与状态

`RuleBuilder` 只持有一次构建所需的值，没有全局可变状态。setter 会覆盖先前值；多次调用 `BuildRules` 只读取 builder，可重复执行且不会消耗或清空配置。`role` 在下传时被克隆，约束字符串以借用形式解析。

数组输入生成零条或一条规则；字典输入通常每个映射条目生成一条规则。字典使用 `HashMap`，所以本文件不保证输出规则的迭代顺序；测试 `assert_rule_sets` 会规范排序后比较，上游 `Bundle::Tidy` 也会重新分组、编号和排序。调用者不得把原始字典顺序当作接口契约。

`NewRule` 返回 `Box<pd::Rule>`，由调用者拥有；`bundle.rs::boxed_rules` 随即拆箱为 `Vec<pd::Rule>`。`Count` 的目标类型是 `i32`，而公开输入是 `u64`；当前实现使用 `as i32` 转换，没有在本文件内检查大于 `i32::MAX` 的值，这是扩展或安全审查时需要保留关注的边界。

唯一静态状态是 `wrongSeparatorRegexp`，其初始化结果在进程内共享且初始化后只读。

## 依赖与调用关系

上游主链为：

`PlacementSettings` → `bundle.rs::NewBundleFromOptions` / `newBundleFromOptions` → `NewBundleFromConstraintsOptions` → `NewRuleBuilder` / `NewRule` → 本文件解析流程 → `pd::Rule` → `Bundle::Tidy` / `Reset` / `RebuildForRange` → DDL 侧表或分区 placement 处理。

RustCodeGraph 对 `bundle.rs` 的结果显示：`NewBundleFromConstraintsOptions` 由 `newBundleFromOptions` 调用，后者由公开入口 `NewBundleFromOptions` 调用；`NewFullTableBundles` 的直接调用者包括 `pkg/ddl/persistent_create_table.rs::create_table` 和 `pkg/ddl/persistent_masking_actions.rs::truncate_table`。这说明规则构造是完整 DDL placement 链的纯计算环节，但不是 job 执行器。

直接下游包括：

- `constraints.rs::NewConstraintsFromYaml`：数组 YAML 解析；空白输入归一为空集合。
- `constraints.rs::preCheckDictConstraintStr`：解释 `#evict-leader`，拒绝未知属性。
- `constraints.rs::NewConstraints`：逐条解析 `{+|-}key=value`，并通过 `AddConstraint` 拒绝冲突。
- `errors.rs::{Error, wrap, Err*}`：保持可识别的错误类别前缀及上下文细节。
- `serde_yaml`：数组/映射语法解析；`regex`：Go 兼容错误分类。
- `pdtypes::placement`：经 `lib.rs::pd` 再导出的角色、标签约束和 HTTP Rule 数据结构。

## 错误处理与边界

所有可预期的输入失败都返回 crate 自定义 `Error`，错误字符串以 `errors.rs` 中的类别常量开头，测试据此前缀判断错误类型。主要边界如下：

- 数组和字典都不能解析：`ErrInvalidConstraintsFormat`，细节同时保留两种解析错误。
- 非空数组约束配零副本：`ErrInvalidConstraintsReplicas`；空字符串配零副本合法并返回空列表。
- 字典条目计数小于等于零：通常为 `ErrInvalidConstraintsMapcnt`；若原文暴露缺冒号或错误分隔符，则优先返回对应映射格式错误。
- `{+region=us-east-2:2}` 这类完整映射在进入 `serde_yaml` 前被识别为 `ErrInvalidConstraintsMappingWrongSeparator`。这是 Rust `serde_yaml` 与 Go `yaml.v2` 失败阶段不同而增加的兼容补偿。
- 标签缺少 `+/-`、键值格式非法、约束互相冲突或包含未知 `#` 属性时，错误从 `NewConstraints` / `preCheckDictConstraintStr` 原样传播。
- `BuildRules` 仅在 `replicasNum != 0` 且未跳过校验时比较总数；零值兼作“由字典自行给出数量”的哨兵。

当前正则只针对源码固定模式初始化，`expect("constant regex is valid")` 不受用户输入影响。另一方面，`u64` 到 `i32` 的副本数转换和求和没有显式溢出保护；现有 SQL 上层通常约束实际副本规模，但本文件本身没有证明该上界。

## 并发与资源生命周期

本文件没有锁、任务、通道、事务、网络或文件 IO。构建过程完全同步，临时 `HashMap`、约束向量和规则向量在函数栈及返回值所有权下管理；出错时 Rust 自动释放已分配的临时对象，不会向调用者返回部分构建结果。

`RuleBuilder` 不包含内部同步原语；并发使用应由每个线程持有自己的实例，或只共享不可变借用。`LazyLock<Regex>` 的初始化由标准库保证线程安全，初始化后只读。PD 规则的提交、重试和生命周期均在本文件之外。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/ddl/placement/rule.go`：builder 字段和 setter、两个 Build 入口、`NewRule`、数组优先/字典回退、正计数校验、属性角色覆盖及总副本数校验均保留。`pkg/ddl/placement/Cargo.toml` 的 `package.metadata.porting.go-package` 也明确指向 `pkg/ddl/placement`。

主要语言适配差异是：

- Go 返回 `*RuleBuilder` 和 `[]*pd.Rule`；Rust 返回拥有所有权的 `RuleBuilder` 与 `Vec<Box<pd::Rule>>`，setter 使用 `&mut Self`。
- Go 使用 `yaml.v2.UnmarshalStrict`；Rust 使用 `serde_yaml`。为维持 Go 测试对错误类别的预期，Rust 在完整映射上提前识别冒号后缺空格的形式。
- Go 的空字典/数组结果可表现为 `nil` slice；Rust 统一表现为 `Vec::new()`，调用方应依据内容而非 nil 性判断。
- Go map 与 Rust `HashMap` 都不提供业务顺序保证；Rust 测试显式以集合方式比较规则。
- Go 的 `int(replicas)` 与 Rust 的 `replicas as i32` 都存在目标整数宽度边界，Rust 当前没有额外收紧语义。

`pkg/ddl/placement/rule_test.rs` 的可执行测试覆盖 Rule 克隆独立性，以及 Go `TestNewRuleAndNewRules` 的关键表格：空约束、零副本、列表/字典、零计数、非法语法、错误分隔符、非法/未知属性和 `#evict-leader`。文件前半还有 Go 草稿注释，但真实验证入口是 `rule_clone_is_independent` 与 `new_rule_and_new_rules_match_go_table`；Go 原始基准在 `rule_test.go`。

## 扩展指南

- 新增输入语法时，优先修改 `newRules` 的分派规则，并确保数组失败不会误吞应属于字典路径的输入；同步扩展 `rule_test.rs` 与 Go 对照用例。
- 新增字典属性时，属性协议常量可放在本文件，但解析和角色覆盖应与现有职责一致落在 `constraints.rs::preCheckDictConstraintStr`；必须定义它对 Leader/Voter/Follower/Learner 各角色的行为，并测试未知属性仍被拒绝。
- 修改副本一致性策略时，从 `RuleBuilder::BuildRules` 与 `SetSkipCheckReplicasConsistent` 入手，并同时检查 `bundle.rs::NewBundleFromConstraintsOptions` 中默认 Follower 的跳过条件，避免破坏 Placement Settings 的默认值逻辑。
- 若要求确定性输出，不应依赖 `HashMap` 的自然顺序；应在本文件明确排序或继续由 `Bundle::Tidy` 规范化，并增加顺序契约测试。
- 若扩大可接受副本数，需先处理 `NewRule` 的 `u64 -> i32` 转换及 `BuildRules` 的 `i32` 求和风险，再核对 PD `HttpRule.Count` 和 Go `int` 的兼容边界。
- 新测试必须继续放在独立的 `pkg/ddl/placement/rule_test.rs`，不要内嵌进生产文件；同时根据语义变化更新 `rule_test.go` 对照或记录明确的有意差异。
- 本模块是纯转换层。需要修改规则 ID、范围或合并策略时，应改 `bundle.rs`；需要改变单条 label 语法/冲突规则时，应改 `constraint.rs` / `constraints.rs`，不要把不相关职责堆入 `rule.rs`。

## 验证依据

- 目标实现：`pkg/ddl/placement/rule.rs`，RustCodeGraph `node --file ... --offset 1 --limit 420` 核对了全部 222 行、所有公开/私有符号及分支。
- crate 边界：`pkg/ddl/placement/Cargo.toml` 与 `pkg/ddl/placement/lib.rs`，确认 crate 名、依赖、`pd::Rule` 别名、模块再导出及独立测试装配。
- 上游调用：RustCodeGraph 对 `NewRuleBuilder`、`BuildRulesWithDictConstraintsOnly`、`NewBundleFromConstraintsOptions` 和 `pkg/ddl/placement/bundle.rs` 的查询，以及 `bundle.rs` 第 69 行起的源码；确认通用字典、Follower、Learner 三条构建路径和后续 `Tidy`。
- 下游实现：`pkg/ddl/placement/constraints.rs::{NewConstraints, preCheckDictConstraintStr, NewConstraintsFromYaml}` 与 `pkg/ddl/placement/errors.rs`，确认属性覆盖、冲突检测、空白处理和错误类别。
- Go 对照：`pkg/ddl/placement/rule.go`，逐项核对 builder、解析分派、计数校验及错误语义。
- 独立测试：`pkg/ddl/placement/rule_test.rs` 与 `pkg/ddl/placement/rule_test.go`，核对成功、错误、角色覆盖和克隆边界。按任务约束，本次为纯文档分析，未运行 Cargo 或代码测试。
