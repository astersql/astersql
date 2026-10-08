# `pkg/session/runtime/load_data.rs`

## 文件定位

该文件是 `astersql-session` crate 中 canonical `ConcreteSession` 的 `LOAD DATA` 执行实现。模块由 [`pkg/session/runtime.rs`](../runtime.rs) 以私有 `mod load_data` 装配；SQL 经解析并进入 [`ConcreteSession::execute_statement`](dispatch.rs) 后，`LoadDataStmt` 分支调用本文件的 `ConcreteSession::execute_load_data`。它不是 `pkg/executor/load_data.rs` 中通用导入执行器的包装层，而是 canonical session 为真实 SQL、KV mutation 和 Go testutil 兼容路径提供的一套独立接线。

crate 边界由 [`pkg/session/Cargo.toml`](../Cargo.toml) 确认：该实现直接依赖 `astersql-lightning-mydump` 解析 CSV，依赖 `astersql-objstore` 访问外部对象，依赖 `astersql-objstore-compressedio` 解压 gzip，并通过同 crate 的 `dml_runtime` 规划表达式和 INSERT。文件没有条件编译项；相关测试则在 crate 根或 `runtime.rs` 中用 `#[cfg(test)]` 独立装配，没有内嵌在生产文件中。

## 核心职责

1. `execute_with_load_data_reader` 为一次 SQL 执行临时安装客户端提供的 reader，并保证成功、错误或栈展开时都恢复原 reader；这是协议层把 `LOAD DATA LOCAL` 字节流交给 session 的入口。
2. `execute_load_data` 校验数据库、表、字段表和 `SET` 赋值，区分客户端输入与远端 URI，读取并可选解压文件，按 `LoadDataStmt` 的格式选项解析记录。
3. 将 CSV 字段映射到普通列或用户变量，计算列赋值，执行 NULL、定长字符串、整数前缀和 DATE 的兼容转换，并按 SQL mode 决定报错还是记录 warning。
4. 把全部合法记录转换成 `ast::InsertStmt`，经 `PlanInsert` 和 `execute_relational_insert_with_load_counts` 复用 canonical DML/事务/KV 管线，而不是自行写入存储。
5. 将重复键错误及 warning 规范化为 MySQL 错误码，最后写入 `Records/Deleted/Skipped/Warnings` 格式的 OK-packet 消息。

当前实现边界必须明确：LOCAL 输入既可来自请求 reader，也可回退到 server 进程可见的本地路径；非 LOCAL 的 server 本地磁盘被拒绝。远端读取当前只实际接线 GCS；S3 固定返回区域访问错误，其他 backend 返回“不支持”。输入与所有待插入行均先聚合进内存，不是 Go 执行器的流式并行实现。

## 主要符号

- `type LocalLoadDataReader = Box<dyn Read + Send>` 与线程局部 `LOCAL_LOAD_DATA_READERS`：以 `ConcreteSessionInner` 的 `Rc` 地址为键保存请求级 reader。线程局部存储避免全局共享，但也约束 reader 的安装和取用必须发生在同一线程。
- `enum LoadFieldTarget`：把输入位置区分为真实 `Column` 和仅供 `SET` 表达式引用的 `UserVariable`。
- `load_data_error(code, sql_state, message)`：生成带 MySQL code/SQLSTATE 文本的 `SessionError`。
- `mydump_value`：把 Mydumper datum 归一为 `Option<String>`；`Null` 保留为 `None`，整数转十进制文本，字节按有损 UTF-8 转换。
- `load_data_glob_match`：供 GCS 对象枚举使用的字节级 `*` 和 `[abc]` 匹配器；没有 `?`、范围、转义或 Unicode 字符级 glob 语义。
- `decode_load_data_file`：仅根据 `.gz` 后缀启用 gzip 解压，其他后缀原样返回。
- `load_numeric`：提取字符串开头的可选正负号、数字和单个小数点，返回 `f64` 以及“存在被截断尾部”的标志。
- `load_assignment_value`：递归求值用户变量、括号、CAST 和五种算术二元运算；其他表达式交给 `dml_runtime::EvalExpr`。除零返回 NULL，非数字尾部生成 DOUBLE 截断 warning。
- `load_data_table_refs`：把目标 `TableName` 包装为供 `InsertStmt` 使用的单表 `TableRefsClause`。
- `ConcreteSession::execute_with_load_data_reader`：公开的请求 reader 执行入口；内部 `Restore` 的 `Drop` 负责清除本次 reader 并恢复嵌套调用前的值，同时用 `FileTransferStatementRUScope` 延迟并完成 statement RU 终态。
- `ConcreteSession::has_file_transfer_reader`：供 `typed_adapter_bridge::SessionStatementRUScope::finish` 判断文件传输语句是否仍持有 reader，从而延迟无 record set 的终态处理。
- `ConcreteSession::LastMessage`、`ConcreteSession::QueryString`：分别暴露最近 OK 消息与 Go `sessionctx.QueryString` 对应文本。
- `ConcreteSession::execute_load_data`：文件主入口，负责从验证、读取、解析、转换到关系写入和结果汇总的完整同步流程。

