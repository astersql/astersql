# `pkg/infoschema/perfschema/tables.rs`

## 文件定位

本文件是 Rust `astersql-infoschema-perfschema` crate 的虚拟表实现层，位于静态 DDL 与注册层之下：`const.rs` 保存建表 SQL，`init.rs::build_performance_schema` 将这些 SQL 解析成 `TableMeta`，本文件再用 `table_from_meta` / `create_perf_schema_table` 把元数据包装成 `PerfSchemaTable`，并为少数有真实数据源的表提供按需取行能力。crate 根 `lib.rs` 对外再导出 `PerfSchemaTable`、`table_from_meta` 和预定义表判断函数。

当前生产接线需要分两层理解。`pkg/session/runtime/system_query.rs::build_virtual_system_catalog` 调用 `build_performance_schema`，把表 ID、表名和列名装入 session 的虚拟系统目录，因此 Rust SQL 层能够看见 `performance_schema` 的元数据。仓库搜索未发现生产代码调用本文件的 `PerfSchemaTable::get_rows` 或 `iter_records`；这些数据读取入口目前只在 `tables_test.rs` 中被直接验证，不能据此断言本地/远端 profile 行已接入 Rust SQL 执行主链。

`Cargo.toml` 指定 `lib.rs` 为 crate 入口，正常依赖只有 `tracing = "0.1"`；大量与 Go 完整实现对应的 AsterSQL crate 依赖被放在 `target.'cfg(any())'.dependencies` 下，恒不启用。这与本文件通过 `RowSource`、`RemoteProfileClient` trait 隔离 session、HTTP、profile 解析等基础设施的轻量实现相符。

## 核心职责

1. 定义 `PERFORMANCE_SCHEMA_DB_ID`、44 个表名常量和惰性初始化的 `TABLE_ID_MAP`，为虚拟库表提供稳定 ID；前 35 个历史表的 ID 顺序与 Go 保持一致，后 9 个客户端兼容表顺序追加。
2. 定义独立于其余 TiDB 类型系统的轻量数据模型：`Datum`、`ColumnInfo`、`IndexInfo`、`TableMeta`、`PerfSchemaTable` 和 `PerfSchemaError`。
3. 提供虚拟表工厂。`table_from_meta` 先查插件注册表，未命中才走默认 `create_perf_schema_table`；默认路径复制列元数据并用 `init_table_indices` 校验/复制索引。
4. 通过 `PerfSchemaTable::get_rows` 按表名路由到本地 profile、远端 TiKV/PD pprof、会话变量、连接属性或连接状态数据源，并完成列投影。
5. 通过 `data_for_remote_profile` 并发采集集群节点的 pprof，容忍单节点缺失地址或请求/解析失败，以 warning 返回软失败，同时对成功结果排序并在每行前添加节点 status 地址。
6. 为本地 TiDB profile 查询记录包含表名、连接 ID、可选用户和客户端 IP 的审计日志；入口是 `RowSource::on_profile_request`。

## 主要符号

