# `pkg/resourcegroup/ruv2/model.rs`

## 文件定位

本文件属于 `astersql-resourcegroup` crate；crate 根 `pkg/resourcegroup/lib.rs` 公开 `ruv2`，`pkg/resourcegroup/ruv2/mod.rs` 再公开 `model`，因此其他 crate 通过 `astersql_resourcegroup::ruv2::model` 使用这里的类型和函数。`pkg/resourcegroup/Cargo.toml` 表明该 crate 的 Go 对照包是 `pkg/resourcegroup`，本文件唯一直接外部依赖是带 derive 功能的 `serde`。

它是 RU v3 语句计量的纯数据与数学内核：接收执行层采集的原始单位，以配置权重计算总 RU。它不采集执行统计、不读取全局配置，也不负责 TiFlash 溢价、按引擎归因或上报；这些接线分别位于 `pkg/executor/statement_ru_plan_walk.rs`、`pkg/executor/statement_ru_result.rs` 和 `pkg/executor/statement_ru_reporting.rs`。

## 核心职责

- 用 `StmtUnits` 表达一次语句计算涉及的 11 类原始工作量，包括 CPU、扫描/网络字节、跨可用区网络子集、编译字节、哈希状态行数、Join 输出行数和写入相关单位。
- 用 `StmtWeights` 表达上述单位的逐项系数，并为配置反序列化提供 kebab-case 字段名；`cross_az_net_byte` 被 `#[serde(skip)]` 排除，当前不能由该结构的序列化配置赋值。
- 用 `StmtUnits::valid`、`StmtWeights::validate` 和 `DDLWeights::validate` 拒绝负数、NaN 与正负无穷；原始单位还必须满足 `cross_az_net_bytes <= net_bytes`。
- 用 `StmtUnits::add`/`sub` 做不带校验的逐字段值运算，供执行层聚合或计算增量；最终合法性由调用者或 `calculate` 检查。
- 用 `calculate` 完成 11 项权重与原始单位的点积，并再次拒绝溢出为无穷或产生 NaN 的结果。

## 主要符号

- `pub struct StmtUnits`：`Clone + Copy + Default + PartialEq` 的原始计量快照。全部字段为公开 `f64`，零值可表示尚无消费。
- `StmtUnits::valid(self) -> bool`：验证所有字段有限且非负，并验证跨 AZ 网络字节是总网络字节的子集。
- `StmtUnits::add(self, other) -> Self` 与 `sub`：返回逐字段和/差，不修改原值，也不保证结果合法。`Copy` 语义使其适合快照和固定数组聚合。
- `pub struct StmtWeights`：语句权重配置。结构体支持 `Serialize`/`Deserialize`，`#[serde(default, rename_all = "kebab-case")]` 使缺失字段回落到派生的全零 `Default`，而不是 `default_weights()` 返回的全一占位权重。
- `default_weights() -> StmtWeights`：除 `cross_az_net_byte` 外将所有语句权重设为 `1.0`；跨 AZ 权重保留为 `0.0`，避免在普通 `net_byte` 已收费后默认重复收费。
- `StmtWeights::validate(self) -> Result<(), String>`：按固定字段顺序返回第一个非法权重的可读错误。
- `pub struct DDLWeights` 与 `default_ddl_weights()`：描述事务 KV 与 ingest KV 字节的两个 DDL 系数，默认均为 `1.0`；当前 Rust 生产接线没有使用本文件的该类型，见“依赖与调用关系”。
- `DDLWeights::validate(self) -> Result<(), String>`：按 `txn-kv-bytes`、`ingest-kv-bytes` 顺序验证系数。
- `pub struct StmtResult { pub total_ru: f64 }`：成功计算后的单一总量。
- 私有 `valid_values(&[f64]) -> bool`：统一执行“有限且非负”谓词；私有 `invalid_weight_error` 统一错误文本，并将正负无穷显式渲染为 `+Inf`/`-Inf`。
- `calculate(units, weights) -> Option<StmtResult>`：公开计算入口。任何输入或最终点积非法时返回 `None`。

## 执行流程

语句主链先在 `pkg/executor/statement_ru_plan_walk.rs` 中把各执行算子的统计累积到 `StmtUnits`。`StatementRUFullReport::add`（`pkg/executor/statement_ru_reporting.rs`）用 `StmtUnits::add` 聚合固定的引擎/算子槽位；计划遍历则用 `sub(before_subtree)` 取得子树增量，并用 `add(root_owned_units)` 补入根节点持有的单位。

普通语句收尾时，`StatementRUCalculator::finalize`（`pkg/executor/statement_ru_result.rs`）调用 `current_statement_ru_weights()` 从全局 `RUV2Config.stmt_weights` 取得权重，再调用 `model::calculate(self.units, weights)`：

