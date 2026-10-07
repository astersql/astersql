# `pkg/infoschema/perfschema/const.rs`

## 文件定位

本文件属于 `astersql-infoschema-perfschema` crate，是 `PERFORMANCE_SCHEMA` 虚拟库的静态 DDL 清单。crate 根 `pkg/infoschema/perfschema/lib.rs` 通过 `#[path = "const.rs"] pub mod consts` 装入它；文件本身不注册虚拟库、不提供行数据，也不执行 SQL。它只把 `pkg/infoschema/perfschema/tables.rs` 中的表名常量拼成建表文本，并由 `pkg/infoschema/perfschema/init.rs::build_performance_schema` 解析成 `TableMeta`。

`pkg/infoschema/perfschema/Cargo.toml` 指定库入口为 `lib.rs`，移植来源为 Go 包 `pkg/infoschema/perfschema`。该 manifest 的正常依赖只有 `tracing`；本文件实际只直接使用标准库 `std::sync::LazyLock` 和 crate 内 `tables::*`，不直接依赖 tracing 或 `cfg(any())` 下列出的尚未接线依赖。

## 核心职责

- 用 44 个 `LazyLock<String>` 保存 44 张虚拟表的完整 `CREATE TABLE` 文本，表名来自 `tables.rs`，避免在 DDL 与 ID/运行时分派处重复硬编码名称。
- 用 `perfSchemaTables: LazyLock<Vec<&'static str>>` 按注册顺序汇总这些 DDL。顺序是兼容契约：初始化按此顺序产生表元数据，而 `tables.rs::TABLE_ID_MAP` 为相同表集合分配稳定 ID。
- 保留列顺序、类型、空值约束、默认值和索引片段。`init.rs::parse_create_table` 依赖这些文本识别表名、列与索引；运行时虚拟表再用生成的列偏移做投影。
- 将高度重复的等待事件表和连接汇总表分别收敛到私有帮助函数 `events_waits_table`、`connection_summary_table`，确保同族表共享列布局。

本文件是元数据声明，不是数据采集实现。TiDB/TiKV/PD profile、会话变量、连接属性等实际取数逻辑位于 `pkg/infoschema/perfschema/tables.rs`。

## 主要符号

- `perfSchemaTables: LazyLock<Vec<&'static str>>`：公开的有序 DDL 注册表，共 44 项。它持有各静态 `String` 的 `'static` 字符串视图，而不复制 DDL。
- 35 个 Go 既有表 DDL：状态/变量表，四张 setup 表，语句、事务和阶段的 current/history/history_long 表，prepared statements、digest 汇总、TiDB/TiKV/PD profile、连接属性以及 `status_by_connection`。代表符号包括 `tableGlobalStatus`、`tableStmtsCurrent`、`tableEventsStatementsSummaryByDigest`、`tableTiDBProfileCPU` 和 `tableStatusByConnection`。
- 9 个 Rust 客户端兼容表 DDL：`tableCondInstances`，三张 `tableEventsWaits*`，`tableAccounts`、`tableHosts`、`tableUsers`，`tableBinaryLogTransactionCompressionStats`，以及 `tableEventsTransactionsSummaryByUserByEventName`。
- `events_waits_table(name: &str) -> String`：私有模板，生成 current/history/history_long 三张等待事件表相同的 19 列布局，仅替换表名。
- `connection_summary_table(name: &str, identity_columns: &str) -> String`：私有模板，生成 accounts/hosts/users 的身份列加四个连接、内存统计列。

文件没有类型、trait、`impl`、条件编译分支或可调用的公开函数。`#![allow(dead_code, non_snake_case, non_upper_case_globals)]` 是为保持 Go 风格符号命名和声明清单而设置的模块级 lint 例外。

## 执行流程

1. 首次访问 `perfSchemaTables` 时，`LazyLock` 初始化闭包依次访问每个表 DDL 静态量。
2. 每个 DDL 静态量在首次访问时把固定 SQL 片段与 `tables.rs` 的 `tableName*` 拼接为一个 `String`；三张等待表和三张连接汇总表分别经私有模板生成。
3. `perfSchemaTables` 保存这些 `String` 的 `&'static str` 视图，后续访问复用同一份已初始化数据。
4. `init.rs::build_performance_schema` 预分配等长表向量，遍历清单并调用 `parse_create_table`。解析器抽取表名、按顶层逗号切分列/索引定义，再用 `tables.rs::TABLE_ID_MAP` 查找固定表 ID。
5. 初始化器设置数据库 ID、public 状态、从 1 开始的列 ID 和与 DDL 顺序一致的列 offset，最终组装名为 `PERFORMANCE_SCHEMA` 的 `VirtualDatabase`。`init.rs::init` 仅在表达式求值器就绪后通过 `Once` 注册它。

