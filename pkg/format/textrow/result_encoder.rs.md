# `pkg/format/textrow/result_encoder.rs`

## 文件定位

本文件属于 Cargo crate `astersql-format-textrow`，crate 根在 [`pkg/format/textrow/lib.rs`](./lib.rs)，并由该入口私有声明 `result_encoder` 模块后公开再导出全部符号。crate 的依赖和 Go 迁移来源记录在 [`pkg/format/textrow/Cargo.toml`](./Cargo.toml)：字符集实现来自 `astersql-parser-charset`，MySQL 类型与 collation 常量来自 `astersql-parser-mysql`，日志来自 `astersql-util-logutil`；`package.metadata.porting.go-package` 指向同目录 Go 包。

它处在 MySQL 结果集序列化的“字符集策略”层，而不负责协议帧本身。相邻的 [`textrow.rs`](./textrow.rs) 将 `chunk::Row` 的单列值格式化为文本，并对字符串类值调用这里的 `UpdateDataEncoding` 与 `EncodeData`。更上游的 [`pkg/server/internal/column/column.rs`](../../server/internal/column/column.rs) 在列定义包中调用 `EncodeMeta`、`ColumnCharsetID`、`IsStringColumnType`，在文本行和二进制行输出中复用 `ResultEncoder`；长度编码、NULL 标志和协议包布局均由服务端列模块负责，而不是本文件的职责。

## 核心职责

本文件集中实现三组规则：

1. 根据会话变量 `@@character_set_results` 建立稳定的结果字符集编码器，供元数据和普通字符串结果使用（`ResultEncoder::NewResultEncoder`）。
2. 根据当前列 collation 动态更新列侧编码器，并在“结果字符集为空、结果字符集为 binary、当前列为 binary”时让列侧编码优先，否则让会话结果字符集优先（`UpdateDataEncoding`、`EncodeData`）。
3. 决定列定义包应该声明的 charset ID，以及哪些 MySQL 类型属于可被结果字符集重写的字符串类列（`ColumnCharsetID`、`IsStringColumnType`）。

编码转换统一使用 `charset::OpEncodeReplace`。因此这里的运行时契约是尽力产生客户端可消费的字节并记录诊断日志，而不是把字符转换错误向调用者传播。文件也管理两个请求/语句内可复用缓冲：字符转换用的 `buffer` 和数值格式化等相邻路径使用的 `scratch`。

## 主要符号

- `pub struct ResultEncoder`：有状态编码器。`encoding` 是会话结果字符集编码器；`data_encoding` 是最近一次 `UpdateDataEncoding` 选择的列侧编码器；`chs_name`、`is_binary`、`is_null` 固化构造时的会话策略；`data_is_binary` 随当前列改变；`buffer` 与 `scratch` 是可释放的复用内存。
- `ResultEncoder::NewResultEncoder(chs: &str) -> Self`：初始化会话结果编码器、空的列侧 noop 编码器、转换缓冲和容量为 48 的 scratch，并缓存 binary/空字符集判定。
- `pub fn NewResultEncoder(chs: &str) -> ResultEncoder`：与 Go 包级构造函数同名的便捷入口，直接转发到关联函数。
- `ResultEncoder::Clean(&mut self)`：把 `buffer` 和 `scratch` 置为 `None`，释放编码器持有的分配。Rust 版本在此后仍允许编码，`encode_with` 会使用临时 `Vec`；但将失去跨调用复用能力。
- `ResultEncoder::UpdateDataEncoding(&mut self, chs_id: u16)`：由列 collation ID 查询字符集名称，更新列侧编码器和 `data_is_binary`。未知 ID 只写 warn 日志，随后仍按查询返回的字符集名称建立编码器。
- `ResultEncoder::ColumnCharsetID(&self, dump_charset: u16, is_string_col: bool) -> u16`：计算列定义包中对外声明的 charset ID。结果字符集为空或列不是字符串类时保留列自身值；列自身为 binary 时也保持 binary；其余字符串类列改为会话结果字符集对应 ID。
- `ResultEncoder::EncodeMeta(&mut self, src: &[u8]) -> Vec<u8>`：元数据始终使用会话结果编码器。
- `ResultEncoder::EncodeData(&mut self, src: &[u8]) -> Vec<u8>`：按会话状态和当前列状态在 `encoding` 与 `data_encoding` 间选择。
- `take_scratch` / `recycle_scratch`：仅供 crate 内的 `textrow.rs::format_with_scratch` 使用。前者取出并清空 scratch，同时返回是否可回收；后者仅在可回收时按输出长度（至少 48）重新建立并保存 scratch 内容。
- `encode_with`：私有的统一转换与错误降级入口。`buffer` 已被 `Clean` 时使用栈上临时 `Vec`，转换失败时写 debug 日志并返回错误对象携带的部分输出。
- `pub fn IsStringColumnType(tp: u8) -> bool`：识别 String/VarString/Varchar/Bit、各 Blob、Enum、Set、JSON 和 TiDB VectorFloat32；其他类型（例如 Longlong）返回 `false`。

