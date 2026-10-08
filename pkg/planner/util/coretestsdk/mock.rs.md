# `pkg/planner/util/coretestsdk/mock.rs`

## 文件定位

本文件属于独立 crate `astersql-planner-util-coretestsdk`，由同目录 `lib.rs` 以公开模块 `pub mod mock` 暴露。它是规划器测试 SDK 的夹具实现，不在 SQL 请求的生产执行链上：调用方取得一组确定的表、索引、分区、视图和会话状态快照，再把它们交给测试套件或被测规划器接口。crate 边界和 Go 来源由 `pkg/planner/util/coretestsdk/Cargo.toml` 的 `[package]`、`[lib]` 与 `[package.metadata.porting].go-package` 明确记录。

当前 Rust 文件并未直接使用真实的 `astersql-meta-model`、`astersql-domain` 或 `astersql-infoschema` 类型，而是在文件内定义最小值对象。`Cargo.toml` 中对应 AsterSQL 组件依赖全部位于 `target.'cfg(windows)'.dependencies`；因此不能从这些依赖声明推断本文件已接通真实 planner/domain。直接的跨 crate 使用证据是 `pkg/planner/core/casetest/dag/dag_test.rs` 导入 `mock_signed_table` 与 `mock_unsigned_table`，同 crate 的聚合入口则是 `pkg/planner/util/coretestsdk/testkit.rs`。

## 核心职责

本文件承担三组职责：

1. 用 `FieldType`、`SchemaState`、`ColumnInfo`、`IndexInfo`、`PartitionInfo`、`TableInfo` 等轻量结构描述规划测试所需的最小元数据。
2. 提供固定、可重复的表夹具：有符号表、无符号表、无主键表、视图、三类分区表、带全局索引的分区表，以及包含 `SchemaState::None` 列的 DDL 中间态表。
3. 提供 `MockContext`、`PlanContext`、`InfoSchema`，让 `testkit.rs` 能组合出持有解析器配置、信息模式和上下文快照的 `PlannerSuite`。

它刻意只保存测试会断言的状态，不执行 SQL、统计信息加载、DDL、事务或优化算法。构造结果全由输入和常量决定，因而适合做 planner 元数据边界的稳定基线。

## 主要符号

- 枚举：`FieldType::{Unspecified, Long, Varchar, Date}` 表示 Go `types.FieldType` 的测试子集；`SchemaState::{None, WriteOnly, Public}` 表示列/索引/表可见性阶段；`PartitionType::{Range, Hash, List}` 表示三种分区形状；`ViewSecurity::Definer` 是当前唯一视图安全模式。
- 元数据值类型：`ColumnInfo` 保存 ID、名称、类型、offset、状态和主键/非空/无符号/无默认值标志；`IndexInfo` 同时保存按名称表示的索引列、已解析的 `column_offsets`、状态以及 unique/global 标志；`PartitionDefinition` 和 `PartitionInfo` 保存边界、表达式、启用状态和分区数；`ViewInfo`、`TableInfo` 汇总视图与表定义。
- 上下文值类型：`InfoSchema { tables }` 是表列表容器；`PlanContext` 是当前数据库与可选信息模式的快照；`MockContext` 记录数据库、除法精度、store/domain/stats 初始化标志、可选信息模式和窗口函数开关。
- 内部构造器：`column` 生成默认 `Public` 列；`index_with_state` 生成索引并将已知列名映射到固定 offset；`index` 固定为 `Public`、非全局索引；`definitions` 生成空边界的分区定义；`partitioned` 在基础表末尾追加 `ptn` 列并挂载分区信息。
- 公开表构造器：`mock_signed_table`、`mock_unsigned_table`、`mock_no_pk_table`、`mock_view`、`mock_range_partition_table`、`mock_hash_partition_table`、`mock_list_partition_table`、`mock_global_index_hash_partition_table`、`mock_state_none_column_table`。
- 公开上下文/模式构造器：`mock_context`、`MockContext::get_plan_context`、`mock_partition_info_schema`。

所有公开结构字段也都是 `pub`，调用者可以在构造后调整夹具；文件自身没有封装这些跨字段不变量。

## 执行流程

基础表流程以 `mock_signed_table` 最完整：先遍历 12 个固定列名，通过后缀选择 `Varchar`、`Date` 或 `Long`，再设置主键、非空和无默认值标志；随后借助 `index`/`index_with_state` 创建 7 个索引，其中 `x` 是 `WriteOnly` 唯一索引，`e_d_c_str_prefix` 的 `c_str` 前缀长度为 10。返回的表名为 `t`，ID 为 1，且 `primary_key_is_handle = true`。

`mock_unsigned_table` 独立构造 `t2` 的三列和两个索引。`mock_no_pk_table` 先调用它，再覆盖 ID、名称、列和索引；这里保留了 Go 夹具的特殊事实：名称虽为“no pk”，`primary_key_is_handle` 仍为 `true`，但列上没有 `primary_key` 标志。`mock_view` 则直接构造 `v`，把 `select b,c,d from t`、`Definer` 和 `root@` 写入 `ViewInfo`。

