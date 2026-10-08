# `pkg/util/partialjson/extract.rs`

## 文件定位

本文件是 `astersql-util-partialjson` crate 的核心实现，crate 边界由 `pkg/util/partialjson/Cargo.toml` 定义，唯一外部依赖是 `serde_json = "1"`。`pkg/util/partialjson/lib.rs` 通过 `pub mod extract` 和 `pub use extract::*` 暴露本文件的公共符号；根 `Cargo.toml` 将该 crate 纳入 workspace，并以 `facade_util_partialjson` 依赖名接入，`pkg/lib.rs::util::partialjson` 再做统一门面导出。

它对应 Go 的 `pkg/util/partialjson/extract.go`，解决“大 JSON 顶层对象中只需要少数成员”时的按需扫描问题：不先把完整文档构造成树，而是顺序读取 token，收集目标成员、跳过非目标成员。全仓精确引用搜索显示，当前 Rust 生产代码尚未调用 `ExtractTopLevelMembers`、`newTopLevelJSONTokenIter` 或 `topLevelJSONTokenIter`；直接调用者仅位于同 crate 的独立测试 `extract_test.rs` 和 `migration_aster_unit_test.rs`。因此当前状态是“workspace 与门面已接线、实现和测试已存在，但尚无生产消费方”。

## 核心职责

- `TokenDecoder` 把输入字节流解码为 `Token`，同时用显式栈机验证对象、数组、冒号、逗号及闭合分隔符的顺序（`TokenDecoder::next_token`、`begin_value`）。
- `topLevelJSONTokenIter` 将底层 token 流提升为顶层对象成员迭代协议：调用者交替读取字段名和字段值；嵌套值会一次扫描到完整闭合（`readName`、`readOrDiscardValue`、`next`）。
- `ExtractTopLevelMembers` 对请求名称去重，顺序扫描顶层成员，只保存目标值的 token 序列，并在全部目标找到后立即停止。
- 数字以输入中的原始文本保存为 `Token::Number(String)`，避免先转成浮点数造成精度或字面形式丢失（`TokenDecoder::parse_scalar`）。
- 非目标嵌套值仍会完整解析和校验其已扫描部分，但在 `discard=true` 时不构造 token 结果向量（`topLevelJSONTokenIter::next`）。

## 主要符号

- `pub enum Token`：对外结果模型。`Delim(char)` 表示四种容器分隔符；`String(String)` 是已解码字符串；`Number(String)` 保留数字原文；`Bool(bool)` 和 `Null` 表示其余标量。它是拥有所有权的数据，不借用输入。
- `enum ErrorKind` 与 `pub struct PartialJsonError`：区分正常结束 `Eof`、容器未闭合的 `UnexpectedEof` 和语法/状态错误 `Syntax`。外部只能通过 `is_eof` 判断正常 EOF，其余分类不公开；错误实现了 `Display` 和 `std::error::Error`。
- `enum State`：容器局部状态，覆盖对象的“首 key/结束、key、冒号、值、逗号/结束”和数组的“首值/结束、值、逗号/结束”。
- `struct Frame`：每层容器保存预期闭合字节 `close` 与当前 `state`。
- `struct TokenDecoder<'a>`：内部字节解码器，持有借用的 `content`、游标 `pos`、嵌套栈 `frames` 和“顶层值是否已消费”标志 `root_consumed`。
- `TokenDecoder::{new, skip_space, begin_value, parse_scalar, next_token}`：分别负责初始化、跳过 JSON 允许的四类 ASCII 空白、推进状态、委托 `serde_json` 解码标量、消费结构 token。
- `pub struct topLevelJSONTokenIter<'a>`：公开但字段私有的顶层对象迭代器，组合 `TokenDecoder` 和独立的嵌套深度 `level`。
- `pub fn newTopLevelJSONTokenIter(&[u8]) -> topLevelJSONTokenIter<'_>`：构造入口，不立即验证输入；首次推进时才要求首 token 为 `{`。
- `topLevelJSONTokenIter::{readName, readOrDiscardValue, next}`：读取名称、读取/丢弃值及底层推进方法。虽然 `next` 公开，注释约定的安全调用协议仍是 `readName` 后紧接 `readOrDiscardValue`。
- `pub fn ExtractTopLevelMembers(&[u8], &[String]) -> Result<HashMap<String, Vec<Token>>, PartialJsonError>`：批量抽取入口。

