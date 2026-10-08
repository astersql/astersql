# `pkg/server/internal/util/util.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-server-internal-util`，crate 根 `pkg/server/internal/util/lib.rs` 以私有模块 `mod util` 引入它，再通过 `pub use util::*` 对外公开其符号。`pkg/server/internal/util/Cargo.toml` 说明该 crate 对应 Go 包 `pkg/server/internal/util`，直接依赖 `astersql-config`、`encoding_rs` 和 `http`。它位于服务器协议边界：一部分函数解析 MySQL 线协议中的 NUL 终止串和 length-encoded 值，一部分负责把客户端字符集输入转成 UTF-8 字节，另有当前主要用于迁移验证的 CORS 包装器和测试配置构造器。

Rust 生产链中的直接证据包括：`pkg/server/conn.rs::handleChangeUser` 使用 `ParseNullTermString` 读取 COM_CHANGE_USER 字段；`pkg/server/internal/parse/parse.rs::{read_lenenc_int,read_lenenc_bytes}` 分别调用 `ParseLengthEncodedInt` 和 `ParseLengthEncodedBytes` 解析握手连接属性；`pkg/server/conn_stmt_params.rs::parseBinaryParams` 使用 `InputDecoder` 处理字符串类预处理语句参数。`LengthEncodedIntSize`、`NewCorsHandler` 和 `NewTestConfig` 在当前 Rust 搜索结果中只见于本 crate 的独立测试，不能据 Go 侧接线宣称它们已进入 Rust 生产主链。

## 核心职责

- `ParseNullTermString` 在不复制数据的前提下，把输入切成第一个 NUL 之前的字段和之后的剩余数据，并用 `Option` 区分“空字段”和“根本没有终止符”。
- `ParseLengthEncodedInt` 与内部 `parse_fixed_integer` 实现 MySQL length-encoded integer 的首字节分派、小端整数拼装、NULL 标记和截断错误语义。
- `ParseLengthEncodedBytes` 先解析长度头，再返回对应载荷切片；即使载荷不足，也保留按声明长度算出的结束偏移，保持 Go 返回值契约。
- `LengthEncodedIntSize` 给出一个整数使用 length-encoded 形式编码后的总字节数。
- `NewInputDecoder` 与 `InputDecoder::DecodeInput` 根据客户端字符集选择是否转码；无需转码或转码失败时保留原始字节。
- `Handler`、`CorsHandler` 和 `NewCorsHandler` 为 `http` crate 的内存请求/响应类型提供最小同步处理器抽象，并在委托前注入配置允许的 CORS 响应头。
- `NewTestConfig` 从服务器默认配置派生本机测试配置，固定监听主机并关闭会改变测试环境的自动 TLS 和 Unix socket。

## 主要符号

- `pub fn ParseNullTermString(input: &[u8]) -> (Option<&[u8]>, &[u8])`：查找首个 `0`；找到时返回借用的前缀与余下切片，未找到时返回 `(None, input)`。空字段是 `Some(&[])`，与无终止符不同。
- `pub fn ParseLengthEncodedInt(input: &[u8]) -> (u64, bool, usize, Option<io::Error>)`：保留 Go 的四返回值模型。`0xfb` 表示 NULL；`0xfc`、`0xfd`、`0xfe` 分别读取后续 2、3、8 字节；其他首字节直接作为值，包括协议中未定义但 Go 当前按普通值处理的 `0xff`。
- `fn parse_fixed_integer(input, width, consumed)`：上述函数的私有公共分支。它先校验 `width + 1` 字节完整，再以小端顺序折叠载荷；截断时返回值零、非 NULL、消费量零及 `UnexpectedEof`。
- `pub fn ParseLengthEncodedBytes(input: &[u8]) -> (Option<&[u8]>, bool, usize, Option<io::Error>)`：组合整数头解析和载荷边界检查。长度零或 NULL 都不返回载荷，但保留头部的 NULL 标记与消费量。
- `pub fn LengthEncodedIntSize(value: u64) -> usize`：边界 `0..=250`、`251..=0xffff`、`0x1_0000..=0xff_ffff`、更大值分别返回 `1/3/4/9`。
- `pub struct InputDecoder { encoding: Option<&'static Encoding> }`：仅持有静态编码描述符，不持有流式解码状态。
- `pub fn NewInputDecoder(charset: &str) -> InputDecoder`：`ascii`、`binary`、`utf8`、`utf8mb4` 和空串直通；`latin1` 映射到 `WINDOWS_1252`；`gbk`、`gb18030` 通过 `Encoding::for_label` 查找；其他名称直通。匹配区分大小写且不裁剪空白。
- `pub fn InputDecoder::DecodeInput(&self, source: &[u8]) -> Vec<u8>`：始终返回独立 `Vec<u8>`；配置为直通时复制源字节，转码成功时返回 UTF-8 字节，解码器报告非法序列时回退到源字节。
- `pub trait Handler` 及其闭包实现：约束处理器接收可变 `Response<Vec<u8>>` 和拥有所有权的 `Request<Vec<u8>>`。任何匹配签名的 `Fn` 都可直接作为处理器。
- `pub struct CorsHandler<H>`、`NewCorsHandler`、`CorsHandler::ServeHTTP`：包装泛型处理器并按值拥有 `Config`。外层 `ServeHTTP` 创建空响应，在配置有效时写两个头，再调用内层处理器并返回响应。
- `pub fn NewTestConfig() -> Config`：返回按值拥有的默认配置，覆盖 `host`、`status.status_host`、`security.auto_tls` 和 `socket`。

