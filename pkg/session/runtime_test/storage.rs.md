# `pkg/session/runtime_test/storage.rs`

## 文件定位

本文件是 `astersql-session` crate 的存储路径回归测试模块，不是生产实现。crate 根在 `pkg/session/Cargo.toml` 中声明为 `lib.rs`；`pkg/session/lib.rs:187-188` 仅在 `cfg(test)` 下装配 `runtime_test`，随后 `pkg/session/runtime_test.rs:330-331` 通过 `#[path = "runtime_test/storage.rs"] mod storage;` 引入本文件。因此其中九个无参数、无返回值的私有 `#[test]` 函数只会进入 Rust 测试构建，不构成应用运行时 API。

被测生产逻辑主要位于 `pkg/session/runtime/relational_scan.rs`，覆盖关系型表记录计数、TiKV 聚合请求与响应、按主键和二级索引构造扫描范围、`LIMIT/OFFSET` 窗口扫描以及 `ConcreteSession` 的存储执行路径。测试使用 `runtime_test.rs` 中定义的 mock iterator/snapshot 和 `concrete_session()`，也使用 `crate::runtime::CreateAnalyzeSession()` 建立带内存存储的完整测试会话。

## 核心职责

本文件用九个回归场景约束四组行为：

1. `COUNT(*)` 的本地回退扫描只访问 record key、总是关闭迭代器；TiKV 快路径生成“表扫描 + Partial1 Count” DAG，并能累加多个 region 的部分结果，也能以 checksum 响应的 `total_kvs` 取得计数。
2. 大 `OFFSET` 先以 `kv::KeyOnly` 定位窗口，再只为最终结果窗口读取 value；整数聚簇主键的稀疏间隙必须通过额外跳过量补偿，不能把“句柄值差”误当作“行数差”。
3. 主键游标把有符号和无符号 `BIGINT` 谓词转换为精确 record-key 范围，并通过真实 `ConcreteSession` 验证普通、降序、游标和无符号分页结果。
4. 关系型 DML 与 `ADD INDEX` 必须维护/回填二级索引 KV；可用的公开本地二级索引应支持单列、复合 tuple、升降序游标及无 `ORDER BY` 的等值 `LIMIT` 查询，并返回正确行序和边界。

这些职责是对生产代码的行为约束，不是独立存储子系统。测试失败通常表示 `relational_scan.rs`、会话 DML/DDL 接线、tablecodec 编码或 mock storage 生命周期发生了不兼容变化。

## 主要符号

- `scalar_count_star_counts_only_record_keys_and_closes_iterator`：向 `count_relational_rows` 注入 `KeyOnlyCountRetriever`，断言 250,000 个 record key 被计数且 `Close()` 通过 `AtomicBool` 可见；父模块的 iterator 在读取 value、点查或反向扫描时会 panic，使访问模式也是断言的一部分。
- `large_offset_locates_with_keys_then_reads_only_the_result_window_values`：以句柄 10/20/30/40/50 构造 `OffsetWindowSnapshot`，调用 `scan_relational_rows_window_key_only(..., offset=3, count=2, ...)`，断言结果为 40/50，且 `KeyOnly` 选项历史严格为 `[true, false]`。
- `integer_handle_seek_compensates_sparse_primary_key_gaps`：验证 `integer_handle_offset_candidate` 从首句柄 10 和 offset 3 产生候选句柄 13；因 13 之前仅有一条实际记录，再以剩余 offset 2 调用 `scan_relational_rows_window_key_only_from`，最终仍应得到 40/50。
- `primary_key_cursor_builds_exact_signed_and_unsigned_record_ranges`：解析 SQL AST 后调用 `relational_primary_key_scan_ranges`。有符号 `id > 12000000` 的下界应是 12000001；无符号值跨越 `i64::MAX` 时应按 tablecodec 的有符号字节序拆分/换算范围，首个解码下界为 `i64::MIN + 1`。
- `scalar_count_star_builds_tikv_aggregation_and_sums_region_partials`：同时检查标量非空常量 COUNT 的路由判定、DAG protobuf 结构、两段部分计数 120,000 + 130,000 的解码累加，以及 checksum 请求/响应契约。
- `relational_select_applies_limit_offset_window`：通过真实测试会话执行建库、建表、插入和四类 SELECT，覆盖无序 OFFSET、降序分页、主键游标与跨有符号边界的无符号主键排序。
- `secondary_index_kv_tracks_relational_dml_and_backfill`：局部闭包 `scan_index_handles` 直接扫描 `GetTableIndexKeyRange`，以 `DecodeIndexHandle` 解码索引项，依次验证 INSERT、UPDATE、DELETE 和事后 `ADD INDEX` 回填。
- `secondary_index_cursor_uses_ordered_index_lookup`：先直接调用 `ConcreteSession::relational_secondary_index_access` 确认命中 `idx_created_id`，再执行单列、tuple、降序和同索引值多句柄查询，验证索引扫描后行查找与稳定排序。
- `secondary_index_equality_limit_does_not_require_order_by`：确认 `user_id = 14 LIMIT 4` 即使没有 `ORDER BY` 也能选择 `idx_user_id`，且只返回前四个匹配行。

