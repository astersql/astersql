# `pkg/util/timeutil/errors.rs`

## 文件定位

实现 `fmt::Display` 和 `std::error::Error`，使各变体能稳定进入 `Result`、日志或上层错误包装。

## 核心职责

1. 用可比较、可克隆的 `TimeUtilError` 枚举表示时区名、系统时区、zoneinfo 路径、UTC 偏移、本地墙钟时间和文件 IO 失败。
2. 实现 `fmt::Display` 和 `std::error::Error`，使各变体能稳定进入 `Result`、日志或上层错误包装。
3. 通过 `ErrUnknownTimeZone.GenWithStackByArgs` 和 `ErrUnknownTimeZone.Equal` 保留 Go `dbterror` 的调用形状，并对齐 MySQL 错误 1298 的文本精度。`pkg/parser/mysql/errcode.rs` 将该错误定义为 1298，`pkg/parser/mysql/errname.rs` 的模板为 `Unknown or incorrect time zone: '%-.64s'`；本文件在显示时只取名称前 64 个 Unicode 字符。

## 主要符号

- `pub enum TimeUtilError`：公开错误联合类型，派生 `Debug + Clone + PartialEq + Eq`。
  - `UnknownTimeZone { name: String }`：`ParseTimeZone` 无法识别名称、格式或超范围偏移时的 MySQL 兼容错误。
  - `InvalidTimeZoneName { name: String }`：`load_named_location`/`LoadLocation` 直接加载非法 IANA 名时的更具体错误。
  - `InvalidSystemTimeZone`：`GetSystemTZ` 发现全局值仍为 `System` 或空串。
  - `UnsupportedZoneInfoPath { path: String }`：`infer_tz_name_from_file_name` 找不到 `zoneinfo.default`/`zoneinfo` 路径标记。
  - `InvalidOffset { seconds: i32 }`：`Location::fixed` 无法用 `chrono::FixedOffset::east_opt` 构造偏移。
  - `InvalidLocalTime`：`resolve_local` 收到 `chrono::LocalResult::None`，即该本地墙钟时间不存在。歧义时间 `Ambiguous` 在 `time_zone.rs` 中选较早值，不返回此错误。
  - `Io { operation: &'static str, path: String, message: String }`：保留操作名、路径显示值和底层 IO 消息。
- `pub(crate) fn TimeUtilError::io(...) -> Self`：crate 内部构造器，由 `infer_one_step_link_for_path` 的 `lstat` 和 `readlink` 错误分支调用。它把 `Path` 和 `std::io::Error` 立即快照为自有字符串，因而返回值不借用路径或底层错误。
- `impl fmt::Display for TimeUtilError`：为所有变体定义面向用户/日志的文本。
- `impl std::error::Error for TimeUtilError`：标记为标准错误；没有覆写 `source()`，因为 `Io` 只保留底层消息而非 `std::io::Error` 对象。
- `pub struct UnknownTimeZoneClass`：零字段标记类型，为 Go 错误类 API 提供命名兼容层。
- `GenWithStackByArgs(&self, name) -> TimeUtilError`：接收任意 `Into<String>` 并生成 `UnknownTimeZone`。名称的原值完整保存，仅 `Display` 截断。函数名保留 Go 风格，由 `lib.rs` 的 `non_snake_case` 允许项接纳。
- `Equal(&self, error: &TimeUtilError) -> bool`：仅对 `UnknownTimeZone { .. }` 返回 `true`，不比较具体名称。
- `pub static ErrUnknownTimeZone`：无内部可变状态的全局分类器实例。

## 执行流程

1. 时区操作在 `pkg/util/timeutil/time_zone.rs` 执行，成功时返回 `Location`、时区名或 UTC 时刻，失败时在就近分支构造 `TimeUtilError`。
2. `ParseTimeZone` 先处理 `SYSTEM`，再尝试 IANA/`Local` 名，然后解析 MySQL `+/-HH:MM[:SS]` 偏移。所有尝试失败，或偏移超过 `-12:59`/`+14:00` 边界时，调用 `ErrUnknownTimeZone.GenWithStackByArgs(value)`。
3. 直接 IANA 加载失败走 `InvalidTimeZoneName`；未初始化的系统时区走 `InvalidSystemTimeZone`；zoneinfo 推断、固定偏移、墙钟转 UTC 和符号链路读取分别使用其余变体。
4. 返回值沿 `Result<_, TimeUtilError>` 传播给调用者。调用者可用枚举模式匹配具体边界，或用 `ErrUnknownTimeZone.Equal(&error)` 按 MySQL 错误类判定，最后由 `Display` 生成文本。

## 数据与状态

`TimeUtilError` 只包含值语义数据：所有可变长内容都以 `String` 拥有，`Io.operation` 则是由调用点传入的 `&'static str`。枚举不持有 `Path`、IO 句柄、时区缓存、锁或 `chrono` 时区对象。`Clone + Eq` 使测试和上层可以做精确值比较，但 `UnknownTimeZoneClass::Equal` 故意只比较变体类别。

`ErrUnknownTimeZone` 是零大小、不可变的 `static`，不记录错误次数或最后一次参数。未知时区的原始 `name` 在枚举中保留全长；64 字符限制只是格式化规则，因此不应把 `to_string()` 后的文本当作原始输入的无损序列化。

## 依赖与调用关系