文件中没有 trait、模块级常量或条件编译分支。

## 执行流程

1. `ExtractTopLevelMembers` 把 `names` 克隆进 `HashSet<String>`。重复名称自然去重，结果 `HashMap` 按去重后的数量预分配；空名称列表直接返回空 map，甚至不会验证 `content`。
2. 首次调用 `readName` 最终进入 `topLevelJSONTokenIter::next(false)`。当 `level == 0` 时，它从 `TokenDecoder::next_token` 读取首 token，只接受 `{`，随后把顶层对象深度设为 1。
3. `TokenDecoder::next_token` 先跳过空格、制表符、回车和换行，再按当前字节处理冒号、逗号、容器开闭符，其他起始字节交给 `parse_scalar`。`begin_value` 在值开始时把父容器推进到“逗号或结束”，或标记唯一顶层值已经消费。
4. `parse_scalar` 使用 `serde_json::Deserializer` 从当前切片只反序列化一个 `serde_json::Value`，按 `byte_offset` 推进游标。对象期待 key 时的字符串把状态推进到 `ObjectColon`；其他字符串和所有非字符串标量都先调用 `begin_value`。数字返回原始输入切片文本。
5. `readName` 要求本次结果恰为一个 `Token::String`。调用者随后必须消费对应值；标量值直接作为单 token 向量返回。
6. 若值以 `{` 或 `[` 开始，迭代器将 `level` 加一，并持续调用解码器，直至匹配的闭合 token 把深度降回 1。`discard=false` 时收集包括开闭分隔符在内的完整 token 序列；`discard=true` 时只推进和校验，不保存 token。
7. 批量入口发现目标名时，从 `remaining` 删除它并保存值；非目标名只丢弃其值。`remaining` 为空后立即返回，因此目标之后的尾部内容不会再解析。
8. 若顶层 `}` 在仍需寻找名称时出现，迭代器把它映射为正常 `Eof`；`ExtractTopLevelMembers` 会把该错误原样返回，表达“请求成员未全部找到”。

## 数据与状态

解析状态分两层：`TokenDecoder.frames` 负责每个 JSON 容器内部的语法状态和匹配闭合符，`topLevelJSONTokenIter.level` 负责判断一个顶层成员值何时完整结束。二者必须同步：解码器在开容器时压栈、闭容器时出栈；迭代器看到相同的 `Delim` token 后增减 `level`。

`root_consumed` 保证输入中只能有一个顶层 JSON 值。`pos` 是字节偏移，语法错误通过 `TokenDecoder::syntax` 附带该偏移。`ExtractTopLevelMembers` 的 `remaining` 表示尚未命中的去重键集合，`result` 保存已经命中的第一处同名顶层成员：一个名称从 `remaining` 删除后，后续重复键会走丢弃分支，不会覆盖先前结果。

内存占用主要由嵌套深度 `frames`、请求键集合、结果 token 及其拥有的字符串决定。丢弃路径不会保存嵌套 token，但 `serde_json` 仍会为逐个字符串/数字标量创建临时值；复杂度对实际扫描前缀近似线性。由于“全部目标找到即返回”，不一定扫描完整输入。

## 依赖与调用关系

上游模块关系为：根 workspace `Cargo.toml` → `astersql-util-partialjson` → `pkg/util/partialjson/lib.rs` → `extract.rs`；统一门面关系为根 `Cargo.toml` 的 `facade_util_partialjson` → `pkg/lib.rs::util::partialjson` → 本 crate 的再导出符号。Go 的 Bazel 目标仍由 `pkg/util/partialjson/BUILD.bazel` 单独声明，和 Rust Cargo crate 是并行构建边界。

