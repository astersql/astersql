# `pkg/server/handler/tikv_handler.rs`

## 文件定位

本文件属于 Cargo crate `astersql-server-handler`，由同目录的 `lib.rs` 以 `pub mod tikv_handler` 导出。它以 Rust 形式保留 Go 文件 `pkg/server/handler/tikv_handler.go` 中 `TikvHandlerTool` 的辅助能力：把 HTTP 路径或查询参数转换为行句柄、索引键、表/分区和 Region/MVCC 查询结果。

当前实现应被视为机械迁移基线，而不是已经接通 TiKV 的生产实现。文件前半段保留了主要控制流，后半段则在同一文件内定义 `Storage`、`RegionCache`、`PdClient`、`Table`、`InfoSchema` 等占位类型；多个关键方法固定返回“external ... dependency”一类错误。仓库中的实际 Rust HTTP 状态路由使用 `pkg/server/handler/tikvhandler/tikv_handler.rs` 中另一套同名 `TikvHandlerTool`，该类型通过 `TikvRuntime` 注入运行时能力；两套类型不是同一个 Rust 类型，不能互换。

`pkg/server/handler/Cargo.toml` 声明了 `astersql-kv`、`astersql-store-helper`、`astersql-table`、`astersql-tablecodec`、`astersql-types` 等真实子 crate 依赖，但本文件目前没有导入或调用这些 crate 的类型。仓库内对本文件 API 的直接 Rust 引用仅见同 crate 的独立测试 `tikv_handler_test.rs` 和 `handler_aster_unit_test.rs`，未发现生产调用者。

## 核心职责

文件目前承载四组职责：

1. 提供 `TikvHandlerTool` 及其 Go 风格方法别名，保留从存储句柄到 Region、MVCC、表元数据查询的 API 形状。
2. 解析 HTTP 输入：`get_handle` 处理整数 handle 或 common handle，`form_value_to_datum_row` 把多值查询参数按列转换成 `Datum`，`handle_mvcc_get_by_hex` 解码十六进制 key。
3. 编排行为流程：`get_mvcc_by_idx_value` 依次查询普通索引键和临时索引键，`get_table`/`get_partition` 解析物理表，`get_regions_meta` 聚合 PD 返回的 Region 信息。
4. 提供少量已经可独立工作的兼容算法：`parse_go_base_zero_int` 模拟 Go `strconv.ParseInt(..., 0, 64)` 的进制前缀规则，`index_key_to_temp_index_key` 改写编码索引 ID，`hex_upper`/`hex_decode` 完成十六进制转换。

这些职责中只有纯解析和字节改写逻辑具备可观察的本地行为；涉及 schema、表、索引编码、Region cache、PD 或 MVCC 的路径仍会在占位边界失败。

## 主要符号