1. `calculate` 先调用 `StmtUnits::valid`，拒绝负数、非有限数以及跨 AZ 字节大于总网络字节的快照。
2. 调用 `StmtWeights::validate`；只关心成功与否，具体错误文本在此入口不会向上返回。
3. 对 11 对字段逐项相乘后求和。跨 AZ 项是普通网络项之外的附加项；默认权重为零。
4. 用 `valid_values` 再验证总和，以捕获有限输入相乘或求和后溢出的情况。
5. 成功时返回 `Some(StmtResult)`；执行层随后另行施加 TiFlash multiplier、计算引擎归因并冻结报告。

EXPLAIN 路径在 `pkg/executor/statement_ru_plan_walk.rs` 中分别对算子自身单位和累计单位调用相同的 `calculate`。任一调用返回 `None`，该算子的 RU 结果被标记为 `Invalid`，而不是使用部分值。

## 数据与状态

本文件所有业务数据都是按值传递的 `f64` 集合，不含引用、堆容器或全局可变状态。`StmtUnits` 和权重结构的派生 `Default` 均为全零；业务占位默认权重必须显式通过 `default_weights()`/`default_ddl_weights()` 取得，二者不能混用。

`StmtUnits` 的重要不变量是每项非负且有限，并且 `cross_az_net_bytes` 是 `net_bytes` 的子集。`add` 和 `sub` 刻意不维持这些不变量：差值可能为负，加法可能溢出，跨 AZ/总网络的分量组合也可能失配。因此中间快照允许暂时不合法，但进入计费结果前必须经过 `valid` 或 `calculate`。

`StmtWeights` 的 serde 字段采用 kebab-case，例如 `cpu-work`。`cross_az_net_byte` 被跳过，序列化时不输出、反序列化时取默认零值。`pkg/config/config.rs` 将该类型作为 `RUV2Config.stmt_weights`，并在 `RUV2Config::default` 中调用 `default_weights()`，所以整个配置段缺省时得到占位全一权重；若用户显式提供部分 `stmt-weights` 对象，缺失字段依据结构自身的 serde default 为零，这一点是扩展配置时需要保持或明确迁移的兼容语义。

## 依赖与调用关系

上游生产调用关系经 RustCodeGraph 文件使用关系和 `rg` 交叉核对如下：

- `pkg/config/config.rs` 重导出 `StmtWeights`，并用 `default_weights()` 初始化 `RUV2Config.stmt_weights`。
- `pkg/executor/statement_ru_result.rs` 保存 `StmtUnits`/`StmtResult`，在 `StatementRUCalculator::finalize` 中调用 `calculate` 得到语句总 RU。
- `pkg/executor/statement_ru_reporting.rs` 用 `StmtUnits::add` 聚合完整报告，并直接读取 `StmtWeights` 计算按引擎归因；该归因函数不是本文件 `calculate` 的内部步骤。
- `pkg/executor/statement_ru_plan_walk.rs` 生产和组合 `StmtUnits`，并为 EXPLAIN 的 self/cumulative RU 两次调用 `calculate`。
- `pkg/executor/adapter.rs` 的接口携带 `StmtUnits`，但当前相关参数以下划线命名，不能据此认定它负责模型计算。

下游只依赖 `serde::{Serialize, Deserialize}` 和标准库浮点运算/格式化，没有 I/O 或服务调用。RustCodeGraph 对目标文件报告 7 个使用文件，但精确 `callers/callees` 命令未产出可用边；上述生产调用点因此由源码符号搜索补证。

本文件的 `DDLWeights`/`default_ddl_weights` 在 Rust 生产代码中没有直接引用；`pkg/config/config.rs` 另行定义了自己的 `DDLWeights`。Go 版本则在同一模型文件定义 DDL 类型并由 Go 配置/DDL 链使用。故 Rust 的 DDL 模型目前应描述为“已移植并受单元测试覆盖，但尚未接入配置主链”，不能写成已经参与 DDL 计费。

## 错误处理与边界

`StmtUnits::valid` 使用布尔结果；`calculate` 将所有失败压缩为 `None`，不区分非法单位、非法权重或结果溢出。需要诊断具体权重时，调用者应先调用 `StmtWeights::validate`/`DDLWeights::validate`，其错误会点名第一个非法字段及数值。验证顺序稳定，但 API 没有承诺聚合全部错误。

边界值零合法；负零按浮点比较也合法。负数、NaN、正负无穷非法。即使所有输入分别有限，乘法或累加仍可能溢出，最终结果校验会拒绝它。`sub` 产生负值和 `add` 产生无穷均不会立即报错，这是 API 的有意边界，不应在扩展时悄悄改成饱和运算。

跨 AZ 网络已经包含于 `net_bytes`，所以默认模型只按 `net_byte` 计一次；仅当代码直接构造非零 `cross_az_net_byte` 时才追加差异化收费。由于该字段 serde skip，当前配置文件不能开启这项附加权重。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、事务或外部句柄，也没有析构清理要求。所有公开模型均由标量组成，主要结构为 `Copy` 值；调用不会修改共享对象。并发安全取决于上游如何取得和发布配置快照，而不是本文件内部状态。