本文件内部主调用链是 `ExtractTopLevelMembers` → `newTopLevelJSONTokenIter` → `readName` / `readOrDiscardValue` → `next` → `TokenDecoder::next_token` → `skip_space` / `parse_scalar` / `begin_value`。唯一外部库调用集中在 `parse_scalar`，使用 `serde_json::Deserializer` 解析单个标量并取得消费字节数。

RustCodeGraph 的文件查询确认本文件已索引且含 43 个符号；精确 `query` 找到本文件和 Go 对照中的迭代器及批量入口。但对这些符号执行 `callers` / `callees` 未返回图边，因此生产调用关系又通过全仓 `rg` 精确引用核对：当前无生产 Rust 调用，只有 `extract_test.rs` 与 `migration_aster_unit_test.rs`。不能据 Go 包的既有用途推断 Rust 生产链已经启用。

## 错误处理与边界

- 空输入在首次推进时产生 `Eof`；只有已经进入顶层对象后意外耗尽，`topLevelJSONTokenIter::next` 才把底层 `Eof` 转为 `UnexpectedEof`。
- 顶层必须是对象；数组、标量或其他首 token 会产生 `expected '{' for topLevelJSONTokenIter` 语法错误。
- 状态机拒绝多余/缺失的冒号和逗号、错误位置的值、不同类型的闭合符、不可结束状态下的闭合符，以及第二个顶层值。错误通常带 `at byte <pos>`。
- `skip_space` 只接受 JSON 标准空白 `0x20`、制表符、CR、LF；`extract_test.rs::test_iter_rejects_non_json_ascii_whitespace` 明确验证垂直制表符和换页符不能被当作空白。
- `readName` 只接受单一字符串 token。违反“名称后必须先消费值”的调用顺序会由底层状态机或这里的形状检查报错。
- 嵌套对象或数组未闭合时返回 `UnexpectedEof`；分隔符类型不匹配时返回 `Syntax`。
- `ExtractTopLevelMembers` 请求不存在的键时返回 `Eof`，而不是返回缺键的部分 map；重复请求名只需匹配一次，重复输入键保留第一次命中。
- 全部目标命中后不再验证后缀；`migration_extract_stops_after_all_requested_keys` 证明目标后的非法字节可以被忽略。空 `names` 时任何输入都返回空 map。这是按需前缀解析契约，不能等同于完整 JSON 合法性验证。
- 错误内部分类除 `is_eof` 外不可观察；调用者若需要区分 `UnexpectedEof` 与一般语法错误，目前只能依赖显示文本，不应新增脆弱的字符串判断，宜先扩展稳定的公开分类 API。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件句柄、网络连接或事务。迭代器独占 `&mut self` 推进，不能被多个调用者同时推进；其生命周期受输入 `&[u8]` 约束，输入在迭代器存活期间必须有效。

解析产生的 `Token`、错误消息、请求集合和结果 map 都拥有内部字符串，因此返回结果不借用输入，可以在迭代器销毁后继续使用。所有资源依靠 Rust 所有权在作用域结束时释放；提前找到全部目标时，尚未扫描的输入不会产生资源。类型没有显式线程安全承诺，若上层需要跨线程共享，应共享不可变输入并为每个任务创建独立迭代器，而不是给单个迭代器增加锁。

## 与 Go 版本的对应关系

Rust 的 `topLevelJSONTokenIter`、`newTopLevelJSONTokenIter`、`readName`、`readOrDiscardValue`、`next` 和 `ExtractTopLevelMembers` 与 `extract.go` 中的同名符号一一对应；主协议、嵌套深度处理、丢弃模式、目标全部命中后提前停止以及缺失成员返回 EOF 的语义保持一致。`extract_test.rs::test_iter` 直接复刻 Go `extract_test.go::TestIter` 的成功与失败表。