## 执行流程

1. NUL 字段路径：`conn.rs::handleChangeUser` 把剩余报文交给局部 `take_string`；后者调用 `ParseNullTermString`，推进借用切片，再以有损 UTF-8 生成用户名、数据库名等字符串。若没有 NUL，工具函数本身不报错而是不消费输入；调用者当前以空值兜底，因此协议完整性若需更严格约束，应在上层处理。
2. length-encoded 整数路径：`ParseLengthEncodedInt` 先拒绝空输入，再按首字节选择 NULL、固定宽度或单字节分支；固定宽度分支验证完整性后从低位到高位拼装 `u64`。
3. length-encoded 字节路径：`ParseLengthEncodedBytes` 调用整数解析器；头部错误原样传播。长度为零立即返回；否则用 `saturating_add` 计算声明结束位置，先检查输入长度，最后才创建载荷切片。
4. 握手属性路径：`parse.rs::read_lenenc_int` 和 `read_lenenc_bytes` 把本文件的 `Option<io::Error>` 转换成包级 `ParseError::MalformedPacket`，随后 `decode_conn_attrs` 成对读取键和值。由此可见本文件只负责字节级协议解码，上层负责策略、指标和错误归一化。
5. 参数字符集路径：`conn_stmt_params.rs::parseBinaryParams` 取得调用者提供的解码器，缺省时构造 UTF-8 直通解码器；仅字符串、枚举、集合、几何和 bit 等被标记为文本解码的参数调用 `DecodeInput`，blob 分支保持原始字节。
6. CORS 路径：`CorsHandler::ServeHTTP` 新建响应；`config.cors` 非空且能解析成 `HeaderValue` 时写入 `Access-Control-Allow-Origin` 与固定的 `GET` 方法，然后无条件调用内层 `Handler`。内层可以继续修改状态、正文和响应头。

## 数据与状态

协议解析函数不维护全局或可变状态，返回的字节切片直接借用输入，生命周期由签名约束；除 `DecodeInput` 外不会复制载荷。`ParseLengthEncodedBytes` 的第三返回值表示从输入起点到载荷末端的总偏移，不只是头长；载荷截断时该值可能大于实际输入长度，这是 Go 兼容契约而不是安全切片索引。

`InputDecoder` 的唯一状态是不可变的静态编码指针或 `None`，每次解码产生新的 `Vec<u8>`，没有跨调用的部分字符缓存。`CorsHandler` 按值拥有处理器和完整 `Config` 快照；构造后的配置修改不会自动反映到包装器。`NewTestConfig` 每次生成独立配置值，不共享可变单例。

## 依赖与调用关系