文件中没有 trait、宏、条件编译项或模块级业务常量；公开 API 是 `ResultEncoder`、其公开方法、包级 `NewResultEncoder` 和 `IsStringColumnType`，scratch 方法与 `encode_with` 分别是 crate 内部和实现内部细节。

## 执行流程

典型列定义流程如下：服务端为会话创建或取得 `ResultEncoder`；`Info::dump` 逐个把 schema、table、column name 等字节交给 `EncodeMeta`；随后用 `IsStringColumnType(self.Type)` 判定类型，再将原始 `dumpCharset` 一并交给 `ColumnCharsetID`。这样元数据文本与元数据中声明的 charset ID 使用同一会话策略，同时 binary 列和非字符串列保持原声明。

典型行数据流程如下：`textrow.rs::FormatValueText` 根据列类型取值；字符串、Blob、Bit、Enum、Set 会先用列的 `Charset` 调用 `UpdateDataEncoding`，JSON 和 VectorFloat32 则使用 `mysql::DefaultCollationID`；随后调用 `EncodeData`。`EncodeData` 在 `is_null || is_binary || data_is_binary` 时选择列侧 `data_encoding`，否则选择会话侧 `encoding`，最后都进入 `encode_with`。服务端再为返回字节添加 length-encoded 外框。`DumpBinaryRow` 对字符串类分支也直接执行同样的更新与编码步骤，因此本策略并不限于文本行函数。

`encode_with` 将目标缓冲、源字节和 `OpEncodeReplace` 交给 `EncodingRef::Transform`。成功时返回转换结果；失败时记录 debug 并返回错误携带的输出。若调用者已经执行 `Clean`，转换仍通过临时缓冲完成。数值格式化路径不进入 `EncodeData`，但相邻 `FormatValueText` 会通过 `take_scratch` 取得 scratch，完成渲染后由 `recycle_scratch` 保存可供下一次调用复用的数据。

## 数据与状态

`ResultEncoder` 同时保存“语句/请求级不变状态”和“当前列状态”。构造后，`encoding`、`chs_name`、`is_binary`、`is_null` 不再改变，代表会话的 `@@character_set_results`；`data_encoding`、`data_is_binary` 必须在处理每个需要字符集转换的字符串类列前由 `UpdateDataEncoding` 刷新。调用顺序因此是一个重要不变量：不能假设上一列留下的 `data_encoding` 适用于下一列。

`ColumnCharsetID` 只读状态，不修改编码器。`EncodeMeta` 不依赖当前列状态。`EncodeData` 则依赖最近一次列侧更新；调用者对字符串类值负责先更新。`buffer` 和 `scratch` 都以 `Option<Vec<u8>>` 表示是否仍保留可复用内存；`Clean` 是单向释放操作，本文件没有重新启用复用缓冲的 API。`take_scratch` 暂时把 scratch 所有权移出结构体，`recycle_scratch` 只在原本存在 scratch 时恢复它，防止 `Clean` 后无意恢复长期持有的缓冲。

所有编码方法返回拥有所有权的 `Vec<u8>`，没有把编码器内部缓冲的借用暴露给调用者。这与 Go 注释中“结果应立即消费”的底层切片复用约束不同；不过调用者仍应把每次返回值视为当前字段的编码结果，而不要依赖内部缓冲实现。

## 依赖与调用关系

下游依赖均经 `lib.rs` 的窄再导出访问：`charset::FindEncodingTakeUTF8AsNoop` 构造编码器，`charset::GetCharsetInfoByID` 从 collation ID 获取字符集，`EncodingRef::Transform` 执行转换，`mysql::BinaryDefaultCollationID`、`CharsetNameToID` 和列类型常量实现 MySQL 兼容判定，`logutil::BgLogger` 记录非致命问题。