## 执行流程

1. `runtime/dispatch.rs` 把解析后的 `LoadDataStmt` 分派给 `execute_load_data`。函数先补全当前数据库，解析 runtime table，并拒绝不存在的表、view 和 sequence。
2. 从非隐藏、非生成列构造可写列集合。显式字段表会检查未知列和重复列；用户变量按去掉 `@` 后的小写名称保存。没有显式字段表时，按所有非隐藏物理列消费 CSV 位置，包括生成列；生成列位置只消费输入，不进入 INSERT，避免后续列错位。
3. LOCAL 分支优先从 `LOCAL_LOAD_DATA_READERS` 取走请求 reader并读完；没有请求 reader 时读取语句路径。非 LOCAL 分支用 `ParseBackend` 解析 URI：GCS 可以按对象名直接读取或遍历后用有限 glob 过滤，匹配结果排序以保证稳定顺序；其他 backend 按当前支持矩阵返回错误。
4. 每个 `.gz` 文件分别解压。对每个源文件独立执行 `IGNORE n LINES`，其跳过发生在 CSV quote 解析之前；多文件拼接时如前一文件没有行终止符，会补一个终止符。
5. 从 AST 生成 `CsvConfig`，默认字段分隔符为制表符、行分隔符为换行、转义符为反斜线、NULL 标记为 `\N`，并允许空行。空行终止符或 `STARTING BY` 包含完整行终止符会提前失败。
6. `NewCSVParser` 循环读取行。字段过多或不足分别产生 1262/1261；strict mode 且没有 `IGNORE` 时立即失败，否则记 warning。每行先建立用户变量和列值映射，再计算 `ColumnAssignments`。
7. 按最终 `insert_columns` 构建值表达式：NULL 写入 NOT NULL 列在宽松模式下警告并替换为 `0`；定长字符串按字符数截断；非法整数使用数值前缀并警告；DATE 的空值、`null` 文本或全零文本归一为 `0000-00-00`。
8. 没有记录时直接成功。否则构造 `InsertStmt`：传递 `LOW_PRIORITY`；`REPLACE` 设置 `IsReplace`；显式 `IGNORE` 或非 REPLACE 的 LOCAL 输入设置 `IgnoreErr`。随后 `PlanInsert` 并调用 `execute_relational_insert_with_load_counts`，因此唯一键、生成列、外键、事务、KV priority 等由共享关系 DML 管线执行。
9. 写入失败时将 `[kv:1062]` 前缀改写为 MySQL 1062/23000；新 warning 中同类前缀也归一化。最终用解析记录数、复制行数和删除行数生成 `last_message`。

## 数据与状态

- reader 状态在 `LOCAL_LOAD_DATA_READERS: RefCell<HashMap<usize, LocalLoadDataReader>>` 中短暂存在。键是当前 `ConcreteSessionInner` 的稳定 `Rc` 指针地址；`execute_load_data` 通过 `remove` 取得所有权，保证消费期间 reader 不再留在 map 中。
- `ConcreteSession` 的 `state` 提供当前数据库、SQL mode、warning 列表、`last_message` 与 `last_query_string`。warning 会由 `set_warning_with_code` 累积，最终消息使用当前 warning 总数。
- `field_targets` 保留 CSV 的物理位置；`insert_columns` 只保留可写真实列并补入 `SET` 目标。两者分离是处理用户变量、生成列占位和赋值覆盖的关键不变量。
- 每行使用临时 `variables` 与 `values_by_column`，再生成 AST value expressions；所有行保存在 `rows` 后一次性构造 INSERT。`row_number` 是一基序号，同时是 `Records` 计数和诊断中的行号。
- 外部文件先组成 `Vec<(name, bytes)>`，再拼成单一 `bytes`；因此内存峰值至少包含远端文件集合、拼接缓冲和最终 INSERT 行值，数据规模扩大时会显著增长。

## 依赖与调用关系