- `PERFORMANCE_SCHEMA_DB_ID`：固定为 `(1_i64 << 62) | 10_000`。`TABLE_ID_MAP` 在此基数上按清单位置加一；`table_id_map` 返回其静态只读引用。
- `is_predefined_table` / `IsPredefinedTable`：把输入转为 ASCII 小写后查询 `TABLE_ID_MAP`，后者是保留 Go API 拼写的别名。这里是 ASCII 语义，不执行 Unicode case folding。
- `Datum`：行单元格的五种表示，即 `Null`、有符号整数、无符号整数、UTF-8 字符串和原始字节。
- `ColumnInfo`、`IndexInfo`、`TableMeta`：轻量元数据。列以 `offset` 参与投影；索引以 `columns: Vec<usize>` 保存列偏移；表保留稳定 ID、所属库、列/索引、public 状态和原始建表 SQL。
- `PerfSchemaError`：区分未知表、非法索引状态、非法投影、不支持的远端节点类型、传输、profile 和插件错误。`Display` 当前直接输出 `Debug` 形式。
- `RowSource`：注入本地 profile、会话变量、连接属性和连接状态。`profile_request_identity` 提供审计身份；默认 `on_profile_request` 使用 `tracing::info!` 记录请求。
- `RemoteProfileClient`：注入节点发现、HTTP/传输、profile 解析和内部 HTTP scheme；scheme 默认是 `http`。
- `VirtualTablePlugin`、`PLUGIN_TABLES`、`register_plugin_table`、`unregister_plugin_table`：按 ASCII 小写表名管理进程级插件工厂，允许在默认索引校验前完全覆盖表创建。
- `PerfSchemaTable`：保存克隆的 `TableMeta`、列和已初始化索引。`columns`、`visible_columns`、`hidden_columns`、`writable_columns`、`deletable_columns`、`physical_id`、`indices`、`deletable_indices` 对齐 Go 虚拟表访问器语义。
- `table_from_meta` / `create_perf_schema_table` / `init_table_indices`：工厂主链。默认构造只接受全部 `public == true` 的索引，否则返回 `InvalidIndexState(index.name)`。
- `PerfSchemaTable::get_rows`：行数据路由与投影核心；`PerfSchemaTable::iter_records` 在其上分配从 0 开始的临时 `i64` handle，并支持 visitor 提前停止。
- `data_for_remote_profile`：远端 profile 的并发 fan-out/fan-in 实现。

## 执行流程

元数据构建流程如下：

1. `init.rs::build_performance_schema` 遍历 `const.rs::perfSchemaTables`，解析每条建表 SQL。
2. 它用本文件的 `TABLE_ID_MAP` 查表 ID，设置 `PERFORMANCE_SCHEMA_DB_ID`、public 状态以及从 1 开始的列 ID 和从 0 开始的列 offset。
3. `table_from_meta` 将表名转为 ASCII 小写并在 `PLUGIN_TABLES` 中查找插件；命中时直接调用插件的 `create` 并原样返回结果。
4. 未命中插件时，`create_perf_schema_table` 克隆表和列；`init_table_indices` 顺序复制 public 索引，遇到首个非 public 索引立即失败。

读取流程由 `get_rows` 驱动：

1. 若表名属于六张本地 TiDB profile 表，先调用 `source.on_profile_request`，因此即使随后采集失败也会留下审计事件。
2. 按精确的小写表名分派：本地 profile 的 profile key 分别为 `cpu`、`heap`、`mutex`、`allocs`、`block`、`goroutine`；TiKV CPU 与六类 PD profile 转给 `data_for_remote_profile`；三类 session 数据转给 `RowSource`；其余表返回空行。
3. 请求列数量与全列数量相等时直接返回完整行。此判断只比较长度，刻意保留 Go 行为：即使请求列顺序不同，也不会重排。
4. 列数不同时，按每个 `ColumnInfo::offset` 克隆单元格；任一 offset 越界则整次读取返回 `InvalidProjection`。
5. `iter_records` 枚举行，以枚举序号作为 handle 调 visitor；visitor 返回 `false` 时正常提前结束，visitor 错误则立即向上传播。

远端 profile 流程如下：

1. `data_for_remote_profile` 先拒绝除 `tikv`、`pd` 外的节点类型，再调用 `RemoteProfileClient::servers`。
2. 缺少 `status_address` 的节点不启动任务，追加历史兼容文本 `TiKV node ... does not contain status address`；该文本即使节点是 PD 也保持 TiKV 字样。
3. 对每个有效节点在 `thread::scope` 中启动一个线程，拼接 `scheme://status_address + uri`，随后依次执行 `fetch(url, true)` 和 `parse_profile(body, goroutines)`。
4. worker panic 被转换为 `Transport("remote profile worker panicked")`；单节点普通错误和 panic 都成为 warning，不使整体失败。
5. 成功结果按 status 地址字典序排序；每个解析结果行前插入 `Datum::String(address)` 后扁平化返回。

## 数据与状态

`TABLE_ID_MAP` 是 `LazyLock<BTreeMap<&'static str, i64>>`：首次访问时构建，之后只读。使用 `BTreeMap` 让迭代顺序由键决定，但稳定 ID 由初始化数组的枚举顺序决定，而不是 map 顺序。`tables_test.rs::test_client_required_registry_is_complete_and_stable` 同时验证 DDL 注册表与 ID map 名称集合相同、ID 唯一、历史 35 表 ID 不漂移，以及新增 9 表的列序和 ID。

