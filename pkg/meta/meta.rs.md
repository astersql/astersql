# `pkg/meta/meta.rs`

## 文件定位

`pkg/meta/meta.rs` 是 `astersql-meta` crate 的核心元数据实现，负责把数据库、表、AutoID、Schema 版本、策略、资源组、DDL 历史等对象映射到统一的 KV 键空间。crate 入口 `pkg/meta/lib.rs` 以私有模块 `mod meta` 挂载本文件，再通过 `pub use meta::*` 暴露公开 API；相邻的 `reader.rs` 在只读接口中复用 `Mutator` 的查询逻辑，`meta_autoid.rs` 则构造单表 AutoID 访问器。

当前 Rust crate 的依赖接线需要谨慎理解：`pkg/meta/Cargo.toml` 声明 crate 名 `astersql-meta`，直接依赖 `astersql-kv`、`astersql-util-codec`、`astersql-meta-model`、`serde/serde_json`、`chrono`、`futures` 等，但本文件使用的 `kv`、`structure`、`model`、`runtime`、`metrics` 等名称由 `pkg/meta/lib.rs` 的 `harness` 模块导出。这里已经实现并测试 Go `pkg/meta/meta.go` 的主要数据布局和事务语义，但不能仅凭本文件断言它已经接上完整 TiKV 生产后端。

文件没有条件编译项；测试模块由 `lib.rs` 在 `cfg(test)` 下从独立的 `meta_test.rs`、`migration_aster_unit_test.rs` 和 `harness_test.rs` 装配，符合测试与生产源码分离约束。

## 核心职责

1. 定义 `m` 前缀下的元数据键布局。全局计数器、Schema 版本、bootstrap 状态和开关使用 string 键；数据库目录使用 `DBs` hash；每个 `DB:<id>` hash 同时保存 `Table:<id>`、`TID:<id>`、`IID:<id>`、`TARID:<id>`、`SID:<id>` 等 field；策略、脱敏策略、资源组与 DDL 历史使用各自的 hash（`META_PREFIX` 及第 46—78 行常量）。
2. 用 `Mutator` 将一个 `kv::Transaction` 包装成 `structure::TxStructure`，在同一事务内完成读取、校验、序列化和写入；构造时设置高优先级及 `AllowedOnAlmostFull`，并保存 MVCC `start_ts`（`new_mutator`）。
3. 提供数据库、表、策略、资源组、Schema diff、bootstrap、系统表版本、BDR/MDL/缓存配置、ingest 参数、RU 统计和 DXF 参数的 CRUD 或标量访问方法（各段 `impl Mutator`）。
4. 提供大规模元数据读取的低内存路径：回调式 hash 遍历、分片并发扫描所有表、只抽取表名/ID 的 partial JSON 快路径，以及仅对带特殊属性的表完整反序列化（`iter_databases`、`iter_tables`、`iter_all_tables`、`fast_unmarshal_table_name_info`、`get_all_name_to_id_and_must_loaded_table_info`）。
5. 保存和倒序读取 DDL 历史，支持按 schema/table 名过滤；同时提供 DDL 回填元素的稳定二进制编码（`LastJobIterator`、`HLastJobIterator`、`Element`）。

## 主要符号

- `Mutator { txn: structure::TxStructure, start_ts: u64 }`：核心状态持有者。`txn` 决定所有读写的事务边界，`start_ts` 保留构造事务的版本。
- `new_mutator(txn, options) -> Mutator`：公开构造入口；按顺序消费 `OptionFn = Box<dyn FnOnce(&mut Mutator)>`。RustCodeGraph 确认其引用 `META_PREFIX` 并实例化 `Mutator`；文本补查发现 `br/pkg/utils/common.rs`、session 相关测试会构造它。
- 键辅助函数：`db_key`/`parse_db_key`、`table_key`/`parse_table_key`、三类 AutoID 键与 `sequence_key`。它们使用可读十进制格式；解析函数先验证前缀，再由 `parse_prefixed_id` 做 UTF-8 与整数解析。
- ID 与版本：`gen_global_id(s)`、`advance_global_ids`、`gen_placement_policy_id`、`gen_masking_policy_id`、`gen_schema_version(s)`。全局、placement、masking 三个 ID 空间分别受三个进程内 `Mutex<()>` 串行化。
- 查询与遍历：`list/get/iter_*` 系列、`iter_all_tables`、`get_all_name_to_id_and_must_loaded_table_info`、`get_table_info_with_attributes`。`list_tables` 逐项调用 `Context::check_error`；`iter_all_tables` 将并发数限制在 1..=15。
- 快速 JSON 路径：`MustLoadFilterAttr`、`CHECK_ATTRIBUTES_IN_ORDER`、`is_table_info_must_load`、`NAME_EXTRACT_REGEXP`、`fast_unmarshal_table_name_info`。这些实现依赖序列化字段与 marker 契约，只在必要时解码完整 `TableInfo`。
- 策略编码：`CURRENT_MAGIC_BYTE_VER`、`attach_magic_byte`、`detach_magic_byte`。policy/masking/resource-group JSON 前置一个版本字节；当前只接受 JSON 类型中的版本 `0x00`。
- DDL 历史：`add_history_ddl_job_public`、`get_history_ddl_job`、`get_*history*_iterator`、`LastJobIterator`、`HLastJobIterator`、`is_job_match`。job ID 使用 8 字节大端序 field，从而支持按 ID 倒序遍历。
- `Element` 与 `decode_element`：编码为 5 字节 `_col_`/`_idx_` 类型前缀加 8 字节大端 `i64`，总长 13 字节。
- `GroupRuStats`、`DailyRuStats`、`RuStats`：与 Go JSON 层次对应的 RU 消费快照模型。
- `NextGenBootTableVersion`、`DDLTableVersion`：以 `repr(i32)` 固定系统表演进阶段的持久化整数值。

