# `pkg/util/logutil/consistency/reporter.rs`

源文件：[`reporter.rs`](./reporter.rs)  
Go 对照：[`reporter.go`](./reporter.go)

## 文件定位

本文件是 `astersql-util-logutil-consistency` crate 的主体实现。crate 入口 [`lib.rs`](./lib.rs) 将私有 `reporter` 模块的公开项全部再导出；[`Cargo.toml`](./Cargo.toml) 声明它依赖 `model`（表、列和索引元数据）、`tablecodec`（键、行、Datum 与 handle 编解码）、`serde`/`serde_json`（诊断 JSON）、`hex` 和 `log`。根 workspace 以 `facade_util_logutil_consistency` 引入该 crate，`pkg/executor/Cargo.toml` 也直接依赖它。

它位于 SQL 执行发现“索引视图与表记录不一致”之后：调用方提供表/索引元数据、键编码器、可选存储适配器和日志下沉；本文件补采 MVCC（多版本并发控制）诊断信息、执行脱敏、记录错误日志，并返回具有 TiDB 兼容错误码的 `ConsistencyError`。当前已接线的 Rust 生产入口是 `pkg/executor/typed_point_get.rs`：唯一索引已经取到 handle、但对应记录不存在且不是弱一致性读取时，构造 `Reporter` 并调用 `ReportLookupInconsistent`。Go 版本还由 `pkg/executor/distsql.go`、`pkg/executor/check_table_index.go`、`pkg/util/admin/admin.go` 等路径调用三个报告入口；不能据此宣称这些 Go 调用点均已在 Rust 接线。

## 核心职责

1. `GetMVCCByKeyResp` 和 `GetMvccByKey` 把存储查询转换为稳定的诊断 JSON，包括大写十六进制键、Region ID、原始 MVCC 响应和可选解码结果；任何诊断查询或序列化失败都降级为空串，不覆盖原始一致性错误。
2. `DecodeRowMvccData` 与 `DecodeIndexMvccData` 分别将 write CF 的 `ShortValue` 和 default CF 的 `Value` 解成按 `start_ts` 分组的行列值或索引 handle，同时把最后观察到的解码错误写入 `decode_error`。
3. `Reporter::{ReportLookupInconsistent, ReportAdminCheckInconsistent, ReportAdminCheckInconsistentWithColInfo}` 构造结构化字段、按配置控制敏感信息与完整 MVCC、附加调用栈、写错误日志，并返回错误码分别为 8133、8223、8134 的错误对象。
4. `MvccWrite`、`MvccValue`、`MvccInfo` 和 `MvccGetByKeyResponse` 保留报告所需的存储响应形状；`goBytes` 与 `toGoJSON` 专门对齐 Go `encoding/json` 的字节 Base64、`omitempty` 和 HTML 安全转义语义。

本文件只负责诊断和错误构造，不判断数据是否一致，也不重试、修复或提交存储数据。

## 主要符号

- `Storage: Send + Sync`：最小存储边界。`get_mvcc_by_encoded_key(&kv::Key)` 返回完整 `MvccGetByKeyResponse`；`region_id_by_key(&kv::Key)` 返回 Region ID。使用字符串错误使报告器不绑定具体存储客户端。
- `MvccWrite`、`MvccValue`、`MvccInfo`、`MvccGetByKeyResponse`：可序列化/反序列化的诊断 DTO。字段名通过 serde 重命名成 Go protobuf JSON 使用的 snake_case；零值和空值按 Go 风格省略。
- `MvccOutMap = BTreeMap<String, Value>`：最终 JSON 的有序顶层映射；`DecodedMvccData` 也是有序映射，使诊断输出在相同输入下顺序稳定。
- `GetMVCCByKeyResp(store, key) -> Result<...>`：对 `Storage` 的直接转发层；`GetMvccByKey(store, Option<Key>, Option<&DecodeMvccFn>) -> String` 是容错的 JSON 组装层。
- `DecodeRowMvccData(&TableInfo)`、`DecodeIndexMvccData(&IndexInfo)`：捕获元数据并返回解码闭包，供 `GetMvccByKey` 注入。前者依赖 `DecodeRowToDatumMap`，后者依赖 `DecodeIndexHandle`。
- `RecordData { Handle, Values }`：报告时使用的行快照；`String`/`Display` 生成 `handle: ..., values: [...]`，`Clone` 通过 `Handle::Copy` 深复制动态 handle。
- `Reporter`：持有两个 `Arc` 编码闭包、表/索引元数据、脱敏模式、可选 `Arc<dyn Storage>` 与 `Arc<dyn LogSink>`。`Reporter::new` 只装配依赖，不执行 I/O。
- `LogSink`、`LogEntry`、`LogField`：可注入的日志边界；`StandardLogSink` 用 `log::error!` 写出消息和字段，测试用实现可收集精确字段。
- `ConsistencyErrorKind` 与 `ConsistencyError`：返回给执行层的稳定错误分类、数字码、格式化参数和生成时的脱敏模式；`Display` 对齐三类 Go errno 模板，且实现 `std::error::Error`。
- `ErrAdminCheckInconsistent`、`ErrLookupInconsistent`、`ErrAdminCheckInconsistentWithColInfo`：保留 Go 命名的错误种类别名。
- 内部辅助：`decodeIndexHandle` 先验证 19 字节索引前缀；`decodeMvccRecordValue` 解码行；`insertDecodedData`、`addMVCCFields`、`formatHandles`、`redactString`、`redactErrorArg`、`addStack` 分别负责结果注入、去载荷元数据、格式化、脱敏和堆栈字段。

