# `pkg/ddl/label/rule.rs`

## 文件定位

`rule.rs` 是 `astersql-ddl-label` crate 中把 SQL 表属性转换为 PD Label Rule 的核心实现。crate 入口 `pkg/ddl/label/lib.rs` 声明并重新导出 `rule` 模块，因此调用者可直接使用 `NewRule`、`NewRuleID`、`RestoreRuleID`、`NewRulePatch` 等 API。`pkg/ddl/label/Cargo.toml` 表明该 crate 默认采用 Classic 语义，`nextgen` feature 会同时启用内核类型和 table codec 的 NextGen 支持。

在完整应用中，本文件位于 DDL 元数据变更与 PD Region Label 配置之间：例如 `pkg/session/runtime/create_table_resources.rs::update_labels` 在表或分区重命名时，用 `NewRuleID` 查找旧规则，克隆已有规则后调用 `Rule::Reset` 改写规则 ID、内部标签和键范围，最后通过 `astersql_domain_infosync::UpdateLabelRules` 提交更新。这里不创建或执行 DDL job，也不管理 schema state、reorg 或 schema version；它只构造要交给上层发送的 PD 数据。

## 核心职责

- `Rule::ApplyAttributesSpec` 将 AST 的 `AttributesSpec` 转成经过校验的 `pd::RegionLabel` 列表；`Default=true` 表示清空标签。
- `Rule::Reset` 把一条已有规则重新绑定到指定数据库、表、可选分区及一组物理 table ID，并同时生成 PD 所需的 key-range 数据。
- `UseKeyspaceAwareRules`、`NewRuleID` 和 `RestoreRuleID` 在 Classic 与 NextGen 部署间维持规则 ID 的内部/用户可见表示。
- `NewRule`、`Rule::Clone`、`Rule::String` 和 `NewRulePatch` 提供创建、复制、JSON 表示及批量更新封装。
- 常量 `RuleIndexDefault`、`RuleIndexDatabase`、`RuleIndexTable`、`RuleIndexPartition` 固定规则优先级层次；本文件的 `Reset` 实际只写表级或分区级索引。

## 主要符号

- `pub type Rule = pd::LabelRule`：别名指向 `lib.rs` 中可序列化的 PD 规则形状，字段为 `ID`、`Index`、`Labels`、`RuleType` 和 `Data`。别名让本文件的 inherent methods 直接操作该结构。
- `NewRule() -> Box<Rule>`：返回默认规则。使用 `Box` 对齐 Go `*Rule` 的所有权形状，而不是建立共享状态。
- `Rule::ApplyAttributesSpec(&mut self, &ast::AttributesSpec) -> Result<(), Error>`：将属性文本包成 YAML 数组解析为 `Vec<String>`，再调用 `attributes.rs::NewLabels` 做严格的 `key=value` 校验、去重和冲突检测。
- `Rule::String(&self) -> String`：调用 `serde_json::to_string`；序列化失败时返回空字符串，与 Go 版本吞掉 marshal 错误的约定一致。
- `Rule::Clone(&self) -> Box<Rule>`：执行结构的深拷贝。`String`、`Vec<RegionLabel>` 与 `serde_json::Value` 均被复制，不共享可变容器。
- `Rule::Reset(...) -> &mut Rule`：核心重绑定操作。它总是先写 `ID`；仅当 `Labels` 非空时，才继续改写内部标签、`RuleType`、`Data` 和 `Index`。
- `UseKeyspaceAwareRules(tikv::Codec) -> bool`：只有 `kerneltype::IsNextGen()`、codec 非空且带 `KeyspaceMeta` 三者同时成立时返回真。
- `NewRuleID(...) -> String`：生成 `schema/<db>/<table>` 或 `schema/<db>/<table>/<partition>`；keyspace 感知时加上 `keyspace/<id>/`。
- `RestoreRuleID(&str) -> String`：NextGen 中仅对严格形如 `keyspace/<任意一段>/schema/...` 的 ID 去掉前两段；其他输入原样返回。
- `NewRulePatch(Vec<Box<Rule>>, Vec<String>) -> Box<pd::LabelRulePatch>`：将待设置规则和待删除 ID 原样装入 PD patch 的 `sets`/`deletes` 字段。

