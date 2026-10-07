# `br/pkg/logutil/logging.rs`

## 文件定位

`logging.rs` 是 Cargo 包 `astersql-br-pkg-logutil` 的结构化日志主体；包入口 [`lib.rs`](lib.rs) 以 `pub mod logging` 装配该文件，并把日志字段构造器、logger、marshaler、脱敏和区间展示 API 再导出给调用方。包清单 [`Cargo.toml`](Cargo.toml) 将它定义为库 crate，直接依赖 `tracing`、`prometheus`、`uuid`、`astersql-lightning-metric` 等。当前实现面向 BR 的 Rust 移植层：备份文件、Region、SST 与脱敏接口来自 [`stubs.rs`](stubs.rs)，不是直接链接完整 kvproto/redact 子系统。

在完整应用中，本文件位于“业务对象/错误/指标 → 稳定结构化字段 → tracing 或测试捕获后端”的边界。RustCodeGraph 将目标文件识别为被大量仓库文件使用的日志公共面；仓库搜索进一步确认真实 Rust 消费者包括 `br/pkg/utils/*.rs`、`br/pkg/conn/util/util.rs`、`br/pkg/rtree/logging.rs` 和 `br/pkg/metautil/metafile.rs`。其中不少调用只使用 `Field`、`ShortError`、`log` 或区间格式化，而不是每个导出函数都已有生产调用。

## 核心职责

1. 用 `EncodedValue`、`Field`、`ObjectEncoder`、`ArrayEncoder` 和两个 marshaler trait 提供一个与 Go zap 常用子集相似的内存表示，并通过 `encode_fields_json` 输出稳定 JSON。
2. 用 `Logger`、`Level`、`log` 模块和 `WarnTerm` 把字段送往 `tracing`，或在测试中送往共享的 `CapturedLog` 缓冲；全局级别在输出前过滤日志。
3. 为 BR 常见对象构造字段：`File(s)`、`StreamBackupTaskInfo`、`RewriteRule`、`Region/Peer`、`SSTMeta(s)`、`BriefSSTMetas`、`Key(s)`、短错误、范围和直方图。
4. 统一敏感数据处理：key/range 经 `RedactKey`，任意字段通过 `RedactAny`/`Redact` 响应 `NeedRedact()`；同时保留 Go 端字段名、缩略阈值和文本格式，便于跨语言日志与测试对照。

## 主要符号

- `EncodedValue` 表示可编码值；`Skip` 是字段级省略标记，`Null` 才是 JSON `null`。`Field::{string,int,uint64,bool,array,object,from_object}` 构造字段，`Field::encode_json` 和 `encode_fields_json` 完成输出，`Field::equals` 服务测试比较。
- `ObjectEncoder`/`ArrayEncoder` 维护有序 `Vec`，`ObjectMarshaler::marshal_object` 与 `ArrayMarshaler::marshal_array` 是对象和数组扩展接口。顺序稳定是当前逐字 JSON 测试的重要契约。
- `Level`、`Logger`、`CapturedLog`、`default_logger`、`log::{L,Warn,GetLevel,SetLevel}` 构成日志运行层。`Logger::With` 复制并追加预置字段；`Logger::log` 先比较 `LOG_LEVEL`，再选择 `Tracing` 或 `Capture` 后端。
- `LevelGuard` 与 `OverrideLevelForTest` 以 RAII 恢复旧级别；`WarnTerm` 同时写普通 Warn 通道和可选的 `LOGGER_TO_TERM`。
- `AbbreviatedArrayMarshaler`、`AbbreviatedArray`、`AbbreviatedStringers` 控制长列表体积。前两者在长度 `<= 4` 时全量输出，后者在长度 `< 4` 时全量输出；超过各自阈值均变成“首项、`(skip N)`、尾项”。
- `File`/`Files`、`StreamBackupTaskInfo`、`RewriteRule(Object)`、`Region(By)`、`Leader`、`Peer`、`SSTMeta(s)`、`BriefSSTMetas`、`Key(s)` 是面向 BR 数据结构的公共字段构造器。
- `AShortError`/`ShortError` 对 `None` 返回 skip；`IntoEncodedValue`、`RedactAny`、`Redact` 支持保留标量、数组和 `Option` 的 JSON 形状或将整个字段替换为 `"?"`。
- `StringifyRange`/`StringifyKeys`/`StringifyManyArray`、`HexBytes` 实现日志友好的 `Display`/数组/JSON 表示；`MarshalHistogram` 将 Prometheus bucket、count、sum 编成对象。

