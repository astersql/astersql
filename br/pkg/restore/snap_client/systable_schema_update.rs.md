# `br/pkg/restore/snap_client/systable_schema_update.rs`

## 文件定位

本文件属于 `astersql-br-pkg-restore-snap-client` library crate。crate 入口 [`lib.rs`](lib.rs) 以 `#[path = "systable_schema_update.rs"]` 装载模块，并通过 `pub use systable_schema_update::*` 暴露其 API；[`Cargo.toml`](Cargo.toml) 的 `package.metadata.porting.go-package` 将该 crate 对应到 Go 包 `br/pkg/restore/snap_client`。它不是独立恢复入口，而是系统表恢复链中的一个兼容层：[`systable_restore.rs`](systable_restore.rs) 的 `updateStatsTableSchema` 在比较真实下游表和 BR 临时上游表时调用这里的分派函数。

当前 crate 的 `model::TableInfo`、`Error` 和 `Result` 来自 [`stubs.rs`](stubs.rs)，Cargo 注释也明确当前 arm64 Darwin 移植使用本地 traits/stubs，而非完整 kv/domain/grpcio 运行栈。因此本文描述的是仓库当前 Rust 实现及其接线，不把它推断成已经接入完整生产 BR 运行时。

## 核心职责

文件只处理 `mysql.stats_meta` 的两代结构兼容：以列 `last_stats_histograms_version` 是否存在区分 V1/V2，并把 BR 临时库 `__TiDB_BR_Temporary_mysql.stats_meta` 调整到真实下游表所需的结构。核心职责分为三层：

1. `getSchemaVersionFromStatsMeta` 从 `TableInfo.Columns` 推断版本。
2. `updateStatsMetaSchema` 比较下游目标结构与临时上游结构，按版本台阶选择 ADD 或 DROP SQL，并通过调用方注入的执行回调提交 SQL。
3. `update_stats_meta_schema_function_map` 限定分派范围，仅允许 `mysql.stats_meta` 进入上述逻辑，其他 schema/table 返回 `None`。

该边界避免通用系统表恢复逻辑误改未声明版本规则的表；SQL 固定作用于 BR 临时表，而不是直接 ALTER 真实 `mysql.stats_meta`。

## 主要符号

- `SchemaVersionType`：`#[repr(i32)]` 的有序枚举，取值为 `InvalidVersion = 0`、`Version1 = 1`、`Version2 = 2`。派生的顺序比较和显式整数值供版本循环使用；当前检测函数只会产生 V1 或 V2，Invalid 是防御性哨兵。
- `upgrade_sqls(schema, table, ver)`：查询从 `ver` 到下一版本的升级 SQL。当前仅定义 `mysql.stats_meta`：台阶 0 是空串，台阶 1 为 `ADD COLUMN IF NOT EXISTS last_stats_histograms_version ...`；未知键返回 `None`。
- `downgrade_sqls(schema, table, ver)`：查询从下一版本回退到 `ver` 的 SQL。当前台阶 1 为 `DROP COLUMN IF EXISTS last_stats_histograms_version`，台阶 0 为空串，未知键返回 `None`。
- `getSchemaVersionFromStatsMeta(&TableInfo)`：线性扫描 `Columns`，按列名的规范化小写字段 `Name.L` 精确匹配 `last_stats_histograms_version`；命中即返回 V2，否则返回 V1。
- `updateStatsMetaSchema(downstream, upstream, execution)`：版本比较与 SQL 执行主函数。`execution` 是 `FnMut(&str) -> Result<()>`，允许真实调用方执行 SQL，也允许测试捕获语句和注入错误。
- `update_stats_meta_schema_function_map(schema, table)`：Go 嵌套函数映射的 Rust 等价分派器。命中时返回函数指针签名，内部闭包转调 `updateStatsMetaSchema`；不命中返回 `None`。

命名保留 Go 风格的 `getSchemaVersionFromStatsMeta`/`updateStatsMetaSchema`，crate 根通过 `#![allow(non_snake_case)]` 接受这种移植命名。

## 执行流程

上游接线位于 `systable_restore.rs::updateStatsTableSchema`：

1. 调用方遍历 `renamed_tables` 中的 `(schema_name, table_name)`。
2. 先调用 `update_stats_meta_schema_function_map`。只有 `("mysql", "stats_meta")` 返回更新函数，其他表直接 `continue`。
3. 通过 `InfoSchema::TableInfoByName` 读取真实下游 `mysql.stats_meta`；再用 `TemporaryDBName("mysql")` 读取临时上游 `__TiDB_BR_Temporary_mysql.stats_meta`。
4. 分派函数进入 `updateStatsMetaSchema`，分别调用 `getSchemaVersionFromStatsMeta` 得到目标版本和临时表当前版本。
5. 若版本相同，立即成功且不调用 `execution`。若下游版本更低，则从 `upstream - 1` 向下遍历到 `downstream`，使用 `downgrade_sqls`；当前 V2 临时表对齐 V1 下游时执行一次 DROP。若下游版本更高，则从 `upstream` 向上遍历到 `downstream - 1`，使用 `upgrade_sqls`；当前 V1 临时表对齐 V2 下游时执行一次 ADD。
6. 每个非空 SQL 按台阶顺序交给 `execution`；任一调用失败立即返回错误，成功走完后返回 `Ok(())`。