直接 Rust 调用证据包括：

- [`pkg/format/textrow/textrow.rs`](./textrow.rs) 的 `FormatValueText`：字符串、Blob、Bit、Enum、Set、JSON、VectorFloat32 分支调用 `UpdateDataEncoding` 与 `EncodeData`；`format_with_scratch` 调用两个 scratch 方法。
- [`pkg/server/internal/column/column.rs`](../../server/internal/column/column.rs) 的 `Info::dump`：调用 `EncodeMeta`、`ColumnCharsetID`、`IsStringColumnType`；`DumpTextRow` 通过 `FormatValueText` 间接使用编码器；`DumpBinaryRow` 在字符串类分支直接调用 `UpdateDataEncoding` 与 `EncodeData`。
- [`pkg/format/textrow/lib.rs`](./lib.rs)：再导出本文件符号，并把独立测试文件通过 `#[path]` 接入。

Cargo 接线方面，workspace 根 `Cargo.toml` 包含 `pkg/format/textrow` 成员和 `facade_format_textrow` 路径依赖；`pkg/server/Cargo.toml` 以及 `pkg/server/internal/column/Cargo.toml` 都以路径依赖接入 `astersql-format-textrow`。RustCodeGraph 将本文件标记为由 `textrow.rs` 使用；精确 `callers/callees` 对这些方法未返回边，因此上述服务端调用关系以精确符号引用搜索和对应源码为补充证据，而没有外推未验证的调用者。

## 错误处理与边界

`UpdateDataEncoding` 遇到未知 charset/collation ID 时不会返回错误：它记录 warn，继续使用 `GetCharsetInfoByID` 返回的字符集名称建立编码器。因此调用者不能从返回值判断更新是否完全有效，排查只能依赖日志和最终字节。

`encode_with` 同样不向上传播转换错误。它用 `OpEncodeReplace` 请求替换不可编码内容；如果转换仍返回错误，则记录 debug，并返回错误中携带的部分输出。这保证协议写出路径仍能取得字节，但也意味着“得到 `Vec`”不等于无损转换成功。新增严格模式时不能悄悄改变现有公开方法签名或容错语义，应单独设计可传播错误的 API，并同步所有协议调用者。

`ColumnCharsetID` 明确保留三类边界：空结果字符集、非字符串列、binary 列。`IsStringColumnType` 是封闭枚举，新增 MySQL 字符串类类型不会自动生效。`EncodeData` 的列侧选择依赖先前 `UpdateDataEncoding`；若绕过更新，可能使用构造时的 noop 列编码或上一列状态。`Clean` 后编码不会 panic，但每次 `encode_with` 都需要临时缓冲；scratch 路径也不再回收。输入是任意字节切片，本文件不验证它是否符合源字符集，也不负责长度前缀、NULL、最大包或列名截断。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件、网络连接或事务。所有可变操作都要求 `&mut ResultEncoder`，Rust 借用规则使同一实例的编码和列状态更新在调用层面串行发生；文件本身没有承诺跨线程共享，也没有内部同步。

预期生命周期是由请求或语句创建一个编码器，依次处理列元数据和行值，最后调用 `Clean` 释放可能因大字段增长的 `buffer` 与 `scratch`。`Clean` 不是析构要求：实例自然离开作用域也会释放内存；它的价值是让仍存活的编码器尽早放弃大分配。Rust 版本允许 `Clean` 后继续编码以避免 panic，但此时只提供功能性退化路径，不再提供内存复用。独立迁移测试 `result_encoder_matches_go_charset_selection_and_cleanup` 明确验证了 `Clean` 后 `EncodeMeta` 仍按 GBK 工作。

## 与 Go 版本的对应关系

直接对照文件是 [`result_encoder.go`](./result_encoder.go)。Rust 保留了 Go 的字段角色和公开行为：会话编码与列编码分离；构造时缓存 `isBinary`/`isNull`；未知 charset ID 只告警；metadata 总走结果字符集；data 在 null/binary/列 binary 时走列字符集；列定义 charset 改写规则和字符串类型集合一致；转换错误只记录日志并返回可用输出。