## 执行流程

典型字段路径是：调用方选择 `File`、`Region`、`ShortError` 等构造器；构造器调用 `Field::from_object` 或直接创建 `Field`；对象 marshaler 按固定次序向 `ObjectEncoder.fields` 写入 `EncodedValue`；logger 合并预置字段与本次字段；`Logger::log` 检查全局级别后，生产路径由 `emit_tracing` 把消息和 `encode_fields_json` 的结果拼成一条 target 为 `br::logutil` 的事件，测试路径则把原始字段推入共享缓冲。

JSON 编码从 `write_json_pair` 进入 `write_json_value`：对象与数组递归编码，字符串交给 `json_escape_str` 处理引号、反斜杠、常用空白和控制字符。顶层 `encode_fields_json` 跳过 `Field.skip == true` 的项；嵌套出现 `EncodedValue::Skip` 时则按 `null` 编码，因此新 marshaler 不应主动把 `Skip` 填入嵌套值。

对象摘要各有独立流程。`FilesMarshaler` 缩略文件名并累计 KV、字节和 size；`SSTMetaMarshaler` 输出范围、Region、CRC 和 UUID，UUID 解析失败时保留 `invalid UUID <hex>`；`BriefSSTMetas` 单次遍历取字节序最小 start、最大 end，并累加 length、KV 数和 KV 字节；`HistogramMarshaler` 读取直方图快照，为每个桶生成六位小数的 `lt_<upper_bound>` 字段，最后附加 `count` 和 `total`。

## 数据与状态

字段值和对象成员均存于 `Vec`，因此对象键顺序、数组顺序和 logger 字段顺序与插入顺序一致。`Logger::With` 克隆已有 `Vec<Field>`，派生 logger 不会修改原实例；每次记录又克隆预置字段再追加本次字段。`Files`、`SSTMetas` 等公共 API 取得值所有权，`MarshalLogObjectForFiles` 则从 slice 克隆后复用同一实现。

进程级可变状态只有 `LOG_LEVEL: LazyLock<RwLock<Level>>` 和 `LOGGER_TO_TERM: LazyLock<RwLock<Option<Logger>>>`。Capture 后端由 `Arc<Mutex<Vec<CapturedLog>>>` 共享，克隆 logger 会共享缓冲。`LevelGuard` 保存覆盖前的 `Level`；析构时恢复。其余 marshaler 是调用期临时值，没有后台任务或持久资源。

数值累计使用普通 `u64`/`i32` 加法与长度转换：当前实现没有显式饱和或溢出处理。`BriefSSTMetas` 的空输入会输出 total 为 0、空 start/end 和零合计；空 `StringifyRange.EndKey` 按正无穷解释并经 `RedactValue("inf")` 展示。

## 依赖与调用关系

上游入口首先是 [`lib.rs`](lib.rs) 的 crate 根再导出。RustCodeGraph 的文件关系把 `logging.rs` 连接到大量仓库文件；精确仓库搜索确认：`br/pkg/conn/util/util.rs` 使用 `Field`、`Level`、`ShortError` 和 `log`，`br/pkg/rtree/logging.rs` 使用 `AbbreviatedStringers`，`br/pkg/metautil/metafile.rs` 使用 `Field`/`log`，`br/pkg/utils` 多个模块使用 `Field`、`ShortError`、`StringifyKeys`、`StringifyRange` 与包级 logger。`br/pkg/logutil/Cargo.toml` 的反向依赖清单还包括 `br/pkg/summary`、`conn/util`、`rtree`、`metautil` 和 `utils`。