当前仅有 V1/V2，所以一次跨版本最多执行一条有效 ALTER；循环形式为未来增加连续版本台阶保留了顺序执行能力。

## 数据与状态

本文件没有全局可变状态。版本规则由纯函数中的静态字符串字面量表示，避免运行期构造映射；输入 `TableInfo` 仅以共享引用读取，函数不修改列元数据。唯一可观察副作用来自调用方提供的 `execution` 回调。

版本判定只依赖 `TableInfo.Columns[*].Name.L`，不检查列类型、unsigned、默认值、索引或列顺序。这意味着“存在同名列”就是当前完整的版本不变量；同名列定义不兼容时仍会被视为 V2。表名取自 `downstream_table_info.Name.L`，而 schema 键在主函数内部固定为 `mysql`。

SQL 使用 `IF NOT EXISTS`/`IF EXISTS` 提供 DDL 层幂等保护；版本相同分支则在调用层保证零 SQL 副作用。

## 依赖与调用关系

直接依赖只有 `crate::stubs::{model, Error, Result}`：`model::TableInfo` 提供表名与列信息，`Result` 统一回调和主函数返回类型，`Error::Errorf` 构造非法版本错误。目标文件自身不直接依赖 Cargo 中列出的外部 crate，也不创建 session、domain 或存储客户端。

已核对的调用边如下：

- `lib.rs` 装载并重导出本模块；测试构建还以独立文件 [`systable_schema_update_test.rs`](systable_schema_update_test.rs) 挂载测试模块。
- `systable_restore.rs::updateStatsTableSchema` → `update_stats_meta_schema_function_map` → `updateStatsMetaSchema`。
- `updateStatsMetaSchema` → `getSchemaVersionFromStatsMeta`、`upgrade_sqls`、`downgrade_sqls`，并在需要变更时 → 注入的 `execution`。
- [`export_test.rs`](export_test.rs) 将两个 Go 风格函数分别重导出为 `GetSchemaVersionFromStatsMeta` 和 `UpdateStatsMetaSchema`，供独立 Rust 测试使用。

RustCodeGraph 的文件级关系显示本文件被 `systable_restore.rs` 和 `systable_schema_update_test.rs` 使用；函数级 `callees` 也确认主函数的三条内部调用边。其 `callers` 查询没有返回函数级结果，因此上游边以已索引的 `systable_restore.rs` 源码接线为直接证据。

## 错误处理与边界

若任一版本为 `InvalidVersion`，`updateStatsMetaSchema` 返回 `Error::Errorf("invalid stats meta schema")`，不执行 SQL；不过当前版本检测函数无法产生 Invalid，所以该分支只为未来检测策略或调用契约变化预留。版本相同返回成功且不触发回调。

SQL 查询函数对未知 schema/table/version 返回 `None`。主流程使用 `unwrap_or("")` 将其静默转为空 SQL并跳过，而不是报“缺少迁移台阶”。分派器通常先阻止未知表进入主流程，但未来新增版本而忘记补 SQL 时，这一设计可能导致函数成功却未完成结构调整，是扩展时必须特别检查的风险。

回调错误用 `?` 原样向上传播并终止后续台阶；`systable_restore.rs::updateStatsTableSchema` 再增加 `failed to update stats table schema` 的 schema/table 上下文。多个台阶不是事务：若前一 ALTER 成功、后一 ALTER 失败，本文件没有回滚机制，重试安全性依赖各条 DDL 的幂等条件。

大小写边界是精确比较 `Name.L`、`"mysql"` 与 `"stats_meta"`；契约假设调用方提供的 `CIStr.L` 已规范化。空列列表会被判断为 V1。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、连接或文件资源。所有借用都限制在单次同步调用内；版本值和 SQL 字符串不逃逸，静态 SQL 返回 `&'static str`。`FnMut` 表明执行器可以维护调用次数、session 或捕获的 SQL 列表，但同一时刻由当前调用栈顺序调用，不提供并发执行保证。

资源所有权留在上层：`InfoSchema` 查询和 SQL session/事务由 `updateStatsTableSchema` 的调用方管理。本文件只保证遇错停止，不负责开始、提交或回滚事务。若未来增加多个版本台阶，需要由上层明确 DDL 是否可原子化、失败后如何恢复，以及执行器能否被重复调用。