## 执行流程

典型写流程从 `new_mutator` 开始：调用者交入事务，构造函数设置事务选项、读取 `start_ts`、用 `META_PREFIX` 创建 `TxStructure` 并执行选项；随后 CRUD 方法先计算 hash/field，调用 `check_*` 辅助完成存在性约束，序列化模型，最后执行 `set`、`hset`、`hdel`、`hclear` 或 `hinc`。例如 `create_table_and_set_auto_id` 先 `create_table_or_view`，再写 RowID；仅当 `auto_random_bits > 0` 时写 AUTO_RANDOM，仅当 `sep_auto_inc()` 且存在自增列时写独立 AUTO_INCREMENT。

读流程通常先确认父数据库存在，再从 hash 读取。`get_table` 解码后补上不在表 JSON 中持久化的 `db_id`；`list_simple_tables` 只取 `id/name`；`get_all_name_to_id_and_must_loaded_table_info` 遍历表 field，始终建立原始名称到 ID 的映射，只把包含外键、分区、锁、TiFlash、副本策略、TTL、affinity 等 marker 的对象完整解码。

全库扫描 `iter_all_tables` 先由 `split_range_int64_max` 将十进制 DB 键空间等分，给每段创建相同 `start_ts` 的 snapshot，并通过 `runtime::spawn` 并发执行有界 hash 扫描；每条记录检查取消状态、过滤非 `Table:` field、解码表并由 `parse_db_key` 回填库 ID。回调包在 `Arc<Mutex<F>>` 中，因此扫描并发而用户回调串行；最后 `runtime::try_join_all` 汇总任务错误。RustCodeGraph 给出的内部调用边为 `iter_all_tables -> split_range_int64_max/db_scan_key/parse_db_key`，并确认引用 `META_PREFIX`、`TABLE_PREFIX`。

Schema 版本通过 `gen_schema_version(s)` 自增；`get_schema_version_with_non_empty_diff` 在当前版本没有对应 diff 且版本大于零时退回一版，避免把尚未形成完整 diff 的版本暴露给读取方。`pkg/meta/reader.rs` 的 Reader 实现委托此方法，`pkg/ddl/schema_version.rs` 和 session DDL 测试通过 Reader 使用该语义。

DDL 历史写入前由 `Job::encode(update_raw_args)` 编码；读取按大端 job ID hash field 倒序迭代。过滤器先通过 partial JSON 提取 `schema_name/table_name`，两类非空过滤条件必须同时满足，再完整 `Job::decode`。

## 数据与状态

- 元数据状态全部位于一个 `TxStructure` 的 `m` 前缀中。数据库定义和数据库内部表/ID 分属两级 hash；删除数据库先清空 `DB:<id>` 再从 `DBs` 删除目录项，删除 policy/masking policy 同样先清其同名 hash 后删目录 field。
- 表的 `revision` 是缓存可见版本：`update_table` 在序列化前原地加一。创建或读取不会自动递增；调用方传入的 `TableInfo` 也会观察到该变化。
- 默认资源组 ID 固定为 `1`。未持久化时，`list_resource_groups` 补入 `default_group_meta()`，`get_resource_group(1)` 也返回内建对象；因此“没有 hash field”不等价于默认组不存在。
- metadata lock 与 schema cache getter 返回 `(value, is_null)`；缺失或空字节都映射为 `is_null = true`。ingest getter 则用 `Option<T>` 表达缺失。浮点 ingest 值持久化时保留两位小数。
- policy/masking/resource-group 值是“magic byte + JSON”，数据库、表、Schema diff、DXF、RU 则直接使用对应 JSON；DDL job 使用模型自己的编码格式。
- `get_oldest_schema_version` 手工组装 SchemaVersion string data key，读取 MVCC writes 中最老记录的 short value 并解析为十进制整数。