- `TikvHandlerTool { helper: Helper }`：公开工具类型。`new_tikv_handler_tool` 是 Rust 风格构造函数，`NewTikvHandlerTool` 是保持 Go 命名的公开别名。`impl TikvHandlerTool` 中的 `GetRegionIDByKey`、`GetHandle`、`GetMvccByIdxValue`、`FormValue2DatumRow`、`GetTableID`、`GetTable`、`GetPartition`、`Schema`、`HandleMvccGetByHex` 和 `GetRegionsMeta` 都只委托到对应模块级函数；`formValue2DatumRow` 是另一个 Go 风格别名。
- `Helper { store, region_cache }`：持有 `Storage` 和私有 `RegionCache`。`Helper::new` 构造空占位 cache；`Helper::trace` 当前原样返回错误，不增加堆栈或上下文。
- `MvccKv` / `mvccKV`：MVCC 查询结果，包含大写十六进制 `key`、`region_id` 和可选 `MvccResponse`。后者只是 Go 风格类型别名。该结构没有 serde 派生，不能仅凭本文件推断其 JSON 序列化行为。
- `RegionMeta`：聚合 Region ID、leader、peers 和 epoch。`Peer`、`RegionEpoch` 目前均为空占位类型。
- `get_region_id_by_key`：通过 `Helper.region_cache.locate_key` 取得 Region ID；当前 `locate_key` 总是返回 `external region cache dependency`。
- `get_handle`：优先读取 `params[HANDLE]` 并按 Go base-0 规则解析整数；无显式 handle 时尝试从主键索引列构造 common handle。
- `get_mvcc_by_idx_value`：从查询值生成索引键，查询普通与临时索引键的 MVCC，并为两者补充 Region ID。
- `form_value_to_datum_row` / `form_value_2_datum_row`：要求每个列名存在；零个值表示 `Null`，一个值执行类型转换，多于一个值返回 Bad Request。包装版本仅经过 `Helper::trace`。
- `get_table_id`、`get_table`、`get_partition`、`schema`：从 Domain/InfoSchema 解析数据库、表和分区，再返回物理 ID 或物理表。
- `handle_mvcc_get_by_hex`：读取 `HEX_KEY`，解码后查询 MVCC 和 Region，并保留输入 key 的大写形式。
- `get_regions_meta`：按输入顺序逐个查询 PD，任一项缺少 meta 就整体返回错误，不返回部分结果。
- `index_key_to_temp_index_key`：若 key 至少有 19 字节，将偏移 `11..19` 的 memcomparable 有符号 index ID 解码、与 `0x7fff_0000_0000_0000` 组合后原位写回副本；短 key 原样返回。
- `Error { message, bad_request }`：本地错误载体。`Error::bad_request` 只用于明确的客户端输入/表形状问题；`message()` 与 `is_bad_request()` 提供测试可见状态。
- 本文件没有 trait、模块级可变状态、条件编译项或业务常量；仅 `index_key_to_temp_index_key` 内有两个局部编码常量。

## 执行流程

### 按 handle 解析记录

`get_handle` 首先检查路径参数 `HANDLE`。若存在且表被标记为 common-handle 表，则拒绝整数路径形式并要求通过 query string 提供主键列；否则调用 `parse_go_base_zero_int`，支持十进制、前导零八进制以及 `0x`、`0b`、`0o` 前缀和正负号，再生成 `Handle::Int`。

若没有显式 handle，函数读取表元信息，寻找主键索引并确认 `is_common_handle`。随后按索引列的 offset 收集列定义，以 UTC `StatementContext` 把 query values 转为 `Datum`，截断索引值，编码 key，并构造 `Handle::Common`。当前占位实现中 `PhysicalTable::meta` 总是返回非 common-handle 的空表，`find_primary_index` 总是 `None`，因此这一分支实际止于 `Clustered common handle not found.`。

### 按索引值查询 MVCC

`get_mvcc_by_idx_value` 创建 UTC 上下文，经 `form_value_to_datum_row` 生成索引行，再由 `Index::gen_index_key` 编码普通索引键。成功时应先调用 `get_mvcc_by_encoded_key` 和 `get_region_id_by_key` 形成第一条 `MvccKv`，再通过 `index_key_to_temp_index_key` 构造临时索引键并重复查询，最终按“普通、临时”的顺序返回两项。

当前 `UrlValues::get` 永远返回 `None`，`Index::gen_index_key` 固定报 `index key dependency`，`get_mvcc_by_encoded_key` 固定报 `external TiKV dependency`；因此现状不能完成该流程。临时键字节改写本身由独立测试覆盖。

### 按表名和分区解析物理表

`get_table` 先调用 `schema`，再用 `extract_table_and_partition_name` 把首个 `name(partition)` 形式拆成表名和分区名，然后调用 `InfoSchema::table_by_name` 和 `get_partition`。分区表必须显式指定分区；非分区表若携带分区名则报错。`get_table_id` 只在成功解析物理表后读取其 ID。

当前 `get_domain` 固定返回 `domain dependency`，所以 `schema` 是最早失败点；后续 schema、分区和物理表方法同样还是占位实现。

### 按十六进制 key 或 Region ID 查询

`handle_mvcc_get_by_hex` 从路径映射取 `HEX_KEY`（缺失时按空字符串处理），先解码，再查 MVCC 和 Region ID，最后构造 `MvccKv`。奇数长度或非十六进制字符在 `hex_decode` 中失败；合法 key 会继续执行并在当前 TiKV 占位边界失败。空字符串可解码为空 key，因此不会被输入校验拒绝，但随后仍会命中外部依赖错误。

