# `pkg/lightning/backend/kv/base.rs`

## 文件定位

本文件是 [`base.rs`](./base.rs) 的中文逻辑说明，也是 `astersql-lightning-backend-kv` crate 的 SQL 行到 TiKV KV 编码基座。crate 由同目录 `Cargo.toml` 定义，入口 `lib.rs` 通过 `mod base` 加载本模块并以 `pub use base::*` 导出其 API。直接上游是 `sql2kv.rs::tableKVEncoder::Encode`：它按列置换组装一行，再调用这里的列值填充、生成列求值和落 KV 能力。直接下游是本 crate 的 `session.rs`、`canonical.rs`，以及 `astersql-tablecodec` 提供的行键、索引键和值编码器。

本文件不负责批次切分、校验和分类或反向解码；这些分别位于 `sql2kv.rs` 和 `kv2sql.rs`。它持有一次编码会话和表级元数据，将“一行已解析的 `encode::Datum`”转换为会话事务中的记录键、记录值及二级索引键值。

## 核心职责

1. `TableDefinition`、`IndexDefinition` 和 `GeneratedCol` 提供 Lightning 轻量编码模型，并通过 `impl encode::Table for TableDefinition` 接入 `EncodingConfig::Table`。
2. `NewBaseKVEncoder` 校验表对象的具体类型，创建 `Session`，收集生成列，并根据 `UseIdentityAutoRowID`、`shard_row_id_bits`、`auto_random_bits` 和 `AutoRandomSeed` 选择 ID 恒等或分片映射函数。
3. `ProcessColDatum` / `getActualDatum` 决定每列实际值，并对 AUTO_INCREMENT、AUTO_RANDOM allocator 做 rebase。
4. `evalGeneratedColumns` 按目标列下标顺序求值简化生成列表达式。
5. `AddRecord` 选择整数主键、Common Handle 或隐式 RowID，编码记录行与每个索引，再写入当前 `Session` 事务；`Record2KV` 取走 KV 对、附加可比序 RowID，并回收行缓冲。
6. `RowArrayMarshaller`、`datumToValueStringForCastError` 和两个 `Log*Failed` 方法为失败路径提供有长度上限的可读诊断信息。

## 主要符号

- `GeneratedExpression::{Copy, Add, Constant}`：当前 Rust 迁移层支持的生成列表达式 AST；`Add` 只接受两个 `Datum::Int`。
- `GeneratedCol { Index, Expr }`：把表达式绑定到目标列下标。`CollectGeneratedColumnsFromTable` 从 `TableDefinition::generated` 收集并按 `Index` 排序，保证依赖前列的表达式按稳定顺序执行。
- `AutoIDFieldType::{Integer, Float}`：供 `sql2kv.rs::GetAutoRecordID` 区分整数提取与浮点四舍五入；本文件的 `ProcessColDatum` 使用 `Integer`。
- `IndexDefinition`：保存索引 ID、列下标、主键/唯一属性；`AddRecord` 用它选择 record 中的索引值，并由 `canonicalIndexInfo` 补成 tablecodec 元数据。
- `TableDefinition`：表名、表 ID、列、默认值、生成列、索引、handle 模式、分片位数和 allocator 的聚合体。`source_meta` 存在时保留真实持久化列 ID、生成列存储属性和索引元数据；否则走轻量模型合成路径。
- `RowArrayMarshaller::MarshalLogArray`：把 Datum 转成 `(kind, value)`；单值超过 512 KiB 时只保留前 1024 字节/字符并标记，总累计值长度达到阈值时提前加入 `truncated` 项。
- `BaseKVEncoder`：核心有状态对象。公开字段兼顾与 Go API 的名字对应，私有 `recordCache` 用于复用行向量。
- `NewBaseKVEncoder`：唯一常规构造入口；若 `EncodingConfig::Table` 缺失或不是 `TableDefinition`，返回字符串错误。
- `BaseKVEncoder::{GetOrCreateRecord, ProcessColDatum, EvalGeneratedColumns, Record2KV}`：构成 `tableKVEncoder::Encode` 的主要阶段。
- `BaseKVEncoder::AddRecord`：真正写入 KV 的边界；调用 `canonicalHandle`、`encodeCanonicalRowWithMeta`/`encodeCanonicalRow`、`tablecodec::GenIndexKey` 和 `GenIndexValuePortal`。
- `encodeComparableVarint`：为 `Pairs::RowID` 生成与 TiDB 整数可比序编码一致的字节；`base_test.rs::record_to_kv_attaches_go_comparable_row_id` 验证 `42` 编码为单字节 `50`。