分区表先用 `definitions` 建立定义，再调用 `partitioned(mock_signed_table(), ...)`。`partitioned` 改写表 ID/名称、追加 `ptn` 列、设置表达式为 `ptn`；Hash/List 的 `num` 等于定义数，Range 刻意为 0。Range 写入 `16`/`32` 上界，List 写入 `1`/`2` 枚举值。全局索引版本在 Hash 分区表上再追加 `b`、`b_global`、`b_c`、`b_c_global`，其中两个 `*_global` 同时为 unique 和 global。

上下文流程较短：`mock_context` 返回固定状态；`get_plan_context` 克隆当前数据库和 `InfoSchema`，形成与后续上下文修改解耦的快照。`mock_partition_info_schema` 从 `mock_signed_table` 派生单表 schema，追加 `ptn` 列并采用调用者传入的 Range 分区定义。

## 数据与状态

核心不变量来自固定 ID、offset 与标志：`mock_signed_table` 的列 ID 为 1..12、offset 为 0..11，索引列 offset 由 `index_with_state` 的名称匹配表解析；追加的 `ptn` 使用最后一列的 ID/offset 加一。未知索引列名在当前实现会静默映射到 offset 0，因此扩展列集合时必须同步更新该匹配表。

构造器返回拥有所有数据的值，没有全局注册表或共享可变状态。`Clone` 派生广泛用于复制表和上下文；`PlanContext` 的 `info_schema` 也是深克隆后的快照。`testkit.rs::create_planner_suite_elements` 按固定顺序聚合九张表，并重新连续分配表与分区 ID，所以构造器内的原始 ID 是夹具默认值，不一定是最终测试套件中的 ID。

`MockContext` 的默认值为数据库 `test`、`division_precision_increment = 4`，三个初始化标志均为真，`info_schema = None`，窗口函数关闭。这里的布尔值只描述模拟状态，不持有真实 store、domain 或 stats handle。

## 依赖与调用关系

向下依赖全部在本文件内。RustCodeGraph 对 `mock_signed_table` 给出的直接被调边为 `mock_range_partition_table`、`mock_hash_partition_table`、`mock_list_partition_table`、`mock_global_index_hash_partition_table` 和 `mock_partition_info_schema`；它自身实例化 `TableInfo` 并调用内部 `column`。其他构造器最终主要落到 `column`、`index`、`index_with_state`、`definitions` 和 `partitioned`。

向上调用有两层：

- `pkg/planner/util/coretestsdk/testkit.rs::create_planner_suite_elements` 调用九个表构造器和 `mock_context`，组装 `PlannerSuite`；`create_planner_suite` 再调用 `MockContext::get_plan_context`。
- `pkg/planner/core/casetest/dag/dag_test.rs::mock_info_schema_from_coretestsdk` 直接使用 `mock_signed_table`、`mock_unsigned_table` 构造该测试所需的信息模式。

同目录 `coretestsdk_aster_unit_test.rs` 是本文件的独立 Rust 回归面，覆盖全部公开表构造器、上下文和 `mock_partition_info_schema`。Go 侧更广泛的 planner/executor 测试通过同名 Go API 使用相同夹具，但这些 Go 调用不是 Rust 静态调用边。

## 错误处理与边界

绝大多数函数无返回错误，因为输入有限且构造过程纯内存化。唯一显式 panic 边界位于 `mock_partition_info_schema`：它对 `mock_signed_table().columns.last()` 使用 `expect("signed table has columns")`；按当前固定基础表该条件成立，若未来允许空基础表则需改写接口或错误策略。

`index_with_state` 对未知列名回退到 offset 0，不会报错；这是最值得注意的静默失败边界。`partitioned` 接受任意 `definitions`，不会检查 Range 边界顺序、Hash/List 数量或表达式合法性。所有结构字段公开，也不会阻止调用者构造“列名与 offset 不一致”“global 但非 unique”等状态。这些是测试夹具的宽松性，不应被理解为生产元数据校验规则。

`mock_no_pk_table` 的名称与 `primary_key_is_handle = true` 看似矛盾，但与 Go 文件和现有回归断言一致；维护者不应仅凭名称“修正”它。Range 分区的 `num = 0` 同样是当前 Go 对照形状，而不是通用分区计数规则。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件、网络连接或真实数据库资源。所有构造器返回独立拥有的值，天然不存在本文件内部的共享并发写入；跨线程使用能力取决于这些普通字段类型，而不是额外同步协议。