本文件不定义常量、类型、trait、impl 或公开 API；它通过 `use super::*` 复用父测试模块中的 mock 类型、同步原语、tablecodec/kv 别名及生产函数导入。

## 执行流程

测试由 Rust test harness 并行发现并分别调用，不存在本文件内部的顺序依赖。低层测试直接构造 `TableInfo`、AST、mock retriever/snapshot 或 tipb protobuf，然后调用单个生产函数并检查结构或状态；高层测试则执行以下链路：

1. `CreateAnalyzeSession()` 建立内存存储、domain 与 `ConcreteSession`。
2. `session.execute(...)` 完成 DDL/DML，元数据由 `session.domain().stats_table(...)` 取回。
3. SELECT 经解析、访问路径选择和关系型 KV 扫描返回 record set，测试反复调用 `next_row()` 直到 `None`。
4. 二级索引物理校验绕过 SQL 层，从当前 storage version 创建 snapshot，扫描表/索引 ID 对应的 key range，解码每个 index handle，并显式关闭 iterator。

生产侧关键链路由 RustCodeGraph 定位到 `pkg/session/runtime/relational_scan.rs`：`count_relational_rows` 扫描 `GenTableRecordPrefix` 并在所有正常/错误路径后关闭 iterator；`relational_count_dag` 构造 `TableScan -> Aggregation(Count, Partial1Mode)`；`decode_relational_count_response` 遍历 chunks 和 datum 并做溢出检查；`integer_handle_offset_candidate` 暂时启用 `KeyOnly`、读取首键、恢复选项并编码候选句柄；`relational_primary_key_scan_ranges` 处理分区、signed/unsigned 域和降序；`relational_secondary_index_access` 过滤非公开、主键、不可见、MV、全局、表达式或前缀索引，再生成本地物理表范围。

## 数据与状态

核心数据是 `astersql_meta_model::TableInfo`、`ColumnInfo` 和 `IndexInfo` 元数据，`kv::Key`/value 字节串，解析后的 `SelectStmt`/`ExprNode`，以及 `tipb::{DagRequest, SelectResponse, ChecksumRequest, ChecksumResponse}` protobuf。record key 由 table ID 与 handle 编码；二级索引 key range 由 table ID 与 index ID 决定，handle 从索引 key/value 解码。

父模块的 `KeyOnlyCountRetriever` 保存预期行数和共享 `Arc<AtomicBool>`；其 iterator 只维护 `remaining`，`Close` 以 `Ordering::Release` 写入关闭标记，测试以 `Ordering::Acquire` 读取。`OffsetWindowSnapshot` 保存有序 KV 行、当前 `key_only` 状态、`option_history`，并用 `Mutex<Vec<bool>>` 记录每次创建 iterator 时的模式；`SetOption(kv::KeyOnly, ...)` 同时改变状态并追加历史。由此测试不仅验证结果，还验证了“定位阶段不读 value、结果阶段恢复读 value”的状态转换。

完整会话场景中的数据库名和表名每个测试各自独立，状态驻留在该测试创建的 mock storage/domain 内。record set 被局部变量持有并消费，没有跨测试共享 SQL 状态。

## 依赖与调用关系