- 模块装配：`pkg/util/timeutil/lib.rs` 声明 `pub mod errors`，并将 `errors_test.rs` 作为独立测试模块接入；测试不内嵌在生产源文件。
- 直接上游：`pkg/util/timeutil/time_zone.rs` 导入 `ErrUnknownTimeZone` 和 `TimeUtilError`。其 `Location::fixed`、`resolve_local`、`infer_one_step_link_for_path`、`infer_tz_name_from_file_name`、`GetSystemTZ`、`load_named_location` 和 `ParseTimeZone` 是各变体的真实构造/返回点。
- 上层边界：仓库其他 Rust 模块主要导入 `astersql_util_timeutil::time_zone` API，而非直接构造本文件的错误。因此错误在完整应用中位于“会话/表达式/调度等时区消费者→ timeutil 公开 API→错误契约”的末端失败边。
- 直接下游：`fmt::Display`、`Path::display`、`std::io::Error::to_string`、`Into<String>` 和模式匹配；无网络、存储、异步运行时或 Cargo 第三方库的直接调用。
- RustCodeGraph 将 `errors.rs` 索引为 17 个符号，并指向 `time_zone.rs` 中返回 `TimeUtilError` 的关联符号；针对重名错误符号的精确 `callers/callees` 查询未输出调用边，故上述直接边由索引化的 `time_zone.rs` 源码节点与限定目录引用搜索交叉核实，不将空图结果解读为“无调用者”。

## 错误处理与边界

- `UnknownTimeZone` 是对外兼容类，适用于 `ParseTimeZone` 的最终失败；`InvalidTimeZoneName` 是内部能力更具体的 IANA 加载失败。扩展时不应把两者随意合并，否则会改变 `ErrUnknownTimeZone.Equal` 及 MySQL 1298 契约。
- 未知时区的显示名按 `chars()` 截取 64 个 Unicode 标量值，而非 64 字节；这能避免在 UTF-8 字节中间截断。`errors_test.rs` 证明保存值仍是 65 字符原名，只有文本为 64 字符。
- `Io` 不保留结构化 `ErrorKind` 或错误链，只保留操作/路径/消息快照。需要按 `ErrorKind` 重试时，应先有意识地扩展变体和测试，不要解析 `Display` 文本。
- `InvalidOffset.seconds` 保留原输入；`InvalidLocalTime` 没有保留墙钟时间或时区，所以上层如需上下文，应在调用边界包装或记录。
- 所有 `Display` 文本都是兼容面，特别是未知时区、非法名和系统时区文本；修改前应对照 Go 测试与上层错误断言。

## 并发与资源生命周期

本文件没有锁、原子、通道、任务、事务、文件句柄或析构顺序。`TimeUtilError` 在错误产生时就完成所有字符串的所有权转换，后续克隆和跨层传递不依赖原路径或 IO 错误的生命期。`ErrUnknownTimeZone` 无内部状态，并发调用 `GenWithStackByArgs`/`Equal` 不会共享可变数据。

时区缓存的 `RwLock`、系统时区的 `Once` 和 `OnceLock` 都属于 `pkg/util/timeutil/time_zone.rs`，不属于本错误文件。错误枚举仅作为这些并发能力的返回值，不参与锁中毒恢复或缓存生命周期。

## 与 Go 版本的对应关系

Rust 枚举的其他变体主要对应 `pkg/util/timeutil/time_zone.go` 中原先用 `fmt.Errorf` 或标准库错误表达的分支：`UnsupportedZoneInfoPath` 对应“path ... is not supported”，`InvalidSystemTimeZone` 对应 `systemTZ` 未设置，`InvalidTimeZoneName` 对应 `time.LoadLocation` 失败后的“invalid name ...”。

`InvalidOffset` 和 `InvalidLocalTime` 是 Rust `chrono` 适配边界的结构化错误，不是 Go `errors.go` 中的独立错误类。Go `time.FixedZone` 和墙钟转换的 API 形状与 `chrono` 不同，因此这两个变体是为了在 Rust `Result` 中不丢失失败语义，不应宣称为 Go 错误类的逐一复刻。

## 扩展指南

- 新增时区失败类别时，先在 `TimeUtilError` 增加语义明确的变体，同步补全 `Display` 的穷尽匹配，再在 `time_zone.rs` 的真实失败点转换。测试应写入独立的 `pkg/util/timeutil/errors_test.rs` 或对应 `time_zone_test.rs`，不内嵌到生产文件。
- 如新错误属于 MySQL 标准错误类，需要核对 `pkg/parser/mysql/errcode.rs`、`errname.rs` 以及 Go `dbterror` 定义，不应只添加一条显示文本。保留 `Equal` 按类判定、参数不参与等价性的契约。
- 修改 `UnknownTimeZone` 格式时，同步验证错误号 1298、`%-.64s` 精度、Unicode 截断、原始 `name` 保留以及 `ParseTimeZone` 的负/正偏移边界。关联测试包括 `errors_test.rs`、`time_zone_test.rs`、`migration_aster_unit_test.rs` 和 Go `time_zone_test.go`。
- 如要让 `Io` 支持错误链或 `ErrorKind`，需要权衡与当前 `Clone + Eq` 派生的兼容性；直接存放 `std::io::Error` 会改变这些 trait 能力。性能上本文件只在错误路径分配字符串，不应在成功热路径额外格式化错误。
- 增加字段或改变变体时，应搜索所有穷尽模式匹配。当前 `Equal` 只识别 `UnknownTimeZone`，新变体不会自动归入该类。