生命周期从执行层创建零值 `StmtUnits` 开始，经过按值累积和快照复制，最终由 `calculate` 生成 `StmtResult`。结果本身不持有原始单位或权重；若需要审计，两者必须由 `StatementRUFinalizedSnapshot` 等上游结构另行保留。

## 与 Go 版本的对应关系

直接对照 `pkg/resourcegroup/ruv2/model.go`：Rust 的 `StmtUnits`、`StmtWeights`、`DDLWeights`、`StmtResult` 分别对应 Go 同名结构；`valid`/`add`/`sub`/`calculate` 对应 `Valid`/`Add`/`Sub`/`Calculate`，字段和点积项一致。Go 的 `(StmtResult, bool)` 在 Rust 中表达为 `Option<StmtResult>`，失败时都不暴露部分结果。

两端都规定：默认语句与 DDL 权重是未经校准的占位值；跨 AZ 默认系数为零；单位和权重必须有限且非负；跨 AZ 字节不得超过总网络字节；算术方法不就地修改接收者；结果溢出必须拒绝。`pkg/resourcegroup/ruv2/model_test.rs` 的三个测试分别覆盖默认值/验证、算术/点积/溢出以及跨 AZ 不变量，与 `model_test.go` 的测试意图一致。

可见差异包括：Go 通过 TOML/JSON `"-"` 标签隐藏跨 AZ 权重，Rust 通过 serde skip 实现；Go 的 `validValues` 同时显式检查 NaN/Inf，Rust 的 `is_finite` 等价覆盖；Rust 为无穷错误文本显式生成 `+Inf`/`-Inf`。更重要的是，Go 的 DDL 权重处于 Go 配置和 DDL 使用链中，而 Rust 配置当前使用 `pkg/config/config.rs` 的独立同名结构，尚未复用这里的 DDL 类型。

## 扩展指南

新增一种语句原始单位时，至少要同步修改 `StmtUnits` 字段、`valid` 的字段列表、`add`、`sub`、`StmtWeights` 及其 serde 字段、`default_weights`、`StmtWeights::validate` 和 `calculate` 的点积；同时更新实际采集者（通常在 `pkg/executor/statement_ru_plan_walk.rs` 或语句收尾路径）以及 `statement_ru_reporting.rs` 的引擎/算子归因。遗漏任一处可能造成字段可采集但不计费、可计费但不校验，或完整报告与总 RU 不一致。

若新增可配置权重，要明确三种默认值：派生 `Default` 的零值、业务函数返回的占位值、部分 serde 对象缺失字段的回落值。调整 `cross_az_net_byte` 的序列化策略会改变配置兼容性和重复收费风险，必须与 Go 标签和配置测试同步评审。

若接通 DDL 权重，应先决定消除还是转换 `pkg/config/config.rs::DDLWeights` 与本文件类型，避免两个同名结构长期漂移；然后接入 DDL 生产调用并补独立测试。所有 Rust 测试继续放在同目录 `model_test.rs`，不要内嵌到生产文件；Go 行为同步在 `model.go`/`model_test.go` 核验。重点回归负数、NaN、正负无穷、有限输入导致的结果溢出、跨 AZ 子集约束，以及 `add`/`sub` 的所有字段对称性。

## 验证依据

- RustCodeGraph：`status` 确认本地索引包含 11,467 个文件；`files --filter pkg/resourcegroup/ruv2` 确认 Rust/Go 源及独立测试；`node --file pkg/resourcegroup/ruv2/model.rs` 读取 207 行和 16 个符号，并报告 7 个使用文件。精确 `callers/callees` 查询没有返回可用结果，故没有将缺失图边解释为“无调用”。
- 目标与模块边界：`pkg/resourcegroup/ruv2/model.rs`、`pkg/resourcegroup/ruv2/mod.rs`、`pkg/resourcegroup/lib.rs`、`pkg/resourcegroup/Cargo.toml`。
- Rust 上游证据：`pkg/config/config.rs`、`pkg/executor/statement_ru_result.rs`、`pkg/executor/statement_ru_reporting.rs`、`pkg/executor/statement_ru_plan_walk.rs`、`pkg/executor/adapter.rs`；使用位置由 `rg` 补查。
- Go 对照：`pkg/resourcegroup/ruv2/model.go` 与 `pkg/resourcegroup/ruv2/model_test.go`。
- Rust 独立测试：`pkg/resourcegroup/ruv2/model_test.rs`，覆盖默认值和错误文本、逐字段加减、总 RU 为 440、最大有限数乘二溢出、跨 AZ 计费及非法输入。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行任务指定的 11 章节结构检查并人工复核上述事实链。