因此，修改本文件会在下一次进程初始化时改变虚拟表元数据；它不会创建物理表，也不会在查询期间重复解析 DDL。

## 数据与状态

全部状态均为进程级只读惰性静态量。45 个公开静态量由 1 个清单和 44 个 DDL 字符串组成；初始化后不会再修改。`perfSchemaTables` 中的借用安全地指向同样具有静态生命周期的 `LazyLock<String>` 内容。

清单分为两个兼容层：前 35 张是 `tables_test.rs::LEGACY_TABLES` 锁定的历史集合，后 9 张是 `CLIENT_REQUIRED_TABLES` 锁定的客户端所需扩展。清单顺序与 `TABLE_ID_MAP` 的名称数组并非机械共享，二者必须人工同步；测试通过集合相等、ID 唯一和逐项 ID 断言防止漂移。

DDL 中最重要的数据不变量是表名必须能在 `TABLE_ID_MAP` 找到、列定义必须可由轻量解析器切分、索引所引用列必须先出现。大小写和空白通常不影响轻量解析，但 DDL 原文保存在 `TableMeta::create_sql`，且类型、默认值、列序与索引结构属于兼容表面，不应仅因格式偏好改写。

## 依赖与调用关系

上游装配链为 `lib.rs::consts` → 本文件；生产调用链为 `init.rs::init` → `build_performance_schema` → `perfSchemaTables` → 各 `table*` DDL。RustCodeGraph 对 `init.rs` 的文件关系还显示其被 `pkg/session/runtime/system_query.rs` 使用，说明注册结果进入系统查询路径；本文件自身没有请求期调用或数据源依赖。

下游直接依赖如下：

- `crate::tables::*` 提供全部 `tableName*` 常量；同文件的 `TABLE_ID_MAP` 必须覆盖 DDL 清单中的每个名称。
- `init.rs::parse_create_table` 消费 DDL 语法，产出 `TableMeta`、`ColumnInfo` 与 `IndexInfo`；非法文本或未知表名在 `build_performance_schema` 中传播为 `InitError`。
- `tables.rs::table_from_meta` 把解析后的元数据包装成虚拟表，并按表名选择真正的行数据源或 profile 获取逻辑。
- `tables_test.rs` 直接调用 `build_performance_schema`，间接覆盖本文件的完整清单、列序、ID 和可包装性；`init_test.rs` 单独覆盖轻量 DDL 解析器的逗号、引号与索引边界。

RustCodeGraph 能定位本文件的两个私有生成函数和 `init.rs` 对 `perfSchemaTables` 的导入，但其“used by”结果对该静态量产生了无关候选；调用关系最终以 `init.rs:24,104-119` 的直接源码引用交叉核验。

## 错误处理与边界

本文件的闭包只做内存字符串拼接和格式化，没有 `Result`、显式错误分支或 I/O。错误在消费端暴露：DDL 缺括号、索引格式不合法或索引引用未知列时，`parse_create_table` 返回 `InitError::InvalidCreateSql`；DDL 表名未进入 `TABLE_ID_MAP` 时，`build_performance_schema` 返回 `InitError::UnknownTable`；`init` 在注册阶段把构建错误转为 panic，并拒绝重复库注册。

轻量解析器不是完整 MySQL parser。扩展 DDL 时必须遵守它当前能识别的形态：表名位于 `CREATE` 头最后一个 token，列/索引以顶层逗号分隔，索引行以 `PRIMARY KEY`、`KEY ` 或 `UNIQUE KEY` 开头。复杂转义、嵌套表达式或新的约束关键字不能假定受支持。

这里的“表已声明”只证明元数据可注册，不等价于表已有数据实现。未知表在 `tables.rs` 包装或取数分派处仍可能产生 `PerfSchemaError::UnknownTable`；新增表必须同时补齐该层行为或明确空表语义。

## 并发与资源生命周期

`LazyLock` 保证每个 DDL 和总清单在并发首次访问时只初始化一次，之后共享不可变内容，无需调用方加锁。初始化过程只分配少量永久驻留的 `String` 和一个引用向量；没有线程、通道、文件、网络连接、事务或显式释放动作。