## 执行流程

完整调用链为 `NewTableKVEncoder` → `NewBaseKVEncoder` → `tableKVEncoder::Encode` → 本文件各阶段：

1. 构造时，`NewBaseKVEncoder` 从 trait object 下转为 `TableDefinition`，调用 `NewSession`，通过 `CollectGeneratedColumnsFromTable` 固化生成列顺序。若配置要求恒等 ID 或表没有分片位，`AutoIDFn(id) == id`；否则由 seed 与 id 混合产生高位 shard，并保留低位增量部分。
2. `sql2kv.rs::tableKVEncoder::Encode` 调用 `GetOrCreateRecord` 取走缓存。它遍历 `Columns`，依据 `columnPermutation` 找输入 Datum；缺列传 `None`。
3. `ProcessColDatum` 调用 `getActualDatum`：非 NULL 输入优先；自增/自随机列缺值时用 `AutoIDFn(rowID)`；普通缺值查 `TableDefinition::defaults`，未配置则为 `Datum::Null`。随后相应 allocator 以实际值 rebase。
4. 若 `GenCols` 非空，`EvalGeneratedColumns` 按排序后的定义计算并原位写回 record。源列或目标列越界、`Add` 操作数不是整数时，返回 `(目标列下标, 错误)`；上游用 `LogEvalGenExprFailed` 增加列名和原始行上下文。
5. 对非 `pk_is_handle` 表，上游额外以逻辑 `rowID` rebase `RowIDAllocType`，再调用 `Record2KV`。
6. `Record2KV` 先调用 `AddRecord`。后者经 `canonicalHandle` 选择 handle，编码 row value，并用 `SessionCtx.Txn().Set` 写记录 KV；随后逐个索引抽取列值、转换为 canonical Datum、判断新排序规则下是否需要 restored data，生成并写入索引 KV。
7. 成功后，`SessionCtx.TakeKvPairs` 取走事务内累积结果，设置可比序 `Pairs::RowID`；传入 record 被存回 `recordCache` 并清空，供下一行复用。上游当前还会以 `sql2kv.rs::comparableI64` 再设置一次同语义的 RowID。

## 数据与状态

`TableDefinition` 是构造后克隆进 encoder 的表级只读配置，但其 `allocators` 是可克隆句柄，`Rebase` 会改变共享 allocator 的上界。`source_meta: Option<Arc<tablecodec::model::TableInfo>>` 决定编码精度：存在时 `encodeCanonicalRowWithMeta` 使用持久化列 ID，跳过整数 handle 主键和非存储生成列；不存在时列 ID 按 1 起连续合成。

`BaseKVEncoder::SessionCtx` 持有行编码设置和事务缓冲；`AddRecord` 的多次 `Txn().Set` 在同一行内依次加入记录与索引。`recordCache` 的所有权通过 `std::mem::take` 移出，只有 `Record2KV` 成功完成后才回收；若更早阶段或 `AddRecord` 失败，当前调用中的局部 record 不会回到缓存，但不会破坏 encoder 的表级状态。

`AutoIDFn` 是 `Box<dyn Fn(i64) -> i64 + Send>`：闭包只捕获 seed、mask 等不可变值，可以随 encoder 跨线程移动，但 `BaseKVEncoder` 本身需要 `&mut self` 完成记录写入，未声明共享并发访问。`AutoRandomColID` 在 Rust 轻量模型中是 auto-random 列的 1-based 位置；实际判断由 `IsAutoRandomCol(colIndex)` 直接读取列属性完成。

## 依赖与调用关系

RustCodeGraph 对 `NewBaseKVEncoder` 给出的被调用边包括 `NewSession` 与 `CollectGeneratedColumnsFromTable`；对 `Record2KV` 给出的被调用边包括 `AddRecord`、`LogKVConvertFailed` 和 `encodeComparableVarint`；对 `AddRecord` 给出的被调用边包括 `canonicalHandle`、`encodeCanonicalRowWithMeta`、`encodeCanonicalRow`、`canonicalTableInfo`、`canonicalIndexInfo` 与 `toCanonicalDatum`。图索引没有返回这些方法的调用者，因此以上游源码搜索补证：`sql2kv.rs::NewTableKVEncoder` 调用构造器，`tableKVEncoder::Encode` 调用其余编码阶段。