直接下游依赖分为四类：标准库的格式化与同步原语；`tracing` 的四级事件宏；`uuid::Uuid::from_slice`；Prometheus `Histogram` 与 `astersql_lightning_metric::read_histogram`。BR 业务类型及 `NeedRedact`、`RedactKey`、`RedactValue` 来自 [`stubs.rs`](stubs.rs)。编码层内部的主调用边包括 `Field::from_object → ObjectMarshaler::marshal_object`、`Logger::log → emit_tracing → encode_fields_json`、`WarnTerm → log::Warn/Logger::log`、`SSTMeta → SSTMetaMarshaler` 和 `BriefSSTMetas → BriefSSTMetasObject`。

RustCodeGraph 对同名 Go/Rust 符号会产生歧义；精确 `query --json` 已分别定位 `br/pkg/logutil/logging.rs::{WarnTerm,BriefSSTMetas,RedactAny,MarshalHistogram}`。调用图查询在这些重名符号上没有稳定返回，所以本文对具体 Rust 消费者采用 `rg --glob '*.rs'` 补证，没有据此宣称每个导出 API 都已接入生产链。

## 错误处理与边界

本文件刻意把大多数日志格式化做成不返回错误：写入 `String` 的 `fmt::Write` 使用 `expect`，锁中毒也使用 `expect`，因此异常条件会 panic，而不是静默丢字段。`HexBytes::MarshalJSON` 签名返回 `Result`，当前手工拼接路径实际总是 `Ok`。`Uuid::from_slice` 是明确的可恢复边界，失败时仍输出原始 UUID 字节的 hex，保证排障信息不消失。

`AShortError`/`ShortError` 对空错误完全省略字段，不输出 null；`MarshalHistogram(None)`、读取不到 metric 或 histogram 时不写任何对象成员。Region/SST marshaler 直接调用桩类型的 `get_region_epoch()`、`get_range()`；当前桩提供默认对象语义，但若未来换成真实外部类型，必须重新验证缺失嵌套消息的行为。

脱敏边界需要特别保持：文件/Region/SST/key/range 的 key 字节走 `RedactKey`，`RedactAny` 在未启用脱敏时保留类型，在启用时统一变成字符串问号；`RewriteRule` 前缀按 Go 行为直接 hex 展示，不走 `RedactKey`。`BriefSSTMetas` 注释继承了 Go 对空 end key 的疑问，当前仅按字节序取最大值，没有赋予无穷语义。

## 并发与资源生命周期

`RwLock` 允许并发读取日志级别和终端 logger，修改通过独占写锁完成；Capture 缓冲使用 `Mutex` 保证多 logger 克隆之间安全追加。`WarnTerm` 在读锁内克隆 `Option<Logger>`，随后释放锁再写日志，避免持锁执行后端输出。没有显式线程、异步任务、通道、文件句柄或网络资源。

`OverrideLevelForTest` 的恢复依赖 `LevelGuard::drop`，适合词法作用域，但全局级别仍是进程共享状态；与 Go 文件中“不要用于并行测试”的约束相同，多个并行用例交错覆盖会互相影响。Capture 调用方读取缓冲时也需取得同一个 mutex；持锁期间不应再次通过相同 Capture logger 记录，以免自死锁。`LOGGER_TO_TERM` 的设置没有 RAII 守卫，测试或嵌入方设置后应显式恢复为 `None`。

## 与 Go 版本的对应关系

直接对照文件是 [`logging.go`](logging.go)，独立测试是 [`logging_test.go`](logging_test.go) 与 [`logging_test.rs`](logging_test.rs)，另有 [`parity_test.rs`](parity_test.rs) 做 crate 根公开契约冒烟。Rust 保留了 Go 的公共函数命名、字段键、列表缩略文本、Region/Peer 文本、无效 UUID 回退、短错误省略、范围格式和 histogram bucket 命名。Rust 测试逐字复用了 Go 的 File、Files、Key(s)、RewriteRule、Region、Leader、SSTMeta、ShortError 期望，并增加 `RedactAny` 类型保持和 histogram 六位小数检查。

