# `br/pkg/stream/rewrite_meta_rawkv.rs`

## 文件定位

本文件属于 `astersql-br-pkg-stream` library crate；`br/pkg/stream/Cargo.toml` 将 crate 根固定为 `lib.rs`，后者通过 `#[path = "rewrite_meta_rawkv.rs"] pub mod rewrite_meta_rawkv` 装入模块，并用 `pub use rewrite_meta_rawkv::*` 扁平导出公共符号。它负责日志恢复阶段的 **meta RawKV** 重写：把上游数据库、表和分区 ID 映射成下游 ID，同时处理 TiKV Default CF / Write CF 中的元数据值。数据面的普通 table record/index key 不属于本文件职责。

Go 对应实现是 `br/pkg/stream/rewrite_meta_rawkv.go`。Go 生产链在 `br/pkg/task/stream.go` 构造 `SchemasReplace`，再由 `br/pkg/restore/log_client/client.go` 的批处理路径调用 `RewriteMetaKvEntry`。RustCodeGraph 对当前 Rust 树的查询以及补充的全仓符号搜索只找到 `br/pkg/stream/rewrite_meta_rawkv_test.rs` 对 Rust `RewriteMetaKvEntry` 的调用，因此当前 Rust 实现是 crate 对外导出的迁移 API 和已测试逻辑，但其生产调用接线尚未在 Rust 侧出现。

## 核心职责

- `SchemasReplace` 保存上游到下游的 DB/表/分区映射、恢复时间戳和删除表跟踪状态；`NewSchemasReplaceWithHooks` 还预先构造 delete-range 所需的扁平表 ID 映射。
- `RewriteMetaKvEntry` 识别 meta DB key，并按 DB 信息、表信息、auto-increment、auto-table-ID、sequence、auto-random 等 Field 形态分派；非 meta key、Key 侧不是 DB key 或未识别 Field 返回 `Ok(None)`。
- `rewriteKeyForDB` / `rewriteKeyForTable` 改写事务 meta key 中的 ID；Write CF 额外把 key 的时间戳替换为 `RewriteTS`，Default CF 保留原时间戳。
- `rewriteValue` 区分 Default CF 和 Write CF：Default CF 直接改写 JSON；Write CF 保留 rollback，给非 rollback 记录标注 physical-import txn source，并区分 delete、无 short value 的 put、带 short value 的 put。
- `rewriteDBInfo` / `rewriteTableInfo` 改写 JSON 中的对象 ID；完整表路径还改表名、严格改分区 ID、关闭 TTL，并触发 `AfterTableRewrittenFn`。
- `deletedTables` 记录 Write CF delete 语义中的表删除状态，并在随后观察到 put 时撤销同一上游表的删除标记，以支持 rename / exchange-partition 往返场景；rollback 原样返回且不记录删除。
- `processIngestIndexAndDeleteRangeFromJob` 把 `Job.DelRangeArgs` 变成 `PreDelRangeQuery` 并通过回调交给上层；它是当前 Rust 的简化 helper，并不等价覆盖 Go 中 ingest recorder 与 `ddl.AddDelRangeJobInternal` 的全部行为。

## 主要符号

- `SchemasReplace`：模块核心可变状态。公开字段 `DbReplaceMap`、`RewriteTS`、`AfterTableRewrittenFn` 分别提供映射、Write CF 重写时间戳和表信息回调；私有字段 `fromPitrIdMap`、`deletedTables`、`record_delete_range`、`global_table_id_map` 控制缺映射策略、删除跟踪和 delete-range 输出。
- `NewSchemasReplace(...) -> SchemasReplace`：无 delete-range hook 的便捷构造器。
- `NewSchemasReplaceWithHooks(...) -> SchemasReplace`：构造主入口。它只把未 `FilteredOut` 的表和分区放入 `global_table_id_map`；过滤库也整体跳过。
- `RewriteMetaKvEntry(&mut self, key, value, cf)`：单条 meta KV 的公开分派入口，返回 `Result<Option<kv::Entry>, Error>`；`None` 表示该条不适用或应过滤，不是空 entry。
- `RewriteKeyForDB` / `rewriteKeyForDB`：DB 列表项 key 重写。DB ID 位于 `RawTxnMetaKey.Field`，这与表 key 的 DB ID 位于 `Key` 不同。
- `rewriteKeyForTable`：泛化的表作用域 key 重写，通过 `parse_field` / `encode_field` 适配 Table、AutoIncrement、AutoTable、Sequence、AutoRandom 五类 Field。
- `rewriteValue` 与私有 `rewriteResult { NewValue, Deleted, Put }`：统一 CF 语义；`Deleted` 和 `Put` 只驱动状态跟踪，不表示本函数直接删除或写入存储。
- `rewriteDBInfo`：反序列化 `model::DBInfo`、换 ID、再序列化。
- `rewriteTableInfo`：完整 `model::TableInfo` 重写；分区必须全部有映射，TTL 被禁用，非空映射名称覆盖原表名，最后调用 hook。
- `rewriteTableInfoInner`：较宽松的私有变体，只改表/已有分区映射，不关闭 TTL、不改名、不调 hook。当前文件没有调用它，RustCodeGraph 也未找到外部调用；不能把它描述为现行热路径。
- `GetDeletedTables`：返回累计删除集合的只读引用，键和值都保留上游 ID。
- `DelRangeParams`、`PreDelRangeQuery`、`BRInsertDeleteRangeSQLPrefix`、`BRInsertDeleteRangeSQLValue`：delete-range 回调的数据契约。
- `processIngestIndexAndDeleteRangeFromJob` / `remap_hex_table_key`：从简化 `model::Job` 生成参数，并优先按 tablecodec 二进制前缀改写 hex key；解码或前缀匹配失败时退回字符串替换。
- `DownstreamID`、`NewDBReplace`、`NewTableReplace`、`TableReplace`：从 `stubs` 再导出，避免调用方深入桩模块。