上游只有测试装配：`pkg/session/lib.rs` 的 `#[cfg(test)] mod runtime_test` 指向 `pkg/session/runtime_test.rs`，后者再装配本文件。RustCodeGraph 对九个测试符号均能唯一定位到本文件；`callers` 为空符合 test harness 通过注册信息而非普通 Rust 调用表达式启动测试的事实。

直接下游包括：

- `pkg/session/runtime/relational_scan.rs` 的 `count_relational_rows`、`relational_count_dag`、`decode_relational_count_response`、`decode_relational_checksum_count_response`、`integer_handle_offset_candidate`、`scan_relational_rows_window_key_only(_from)`、`relational_primary_key_scan_ranges`、`is_scalar_count_non_null_constant` 和 `ConcreteSession::relational_secondary_index_access`。
- `pkg/session/runtime_test.rs` 的 `KeyOnlyCountRetriever`、`OffsetWindowSnapshot`、`concrete_session()` 与通用导入；`crate::runtime::CreateAnalyzeSession()` 提供完整会话夹具。
- `astersql-kv` 提供 snapshot/retriever/iterator 与 `KeyOnly` 选项；`astersql-tablecodec` 提供 record/index key 编解码；`astersql-parser*` 提供 SQL AST 和类型 flag；`astersql-meta-model` 提供表/列/索引元数据；`protobuf` 和 `tipb` 提供 TiKV 请求响应结构。

`pkg/session/Cargo.toml` 将 crate 映射到 Go 包 `pkg/session`，声明 `nextgen` feature，但本文件没有条件编译分支，不因该 feature 改变测试集合。上述依赖均是该 crate 的普通依赖；测试夹具的 mock storage 也来自 workspace 内的 store crate。

## 错误处理与边界

测试用 `expect`/`assert` 将任何意外 `SessionError`、KV 错误、解析错误或 protobuf 错误转为立即失败。刻意覆盖的边界包括：空洞整数主键、跨 `i64::MAX` 的 unsigned 主键域、正反排序、offset 与 count 的组合、tuple 游标中相同首列值、无 `ORDER BY` 等值索引查找，以及 DML 后的陈旧索引项清理和 `ADD INDEX` 回填。

父 mock 对不允许的调用直接 panic：COUNT 路径不能读取 value、点查或反向扫描；OFFSET 路径不能点查或 batch-get，key-only iterator 不能读取 value。这些 panic 是负向契约。生产实现还显式处理计数溢出、非法 TiKV datum 类型、TiKV response error、空表/非整数 handle、候选 handle 加法溢出，以及仅允许物理正序执行整数 handle OFFSET seek；本文件对其中部分成功路径有直接覆盖，错误分支并未全部覆盖。

测试未证明任意复杂谓词、表达式/前缀/全局/MV/不可见索引可走该快路径；`relational_secondary_index_access` 明确过滤这些索引，未匹配时返回 `None` 交由其他执行路径处理。

## 并发与资源生命周期

九个测试可由 test harness 并行执行，但每个完整会话测试使用独立数据库名和独立 `CreateAnalyzeSession()` 结果，避免共享表状态。父模块的 `Arc<AtomicBool>` 仅验证 iterator 关闭的跨所有权可见性；`Mutex<Vec<bool>>` 保护 iterator 模式历史，使 `Snapshot::Iter(&self, ...)` 能安全记录调用。这里没有线程创建、异步任务或 channel。

资源约束的重点是 iterator：`count_relational_rows` 无论循环成功还是返回错误都在返回前调用 `Close()`；二级索引扫描闭包在读完范围后也显式 `Close()`。snapshot 和 record set 由局部所有权在测试结束时释放，domain/storage 的生命周期受返回的会话与 `_domain` guard 约束。扩展测试时应继续显式验证错误路径的关闭与 `KeyOnly` 恢复，避免只检查行值而遗漏资源泄漏或状态污染。

## 与 Go 版本的对应关系

仓库中没有与 `pkg/session/runtime_test/storage.rs` 一一对应的 Go 文件或同名测试；这是 Rust 迁移层针对 `ConcreteSession` 关系型 KV 路径新增的聚焦回归集合。Go 侧仍提供协议与编码语义来源：`pkg/tablecodec/tablecodec.go` 的 `EncodeRowKeyWithHandle`、`DecodeRecordKey`、`DecodeIndexHandle`、`GetTableIndexKeyRange` 对应本测试使用的 Rust tablecodec API，`pkg/kv/option.go` 将 `KeyOnly` 定义为仅取 key 的扫描选项。