## 执行流程

1. 上层用 `NewRule` 创建空规则，或从 infosync 读取并克隆已有规则。
2. 对 SQL `ATTRIBUTES` 输入，`ApplyAttributesSpec` 先处理 `Default`；非默认输入被拼成 `[<attributes>]` 后由 `serde_yaml` 解析，随后 `NewLabels` 校验每个属性。解析或语义校验失败时保留错误并终止。
3. `Reset` 根据 `partName` 是否为空判定表规则或分区规则，并通过 `UseKeyspaceAwareRules` 固定本次调用的编码模式。
4. `Reset` 调用 `NewRuleID` 写入内部规则 ID。如果当前 `Labels` 为空，函数立即返回；这是默认/空 attributes 只保留 ID、不生成有效 key-range 规则的既有语义。
5. 对非空标签，函数遍历现有标签：更新已存在的 `db`、`table`、可适用的 `partition` 与 keyspace 标签；缺少的内部标签按 keyspace、db、table、partition 的顺序补入。Classic 模式下已有 `keyspace` 标签不会被当作内部标签改写。
6. 函数设置 `RuleType = "key-range"`，对传入的 `ids` 升序排序，然后逐个用 `tablecodec::GenTablePrefix(id)` 和 `GenTablePrefix(id.wrapping_add(1))` 构造半开区间 `[start, end)`。`wrapping_add` 明确保留 Go `int64` 在最大值处回绕的行为。
7. Classic 路径直接用 `codec::EncodeBytes` 编码表前缀；NextGen 路径调用 `tikv::Codec::EncodeRegionRange`，让 keyspace 前缀参与整个 Region 边界编码。结果以小写十六进制写入 `Data` 数组的 `start_key`/`end_key`。
8. 最后把 `Index` 设为表级 `2` 或分区级 `3` 并返回同一个可变规则。上层可将若干规则封装成 `LabelRulePatch` 并交给 infosync/PD。

## 数据与状态

本文件没有全局可变状态。模块常量包括 ID 前缀、规则类型、四档索引及两个格式说明常量。单条 `Rule` 的所有状态都由调用者持有：

- `ID` 是 PD 规则的唯一字符串标识；内部形式可能包含 keyspace 前缀。
- `Labels` 包含用户标签和由 `Reset` 注入的定位标签。现有同名内部标签会被覆写，缺少项会追加；用户标签顺序保持不变。
- `Data` 是 JSON 数组，每个物理 ID 对应一个 key range。由于 `ids.sort()`，输出顺序与调用者传入顺序无关且稳定；重复 ID 不会去重，会生成重复区间。
- `RuleType` 在有标签的 `Reset` 中成为 `key-range`，`Index` 成为表级或分区级。若标签为空，早返回意味着这些字段不会被清理或重建；对新规则它们保持默认值，对复用规则则保留旧值。调用者不可把“只有 ID 的规则”误认为完整可提交规则。
- `TableIDFormat` 和 `PartitionIDFormat` 只是公开的占位符说明字符串；`NewRuleID` 当前直接用 `format!` 拼接，没有解析这两个常量。

## 依赖与调用关系

上游直接证据：

- RustCodeGraph 将 `pkg/session/runtime/create_table_resources.rs` 列为生产使用文件；其中 `update_labels` 调用 `NewRuleID` 生成表/分区旧 ID，读取规则后调用 `Rule::Reset` 生成重命名后的规则，再交给 infosync 更新。
- `pkg/ddl/label/lib.rs` 以 `pub mod rule; pub use rule::*;` 暴露本文件，并定义其依赖的 `pd::LabelRule`、`pd::LabelRulePatch` 与轻量 `tikv::Codec`。
- workspace/Cargo 清单显示该 crate 还被 `pkg/ddl`、`pkg/domain/infosync`、`pkg/executor` 和 `pkg/store/gcworker` 依赖；仅凭依赖声明不能推断它们都直接调用本文件 API。