所有权实现存在必要差异。Go 使用 `*bytes.Buffer` 和复用 `[]byte`，`Encode*` 返回值可能由内部缓冲支撑，因此注释要求立即消费；Rust 方法返回独立 `Vec<u8>`，`buffer`/`scratch` 用 `Option` 表示 `Clean` 状态。Go 的 `Clean` 后注释要求不再复用编码器，而 Rust 的 `encode_with` 显式回退到临时 `Vec`，并由迁移测试验证仍能工作。Rust 的 `recycle_scratch` 会按输出长度重新分配并复制保存内容，这是安全的所有权适配，不应被误解为与 Go 完全相同的切片别名模型。

Go 测试 [`result_encoder_test.go`](./result_encoder_test.go) 与 Rust 测试 [`result_encoder_test.rs`](./result_encoder_test.rs) 一一覆盖 UTF-8 noop、中文“一”到 GBK 字节 `d2 bb`、binary 透传，以及字符串类型集合和 Longlong 反例。Rust 额外的 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 覆盖 `EncodeData` 的结果/列字符集切换、binary collation、`ColumnCharsetID` 三个关键分支和 `Clean` 后编码；[`textrow_test.rs`](./textrow_test.rs) 则通过 `FormatValueText` 验证 GBK 字符串的集成路径。

## 扩展指南

新增一种字符串类 MySQL 类型时，至少同步检查 `IsStringColumnType`、`textrow.rs::FormatValueText` 和服务端 `DumpBinaryRow` 的类型分支；若它的逻辑值固定使用 UTF-8（类似 JSON/VectorFloat32），应明确选择 `mysql::DefaultCollationID`，否则使用列的 `Charset`。测试必须放在独立测试文件中：类型分类和元数据规则优先扩展 `result_encoder_test.rs` / `migration_aster_unit_test.rs`，值格式化扩展 `textrow_test.rs`，协议列定义或二进制行行为扩展 `pkg/server/internal/column/column_test.rs`。

修改字符集优先级时，应分别覆盖 `character_set_results` 为普通字符集、空值和 binary，以及列为普通/binary collation 的组合；同时核对 `ColumnCharsetID` 的声明值与 `EncodeData` 的真实字节保持一致。修改 `Clean`、buffer 或 scratch 时，需要保留“释放后不 panic、不恢复长期复用状态”的当前契约，并关注大行导致的峰值内存、每字段分配和复制成本。

若需要严格错误报告，建议新增返回 `Result` 的显式入口并逐层接线，而不是改变 `EncodeMeta`/`EncodeData` 的尽力输出行为。若添加共享或并发使用方式，需要先定义列状态的隔离边界；当前 `data_encoding` 是逐列可变状态，直接在多个并发编码流之间共享会产生语义冲突，即使通过锁绕过借用限制也不安全。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标目录 10 个文件；`files --filter pkg/format/textrow` 确认目标、crate 入口、Go 对照和独立测试均已索引；`node --file pkg/format/textrow/result_encoder.rs` 读取完整 191 行并报告 13 个符号、由 `textrow.rs` 使用。
- RustCodeGraph 精确查询：`query ResultEncoder`、`query EncodeData`、`query ColumnCharsetID`、`query UpdateDataEncoding`、`query IsStringColumnType` 确认 Rust/Go 同名符号及测试；对 Rust 方法执行 `callers/callees` 未返回边，因而又用精确引用搜索并阅读 `textrow.rs` 与 `pkg/server/internal/column/column.rs` 补齐调用证据。
- 已读 Rust 路径：`pkg/format/textrow/result_encoder.rs`、`lib.rs`、`textrow.rs`、`result_encoder_test.rs`、`migration_aster_unit_test.rs`、`textrow_test.rs`，以及上游 `pkg/server/internal/column/column.rs` 的元数据、文本行和二进制行相关区段。
- 已读配置与 Go 路径：`pkg/format/textrow/Cargo.toml`、workspace `Cargo.toml` 中相关成员/依赖、`pkg/server/Cargo.toml`、`pkg/server/internal/column/Cargo.toml`、`pkg/format/textrow/result_encoder.go`、`result_encoder_test.go`、`textrow.go`。
- 关键可复核行为：GBK metadata 编码为 `d2 bb`；binary metadata 透传；binary 列的数据绕过结果字符集；非字符串和 binary 列的声明 charset 不被改写；`Clean` 后 Rust 编码仍可工作；Longlong 不属于字符串列类型。
- 本任务是纯文档分析，按计划未运行 Cargo。最终仅运行任务指定的 11 章节结构检查，并人工核对本文没有把协议 framing、并发保证或未出现的严格错误传播归于本文件。