`get_regions_meta` 按 `region_ids` 顺序同步调用 `PdClient::get_region_by_id`；成功响应必须含 `meta`，随后复制 leader、peers 和 epoch。当前 PD 方法总是返回 `external PD dependency`，空输入则直接成功返回空向量。

## 数据与状态

`TikvHandlerTool` 拥有一个 `Helper`，后者拥有 `Storage` 和 `RegionCache`；`RegionCache` 再拥有 `PdClient`。这些对象都可 `Clone`，但当前均不携带真实连接、缓存内容、运行时句柄或同步原语。构造函数每次创建新的空占位 cache。

输入数据主要是借用：编码 key 使用 `&[u8]`，Region ID 使用 `&[u64]`，表和索引通过引用传入。输出则拥有自己的数据：handle 持有 `Vec<u8>`，`MvccKv.key` 持有 `String`，Region 结果克隆/移动 peers 和 epoch。`index_key_to_temp_index_key` 总是复制输入后修改，不改变调用方传入的切片；这与 Go 版本就地修改 `encodedKey` 的实现方式不同，但返回给调用方的普通键与临时键仍可保持分离。

`StatementContext` 只保留 API 外形，不真正存储时区或错误上下文：`time_zone()` 固定返回 `UTC`，`set_time_zone` 无副作用，`handle_error` 原样透传结果。`Datum::convert_to` 也不做真实列类型转换。因而文档不能把这些占位方法描述为完成了 TiDB 类型或时区语义。

文件没有全局可变状态、缓存淘汰、事务对象或生命周期管理。所有聚合向量都在栈上顺序构造；遇到第一个错误立即丢弃尚未返回的局部结果。

## 依赖与调用关系

模块装配关系是 `pkg/server/handler/lib.rs -> pub mod tikv_handler -> 本文件`。RustCodeGraph 能识别本文件的 `TikvHandlerTool`、`get_handle`、`get_mvcc_by_idx_value`、`handle_mvcc_get_by_hex` 和 `get_regions_meta` 等符号；精确 `callers/callees` 查询在本地大型索引上超过 60 秒未返回，因此调用边又用直接引用搜索核验。

已确认的直接上游只有：

- `pkg/server/handler/tikv_handler_test.rs`：调用 `NewTikvHandlerTool`、`get_handle`、`handle_mvcc_get_by_hex`。
- `pkg/server/handler/handler_aster_unit_test.rs`：调用 `NewTikvHandlerTool`、`get_handle`、`index_key_to_temp_index_key`。
- `impl TikvHandlerTool` 的 Go 风格方法：在本文件内部委托给同名模块级函数。

已确认的内部下游链包括：

- `get_handle -> parse_go_base_zero_int`，或 `find_primary_index -> form_value_to_datum_row -> truncate_index_values -> encode_key -> Handle::common`。
- `get_mvcc_by_idx_value -> form_value_to_datum_row -> Index::gen_index_key -> get_mvcc_by_encoded_key/get_region_id_by_key -> index_key_to_temp_index_key`。
- `get_table -> schema/get_domain -> InfoSchema::table_by_name -> get_partition/find_partition_by_name`。
- `handle_mvcc_get_by_hex -> hex_decode -> get_mvcc_by_encoded_key/get_region_id_by_key`。
- `get_regions_meta -> RegionCache.pd_client.get_region_by_id`。

`pkg/server/handler/tikvhandler/tikv_handler.rs` 是相邻但独立的 handler crate 实现。`pkg/server/http_status.rs` 构造并使用的是那一文件的类型；它通过 `TikvRuntime` 调用 schema、table、handle 和 MVCC 能力，而不是调用本文件的公开函数。因此，本文件目前不在已检索到的 Rust HTTP 请求主链上。

## 错误处理与边界