下游调用：

- `serde_yaml` 解析 `AttributesSpec.Attributes`，`attributes.rs::NewLabels` 完成逐项格式和冲突校验。
- `kerneltype::IsNextGen` 与 `tikv::Codec::{is_nil, GetKeyspaceMeta, GetKeyspaceID, EncodeRegionRange}` 决定 keyspace 作用域及边界编码。
- `tablecodec::GenTablePrefix` 生成每个物理表/分区的原始前缀，`codec::EncodeBytes` 提供 Classic mem-comparable 编码，`hex::encode` 生成 PD JSON 字符串。
- `serde_json` 同时承担整条规则序列化与 `Data` 数组构造。

RustCodeGraph 的文件级关系还列出 `rule_test.rs`、`migration_aster_unit_test.rs` 和 `pkg/ddl/placement/meta_bundle_test.rs`；前两者直接验证本文件，后者属于测试引用。对通用方法名 `Reset` 的图查询发生全库同名歧义，因此生产调用边另用精确源码搜索确认，没有把歧义结果当作事实。

## 错误处理与边界

- `ApplyAttributesSpec` 是本文件唯一显式返回 `Result` 的业务入口。YAML 解析错误转换为 `Error::InvalidAttributesSpec`；成功解析后，缺等号、空 key/value、额外等号或同 key 不同 value 等错误由 `NewLabels` 返回。
- `Default=true` 无条件清空原有 `Labels`。空 attributes 可被解析为空列表；两者在后续 `Reset` 中都触发早返回。
- `String` 故意不传播 JSON 序列化错误，而返回空串。当前 `Rule` 字段可序列化，因此常规数据下不会失败，但调用方仍不应把空串当合法 JSON。
- 名称未经转义便进入 `/` 分隔的 ID；本文件没有验证数据库、表或分区名。调用者必须保证名称满足系统上游约束，否则 `RestoreRuleID` 的路径判断可能无法表达预期层级。
- `RestoreRuleID` 不校验 keyspace ID 是否为数字，只检查段位置与前缀；这是与 Go 版本一致的字符串兼容逻辑。
- `Reset` 接受空 `ids`，会生成空 `Data` 数组；接受重复或负数 ID，并按 `GenTablePrefix` 的行为编码。本文件不验证 ID 的业务合法性。
- `id.wrapping_add(1)` 处理 `i64::MAX`，对应的结束前缀使用 `i64::MIN`，避免 Rust debug 构建 panic，并由独立回归测试固定。
- keyspace codec 的 24 位 ID 上限由 `lib.rs::tikv::NewCodecV2` 校验，不在本文件重复校验。

## 并发与资源生命周期

所有 API 都是同步、内存内操作，不启动任务、不持有锁、不使用通道，也不执行网络 I/O。`Rule::Reset` 需要 `&mut self`，Rust 借用规则防止同一规则被并发修改；codec 和字符串按值传入，部分内部调用通过 `clone` 获得短生命周期副本。

`NewRule`、`Clone` 与 `NewRulePatch` 返回 `Box`，其释放由 Rust 所有权自动管理。`NewRulePatch` 消费 `setRules` 和 `deleteRules`，不会保留对调用者容器的借用。真正的 PD 请求、重试和远端一致性由 infosync/上层负责，本文件没有回滚、超时或资源清理阶段。