`PLUGIN_TABLES` 是 `LazyLock<Mutex<BTreeMap<String, Arc<dyn VirtualTablePlugin>>>>`，状态跨调用、跨线程、贯穿进程生命周期。重复注册同一小写表名会替换旧插件；注销不存在的名字是无操作。插件在持锁区内只被 `Arc::clone`，真正的 `plugin.create` 在锁释放后执行，避免插件回调长期占用注册表锁或重入死锁。

`PerfSchemaTable` 拥有 `TableMeta`、列和索引的克隆，不借用初始化注册表。返回的 profile/session 行也是拥有所有权的 `Vec<Vec<Datum>>`；投影会进一步克隆选中单元格。`warnings: &mut Vec<String>` 是调用方管理的语句级软错误汇聚口，本文件不会清空既有 warning。

没有持久化行存储、事务状态或缓存；profile 和 session 数据在每次 `get_rows` 时采集。`iter_records` 的 handle 只反映本次结果中的行序号，不是持久主键。

## 依赖与调用关系

上游直接关系：

- `lib.rs` 声明 `tables` 模块并再导出主要工厂和查询 API。
- `init.rs::build_performance_schema` 使用 `ColumnInfo`、`IndexInfo`、`TableMeta`、`PERFORMANCE_SCHEMA_DB_ID` 和 `TABLE_ID_MAP` 构建权威元数据。
- `pkg/session/runtime/system_query.rs::build_virtual_system_catalog` 消费 `build_performance_schema` 的结果，将 performance_schema 表加入 Rust session 元数据目录。
- `tables_test.rs` 是 `get_rows`、远端采集、插件覆盖、索引校验和访问器的直接 Rust 调用者。RustCodeGraph 还列出 `pkg/infoschema/go_merge_45_test.rs` 使用目标文件，用于移植一致性检查。

下游抽象关系：

- 本地数据依赖由 `RowSource` 实现者提供；本文件本身不依赖 session crate 或实际 profile collector。
- 远端节点发现、HTTP 请求和 profile 解析由 `RemoteProfileClient` 实现者提供；本文件只规定调用顺序、URL、`allow_follower = true`、错误降级和结果整形。
- 唯一正常启用的第三方依赖是 `tracing`，用于本地 profile 审计日志。
- `std::thread::scope` 管理远端并发任务，`Arc`/`Mutex` 管理插件注册和测试/实现者可能共享的对象。

当前未找到生产态 `RowSource` 或 `RemoteProfileClient` 实现，也未找到生产调用 `get_rows` / `iter_records`。所以本文件已经定义并测试数据面契约，但从 SQL executor 到这些入口的 Rust 生产调用边仍属未接线或至少未由仓库静态证据证明。

## 错误处理与边界

- `is_predefined_table` 对大小写不敏感，但仅识别 `TABLE_ID_MAP` 清单中的名字；空串和未知名字返回 `false`。
- 插件错误优先于默认索引错误：若同名插件存在，`table_from_meta` 不执行 `init_table_indices`。测试 `plugin_and_invalid_index_paths_match_go_factory_order` 固化了这一顺序。
- `init_table_indices` 对任何非 public 索引返回 `InvalidIndexState`；之前已复制到局部表对象的索引随错误对象销毁，不会暴露部分构造结果。
- `get_rows` 对未专门处理的预定义表返回空集合，而不是 `UnknownTable`；`PerfSchemaError::UnknownTable` 在本文件当前路径中没有构造点。
- 本地数据源错误直接失败；审计日志先于本地 profile 采集。测试验证采集失败仍有日志且错误保持为 `Profile`。
- 远端 `servers` 发现失败和不支持节点类型是整体硬错误；发现完成后，空 status 地址、单节点 fetch/parse 错误以及 worker panic 都降级为 warning，其他节点继续贡献结果。因此空结果既可能表示没有节点/数据，也可能表示所有节点软失败，调用方必须查看 warnings。
- 等长投影只比较列数，不校验列身份或顺序；非等长投影严格按 offset，越界为 `InvalidProjection`。扩展时不能在不了解 Go 兼容性的情况下“修正”等长重排行为。
- 插件注册表和 `init.rs` 注册表的 `Mutex` 若中毒会通过 `expect` panic；这不是可恢复的 `PerfSchemaError`。
- URL 由实现者给出的 scheme、未经本文件规范化的 status 地址和固定 URI 直接拼接；鉴权、超时、HTTP 状态码及响应体关闭都属于 `RemoteProfileClient::fetch` 的职责。