注册生命周期由 `init.rs` 管理：`AtomicBool` 以 Acquire/Release 表示表达式求值器是否就绪，`Once` 保证注册最多一次，`Mutex<Vec<VirtualDatabase>>` 保护注册表。本文件不参与这些同步操作，只提供其只读输入。由于静态 DDL 生命周期覆盖整个进程，`perfSchemaTables` 中保存借用而非复制是有效的。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/infoschema/perfschema/const.go` 和 `init.go`。Rust 的前 35 张表与 Go `perfSchemaTables` 清单包含相同名称并保持相同注册顺序；各对应 DDL 沿用 Go 的列顺序、类型与索引片段。Rust 用运行时一次性 `LazyLock<String>` 替代 Go 编译期字符串常量，因为 Rust 需要把多个 `&str`（包括表名常量）拼成拥有所有权的字符串。

Rust 当前不是对 Go 文件的完全逐项镜像：它在 Go 的 35 张表之后追加了 9 张客户端兼容表，而当前 `const.go` 没有这些 DDL。该差异由 `tables_test.rs::test_client_required_registry_is_complete_and_stable` 明确记录并验证。因此维护时应分别保护“Go 既有 35 表的对齐”和“Rust 追加 9 表的客户端契约”，不能用 Go 清单直接覆盖 Rust 清单。

初始化语义也有实现差异：Go `Init` 使用完整 parser 和 `ddl.BuildTableInfoFromAST`，Rust `build_performance_schema` 使用本 crate 的轻量解析器；二者都为列赋连续 ID、设置固定数据库 ID/public 状态，并在依赖就绪后执行一次注册。DDL 语义的改动必须同时评估完整 Go parser 与 Rust 轻量解析器的接受范围。

## 扩展指南

新增或修改表时，最小安全接线顺序是：

1. 在本文件新增或修改 `table*` DDL，并把新表放入 `perfSchemaTables` 的兼容位置；同族表优先复用现有帮助函数，但不要为了复用改变列定义。
2. 在 `tables.rs` 新增对应 `tableName*`，同步 `TABLE_ID_MAP`，保持全部旧表 ID 不变。新表通常只能追加，避免因插入中间位置重编号。
3. 检查 `init.rs::parse_create_table` 能否解析所用约束与索引语法；若必须扩展解析器，在独立的 `init_test.rs` 增加解析回归测试，不要把测试写进 `const.rs`。
4. 在独立的 `tables_test.rs` 扩展清单完整性、预期列序、ID 稳定性及 `table_from_meta` 行为测试；如需验证 SQL 可见行为，同步相应 Go 测试或上层 session 集成测试。
5. 若表需要动态数据，在 `tables.rs` 的包装/取数分派和相应 `RowSource`/远端 profile 接口补齐实现；仅新增 DDL 会产生元数据，不会自动产生查询结果。
6. 对 Go 既有表的修改同步核对 `const.go`；对 Rust 专有客户端表，记录为何与 Go 不同，避免错误“回归”为 Go 的较小集合。

主要风险是旧表 ID 漂移、列 offset 改变、轻量解析器不接受新语法，以及元数据已经可见但运行时数据源未接线。DDL 只在首次初始化时解析，性能风险很小；更值得关注的是新增永久静态字符串的内存和查询期数据实现的开销。

## 验证依据

- 目标源码：`pkg/infoschema/perfschema/const.rs`，确认 45 个公开静态量、44 项 DDL 清单，以及 `events_waits_table`、`connection_summary_table` 两个私有函数。
- crate 与装配：`pkg/infoschema/perfschema/Cargo.toml`、`lib.rs`；最近的 `pkg/infoschema/doc.go` 不存在，因此以 crate 根文档作为包契约。
- 初始化链：`pkg/infoschema/perfschema/init.rs` 中的 `perfSchemaTables` 导入、`build_performance_schema`、`parse_create_table`、`init`。
- 名称、ID 与取数边界：`pkg/infoschema/perfschema/tables.rs` 的 `tableName*`、`TABLE_ID_MAP`、`table_from_meta` 和行数据源接口。
- Go 对照：`pkg/infoschema/perfschema/const.go` 的 35 项清单及 DDL，`init.go::Init` 的完整 parser 构建与一次性注册流程。
- 独立测试：`pkg/infoschema/perfschema/tables_test.rs::test_perf_schema_tables`、`test_client_required_registry_is_complete_and_stable`、`test_tikv_profile_cpu_table_meta`、`test_session_connect_attrs_table_meta`、`test_init_registers_when_eval_ready`；`init_test.rs` 覆盖解析器的复杂定义切分。
- RustCodeGraph：`status` 显示索引含目标 Rust 文件；`files --filter pkg/infoschema/perfschema` 确认模块文件面；`node --file` 核对 `const.rs`、`init.rs`、`lib.rs`；`query` 定位两个私有生成函数。由于图对静态变量调用边识别有限，关键消费边以源码直接引用复核。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工复核本文没有把元数据声明误写成运行时数据实现。