`Error` 只区分普通错误与 `bad_request`。下列输入契约会显式标为 Bad Request：common-handle 表使用整数路径 handle、缺少 common handle 主键、缺少索引列 query 值、同一列给出多个值。整数解析失败、十六进制解码失败、外部依赖失败、表/分区失败则是普通错误。`tikv_handler_test.rs` 专门断言无效整数和无效 hex 不会被重新分类为 Bad Request。

`parse_go_base_zero_int` 对空输入、只有符号、前缀后无数字、非法数字和 i64 溢出统一返回 `invalid integer handle`；它专门允许 `-9223372036854775808`，拒绝更大的绝对值。`hex_decode` 要求偶数长度且每对字符均为 ASCII 十六进制；错误统一为 `invalid hex key`。

主要占位边界及固定错误如下：`RegionCache::locate_key`、`PdClient::get_region_by_id`、`TikvHandlerTool::get_mvcc_by_encoded_key`、`Index::gen_index_key`、`InfoSchema::table_by_name`、`Table::as_physical`、`PartitionedTable::get_partition`、`find_partition_by_name` 和 `get_domain`。这些不是可恢复的真实适配器，任何依赖它们的成功路径都尚未得到当前文件支持。

`extract_table_and_partition_name` 只取第一个 `(` 和第一个 `)`，没有验证括号顺序、尾随文本或嵌套语法；它是兼容性辅助而非完整 SQL 标识符解析器。`index_key_to_temp_index_key` 对短于 19 字节的输入静默原样返回，也不验证 `t..._i` 前缀；调用者必须保证传入标准编码索引键。

## 并发与资源生命周期

本文件全部 API 都是同步函数，没有 `async`、线程、任务、通道、锁或原子变量。`get_regions_meta` 和普通/临时索引 MVCC 查询均严格串行执行；如果未来接入真实网络客户端，这种串行行为会直接影响多 Region 请求的延迟。

Go 对照实现为 Region cache 定位创建带 500 ms 预算的 backoffer，并向 PD 传入 context；Rust 占位签名没有 context、超时、取消或重试参数。当前也不存在连接池或 client 关闭动作，因为 `Storage`、`PdClient` 与 `RegionCache` 都是零状态类型。接入真实依赖时必须明确共享所有权、请求取消、重试预算与 cache 生命周期，不能只替换固定错误而保留空类型。

函数以 `Result` 进行早退：普通索引查询成功而临时索引查询失败时不会返回第一项；Region 列表中后项失败时也不会返回前面的元数据。这一“全有或全无”的返回形状应在并发化时保持，除非同时修改上层 API 和测试。

## 与 Go 版本的对应关系

Rust 的公开函数和 Go 风格方法基本逐项对应 `pkg/server/handler/tikv_handler.go`：构造工具、定位 Region、解析 handle、按索引查 MVCC、query-to-Datum、表/分区解析、读取 schema、按 hex 查 MVCC和批量读取 Region meta。错误文案和 UTC 选择也保留了迁移痕迹。

关键差异如下：

- Go 嵌入真实 `helper.Helper`，调用 TiKV client、PD client、Domain、InfoSchema、tablecodec、codec 和真实类型转换；Rust 在本文件内重定义同名占位类型，真实 Cargo 依赖尚未接线。
- Go 的 `GetRegionIDByKey` 包含 backoffer 并把 TiKV driver 错误转为 TiDB 错误；Rust 只调用固定失败的 `locate_key`，也没有错误映射层。
- Go 的 `GetMvccByIdxValue` 对同一 `encodedKey` 就地执行 `IndexKey2TempIndexKey`；Rust 返回一个修改后的副本。这避免覆盖普通键结果，但是否完全覆盖 tablecodec 对异常键的语义仍未验证。
- Go 的 Datum 转换使用列 `FieldType` 和 statement type context；Rust `ColumnInfo` 只有名字，`Datum::convert_to` 原样返回，因此没有类型校验、截断或警告处理。
- Go 的分区错误包含更具体的表名/用法，且分区表缺少分区时返回一个物理表值和错误；Rust 只返回错误，文案也更短。调用者不应依赖两边完全一致的错误字符串。
- Go 的 `RegionMeta`/`mvccKV` 带 JSON tag，Rust 结构没有 serde 属性；Go 的 failpoint `errGetRegionByIDEmpty` 可把 `region.Meta` 置空，Rust 没有对应 failpoint，但保留了 `meta == None` 的错误分支。
- Go 返回指针和接口对象，Rust 返回拥有值；Rust 多数占位类型是零大小类型，不能据此推断真实资源成本。