从 DDL 框架角度看，这是一条元数据配置构造路径，而非 job worker：它自身没有持久化检查点、owner failover、schema state 迁移、reorg/backfill、MDL 或 schema sync 行为。是否在某个 DDL job 阶段调用及失败后如何补偿，取决于上游调用者。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ddl/label/rule.go`，Rust 保留了相同的公开符号名、常量数值和主要分支：严格解析 attributes、空标签早返回、内部标签注入顺序、ID 格式、ID 排序、Classic/NextGen 编码、规则索引和 patch 形状均对应 Go 实现。

需要注意的语言映射：

- Go 用 `type Rule pd.LabelRule`，Rust 使用类型别名；Go 指针由 Rust `Box<Rule>` 表达。
- Go `Clone` 的结构赋值对当前字段形成值复制；Rust 的派生 `Clone` 对容器执行深拷贝，外部可观察行为一致且更明确。
- Go `Data` 是 `any` 持有 `[]any/map[string]string`，Rust 固定为 `serde_json::Value::Array`，序列化后的 PD JSON 形状一致。
- Go variadic `ids ...int64` 对应 Rust `Vec<i64>`；两者都会原地排序各自拥有的参数集合。
- Go `ids[i]+1` 在 `int64` 边界回绕；Rust 明确使用 `wrapping_add(1)` 保留该行为。
- Rust 的 `Error::InvalidAttributesSpec` 包装 YAML 错误类型，而 Go 直接返回 YAML error；调用者都能判断失败，但具体错误类型不同。

`pkg/ddl/label/rule_test.rs` 基本逐项对齐 `rule_test.go`，并额外覆盖最大 table ID 回绕；`migration_aster_unit_test.rs` 进一步验证 ID 排序、nil/V1/V2 codec 判定、JSON 与 patch 形状。当前证据表明该文件不是桩或未接线门面，至少已接入 Rust 的表资源重命名标签更新路径。

## 扩展指南

- 新增规则类型或索引层级时，应首先修改 `Rule::Reset` 中 `RuleType`、`Index` 和 `Data` 的共同不变量，并同步 `pd::LabelRule` 的序列化约定；不要只增加常量而遗漏 PD 数据形状。
- 改变规则 ID 结构时，必须成对更新 `NewRuleID` 与 `RestoreRuleID`，并检查 `pkg/session/runtime/create_table_resources.rs::update_labels` 的旧规则查询/删除逻辑；还要评估已存 PD 规则 ID 的向后兼容与清理策略。
- 增加内部标签时，在 `attributes.rs` 中定义键及恢复过滤策略，在 `Reset` 中同时处理“已有则覆写”和“缺少则追加”，并固定追加顺序以避免测试和序列化漂移。
- 改变 key range 编码时，需分别验证 Classic `EncodeBytes` 与 NextGen `EncodeRegionRange`，特别是半开区间、keyspace 前缀、空/极值 ID 和多个无序 ID。错误编码会令 PD 把标签绑定到错误 Region，属于高正确性风险。
- 回归测试应放在独立文件 `pkg/ddl/label/rule_test.rs`；跨模块迁移不变量可补充在 `migration_aster_unit_test.rs`，并同步检查 Go 的 `rule_test.go` 意图。不要把测试嵌入 `rule.rs`。
- 若扩展涉及远端提交、重试或 DDL 生命周期，应在 infosync 或 DDL 调用层实现，本文件保持确定性的规则构造职责。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/ddl/label` 确认目标及 Go/Rust 测试文件；`node --file pkg/ddl/label/rule.rs --offset 1 --limit 400` 读取目标文件 281 行并报告 22 个符号、4 个使用文件；对主要符号执行了 `query`。`Reset` 的 callers/callees 因同名符号无法可靠消歧，已明确降级为精确源码搜索。
- 源码与边界：`pkg/ddl/label/rule.rs`、`pkg/ddl/label/attributes.rs`、`pkg/ddl/label/errors.rs`、`pkg/ddl/label/lib.rs`。
- crate 与 feature：`pkg/ddl/label/Cargo.toml`，以及 workspace 和直接消费者的 Cargo 清单搜索结果。
- 生产入口：`pkg/session/runtime/create_table_resources.rs::update_labels`。
- Go 对照：`pkg/ddl/label/rule.go`。
- 独立测试：`pkg/ddl/label/rule_test.rs`、`pkg/ddl/label/migration_aster_unit_test.rs`、`pkg/ddl/label/rule_test.go`。覆盖合法/非法属性、Default/空属性、表/分区 Reset、Classic/NextGen、规则恢复、ID 排序、JSON/Clone/Patch 与 `i64::MAX` 回绕。
- 本任务是纯文档分析，按计划不运行 Cargo；最终结构检查要求本文恰好包含约定的 11 个二级章节。