## 依赖与调用关系

上游边界以 `pkg/meta/lib.rs` 的再导出为主。`pkg/meta/reader.rs` 将 Mutator 查询方法适配到 Reader；`pkg/meta/meta_autoid.rs` 通过 `new_auto_id_accessors(self, db_id, table_id)` 复用事务结构。已找到的仓库调用证据包括：`br/pkg/utils/common.rs` 由备份恢复工具的事务构造 `Mutator`；`pkg/ddl/schema_version.rs` 经 Reader 获取“具有非空 diff 的版本”；session runtime 与 bootstrap 测试使用版本和系统表 API。RustCodeGraph 对若干公开方法未给出 callers，这属于当前索引的调用边覆盖限制，不能据此认定它们未使用。

主要下游依赖如下：`structure::TxStructure` 提供 string/hash 操作、流式遍历和反向迭代；`kv` 提供事务、快照、Key 和事务选项；`model` 提供所有持久化模型及 Job 编解码；`json`/`partialjson` 分别承担完整和局部 JSON 处理；`codec` 生成有界扫描键和 MVCC 数据键；`runtime` 负责并发任务；`context` 传播取消；`metrics` 观测 Schema diff 与历史 job 读取；`helper::Helper` 查询底层 MVCC 信息。

Cargo 没有声明 feature 或条件依赖。`package.metadata.porting.go-package = "pkg/meta"` 明确标注 Go 来源，`legacy-tasks = ["task-560", "task-579"]` 是迁移元数据，而非运行时开关。

## 错误处理与边界

所有可失败路径统一返回 `errors::Error` 并用 `?` 传播底层 KV、UTF-8、整数、JSON 或 Job 编解码错误。`require_hash_value/absent` 将“值缺失/已存在”转换为领域错误，创建与更新分别要求不存在和存在；数据库/表的子操作通常先验证父对象。

关键边界包括：全局 ID 超过 `metadef::MAX_USER_GLOBAL_ID` 时失败；policy、masking policy、resource group 创建拒绝 ID 0；未知或非当前 magic byte 被拒绝；`decode_element` 拒绝短于 13 字节或非 `_col_`/`_idx_` 前缀；找不到 SchemaVersion MVCC write 时 `get_oldest_schema_version` 返回明确错误。`parse_table_key` 先检查 `Table` 起始串，最终仍由 `parse_prefixed_id` 要求冒号后的合法整数；相关迁移测试还覆盖 `Tabletop:1` 的失败。

取消与回调错误不会被吞掉：`list_tables` 和 `iter_all_tables` 调用 `Context::check_error`，`iter_databases/iter_tables` 将访问者错误原样向上返回，历史迭代在过滤、解码或 `next` 失败时立即终止。

需要保留的兼容边界是快速 JSON 路径对字段名称、大小写和序列化顺序的依赖；修改模型序列化时，marker 列表、partial JSON 提取和测试必须同步。另一个边界是 `split_range_int64_max(n)` 假定 `n > 0`；公开的 `iter_all_tables` 会先 clamp，因此不要绕过入口用零调用该辅助函数。

## 并发与资源生命周期

`Mutator` 自身不建立后台任务，也不提交/回滚事务；事务生命周期属于构造它的调用者。所有写入共享同一个 `TxStructure`，因此跨方法原子性取决于外层 `kv::Transaction` 的提交边界。

三个静态互斥量只保护各自 ID 分配的读改写临界区，锁守卫随方法返回释放；它们彼此独立，避免不同 ID 空间无谓串行。锁中毒目前通过 `lock().unwrap()` 触发 panic，而非转换为 `errors::Error`，这是扩展时需要留意的行为。

`iter_all_tables` 为每个范围创建 snapshot 与异步任务，任务完成后由 `try_join_all` 回收；`Context` 克隆进入各任务。`Arc<Mutex<F>>` 保证用户回调不并发执行，回调锁也随调用退出释放。与 Go 实现相比，Rust 保留了并发上限、同一版本 snapshot、取消检查和串行回调语义；当前 harness 是否完整复现 Go 中请求来源 snapshot option，需要在接真实后端时单独验证。