## 执行流程

1. 调用方用上游 ID 为键的 `HashMap<UpstreamID, DBReplace>` 构造 `SchemasReplace`。构造阶段遍历未过滤的 DB/表，把表和分区的旧→新 ID 展平，供后续 delete-range 使用。
2. `RewriteMetaKvEntry` 先调用 `utils::IsMetaDBKey`。若不是 meta DB key，Rust 当前直接返回 `None`；若是，则用 `ParseTxnMetaKeyFrom` 解出 `Key`、`Field`、`Ts`。
3. 若 `Field` 是 DB key，走 `rewriteEntryForDB`：先根据 CF 改写 DBInfo 值，再改 Field 中的 DB ID；Write CF delete 会在 `deletedTables` 中为该 DB 建立空集合。
4. 否则要求 `Key` 是 DB key，再检查 Field 类型。普通 Table key 走 `rewriteEntryForTable`；四类计数/sequence key 走 `rewriteEntryForTableScopedKey`，只改 key 并原样复制 value。
5. 表信息路径用 `rewriteValue` 处理值，用 `rewriteKeyForTable` 同时替换 Key 中的 DB ID 和 Field 中的表 ID。完整 JSON 重写由 `rewriteTableInfo` 完成：严格映射分区、关闭 TTL、按需改名并触发 hook。
6. Write CF rollback 原样返回且不改变删除集合；delete 被标记为 physical import 并把旧表 ID 加入 `deletedTables`；put 同样被标记，且会移除此前相同 DB/表的删除记录。
7. DDL delete-range helper 是独立公开路径：`NeedGC == false` 时无操作；否则逐个 remap `DelRangeArgs` 的 start/end key，组装固定 SQL 模板和参数。若提供了 `record_delete_range` 回调则调用一次；没有回调时结果被丢弃但仍返回成功。

## 数据与状态

映射以 **上游 ID 为 HashMap key、下游 ID 为 value 字段**：`DbReplaceMap[old_db].DbID` 是新 DB ID，`TableMap[old_table].TableID` 是新表 ID，`PartitionMap[old_partition]` 是新分区 ID。`FilteredOut` 控制库/表过滤；DB 的 `Reused` 只在 DB 列表项路径中参与过滤，表 key 路径没有单独检查 `Reused`。

`RewriteTS` 只写入重编码后的 Write CF meta key，不直接改 Write CF value 中记录的 start TS。值侧通过 `RawWriteCFValue` 保存 write type、short value 和 txn source。Default CF JSON 与 Write CF short value 复用同一 DB/Table JSON 改写闭包；没有 short value 的 put 无需读取 Default CF 内容。

`deletedTables: HashMap<UpstreamID, HashSet<UpstreamID>>` 是随调用累积的会话状态。表 delete 插入旧表 ID；DB delete 只创建空集合；后续表 put 可撤销旧表 ID。`AfterTableRewrittenFn` 和 `record_delete_range` 是 `FnMut + Send`，可保存跨调用状态，但调用发生在持有 `&mut self` 的同步路径内。

`global_table_id_map` 在构造时拍快照；构造后若调用方直接修改公开的 `DbReplaceMap`，该扁平映射不会自动同步。delete-range 映射查不到 ID 时保留上游 ID，而 Go wrapper 的 `RewriteTableID` 会返回 `exists=false` 交给 DDL 流程决定如何处理，这是重要兼容差异。

## 依赖与调用关系

上游模块边界由 `br/pkg/stream/lib.rs` 提供，公共 API 可通过 crate 根或 `rewrite_meta_rawkv` 模块访问。当前 Rust 直接依赖：