## 执行流程

`GetMvccByKey` 的流程如下：

1. `key` 为 `None` 时立即返回空串；否则通过 `GetMVCCByKeyResp` 查询，查询失败也返回空串。
2. 建立输出映射：`key` 是编码键的 uppercase hex；`regionID` 来自 `getRegionIDByKey`，Region 定位失败回退为 `0`；`mvcc` 是完整响应的 JSON 值。
3. 若传入解码闭包，则由闭包向同一映射加入 `decoded` 和可选 `decode_error`。
4. `toGoJSON` 序列化并把 `<`、`>`、`&`、U+2028、U+2029 转成 Go 默认转义形式。序列化失败返回空串。
5. 输出超过 5000 字节时退到合法 UTF-8 字符边界，保留前缀并追加 `[truncated]...`。因此截断后的文本不保证仍是完整 JSON，它是受限长度的日志载荷。

行解码闭包先按列 ID 构造 `FieldType` 映射，再依次扫描 `Info.Writes` 的非空 `ShortValue` 和 `Info.Values` 的非空 `Value`。`decodeMvccRecordValue` 使用 UTC 解码，每个已出现列以原始列名为键；NULL 显示为 `nil`，其他 Datum 用 `ToString`。索引解码闭包以相同顺序扫描版本，并用索引列数从键和值解出 handle。相同 `start_ts` 后写入的 value 会覆盖先前 write 的映射项。

三个报告入口共享“组字段 → 可选补采 MVCC → `addStack` → `self.log` → 构造 `ConsistencyError`”主干：

- `ReportLookupInconsistent` 记录计数、缺失 handle 和最多前 50 个完整 handle；仅当脱敏模式不是 `ON` 且存在存储时，才为缺失记录与缺失索引逐项增加完整 MVCC JSON。
- `ReportAdminCheckInconsistentWithColInfo` 记录列名、handle、索引值、表值和比较错误；完整 MVCC 同样只在非 `ON` 模式采集。
- `ReportAdminCheckInconsistent` 额外为整数 handle 写 `int_handle`。只要存在存储，就尝试查询行与可选索引的 MVCC，并通过 `addMVCCFields` 写出已清空 `ShortValue`/`Value` 的版本元数据；完整 MVCC JSON 仍受 `ON` 限制。这一区分保证开启脱敏后仍可观察时间戳等非载荷元数据。

## 数据与状态

`Reporter` 自身没有可变业务状态；一次调用所需字段都在栈上创建。表和索引元数据按值归 `Reporter` 所有，编码器、存储和日志器使用 `Arc` 共享。`Storage` 与 `LogSink` 要求 `Send + Sync`，因此报告器可安全被并发执行上下文共享；但具体线程安全性仍依赖实现者满足 trait 契约。

MVCC 数据分为三层：`MvccGetByKeyResponse` 包含 Region/键错误与可选 `Info`；`MvccInfo` 包含 opaque `Lock`、`Writes` 和 `Values`；write/value 的字节载荷由 `goBytes` 以标准带填充 Base64 编码。`addMVCCFields` 总是在克隆对象上清空载荷，不修改原响应。