- 上游主链：`ConcreteSession::execute`/解析流程 → `runtime/dispatch.rs::execute_statement` 的 `LoadDataStmt` 分支 → `execute_load_data`。
- 客户端传输链：协议或测试调用 `execute_with_load_data_reader` → 安装线程局部 reader → 普通 `execute` → `execute_load_data` 取走 reader；`typed_adapter_bridge.rs` 通过 `has_file_transfer_reader` 协调 statement RU terminal 的延迟完成。
- CSV 下游：`astersql_lightning_mydump::{NewCSVParser, Parser, StringReader, CsvConfig}` 提供兼容解析和 `MydumpDatum`。
- 存储下游：`astersql_objstore::parse::ParseBackend` 识别 URI；GCS 分支使用 `new_gcs_storage`、`WalkDir` 与 `ReadFile`；gzip 使用 `astersql_objstore_compressedio::new_reader`。
- 表达式与写入下游：`load_assignment_value` 调用 `dml_runtime::EvalExpr`；主流程调用 `dml_runtime::PlanInsert`，继而进入 `execute_relational_insert_with_load_counts` 的共享 DML/KV 实现。
- RustCodeGraph 对 `execute_load_data` 的 callees 证实了上述核心边：`NewCSVParser`、`ReadRow`、`LastRow`、`new_gcs_storage`、`ReadFile`、`PlanInsert` 以及本文件的各辅助函数。图索引没有识别 impl 方法的上游 caller，因此上游分派使用 `runtime/dispatch.rs` 的直接引用补证。

## 错误处理与边界

- 预验证错误保留 MySQL 兼容 code/SQLSTATE：未选择数据库 1046/3D000，表不存在 1146/42S02，不可更新目标 1288/HY000，未知列 1054/42S22，重复列 1110/42000。
- URI、权限和读取错误分别使用 8158、8159、8160；不允许 server 本地磁盘使用 8154；非法行配置使用 8162。LOCAL 文件系统错误当前保留 Rust I/O 文本，而不是统一包装为 MySQL code。
- strict mode 的判定是 `HasStrictMode()` 且 `OnDuplicate != Ignore`。字段数不符、赋值数值截断、NULL 到 NOT NULL、字符串超长会在严格模式失败或在宽松模式转 warning；整数转换路径即使产生 1366 warning 仍会截取数值前缀。
- 算术赋值使用 `f64`，可能有精度与溢出语义差异；除零返回 NULL。`mydump_value` 对非法 UTF-8 有损替换，不能保持原始字节。
- `expect("gzip returns a decompression reader")`、`expect("validated LOAD DATA column")` 依赖前置库契约和本函数验证不变量；若这些不变量被破坏会 panic。
- glob 是有限实现，且 GCS 创建使用 `no_credentials: true`、`send_credentials: false`；不能据此宣称已支持生产凭据发现、完整 glob 或其他云存储。
- failpoint `executor/commitOneTaskErr` 在规划 INSERT 前返回模拟提交错误，供失败路径验证。

## 并发与资源生命周期

当前主导入算法本身无 worker、任务或 channel：远端文件顺序读取，CSV 顺序解析，收集完整行集后同步调用关系插入。与 Go 的并行 encoder/committer 不同，这里不存在生产者背压或流式批次提交。

请求 reader 必须是 `Read + Send + 'static`，但存储在 `thread_local!` 中，实际安装和消费依赖同线程执行。`execute_with_load_data_reader` 的 `Restore` guard 会在正常返回、错误及 panic 栈展开时移除本次 reader，并恢复可能存在的外层 reader；`execute_load_data` 取出 reader 后，由局部变量在事务成功或失败结束时 drop。`pkg/executor/test/loaddatatest/load_data_test.rs` 的低优先级用例用自定义 `Drop` reader 验证成功和错误都会关闭资源，`pkg/session/load_data_runtime_test.rs` 验证可重试死锁后 reader 不被复用且连接仍可继续查询。

`RefCell` 和 session `state.borrow_mut()` 表明该路径依赖单线程、非重入借用纪律，不提供跨线程共享同步。外部 storage、解压 reader、parser 和临时 buffers 都由栈上所有权释放；没有显式 close 的后台任务，因为当前实现没有后台 worker。

## 与 Go 版本的对应关系

Go 的正式对照位于 [`pkg/executor/load_data.go`](../../executor/load_data.go)：`LoadDataExec::Open/Next/Close` 管理 local reader，`LoadDataWorker::loadRemote/LoadLocal/load` 通过 importer controller 建立 reader，并以 `errgroup`、`readerInfoCh`、encoder 和 committer 并行流式处理；`setResult` 生成同样的 `Records/Deleted/Skipped/Warnings` 消息。Rust 本文件对应的是这些行为在 canonical session 上的聚焦实现，而不是逐类型直译。