crate 依赖由 `pkg/lightning/backend/kv/Cargo.toml` 限定为：`encode`（Datum、Column、Table/Encoder 配置接口）、`tablecodec`（canonical TiDB 键值与元数据编码）和 `verification`（KV 对/校验和；主要由同 crate 的 `sql2kv.rs` 使用）。本文件内部还依赖同 crate 的 allocator、session、canonical 和 `Pairs`。

记录/索引下游关系是：`AddRecord` → `canonical.rs` 适配 Lightning Datum/表模型 → `tablecodec` 生成 TiDB 可识别的 row/index bytes → `Session::Txn().Set` 暂存 → `Session::TakeKvPairs` 输出。反向解码由 `kv2sql.rs` 使用相同 canonical/tablecodec 规则完成，`sql2kv_test.rs` 的 encode/decode 往返测试覆盖两端契约。

## 错误处理与边界

本文件统一以 `String` 或 `(usize, String)` 返回可恢复错误。构造阶段拒绝缺失或错误具体类型的表；列访问可返回 `column out of range`；AUTO_RANDOM rebase 拒绝非整数值；canonical Datum 解析、handle 构造、行/索引编码和事务写入错误均用 `?` 向上传播。`Record2KV` 在 `AddRecord` 失败时用 `LogKVConvertFailed(originalRow, -1, "record", error)` 包装上下文。

生成列严格受当前简化 AST 限制：`Copy` 源列必须存在，`Add` 两侧必须是 `Datum::Int`，目标下标必须存在；失败不会部分回滚此前已经写回的生成列。因此调用方应把失败的整行视为不可用，不应复用其 record 内容。

日志边界由 `maxLogLength = 512 * 1024` 控制。这里的累计量是 Rust `String::len()` 的字节数；对超长字符串直接调用 `truncate(1024)` 要求 1024 是 UTF-8 字符边界，若一个多字节字符跨越该位置会 panic，这是当前实现应注意的边界。与 Go 版不同，Rust marshaller 不执行日志脱敏，两个 `Log*Failed` 返回格式化字符串而非直接写 logger。

`ProcessColDatum` 的 `_needCast` 和 `getActualDatum` 的 `_needCast` 当前未使用；显式 `Datum::Null` 被视为缺值。`TruncateWarns` 当前为空实现。因此不能从 Go 同名方法推断 Rust 已具有列类型转换、NOT NULL 错误上下文或 session warning 清理能力。

## 并发与资源生命周期

每个 `BaseKVEncoder` 在 `NewBaseKVEncoder` 中拥有一个 `Session`，正常生命周期由包装它的 `tableKVEncoder` 管理；`tableKVEncoder::Close` 调用 `SessionCtx.Close` 并禁止后续 `Encode`。本文件本身没有线程、异步任务、锁或通道。

编码一行时需要 `&mut BaseKVEncoder`，使 `recordCache` 与 `Session` 事务缓冲只能被该调用独占。`AutoIDFn` 的 `Send` 约束仅允许随所有者移动，不等价于 `Sync`，也不表示一个 encoder 可并发编码多行。allocator 的 clone/rebase 语义来自 `allocator.rs`；扩展调用方若并行导入，应为工作单元建立独立 encoder/session，并遵守 allocator 实现的共享规则。