- crate 装配：`pkg/server/internal/util/lib.rs` 再导出本文件全部公开符号；`Cargo.toml` 的 `config_crate` 重命名为本 crate 的 `crate::config`，供 `Config` 使用。
- 下游标准/外部依赖：`std::io::{ErrorKind, Error}` 表达截断；`encoding_rs::{Encoding, WINDOWS_1252}` 执行无 BOM 解码；`http::{Request, Response, HeaderValue, header}` 表达测试/原生处理器边界。
- Rust 上游：`pkg/server/conn.rs::handleChangeUser -> ParseNullTermString`；`pkg/server/internal/parse/parse.rs::read_lenenc_int -> ParseLengthEncodedInt`；`read_lenenc_bytes -> ParseLengthEncodedBytes -> ParseLengthEncodedInt`；`pkg/server/conn_stmt_params.rs::parseBinaryParams -> NewInputDecoder/InputDecoder::DecodeInput`。
- Rust 测试上游：`pkg/server/internal/util/util_test.rs` 覆盖基础解析和精确字符集名；`migration_aster_unit_test.rs` 覆盖完整载荷、解码、CORS 和测试配置。`pkg/server/internal/column/column_test.rs` 还通过再导出间接验证 `ParseLengthEncodedBytes`。
- Go 生产对照：`pkg/server/conn.go` 使用 NUL 解析和编码尺寸；`pkg/server/conn_stmt_params.go` 使用整数解析与解码器；`pkg/server/internal/parse/parse.go` 使用两类 length-encoded 解析；`pkg/server/http_status.go` 把 `NewCorsHandler` 接入真实状态 HTTP 服务。最后一条在 Rust 当前搜索中没有等价生产调用，属于迁移接线差异。

## 错误处理与边界

- 空输入以及 `0xfc/0xfd/0xfe` 载荷不足返回 `UnexpectedEof`、消费量零；这里使用 `std::io::ErrorKind::UnexpectedEof` 对应 Go 的 `io.EOF` 类别，但不是同一种语言级错误值。
- `0xfb` 同时携带 `(value=0, is_null=true)`；普通零字节携带 `(value=0, is_null=false)`，调用者不得只看数值。
- `ParseLengthEncodedBytes` 区分“头部截断”和“数据截断”：前者沿用整数解析器的消费量零，后者返回声明的结束偏移以及 `UnexpectedEof`。结束偏移用 `saturating_add` 防止 `usize` 算术溢出；在 32 位目标上把 `u64` 转为 `usize` 会截断后再饱和相加，当前文档未发现针对此平台差异的测试，因此不应把它表述为跨位宽完全等价。
- 长度为零返回 `None`，不是 `Some(&[])`；NULL 也返回 `None`，二者必须通过 `is_null` 区分。
- `ParseNullTermString` 没有终止符时不报错并返回原输入为剩余值。若上层循环在该结果上不推进，可能造成停滞；当前已读调用点不是循环。
- `NewInputDecoder` 对未知、大小写不同或带空白的标签静默选择直通。已知编码遇到非法序列也回退原字节，不暴露错误；调用者若需要拒绝非法文本，必须在更高层增加显式校验。
- CORS origin 不是合法 HTTP 头值时，代码跳过两项 CORS 头但仍调用内层处理器；配置只支持单个原样 origin 和固定 `GET`，没有预检、凭据或动态 origin 逻辑。

## 并发与资源生命周期

本文件没有锁、原子变量、通道、异步任务、事务或 I/O 资源所有权。纯解析函数只读借用输入，可以安全地被多个线程独立调用。`InputDecoder` 只包含 `&'static Encoding`，调用时不修改状态；并发可用性因此取决于字段类型和 `encoding_rs::Encoding` 的线程安全实现，文件本身未添加同步层。

`CorsHandler::ServeHTTP` 接收 `&self`，每个请求创建新的响应值，避免在包装器内共享响应状态；是否能跨线程共享仍由泛型 `H` 及 `Config` 的自动 trait 决定，因为 `Handler` 没有声明 `Send + Sync`。请求所有权传入内层，响应所有权最终返回；不存在显式关闭或清理阶段。`NewTestConfig` 和解码结果均按值返回，由 Rust 所有权在作用域结束时自动释放。

## 与 Go 版本的对应关系

主要符号与 `pkg/server/internal/util/util.go` 一一对应：NUL 切分、length-encoded 首字节规则、整数尺寸边界、失败时保留原输入的解码策略、CORS 头写入顺序及测试配置覆盖字段均保留。Rust 以 `Option<&[u8]>` 表达 Go 的 nil 切片，以 `Option<io::Error>` 模拟多返回值错误，并以借用切片避免协议解析复制。