实现差异主要来自运行时适配：Go 返回 `zap.Field` 并让 zap encoder 报错，Rust 使用自有 `Field`/`EncodedValue` 且 marshaler 无错误返回；Go 的全局 logger 来自 PingCAP log，Rust 映射到 `tracing`；Go protobuf/redact 是真实依赖，Rust Cargo 注释明确当前使用本地 stubs；Go `OverrideLevelForTest(t, lvl)` 通过 `t.Cleanup` 恢复，Rust 返回必须持有到作用域末尾的 `LevelGuard`；Rust `MarshalHistogram` 接受 `Option<Histogram>` 表达 Go 的 nil。

还需注意 `AbbreviatedArrayMarshaler` 的 `<= 4` 与 `AbbreviatedStringers` 的 `< 4` 是 Go 源码原有差异，不应为“统一风格”而擅自改动。Rust 的日志 payload 目前是拼入 tracing 消息字符串的 JSON 摘要，并非把每个 `Field` 注册成 tracing 的原生结构化字段；消费端若依赖 zap 式字段查询，需要先设计兼容迁移。

## 扩展指南

新增 BR 日志对象时，优先新增小型 marshaler 并通过 `Field::from_object` 暴露构造器，保持 Go 字段名、顺序、数值类型、脱敏路径和空值行为；若是通用值类型，再扩展 `IntoEncodedValue`。新增数组对象时实现 `ArrayMarshaler`，不要把测试逻辑放进本源文件，应在独立的 [`logging_test.rs`](logging_test.rs) 中补充正常、空输入、阈值边界和异常数据用例，并在 Go 有对应契约时同步核对 [`logging_test.go`](logging_test.go)。

修改 logger 时应从 `Logger::log`、`emit_tracing`、`log` 模块和 `WarnTerm` 接入，并评估全局锁、字段克隆成本、双写重复和 tracing 消费格式。修改脱敏时必须覆盖 `RedactKey`、`RedactValue`、`RedactAny`、`Redact` 的不同语义，防止嵌套对象或 rewrite prefix 意外泄漏或过度遮罩。修改聚合算法时应保留单次遍历和缩略输出，特别关注 `u64` 累加溢出、大列表分配及空 end key 的兼容含义。

若将本 crate 从 stubs 接到真实 kvproto/redact，上游依赖必须按仓库规则在独立上游仓库移植并以已发布 tag 引用；同时重新验证 getter 的缺省语义、所有权签名、protobuf `String()` 格式和脱敏全局状态。该变更不应仅以当前 stubs 测试通过作为兼容证据。

## 验证依据

- RustCodeGraph：运行 `status`，索引报告 7,032 个 Rust 文件；运行 `files --filter br/pkg/logutil`，确认源、入口、stubs、Rust/Go 测试均在图中；运行 `explore "br/pkg/logutil/logging.rs logging LogConfig init_log init_logger"` 获取文件使用关系和符号上下文；使用 `query --json` 精确区分 Go/Rust 的 `WarnTerm`、`BriefSSTMetas`、`RedactAny`、`MarshalHistogram`。
- 完整阅读的实现与边界文件：[`logging.rs`](logging.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`logging.go`](logging.go)、[`logging_test.rs`](logging_test.rs)、[`logging_test.go`](logging_test.go)、[`parity_test.rs`](parity_test.rs)；并通过仓库搜索核对 Rust 消费者和 Cargo 反向依赖。
- 测试证据：`logging_test.rs` 覆盖 Files/Keys 的 0、4、5、1024 边界，非法 UUID，短错误，上下文 logger，`RedactAny` 类型和 histogram key；`parity_test.rs` 覆盖 crate 根再导出、Go/Rust 关键公开契约与级别/上下文生命周期。
- 本任务是纯文档分析，按任务约束未运行 Cargo，也未修改 Rust、Go、Cargo 或 `plan.md`。交付前以任务指定命令验证文档存在且恰有 11 个固定二级标题，并人工复核链接、关键符号、调用边、迁移限制和扩展风险。