## 与 Go 版本的对应关系

Go 对照文件 [`systable_schema_update.go`](systable_schema_update.go) 定义相同的三值版本枚举、V1↔V2 SQL、列存在性检测、升降级循环和 `mysql.stats_meta` 函数映射。Rust 版本保留了以下可观察语义：

- 同名列存在为 V2，否则为 V1。
- 下游 V1、临时上游 V2 时对临时表执行 DROP；下游 V2、临时上游 V1 时执行 ADD。
- 同版本不执行 SQL；执行错误立即返回。
- 仅 `mysql.stats_meta` 注册更新函数。

实现形态存在几处差异：Go 用嵌套 map/切片保存 SQL 和函数，Rust 用 `match` 查询函数；Go 回调签名携带 `context.Context`，Rust 回调只接收 SQL 字符串，因此取消、deadline 和 session 生命周期必须由外层执行器承载；Go 直接索引 SQL 切片，而 Rust 对缺失项回退为空串，缺失迁移台阶时的失败可见性更弱。Go 通过 `errors.Trace` 包装执行错误，Rust 在本函数中原样传播，随后由 `updateStatsTableSchema` 添加上下文。

Go 测试 [`systable_schema_update_test.go`](systable_schema_update_test.go) 使用真实 session/InfoSchema 建表并观察 ALTER 后结构；Rust 独立测试使用 `TableInfo` fixture 和捕获回调，验证同样的版本与 SQL 选择，但不验证真实 TiDB DDL 执行。这是当前测试环境边界，不应描述为完整集成等价。

## 扩展指南

新增 stats_meta 版本时，应作为一个一致变更同时处理以下位置：

1. 在 `SchemaVersionType` 追加保持单调递增的版本值，并扩展 `getSchemaVersionFromStatsMeta` 的结构判定；避免只凭一个列名误识别更高版本。
2. 为每个相邻版本补齐 `upgrade_sqls` 和 `downgrade_sqls` 台阶，确认循环索引方向，并考虑把“缺少台阶”从空串跳过改为显式错误。
3. 在独立测试 `systable_schema_update_test.rs` 增加每个方向、同版本、未知映射、执行器失败及多台阶中途失败场景；不要把测试嵌入生产 `.rs` 文件。
4. 同步 Go 文件和 `systable_schema_update_test.go`，或明确记录两端有意差异。若新增其他系统表，还要扩展 `update_stats_meta_schema_function_map`，并确认 `systable_restore.rs::updateStatsTableSchema` 的输入集合确实包含它。
5. 评估兼容性和性能：ALTER 系统统计临时表会影响恢复流程；版本误判可能丢列或使后续导入不匹配。列扫描是 O(列数)，通常不是瓶颈，真正成本在 DDL 执行和潜在多台阶重试。

扩展后至少人工确认 SQL 始终指向 `__TiDB_BR_Temporary_mysql`，真实下游表只作为目标结构依据；还要验证回调错误不会被吞掉，以及已完成台阶后的重试具有幂等性。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件和 4,415 个 Go 文件；目标目录内列出源文件、Go 对照和独立测试。
- RustCodeGraph `node --file br/pkg/restore/snap_client/systable_schema_update.rs`：读取目标文件 1–125 行，确认 1 个枚举、5 个公开函数、SQL 字面量和完整分支。
- RustCodeGraph `query`：定位 `SchemaVersionType`、`upgrade_sqls`、`downgrade_sqls`、`getSchemaVersionFromStatsMeta`、`updateStatsMetaSchema`、`update_stats_meta_schema_function_map`。
- RustCodeGraph `callees updateStatsMetaSchema`：确认 Rust 主函数调用版本检测及两类 SQL 查询；`callees update_stats_meta_schema_function_map` 确认分派到主函数。
- RustCodeGraph 节点读取：`systable_restore.rs` 250–298 行证明真实下游/临时上游查询与调用链；`export_test.rs` 68–77 行证明测试重导出；`lib.rs` 38–74、112–114 行证明模块装载、公开导出与独立测试挂载。
- Go 对照：`systable_schema_update.go` 24–101 行；Go 测试：`systable_schema_update_test.go` 30–127 行。
- Rust 测试：`systable_schema_update_test.rs` 32–138 行，覆盖 V1/V2 判定、同版本零执行、DROP/ADD 精确 SQL，以及回调错误传播。
- crate 配置：`br/pkg/restore/snap_client/Cargo.toml`；该目录没有 `doc.go`，包契约以 crate 入口 `lib.rs` 和 Cargo metadata 为准。

本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的结构命令验证文档存在且固定二级标题恰好为 11 个，并人工复核唯一新增产物为本文件。