脱敏模式是字符串约定：`OFF` 原样输出，`ON` 清空日志中的敏感字符串并让错误模板以 `?` 替代指定参数，`MARKER` 用 `‹...›` 包络且双写已有标记字符。未知模式在 debug 构建触发断言，随后返回空串；调用方应只传受支持值。

## 依赖与调用关系

上游关系：

- `pkg/executor/typed_point_get.rs` 是已验证的 Rust 生产调用者。它在唯一索引命中、记录键缺失且具备诊断元数据时构造 `Reporter`，调用 `ReportLookupInconsistent(1, 0, ...)`，再把其显示文本转换为执行错误。
- `pkg/executor/typed_point_get_test.rs::indexed_point_get_mismatch_uses_structured_consistency_reporter` 覆盖该接线，并注入自定义 `LogSink` 观察日志。
- [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 直接覆盖本文件全部主要公共路径。RustCodeGraph 还识别到三个报告入口对 `GetMvccByKey`、行/索引解码器和日志辅助的内部调用边。

下游关系：

- `model::{TableInfo, IndexInfo}` 提供名字、列类型和索引列数。
- `tablecodec::{kv, types}` 提供键、handle、Datum、行与索引解码；恶意或损坏索引键在本文件先做长度保护，避免下游切片越界。
- `serde_json` 构造 MVCC 和 decoded JSON；`hex` 生成诊断键；`log` 仅由默认日志下沉使用。
- 存储 I/O 完全经 `Storage`，日志 I/O 完全经 `LogSink`，使核心逻辑可使用假实现独立测试。

Go 的直接调用证据包括 `pkg/executor/distsql.go` 的 lookup/列级报告、`pkg/executor/check_table_index.go` 和 `pkg/util/admin/admin.go` 的 admin check 报告，以及 `pkg/session/session.go` 的 MVCC 查看辅助；它们用于核对原始意图，不等于 Rust 当前调用覆盖面。

## 错误处理与边界

- MVCC 是附加诊断，不得遮蔽主错误：空键、存储失败、Region 定位失败和 JSON 失败分别降级为空串或 Region `0`；三个报告入口仍然记录主日志并返回一致性错误。
- `DecodeRowMvccData`/`DecodeIndexMvccData` 在 `Info` 缺失时不写 decoded 字段，空载荷版本被跳过。只有至少一个成功形成的数据映射时，`insertDecodedData` 才写 `decoded`；错误字符串随该映射写入。
- 解码循环中的 `err` 代表最后一次相关尝试的状态：后续成功会清除先前错误，后续失败会留下错误。扩展时不能误写成“累计所有版本错误”。
- `decodeIndexHandle` 拒绝短于 19 字节的索引键，也拒绝下游返回 `None` 的“无 handle”值。
- `ReportLookupInconsistent` 只限制 `fullHd` 展示为 50 个；`missHd`、`missRowIdx` 的 MVCC 补采数量没有内部上限，调用方需考虑诊断 I/O 和日志规模。
- `ReportAdminCheckInconsistentWithColInfo` 要求 `idxRow: &RecordData`，因此该入口不能表达缺失索引行；可空行场景由 `ReportAdminCheckInconsistent` 的两个 `Option<&RecordData>` 处理。
- `ConsistencyError::Display` 在参数不足时以空串补位。未知脱敏模式产生空敏感值；这是一种安全降级，但也会降低诊断可用性。
- 5000 的限制在序列化后按 UTF-8 字节截断；追加截断标识会使总长度大于 5000，且结果可能不再可解析为 JSON。

## 并发与资源生命周期

本文件不启动线程、异步任务、通道或事务，也没有锁。一次报告调用同步完成存储查询、解码、堆栈捕获和日志写入；调用延迟直接计入上游执行路径。`ReportLookupInconsistent` 可能按缺失项数量串行发起多次查询，`ReportAdminCheckInconsistent` 在非 `ON` 模式下可能为同一行/索引各查询两次：一次取去载荷元数据，一次生成完整解码 JSON。

编码闭包、存储和日志器由 `Arc` 保活到最后一个引用释放。返回解码闭包借用 `TableInfo`/`IndexInfo`，其生命周期不能超过被捕获元数据；当前调用均在表达式内立即使用。`Backtrace::capture` 是否真正收集完整堆栈受运行时配置影响，但字段总会被追加。存储返回值由调用栈拥有，元数据字段使用克隆后清空载荷，不会改变后续完整解码所见内容。

## 与 Go 版本的对应关系

[`reporter.go`](./reporter.go) 是逐项语义基准：公开命名、三个错误类别与错误码、50 个 full handle 上限、5000 字节 MVCC 截断、Region 失败回退、write/value 解码顺序、`ON` 时跳过完整 MVCC、admin check 仍保留去载荷元数据，均由 Rust 保留。

Rust 为适应 crate 边界做了显式抽象：Go 的 `helper.Storage` 与 Region cache 被压缩为 `Storage` 两方法；上下文日志器替换为可注入 `LogSink`；protobuf 响应替换为本地 serde DTO；Go 的 `error` 替换为含 kind/code/args/redact mode 的 `ConsistencyError`。`goBytes` 和 `toGoJSON` 补足 serde 与 Go JSON 的差异。

已确认的细节差异或迁移状态：

- Rust 截断时回退到 UTF-8 字符边界，避免 Go 按字节切片可能产生的非法 UTF-8；ASCII 输入时行为一致。
- Rust 的 `MvccInfo::Lock` 是不透明 JSON，只保存、不解释。
- Rust 当前生产接线证据仅见 typed point get 的 lookup mismatch；Go 中 admin/distsql/session 的广泛入口不能当作 Rust 已完成移植的证据。
- 独立迁移测试以假存储和假日志器覆盖兼容行为，不依赖真实 TiKV。

## 扩展指南

- 新增 MVCC 响应字段时，修改相应 DTO 与 serde 属性，并同步 `mvcc_json_matches_go_protobuf_tags_and_byte_encoding`；先核对 Go protobuf JSON 的字段名、零值省略和字节编码。
- 新增存储能力应优先保持 `Storage` 最小化；若必须扩 trait，应同时更新所有生产适配器和 `migration_aster_unit_test.rs::MockStorage`，并验证失败仍不覆盖主一致性错误。
- 修改行/索引解码时，分别从 `DecodeRowMvccData`、`DecodeIndexMvccData` 或其私有辅助接入；保留 write-before-value、按 start_ts 分组、空载荷跳过与最后错误状态语义，并扩展 `row_and_index_mvcc_decoders_match_go_write_value_and_error_behavior`。
- 新增报告类型时，应同步 `ConsistencyErrorKind`、`Display` 模板、Go 风格别名、日志字段与错误码测试；敏感字段必须经过 `redactString`/`redactErrorArg`，并明确 `ON` 下是否允许非载荷元数据。
- 优化诊断 I/O 时重点审查 `ReportLookupInconsistent` 的逐项同步查询及 `ReportAdminCheckInconsistent` 的重复查询；缓存或批量化不能改变失败降级、脱敏和日志字段形状。
- 生产逻辑与 Rust 测试应保持分文件。优先扩展 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)；若改动 typed point get 接线，则同步 `pkg/executor/typed_point_get_test.rs`。不要把测试内嵌到 `reporter.rs`。