`HLastJobIterator` 持有 `ReverseHashIterator`，每次 `get_last_jobs` 从当前位置继续推进，不自动 rewind；返回的新 `Vec` 只复用容量语义而不接受 Go API 的调用方 buffer。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/meta/meta.go`。Rust 基本保持 Go 的键常量、两级 hash 布局、ID/Schema 自增、CRUD 前置校验、默认资源组、magic byte、表快速加载条件、DDL job 大端 ID 与倒序迭代、系统库 classic/nextgen 分支以及表 revision 自增。

明确的语言适配包括：Go `Option func(*Mutator)` 变为一次性 `OptionFn`；指针/`nil` 结果变为值、`Option<T>` 或 `(value, is_null)`；Go Reader 接口由 `reader.rs` 的 Rust trait 实现；Go 的回调对象指针变为拥有所有权的模型值；Go `LastJobIterator.GetLastJobs(num, jobs)` 变为返回新 `Vec<Job>`。`IsJobMatch` 的 Rust 条件显式加括号，表达注释所述“schema 条件与 table 条件同时满足”；这比 Go 源码中容易受运算符优先级影响的书写更明确。

测试对应关系可由 `pkg/meta/meta_test.rs` 与 `pkg/meta/meta_test.go` 逐项核验：policy/masking/resource-group CRUD、`TestMeta`、snapshot、Element、键往返、数据库遍历、系统库、must-load、名称提取、数据库存在性、boot table version 和 DXF 因子均有对应 Rust 用例。`pkg/meta/migration_aster_unit_test.rs` 另行覆盖 Go 键字节布局、范围切分、库表/Reader 往返、AutoID、标量/策略/历史状态。当前 Rust 测试是基于共享内存 harness 的契约验证，不等价于真实 TiKV 集成测试。

## 扩展指南

新增一种元数据对象时，应先决定它是 string 键、目录 hash，还是某个数据库 hash 的 field，并保持 `m` 前缀与 Go 兼容；随后在本文件增加集中常量、键构造/解析、存在性校验和成对 CRUD。若值需要演进格式，复用或明确扩展 magic byte 协议，不能静默改变已有 JSON 字节。相关测试应放在独立的 `pkg/meta/meta_test.rs` 或迁移契约文件中，不要内嵌到 `meta.rs`。

修改表 JSON 或 fast path 时，必须同步审查 `CHECK_ATTRIBUTES_IN_ORDER`、`NAME_EXTRACT_REGEXP`、`is_table_info_must_load`、`fast_unmarshal_table_name_info` 和 `extract_schema_and_table_name_from_job`，并补充字段缺失、顺序变化、转义、错误类型和普通表不完整加载的测试；否则可能造成 infoschema 漏载或性能退化。

新增可并发扫描逻辑时，应沿用 `iter_all_tables` 的共同 snapshot、取消传播、1..=15 并发限制和串行回调约束；如果希望回调并行，必须先改变 API 的线程安全契约并评估调用方顺序假设。新增 ID 空间要明确是否需要独立锁及溢出上限。新增表更新路径必须决定是否递增 `revision`。

接真实生产后端时，重点核验 `new_mutator` 的事务优先级/磁盘满策略、snapshot 的内部请求来源选项、MVCC key 编码、metrics 标签、错误类型兼容性和提交生命周期。性能风险主要集中在整库 `hget_all`、完整 JSON 解码、全局回调锁和静态 ID 锁；兼容风险集中在持久化键、magic byte、枚举整数值和 JSON marker。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标 `pkg/meta/meta.rs` 含 239 个符号；通过 `node --file` 分段阅读完整 1—1409 行；`query` 确认 Rust/Go `Mutator`、`new_mutator`、`iter_all_tables` 定义；`callees iter_all_tables` 确认 `split_range_int64_max`、`db_scan_key`、`parse_db_key` 以及前缀常量边。公开函数的部分 callers 查询为空，随后仅对这项索引缺口使用 `rg` 补查。
- 源码与装配：`pkg/meta/meta.rs`、`pkg/meta/lib.rs`、`pkg/meta/reader.rs`、`pkg/meta/Cargo.toml`；直接调用点补查覆盖 `br/pkg/utils/common.rs`、`pkg/ddl/schema_version.rs` 和 session runtime/bootstrap 测试。
- Go 对照：`pkg/meta/meta.go`；核验了 `NewMutator`、ID 与键布局、CRUD、`IterAllTables`、magic byte、DDL history、ingest、bootstrap、Element/RU 等对应实现。
- 独立测试：`pkg/meta/meta_test.rs`、`pkg/meta/migration_aster_unit_test.rs`、`pkg/meta/harness_test.rs`；Go 对照测试为 `pkg/meta/meta_test.go` 和 `pkg/meta/main_test.go`。本任务遵循纯文档约束，没有运行 Cargo 或代码测试。
- 人工复核结论：本文件存在的原因是提供统一、事务化且与 Go 持久化格式兼容的元数据访问层；运行主线是“事务包装 → 键/存在性校验 → 编解码 → structure KV 操作”，大扫描与快速 JSON 路径是其主要性能分支；安全扩展必须同步持久化格式、Go 对照和独立测试。