## 并发与资源生命周期

远端采集采用一次调用一个 scoped-thread 集合：所有 worker 都在 `thread::scope` 返回前 join，因此不会留下后台线程，也不要求 client 为 `'static`；trait 的 `Send + Sync` 约束允许多个 worker 共享同一引用。线程数等于具有非空 status 地址的节点数，没有本地并发上限或背压；大集群查询的线程创建和同时请求量是主要性能风险。

结果收集按创建顺序 join worker，但最终成功结果仍按地址排序，从而屏蔽网络完成顺序。warning 的顺序则跟 join/服务器输入顺序相关，不承诺按地址排序。panic join 不再 panic 到调用方，而是生成 transport warning。

插件注册使用进程级 `Mutex`；注册/注销与建表查询串行访问 map。`table_from_meta` 克隆 `Arc` 后释放锁再调用插件。插件对象的销毁发生在其注册被替换/删除且所有已克隆 `Arc` 释放之后。

本文件不直接拥有 socket、HTTP response、session 或事务；相应资源的打开、超时、关闭和取消均由 trait 实现者负责。`get_rows` 没有 cancellation 参数，Rust 接口也未表达 Go `context.Context` 的取消语义。

## 与 Go 版本的对应关系

`tables.go` 是直接语义基准：Rust 表名常量、历史 ID 顺序、`IsPredefinedTable`、插件优先工厂、虚拟表访问器、索引初始化、表名分派、等长投影捷径、visitor 提前停止，以及远端结果按地址排序并前置地址列，都与 Go 结构一一对应。

主要保持点包括：

- 六类 TiDB profile 在采集前记录表名和 session 身份；profile key 与 Go 相同。
- TiKV CPU 使用 `/debug/pprof/profile?seconds=30`；PD 使用 `/pd/api/v1/debug/pprof/...` 路径，goroutine 使用 `debug=2`。
- 远端请求统一传入 `allow_follower = true`，对应 Go 设置 `PD-Allow-follower-handle: true`；具体 header 由 client 实现负责。
- 缺失 status 地址保留 Go 的历史 TiKV warning 文本；节点错误降级为语句 warning；成功行首列是 status 地址。
- `visible_columns` 返回全部列、`hidden_columns` 为空，即使 `ColumnInfo.hidden` 为 true，也与 Go `VisibleCols` / `HiddenCols` 行为一致。

需要明确的差异与移植边界：

- Go `tableIDMap` 当前列出 35 表；Rust 追加了 9 个客户端需要的兼容表，并由 Rust DDL/测试保证元数据完整性。
- Go 使用完整 `model.TableInfo`、`table.Column`、`table.Index`、session context、infoschema 辅助函数和真实 HTTP/profile collector；Rust 使用本文件轻量类型和两个 trait，正常 Cargo 构建并未启用相应 AsterSQL 依赖。
- Go `init.go` 通过 `infoschema.RegisterVirtualTable(dbInfo, tableFromMeta)` 注册完整表驱动；Rust `init.rs` 保存自己的虚拟数据库快照，而 session 侧另行抽取元数据。目前没有证据表明 Rust 数据面工厂被 builder 或 executor 注册。
- Go 远端实现包含 HTTP request 创建、header、状态码检查、response body 关闭、failpoint 和 panic recovery；Rust 将这些细节移入 `RemoteProfileClient`，本文件只捕获 worker 本身越过 client 边界的 panic。
- Go 接收 `context.Context`，Rust `get_rows` / `data_for_remote_profile` 没有取消或 deadline 参数。
- Go 非等长投影直接索引，元数据错误可能 panic；Rust 用 `row.get` 将越界转为 `InvalidProjection`。