实现机制存在明确差异：Go 直接使用 `encoding/json.Decoder.Token` 和 `UseNumber`，token 是 `json.Token` 接口值；Rust 用 `TokenDecoder`、`State`、`Frame` 自行维护结构状态，只把标量交给 `serde_json`，并用强类型 `Token` 表示结果。Go 的数字是 `json.Number`，Rust 是保留原字节文本的 `String`。Go 以 `io.EOF` / `io.ErrUnexpectedEOF` 表示边界，Rust 用 `PartialJsonError` 内部分类并公开 `is_eof`。

Rust 对错误文案不承诺逐字等同 Go：例如非法 `{a}` 和 `{]` 的具体消息来自不同解析机制，测试只校验对应语义片段。Rust 还通过显式匹配闭合符和标准空白测试固定了边界。`migration_aster_unit_test.rs` 补充验证了 Go 测试未覆盖的批量抽取、跳过深层非目标值、提前返回、空请求及缺失键行为。

## 扩展指南

- 若增加 token 类型或改变标量表示，修改 `Token` 与 `TokenDecoder::parse_scalar`，并同步独立测试 `extract_test.rs` 和 `migration_aster_unit_test.rs`；尤其要保护大整数、指数、负数和数字原文字面形式。
- 若增加公开的流式遍历能力，优先在 `topLevelJSONTokenIter` 上提供保持“名称后消费值”不变量的专用方法，不要让调用方任意操纵 `TokenDecoder`。必要时可收紧或重新设计当前公开的 `next`，但要评估门面 API 兼容性。
- 若要公开稳定错误分类，在 `PartialJsonError` 上增加类型化查询或公开枚举，并为 EOF、unexpected EOF、语法错误分别补测试；不要以现有消息文本作为 API。
- 若改变提前停止策略，应同时决定是否仍允许目标后的非法 JSON，并更新 `migration_extract_stops_after_all_requested_keys`。把该函数改成完整合法性验证会改变 Go 对齐语义和性能特征。
- 若优化丢弃路径，必须保持完整推进嵌套结构与语法状态，不能仅搜索下一个逗号或括号；字符串转义和深层容器会使这种简化失效。
- 若把本 crate 接入生产调用，优先通过 `pkg/lib.rs::util::partialjson` 门面导入，并新增调用方目录中的独立测试；不要把 Rust 测试嵌入生产源文件。还应验证调用方是否期望缺失键返回错误、是否接受首个重复键以及是否依赖提前停止。
- 任何行为调整都应对照 `extract.go` / `extract_test.go`，除非任务明确要求产生跨语言差异；性能改动应重点关注扫描字节数、嵌套深度栈和被保留 token 的分配量。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/util/partialjson` 列出 Rust/Go 实现和测试；`node --file pkg/util/partialjson/extract.rs --offset 1 --limit 260` 与 `--offset 261 --limit 220` 覆盖完整 412 行；`query topLevelJSONTokenIter`、`query ExtractTopLevelMembers` 定位 Rust/Go 同名符号；对关键符号执行的 `callers` / `callees` 未返回边，已用精确文本引用搜索补证。
- Rust 源与装配：`pkg/util/partialjson/extract.rs`、`pkg/util/partialjson/lib.rs`、`pkg/util/partialjson/Cargo.toml`、根 `Cargo.toml`、`pkg/lib.rs`。
- Go 对照与构建边界：`pkg/util/partialjson/extract.go`、`pkg/util/partialjson/extract_test.go`、`pkg/util/partialjson/BUILD.bazel`。
- 独立 Rust 测试：`pkg/util/partialjson/extract_test.rs` 覆盖迭代顺序、嵌套 token、错误和空白边界；`pkg/util/partialjson/migration_aster_unit_test.rs` 覆盖批量抽取、丢弃、提前停止、空请求和缺失成员。
- 全仓精确搜索：生产 `.rs` 中没有 `ExtractTopLevelMembers` 或 `newTopLevelJSONTokenIter` 的调用；根 workspace 和 facade 引用证明 crate 已装配但尚未生产接线。
- 本任务只创建说明文档，按计划不运行 Cargo；结构验证要求目标文件存在且恰有 11 个固定二级章节。