- `crate::meta_kv::{ParseTxnMetaKeyFrom, RawWriteCFValue}`：事务 meta key 与 Write CF value 编解码。
- `crate::stubs::{meta, model, utils, DBReplace, TableReplace, kv::Entry, tablecodec}`：迁移期的 TiDB meta/model/工具与映射类型。
- `astersql_br_pkg_utils_consts::{DefaultCF, WriteCF}`：CF 名称常量，对应 `Cargo.toml` 的本地 path dependency。
- `serde_json`：DBInfo/TableInfo JSON 往返；`hex`：delete-range table prefix 编解码；二者都由本 crate 的 `Cargo.toml` 直接声明。
- 标准库 `HashMap` / `HashSet`：映射和删除集合。

内部主调用边为 `RewriteMetaKvEntry → rewriteEntryForDB/rewriteEntryForTable/rewriteEntryForTableScopedKey`；DB/表 entry 路径再调用 `rewriteValue` 和各自 key/value 重写器。RustCodeGraph 对 `NewSchemasReplaceWithHooks` 给出定义节点，但对方法名查询未解析出可靠的外部 caller/callee；以精确源码搜索补充后，Rust 生产文件仍无入口调用，只有 `rewrite_meta_rawkv_test.rs` 和 `parity_test.rs` 构造或调用这些 API。

Go 侧的真实应用链是 `task/stream.go: NewSchemasReplace` → `restore/log_client/client.go: RewriteMetaKvEntry`。因此扩展 Rust 生产链时，应先在 Rust log-client 等价批处理位置接入，而不是假设 crate 的再导出本身代表已经运行。

## 错误处理与边界

- meta key 解析、DB/Table Field 解析、Write CF 解析、JSON 反序列化/序列化都转换为 `stubs::errors::Error` 并向上传播。
- 普通模式下 DB、表或完整表重写所需的分区映射缺失，会返回带 `berrors::ErrInvalidArgument` 注解的错误。`fromPitrIdMap == true` 时 DB/表缺映射改为 `Ok(None)`，但完整表路径的分区缺映射仍报错。
- `FilteredOut`、DB 列表项的 `Reused`、系统/临时系统库、非目标 key 与未知 Field 都以 `None` 静默过滤。
- `rewriteValue` 只接受 `DefaultCF` 和 `WriteCF`；其他字符串会 `panic!("not support cf")`，因此 CF 必须由受控常量传入。
- Write CF rollback 原样保留，不加 physical-import 标记；delete 和 put 会加标记。带 short value 的 rewrite 闭包若返回 `None`，Rust 用空字节替换 short value；这沿用了当前实现，但接线方不应把 `None` 误认为整条 entry 已过滤，因为 key 过滤在之后单独发生。
- `rewriteTableInfo` 是严格完整路径；`rewriteTableInfoInner` 对缺分区映射保留旧 ID，而且当前未调用。新增调用前必须明确选择严格还是宽松契约。
- `remap_hex_table_key` 的字符串回退是 best-effort，可能处理非标准输入；安全性更高的扩展应优先保持 tablecodec 前缀编解码，并为异常格式增加独立测试。

## 并发与资源生命周期

本文件不启动线程、异步任务、通道、事务或 I/O。一个 `SchemasReplace` 的生命周期通常覆盖一批日志 entry：映射和 `RewriteTS` 在构造时提供，`deletedTables` 与两个 `FnMut` hook 随顺序调用持续积累状态。

类型没有内部锁。修改入口采用 `&mut self`，Rust 借用规则阻止同一实例的无同步并发调用；若上层跨线程共享，必须自行放入互斥容器。hook 被约束为 `Send`，但 `SchemasReplace` 是否跨线程共享仍取决于上层包装，不能据此宣称它是并发安全的共享对象。

回调所有权由 `Box` 持有，随 `SchemasReplace` drop 释放。返回的 `GetDeletedTables` 引用受 `&self` 生命周期约束。JSON、entry 和 delete-range query 均按值分配，没有文件句柄或外部资源需要显式关闭；主要资源成本是每条 entry 的解析、序列化与 `Vec` 分配。

## 与 Go 版本的对应关系

Rust 保留了 Go 的主要 key/value 规则：DB ID 位于 DB-list Field，表路径同时改 DB Key 与表 Field，Write CF key 使用恢复 TS；rollback 原样保留，delete/put 标记 physical-import source；表 JSON 改 ID/分区/名称、关闭 TTL；PITR 缺 DB/表映射时跳过；deletedTables 支持 delete 后 put 的撤销。

当前 Rust 并非 Go 文件的完整等价移植：