Go 测试 `tables_test.go` 通过 TestKit 验证 SQL 查询、真实 mock HTTP pprof 解析和连接属性行；Rust 测试覆盖元数据及抽象边界，但不等价于这组端到端验证。

## 扩展指南

新增 performance_schema 表时，应同步检查至少四处：`const.rs::perfSchemaTables` 中的 DDL、本文件的表名常量与 `TABLE_ID_MAP` 顺序、`get_rows`（若不是空表）的数据路由，以及 `tables_test.rs::test_client_required_registry_is_complete_and_stable` 的名称/列序/ID 断言。既有历史表不可插入重排，否则会改变稳定 ID；新表应追加并核对 Go 或客户端兼容要求。

新增本地数据表应优先扩展 `RowSource` 并在 `get_rows` 增加明确分支；若新增 profile 表，还要同步审计日志匹配集合，确保日志先于采集。新增远端节点类型或 profile URI时，应同步 `data_for_remote_profile` 的节点白名单、URL 路由、goroutine 解析标志、warning 兼容文本和独立测试。不要把传输细节塞回表逻辑，除非同时重新评估 Cargo 依赖边界。

修改列投影时必须保留或有意迁移 Go 的“等长即直返”契约，并为重排、重复 offset、越界 offset 增加 `tables_test.rs` 回归。修改远端并发策略时，应测试：节点发现硬失败、缺失地址、部分请求失败、解析失败、worker panic、稳定排序、全失败但整体成功，以及大节点数下的资源上限。

插件扩展应使用小写无关的 `register_plugin_table` / `unregister_plugin_table`，并避免依赖 map 锁在回调期间仍持有。测试必须放在独立的 `tables_test.rs`，不可内嵌到生产源文件；应覆盖插件优先级、替换/注销和错误传播。

若目标是让 profile/session 行真正经 Rust SQL 查询返回，不能只改本文件：还需在 session/executor 虚拟表读取链中提供生产 `RowSource`、`RemoteProfileClient`，注册或调用 `table_from_meta`，并以 SQL 集成测试证明数据面接线。该工作超出本单文件文档任务当前事实范围。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的 `tables.rs` 被识别为含 106 个符号。
- RustCodeGraph `files --filter pkg/infoschema/perfschema`：确认 crate 内 `const.rs`、`init.rs`、`lib.rs`、`tables.rs` 及独立测试的边界。
- RustCodeGraph `node --file pkg/infoschema/perfschema/tables.rs --offset 1 --limit 1000`：核对目标文件全部 562 行、主要类型、函数、分支和并发实现；图报告直接使用文件包括 `pkg/infoschema/perfschema/tables_test.rs` 与 `pkg/infoschema/go_merge_45_test.rs`。
- RustCodeGraph `node` 读取 `pkg/infoschema/perfschema/init.rs`、`pkg/session/runtime/system_query.rs`、`pkg/infoschema/builder.rs` 和 `pkg/infoschema/perfschema/tables_test.rs`：核对元数据构建、session 目录接线、虚拟表驱动形态及测试边界。对通用方法名执行的 `callers` / `callees` 查询没有返回可用调用边，随后用精确标识符仓库搜索补证。
- `pkg/infoschema/perfschema/Cargo.toml` 与 `lib.rs`：核对 crate 入口、正常依赖、恒禁用的移植依赖、模块声明和再导出。
- `pkg/infoschema/perfschema/tables.go`、`init.go` 与 `tables_test.go`：核对 Go 工厂、真实 session/HTTP/profile 实现、注册路径、错误降级、排序、投影和端到端测试意图。
- `pkg/infoschema/perfschema/tables_test.rs`：核对稳定 ID/列序、访问器、审计字段与时序、所有本地 profile 分派、PD URI、投影兼容、远端 warning/排序/行形状、不支持节点类型、插件优先级和索引错误。
- 仓库精确搜索 `table_from_meta`、`get_rows`、`iter_records`、`data_for_remote_profile` 及 crate 名：确认元数据生产接线位于 `system_query.rs`，而本文件数据面入口未发现测试外生产调用。该结论是静态仓库证据，不等同于运行时覆盖证明。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前仅执行任务指定的 11 章节结构验证，并人工检查没有把未接线能力描述为已接线。