已对齐的外部语义包括：LOCAL 与 remote 分支、压缩读取、字段/用户变量和 `SET`、strict/IGNORE warning 策略、REPLACE/重复键计数、LOW_PRIORITY 传递、reader 清理以及 OK 消息格式。相关 Rust 证据见 `pkg/session/load_data_runtime_test.rs` 和 `pkg/executor/test/loaddatatest/load_data_test.rs`。

仍有结构性差异：Go 使用 importer 的数据文件发现、并行编码/提交、批次事务和 controller 生命周期；Rust 当前把输入与 AST 行全部缓存在内存，远端实际只接线 GCS，并通过通用关系 INSERT 完成写入。扩展时应以当前事实为基线，不应把 Go 的并发、存储覆盖面或流式内存上限描述成 Rust 已具备能力。

## 扩展指南

- 新增对象存储 backend：在 `execute_load_data` 的 `StorageBackend` match 中接线，并复用 storage API；必须增加独立 Rust 测试覆盖单对象、glob、多文件顺序、不存在对象、权限错误和压缩文件，避免只解除“不支持”错误而没有真实读取。
- 扩展 glob：优先替换或增强 `load_data_glob_match`，明确 `?`、字符范围、转义和 Unicode 规则，并为每种语义添加独立测试。
- 增加压缩格式：修改 `decode_load_data_file` 的后缀识别和 `CompressType` 映射，覆盖损坏流、空文件和多文件混合格式；不要在生产源文件中内嵌测试。
- 扩展表达式：修改 `load_assignment_value` 并与 `dml_runtime::EvalExpr` 的类型和 warning 规则对齐，尤其关注 decimal 精度、除零、NULL 传播、字符集和每行 warning 复制。
- 改善大文件性能：需要重构 `source_files`、拼接 `bytes` 和 `rows` 三层全量缓冲，设计流式 parser 与批次 mutation；这会影响事务原子性、重试、warning/计数、reader 生命周期和 Go 对齐，不能只局部替换容器。
- 修改列映射或转换：保持“输入物理位置”和“最终可写列”分离，并同步验证生成列占位、隐藏列、用户变量大小写、重复列、NOT NULL、字符串截断、整数前缀及 DATE 零值。
- 相关测试应优先扩展 `pkg/session/load_data_runtime_test.rs`；关系写入、REPLACE、LOW_PRIORITY 和共享 store 行为可扩展 `pkg/executor/test/loaddatatest/load_data_test.rs`；RU terminal/reader 清理可扩展 `pkg/session/runtime/scan_adapter_runtime_test.rs`；生成列/embedding 联动分别有 `pkg/session/dml_runtime_test.rs` 和 `pkg/session/runtime/inference_test.rs`。

## 验证依据

- RustCodeGraph：`status` 显示目标仓库索引可用；`files --filter pkg/session/runtime/load_data.rs` 确认文件含 27 个符号；两次 `node --file` 读取 1–779 行；`query/node/callees` 核对 `execute_load_data`、`execute_with_load_data_reader`、`load_assignment_value` 及其 CSV、对象存储、表达式和 INSERT 调用边。caller 图对 impl 方法无结果，故用源码引用补证。
- 生产源码：`pkg/session/runtime/load_data.rs`；模块与包契约：`pkg/session/runtime.rs`、`pkg/session/lib.rs`；上游分派：`pkg/session/runtime/dispatch.rs`；RU/reader 桥：`pkg/session/runtime/typed_adapter_bridge.rs`；crate 依赖：`pkg/session/Cargo.toml`。
- Go 对照：`pkg/executor/load_data.go` 中的 `LoadDataExec`、`LoadDataWorker::{loadRemote, LoadLocal, load, setResult, Close}`。
- 独立 Rust 测试：`pkg/session/load_data_runtime_test.rs` 覆盖 LOCAL/server 文件边界、缺失文件、用户变量、view/sequence、整数前缀、IGNORE LINES 和死锁后的 reader/连接；`pkg/executor/test/loaddatatest/load_data_test.rs` 覆盖格式、NULL、REPLACE、重复键计数、LOW_PRIORITY 和 reader drop；`pkg/session/runtime/scan_adapter_runtime_test.rs` 覆盖文件传输 RU terminal 清理；`pkg/session/dml_runtime_test.rs` 与 `pkg/session/runtime/inference_test.rs` 覆盖共享 mutation、生成列和行号诊断。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付验证使用任务文件给定的 11 章节结构命令，并人工检查所有“已支持”结论均有限定和源码/测试依据。