- Go `SchemasReplace` 还持有 `ingestrec.IngestRecorder`、`tiflashrec.TiFlashRecorder`、`setRestoreTableMode` 和完整 delete-range executor wrapper；Rust 用 hook 与简化结构替代，未提供 `GetIngestRecorder`。
- Go `RewriteMetaKvEntry` 对非 meta DB key 还识别 Default CF 的 DDL job history，解码 job 后处理 ingest index 和 delete range；Rust 在 `IsMetaDBKey == false` 时立即返回 `None`，其公开 helper 只能由调用方显式调用。
- Go `processIngestIndexAndDeleteRangeFromJob` 既调用 `ddl.AddDelRangeJobInternal`，又调用 `tryRecordIngestIndex`，并展开 multi-schema subjob；Rust helper 只依据简化模型中的 `NeedGC` / `DelRangeArgs` 生成 query，不记录 ingest index。
- Go 可把表设为 restore mode，并使用 TiFlash recorder；Rust `rewriteTableInfo` 没有这两个状态。
- Go delete-range ID 缺失通过 `(id, exists)` 报给 DDL 层；Rust fallback 为原 ID。Go 还由 DDL 层生成实际 SQL；Rust固定为单行占位模板。
- Rust 独有 `rewriteTableInfoInner` 宽松变体，但当前未接线；现行 `rewriteEntryForTable` 实际调用完整 `rewriteTableInfo`，应以代码调用为准，而不是其附近“轻量”注释。

因此这里的“对应”表示核心迁移语义有测试覆盖，不表示 Go 生产能力已全部接线。新增功能必须同时核对 Go 源文件和 Rust 独立测试，避免把简化 helper 当作完整 DDL 恢复实现。

## 扩展指南

- 新增一种 meta Field：在 `RewriteMetaKvEntry` 增加识别分支，优先复用 `rewriteEntryForTableScopedKey`，并在 `br/pkg/stream/rewrite_meta_rawkv_test.rs::rewrite_meta_entry_routes_all_table_key_kinds` 增加 Default/Write CF 路由断言。
- 改映射或过滤规则：同步检查 `rewriteKeyForDB`、`rewriteKeyForTable`、`rewriteDBInfo`、`rewriteTableInfo` 的 `FilteredOut` / `Reused` / PITR 分支，避免 key 和 value 一侧保留、一侧过滤。还要测试系统库、缺 DB、缺表和缺分区。
- 改 Write CF 行为：围绕 `rewriteValue` 增加 rollback/delete/put、带/不带 short value、physical-import source 和错误 CF 用例；不要把 Rust 测试嵌入生产文件，继续使用同目录独立 `rewrite_meta_rawkv_test.rs`。
- 扩展表 JSON：以 `rewriteTableInfo` 为主，明确表名、TTL、分区和 hook 顺序；若启用 `rewriteTableInfoInner`，先解决其宽松分区策略与完整路径的契约差异并新增专门测试。
- 完整移植 Go DDL job：需要在 Rust 生产调用链补 DDL job history 解码、ingest recorder、multi-schema 展开和 delete-range executor 语义；这不是只修改当前 helper 即可完成的小改动。接线后应在 Rust log-client 批处理测试中证明 `None` 表示跳过 entry、但副作用已被记录。
- 修改构造后的映射：要么禁止直接改变公开 `DbReplaceMap`，要么同步重建 `global_table_id_map`；否则 delete-range 使用过期快照。
- 性能敏感修改：JSON 往返和 key/value `Vec` 分配位于逐 entry 路径。优化必须保持 Default/Write CF 配对、`RewriteTS` 和 hook/deletedTables 的可观察顺序。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/stream/rewrite_meta_rawkv.rs` 找到目标文件和 32 个符号；`node --file ... --offset 1 --limit 500` 与 `--offset 501 --limit 220` 读取了 613 行完整源码；`explore` 验证了本文件主要符号及 Go/Rust 同名关系；对 `RewriteMetaKvEntry` / `processIngestIndexAndDeleteRangeFromJob` 的 method query 未返回可用节点，因此用精确源码搜索补齐调用证据。
- Rust 源与 crate 边界：`br/pkg/stream/rewrite_meta_rawkv.rs`、`br/pkg/stream/lib.rs`、`br/pkg/stream/Cargo.toml`。
- Rust 独立测试：`br/pkg/stream/rewrite_meta_rawkv_test.rs`，覆盖 DB/table key、五类表作用域 Field、rollback/delete physical-import 标记、完整表重写、分区交换、TTL、PITR 缺映射、delete-range 和 hook；`br/pkg/stream/parity_test.rs` 覆盖公开构造契约。
- Go 对照：`br/pkg/stream/rewrite_meta_rawkv.go`、`br/pkg/stream/rewrite_meta_rawkv_test.go`；生产调用证据来自 `br/pkg/task/stream.go` 和 `br/pkg/restore/log_client/client.go`。
- 本任务是只读分析加文档，不运行 Cargo。交付前使用任务指定命令检查文件存在且恰有 11 个固定二级标题，并人工复核本文明确区分当前 Rust 事实、Go 对照能力和未接线部分。