SQL 行为可由现有 Go 测试作旁证而非逐函数镜像：`pkg/executor/executor_required_rows_test.go::TestLimitRequiredRows` 约束 LIMIT/OFFSET 所需行数，`pkg/executor/test/indexmergereadtest/index_merge_reader_test.go::TestOrderByWithLimit` 等覆盖索引、顺序与 LIMIT 的组合，其他 executor 测试广泛覆盖 `ADD INDEX` 后查询。Rust 文件更直接地检查 KV 字节范围、tipb 聚合结构和 mock iterator 模式，因此不能删除这些低层断言而仅依赖 Go SQL 测试。

## 扩展指南

- 修改 COUNT 快路径时，应扩展 `scalar_count_star_counts_only_record_keys_and_closes_iterator` 或 `scalar_count_star_builds_tikv_aggregation_and_sums_region_partials`，同步检查 DAG executor 顺序、字段类型/flag、部分结果类型、checksum 配置、溢出和错误响应；生产接入点在 `relational_scan.rs` 的 COUNT 相关函数。
- 修改分页或主键范围时，应在 `large_offset_*`、`integer_handle_seek_*`、`primary_key_cursor_*` 和 `relational_select_applies_*` 增加边界，尤其是空表、offset 0、count 0、最大 offset、倒序、分区、负数及 unsigned 环绕。mock 类型继续放在独立父测试文件或同目录独立测试辅助文件中，不应把测试逻辑嵌入生产源文件。
- 修改二级索引选择或行查找时，应同步 `secondary_index_cursor_*` 与 `secondary_index_equality_*`，并明确新索引类型是否仍应被 `relational_secondary_index_access` 排除；修改 DML 索引维护时同步 `secondary_index_kv_tracks_*` 的 INSERT/UPDATE/DELETE/backfill 物理断言。
- 新增场景优先延伸本独立测试文件，保持独立数据库名、确定性行序和 iterator 关闭；若引入共享线程或异步资源，应增加终止/回收断言。
- 兼容性风险集中在 Go tablecodec 字节序、unsigned handle 映射、tipb protobuf 字段和 TiDB SQL 排序/NULL 语义；性能风险集中在错误地读取 OFFSET 前的 value、退化为全表扫描、过量 batch-get，以及遗漏索引范围导致回表放大。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标目录中的 `storage.rs` 已索引为含 10 个符号的文件。使用 `node --file` 分段读取了全部 742 行，并用 `query` 唯一定位九个测试函数。
- 装配与夹具：`pkg/session/lib.rs:187-188`、`pkg/session/runtime_test.rs:16-43,96-303,320-333`；后者定义共享 mock、`concrete_session()` 和 `storage` 子模块装配。`pkg/session` 下不存在 `doc.go`，因此无额外包契约文件可读。
- 生产符号查询：RustCodeGraph `node` 核对了 `pkg/session/runtime/relational_scan.rs` 中 `count_relational_rows`（193）、`relational_count_dag`（254）、`decode_relational_count_response`（460）、`relational_primary_key_scan_ranges`（1498）、`integer_handle_offset_candidate`（1810）、`scan_relational_rows_window_key_only_from`（1858）和 `relational_secondary_index_access`（1993）；其调用轨迹显示这些函数继续调用 KV iterator、tablecodec、谓词范围与二级索引 bounds 辅助逻辑。
- crate 与 Go 对照：读取 `pkg/session/Cargo.toml`；检索并核对 `pkg/tablecodec/tablecodec.go:114,143,1014,1220`、`pkg/kv/option.go:38-39`、`pkg/executor/executor_required_rows_test.go:131` 和 `pkg/executor/test/indexmergereadtest/index_merge_reader_test.go:657`。未发现目标文件的一一对应 Go 测试，文中未将这些旁证冒充直接移植来源。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证目标文件存在且固定二级标题恰好为 11，并人工复核所有九个测试、生产实现位置、资源边界与扩展入口均有明确来源。