Go `MockContext` 会创建 mock store、mock domain 和 stats handle，并可能在创建 stats handle 失败时 panic；Rust `MockContext` 只以 `store_initialized`、`domain_bound`、`stats_handle_created` 三个布尔值记录期望状态。真正的测试套件生命周期在 `testkit.rs::PlannerSuite::close`：它把 `closed` 置真并把 `stats_handle_created` 置假，不执行真实资源释放。因此文档或测试不能把这些标志当作资源确已初始化/关闭的运行时证据。

## 与 Go 版本的对应关系

权威对照是同目录 `mock.go`。公开构造器基本一一对应：Rust snake_case 名称对应 Go `Mock*` 名称；表/列/索引/分区的固定名称、ID、offset、状态、标志、边界和值由 `coretestsdk_aster_unit_test.rs` 针对关键字段回归。

主要差异如下：

- Go 使用真实 `model.TableInfo`、`model.ColumnInfo`、`infoschema.InfoSchema`、`mock.Context` 等类型；Rust使用本文件的最小结构，尚不是生产类型适配层。
- Go `newStringType` 设置默认 charset/collation；Rust `FieldType::Varchar` 不保存 charset/collation。Rust 也不表达 Go `FieldType` 的完整 flag/type 信息，而把部分 flag 拆成 `ColumnInfo` 布尔字段。
- Go `MockContext` 真正创建 mock store/domain/stats handle，并绑定 schema validator；Rust仅保存状态标志和可选 schema 快照。
- Go 的索引列直接携带 offset；Rust同时保存名称/前缀长度与由硬编码名称表产生的 `column_offsets`。
- Go 的 `infoschema.MockInfoSchema` 返回完整接口实现；Rust `InfoSchema` 只是 `Vec<TableInfo>` 容器。

因此，“形状和测试意图对齐”是当前可验证结论，“可替代 Go 生产对象”并未得到代码支持。

## 扩展指南

新增列时，应同时检查 `column` 的默认值、目标构造器中的 ID/offset/标志，以及 `index_with_state` 的列名到 offset 映射；若新列参与索引却未更新映射，会静默得到 0。新增索引状态或分区类型时，应先扩展相应枚举/结构，再调整构造器和 `partitioned` 的 `num` 规则，避免只改变表面名称。

新增公开夹具最可能修改本文件的构造器区，并需要在 `pkg/planner/util/coretestsdk/testkit.rs::create_planner_suite_elements` 决定是否纳入默认九表集合；纳入后会改变后续表/分区的重编号和测试快照。对应回归必须放在独立文件 `pkg/planner/util/coretestsdk/coretestsdk_aster_unit_test.rs`，不要把测试嵌入 `mock.rs`。若语义来自 TiDB，需同步核对 `pkg/planner/util/coretestsdk/mock.go`，保留 Go 的特殊状态而非凭名称简化。

若目标是接入真实 Rust planner/domain，应把它作为独立迁移工作：明确转换边界或替换最小类型、处理 charset/collation 和完整 flags、定义真实资源错误与生命周期，并检查所有依赖此 crate 的测试。当前 `cfg(windows)` 依赖布局和现有轻量 API 都表明这不是可在单个夹具函数中安全完成的局部修改。

兼容风险主要是固定名称、ID/offset、索引顺序和公开字段的断言；性能风险很低，数据规模固定且只做小向量构造/克隆。扩大默认表集合或把轻量类型替换成真实对象时，才可能显著增加测试初始化成本。

## 验证依据

- RustCodeGraph 索引状态：仓库索引包含 `pkg/planner/util/coretestsdk/mock.rs`，识别 43 个符号；`node --file` 核对了全文件 737 行和“used by 56 files”的文件级使用概览。
- RustCodeGraph 符号证据：`node mock_signed_table` 核对其对 `TableInfo`/`column` 的依赖，以及五个派生构造器的直接调用边；`node mock_context` 核对其仅实例化本地 `MockContext`；`node mock_partition_info_schema` 核对其调用 `mock_signed_table` 并实例化 `ColumnInfo`、`PartitionInfo`、`InfoSchema`。
- 源码与 crate 边界：`pkg/planner/util/coretestsdk/mock.rs`、`pkg/planner/util/coretestsdk/lib.rs`、`pkg/planner/util/coretestsdk/Cargo.toml`。
- 直接 Rust 消费面：`pkg/planner/util/coretestsdk/testkit.rs`、`pkg/planner/core/casetest/dag/dag_test.rs`。
- 独立 Rust 回归：`pkg/planner/util/coretestsdk/coretestsdk_aster_unit_test.rs`，覆盖基础表属性、派生表/分区形状、全局索引、`SchemaState::None`、上下文与分区 schema。
- Go 语义对照：`pkg/planner/util/coretestsdk/mock.go`；其真实模型、上下文初始化和各构造器固定字段用于识别 Rust 的对齐范围与缺口。
- 本任务是只读源码分析加 Markdown 新增，按计划不运行 Cargo；交付前以固定 11 章节命令做结构验证，并人工复核本文件不是生产 planner/domain 实现。