成功路径中，record 向量在 `GetOrCreateRecord` 与 `Record2KV` 之间由一次编码独占，最后清空并缓存；KV 对由 `TakeKvPairs` 从 session 中取走。错误路径可能放弃缓存复用，但 Rust 所有权会回收局部内存，不产生悬垂引用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/backend/kv/base.go`，测试对照是 `base_test.go`；结构上的主要对应为 `BaseKVEncoder`、`GeneratedCol`、`RowArrayMarshaller`、`NewBaseKVEncoder`、`Record2KV`、`ProcessColDatum`、`getActualDatum`、生成列求值和错误日志方法。Rust 的 `base_test.rs` 保留了日志内容、Datum 调试字符串、可比序 RowID 和 sentinel 类型名的针对性验证。

当前 Rust 并非 Go 实现的逐项等价替换：

- Go 使用真实 `table.Table`、`model.TableInfo`、表达式引擎和 `table.AddRecord`；Rust 用 `TableDefinition` 加 `canonical.rs`/`tablecodec` 重建必要语义，并在 `source_meta` 存在时提高与持久化元数据的一致性。
- Go 的生成列是 `expression.Expression`，支持完整 SQL 求值和列类型 cast；Rust 仅有 Copy/Add/Constant，且 Add 只支持有符号整数。
- Go `getActualDatum` 执行 `CastColumnValue`、NOT NULL 处理、生成列占位和默认值表达式；Rust 不 cast、不执行 NOT NULL 约束，默认值只是预先存入 map 的 Datum。
- Go 的 AUTO_RANDOM 使用 `NewShardIDFormat` 并只以 incremental mask rebase，ShardRowID 使用按行重新 seed 的随机算法；Rust 使用统一乘法/XOR 混合逻辑，且 rebase 直接使用最终整数。这些算法差异属于兼容风险，修改前必须以 Go 测试和 tablecodec 行为重新核对。
- Go 版写结构化、脱敏 zap 日志并返回规范化 cast error；Rust 返回普通字符串。Go 的 `TruncateWarns` 清 session warnings，Rust 当前 no-op。
- Go `Record2KV` 给每个 `KvPair` 设置 RowID；Rust 的 `Pairs` 在行级保存一个 `RowID` 字段，这是 Rust `encode::Row` 接口的模型差异。

## 扩展指南

新增 Datum/列类型时，应先扩展 `encode::Datum`/`Column`，再同步本文件的 `datumKindAndValue` 与 `datumToValueStringForCastError`，以及 `canonical.rs::{toCanonicalDatum, fromCanonicalDatum, fieldType}`；测试应放在独立的 `base_test.rs`、`canonical_test.rs` 或 `sql2kv_test.rs`，不要嵌入生产文件。

扩展生成列表达式时，修改 `GeneratedExpression` 和 `evalGeneratedColumns`，保持 `CollectGeneratedColumnsFromTable` 的确定性排序；增加源列越界、类型不匹配、目标越界和依赖顺序测试。若目标是 Go 等价能力，不应继续堆叠小型 AST，而应评估接入现有表达式/类型转换基础设施。

调整 handle、行值或索引编码时，主要接点是 `AddRecord` 和 `canonical.rs`。必须同时覆盖整数主键、隐式 RowID、Common Handle、普通/唯一索引、新排序规则 restored data、持久化列 ID以及 row format v1/v2，并验证 `kv2sql.rs` 可反解。此处的兼容风险最高：错误字节可能成功生成但不能被 TiDB 正确读取。

调整自增/自随机逻辑时，修改 `NewBaseKVEncoder`、`ProcessColDatum` 和 allocator rebase 规则；应与 Go 的 shard 位布局、符号/无符号列、range bits、溢出及 incremental mask 行为对齐。性能上要保留 record 缓存，并注意每个索引当前会 clone 表元数据、索引元数据和 canonical values，新增索引能力时应评估分配量。

修改诊断输出时必须保留长度上限并补充 UTF-8 边界、脱敏和不可打印 bytes 测试。若实现 warning 清理，应让 `TruncateWarns` 委托 `Session` 的真实 warning 状态，而不是只改变方法表象。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 `pkg/lightning/backend/kv/base.rs`；`files --filter pkg/lightning/backend/kv` 显示该模块 Rust/Go 源及独立测试均在索引内。
- RustCodeGraph 调用证据：`callees NewBaseKVEncoder --file .../base.rs` 指向 `NewSession`、`CollectGeneratedColumnsFromTable`；`callees Record2KV` 指向 `AddRecord`、`LogKVConvertFailed`、`encodeComparableVarint`；`callees AddRecord` 指向 `canonical.rs` 中的 handle、row、table/index 元数据与 Datum 适配函数。调用者查询未返回结果，故用 `rg` 和 `sql2kv.rs` 源码核实直接上游。
- 已读生产代码：`base.rs` 全文、`sql2kv.rs` 的构造和 `Encoder::Encode` 主链、`canonical.rs` 的 Datum/行/索引/handle 适配、`lib.rs` 模块导出、`Cargo.toml` crate 依赖。
- 已读对照代码：`base.go` 全文及 `base_test.go`；它们用于确认同名职责和明确 Rust 当前未移植或语义不同之处。
- 已读 Rust 测试：`base_test.rs`；`sql2kv_test.rs` 中的日志编组、基本记录/索引编码、编码解码往返、唯一索引、row format v2、时间、缺失自增值、生成列、AUTO_RANDOM 和 ShardRowID 测试。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前另运行任务规定的 11 章节结构命令，并人工复核本文只陈述上述源码、图索引、Cargo、Go 对照和测试能够支持的事实。