## 验证依据

- RustCodeGraph：`status` 显示目标仓库索引有效（包含 Rust/Go）；`files --filter pkg/util/logutil/consistency` 确认 crate 的 `lib.rs`、`reporter.rs`、Go 对照和独立测试；`explore "pkg/util/logutil/consistency/reporter.rs Reporter report consistency"` 识别目标文件 51 个符号及 `GetMvccByKey`、两个解码器、三个报告入口的内部调用关系；`node --file ... --offset ...` 完整核对 1021 行源码。精确 `callers/callees` 命令未产生可用文本，因此上游关系另以仓库搜索和调用点源码确认。
- 源码与边界：[`reporter.rs`](./reporter.rs)、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)、根 `Cargo.toml`、`pkg/executor/Cargo.toml`。
- Go 语义：[`reporter.go`](./reporter.go)；调用点 `pkg/executor/distsql.go`、`pkg/executor/check_table_index.go`、`pkg/util/admin/admin.go`、`pkg/session/session.go`。
- Rust 调用与测试：`pkg/executor/typed_point_get.rs`、`pkg/executor/typed_point_get_test.rs`、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)。后者的五个测试覆盖空键/失败降级/截断、Go JSON 兼容、行与索引解码、lookup 报告、admin 两类报告及脱敏边界。
- 本任务是只新增说明文档的静态分析，按计划不运行 Cargo；结论来自索引、源码、Cargo、Go 对照与独立测试阅读，不代表本轮执行了运行时测试或真实 TiKV 验证。