需要注意的实现层差异如下：Go `InputDecoder` 使用 `pkg/parser/charset` 的编码抽象，Rust 直接使用 `encoding_rs`，并明确把 `latin1` 映射为 Windows-1252；现有测试只确认代表性字符和不受支持标签的直通，不能证明所有字符映射完全一致。Go 构造器返回指针，Rust 构造器返回拥有所有权的值。Go CORS 包装器实现 `net/http.Handler` 并持有配置指针，已由 `http_status.go` 接入状态服务；Rust 使用包内最小 `Handler` trait、内存 body 和配置快照，目前只在迁移测试出现。Go `NewTestConfig` 被大量服务器测试复用，Rust 版本目前只由本 crate 迁移测试使用。

## 扩展指南

- 增加或调整 MySQL 编码规则时，优先修改 `ParseLengthEncodedInt`/`parse_fixed_integer` 或 `ParseLengthEncodedBytes`，保持 `(值, NULL, 消费量, 错误)` 四元组的每条边界与 Go 一致；同步扩展独立文件 `pkg/server/internal/util/util_test.rs` 和 `migration_aster_unit_test.rs`，不要把测试嵌入本源文件。还应检查 `pkg/server/internal/parse/parse.rs` 对错误归一化及偏移推进的假设。
- 增加字符集时修改 `NewInputDecoder`，并用有效字节、非法序列、标签大小写和空白分别测试；同时对照 `pkg/parser/charset`，避免仅凭 `encoding_rs` 支持某标签就扩大 Go 公开语义。解码热路径会分配 `Vec<u8>`，任何缓存或零拷贝优化都要验证参数生命周期与吞吐收益。
- 将 CORS 接入 Rust 生产 HTTP 服务前，应确认真实服务所用请求/响应类型与并发约束；若要求动态配置，当前按值保存 `Config` 的设计需要调整。不得把现有测试用最小 trait 直接当作完整 `net/http` 等价物。
- 调整 `NewTestConfig` 时应与 Go 同名函数及服务器测试约定同步，并保持测试专用 API 与生产默认配置分离。
- 所有 Rust 行为修改都应保留文件现有版权头，并在修改后执行仓库要求的格式化与针对性测试；本说明任务本身不修改源码，也未运行 Cargo。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/server/internal/util` 确认目标、crate 根、Go 对照与独立测试均已索引。
- RustCodeGraph 文件节点：`pkg/server/internal/util/util.rs`（1--218 行，20 个符号）；`util.go`；`util_test.rs`；`util_test.go`；`migration_aster_unit_test.rs`；`lib.rs`；以及直接调用点 `pkg/server/conn.rs::handleChangeUser`、`pkg/server/conn_stmt_params.rs::parseBinaryParams`、`pkg/server/internal/parse/parse.rs::{read_lenenc_int,read_lenenc_bytes}`。
- 精确符号查询：`query ParseNullTermString`、`ParseLengthEncodedInt`、`ParseLengthEncodedBytes`、`LengthEncodedIntSize`、`NewInputDecoder`、`DecodeInput`、`NewCorsHandler`、`ServeHTTP`、`NewTestConfig`。节点 ID 形式的 `callers` 查询未在 30 秒内返回，因此调用关系又以已索引文件节点和限定 `pkg/server` 的直接引用搜索交叉核验，未用超时结果推断事实。
- 配置证据：`pkg/server/internal/util/Cargo.toml` 的 crate 名、`lib.rs` 路径、三个直接依赖和 `package.metadata.porting.go-package`；crate 根的 `mod util`/`pub use util::*` 证明公开边界。
- Go/测试证据：`pkg/server/internal/util/util.go`、`util_test.go`、`util_test.rs`、`migration_aster_unit_test.rs`；生产接线差异另由 `pkg/server/conn.go`、`conn_stmt_params.go`、`internal/parse/parse.go` 和 `http_status.go` 的直接引用确认。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证使用任务规定的 11 个固定二级标题结构检查，并人工复核所有“已接线”陈述都有上述源码或调用点依据。