Go 回归 `pkg/server/handler/tests/http_handler_serial_test.go::TestRegionsFromMeta` 验证 `/regions/meta` 返回非零 ID，并启用 `errGetRegionByIDEmpty` 确认空 meta 不触发 panic。该测试是 Go 行为证据，不代表本 Rust 文件的 PD 路径已可运行。

## 扩展指南

若要把此迁移基线接入真实功能，最小安全顺序是：

1. 先决定本文件是否继续存在，还是让调用方统一使用 `pkg/server/handler/tikvhandler/tikv_handler.rs` 的 `TikvRuntime` 实现；两套同名工具继续并存会扩大类型和行为漂移。
2. 若保留本文件，优先用 `Cargo.toml` 已声明的真实 crate 类型替换 `Storage`、`RegionCache`、`PdClient`、表/schema/Datum/handle/错误占位类型，并删除对应固定错误；不要另造第三套适配模型。
3. 接线 `get_region_id_by_key`、`get_mvcc_by_encoded_key`、`schema` 与 `get_regions_meta` 时同步补上 Go 中的错误转换、context/超时/重试和空 meta 行为。
4. 接线 common handle 与索引查询时同步替换 `find_primary_index`、`truncate_index_values`、`encode_key`、`Index::gen_index_key` 和真实列类型转换，并确认临时索引 ID 编码与 `tablecodec` 一致。
5. 保持测试逻辑在独立文件中。直接扩展 `pkg/server/handler/tikv_handler_test.rs` 覆盖输入分类与失败边界；编码兼容回归可扩展 `handler_aster_unit_test.rs`。涉及 HTTP 路由的行为应在实际使用 `tikvhandler` crate 的测试面增加，而不是把测试内嵌进本源文件。

兼容风险主要是 Go 风格公开方法名、错误分类/文案、base-0 数字规则、索引 key 字节布局、common handle 列顺序和分区名解析。性能风险主要来自逐 Region 串行 PD RPC、普通与临时索引串行双查以及无界复制 key/peer 列表。正确性验证必须覆盖部分成功后的早退、i64 边界、短/畸形索引键、缺失/重复 query 值、类型转换失败、分区表与非分区表的交叉输入，以及 PD 返回空 meta。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/server/handler` 确认本文件、同路径 Go 文件、两个 Rust 测试及相邻 `tikvhandler` 实现均已索引。
- RustCodeGraph `node --file pkg/server/handler/tikv_handler.rs`：分三段读取并核对全部 678 行，确认公开 API、内部委托、所有占位类型与固定失败边界。
- RustCodeGraph `query`：核对 `TikvHandlerTool`、`get_handle`、`get_mvcc_by_idx_value`、`get_regions_meta`、`handle_mvcc_get_by_hex`，并识别同名 Go 符号及相邻 crate 的同名 Rust 类型。`callers/callees` 精确查询等待超过 60 秒仍未返回，已中止；随后以直接引用搜索补充调用证据。
- crate 与模块证据：`pkg/server/handler/Cargo.toml`、`pkg/server/handler/lib.rs`。
- Go 对照：`pkg/server/handler/tikv_handler.go`；相关回归：`pkg/server/handler/tests/http_handler_serial_test.go::TestRegionsFromMeta`。
- Rust 独立测试：`pkg/server/handler/tikv_handler_test.rs`、`pkg/server/handler/handler_aster_unit_test.rs`。
- 实际 Rust HTTP 接线对照：`pkg/server/http_status.rs`、`pkg/server/handler/tikvhandler/tikv_handler.rs`，用于确认生产路由使用另一套 `TikvHandlerTool`/`TikvRuntime` 边界。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证目标文档存在且恰好包含 11 个固定二级章节，并人工复核未把占位路径描述为已支持。
