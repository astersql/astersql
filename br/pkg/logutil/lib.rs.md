# [`br/pkg/logutil/lib.rs`](lib.rs)

## 文件定位

`br/pkg/logutil/lib.rs` 是 Cargo 包 `astersql-br-pkg-logutil` 的 crate 根。`br/pkg/logutil/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定该入口，并以 `package.metadata.porting.go-package = "br/pkg/logutil"` 表明它承接 Go 包 `br/pkg/logutil` 的公开能力。根工作区 `Cargo.toml` 也把 `br/pkg/logutil` 列为 workspace member。

该文件自身不执行日志编码、脱敏或速率计算。它用 `#[path]` 装配四个公开生产模块 `stubs.rs`、`context.rs`、`logging.rs`、`rate.rs`，再把调用方常用类型和函数提升到 crate 根（`lib.rs:19-47`）。`parity_test.rs` 与 `logging_test.rs` 只在 `cfg(test)` 下作为私有测试模块挂载（`lib.rs:49-57`），因此测试逻辑与生产入口分离。

crate 级 `allow` 覆盖 Go 风格命名、阶段性未使用导入与死代码（`lib.rs:10-17`）。这使 `ContextWithField`、`TraceRateOver`、`StartKey` 等迁移接口可以保持与 Go 相近的名字，但也会降低编译器对未接线符号的提示强度；不能仅凭“可编译、可导出”推断某项能力已被生产调用。

## 核心职责

1. 确立 BR 日志工具的 crate 边界，把上下文 logger、结构化字段、速率追踪和迁移期依赖桩组合成一个可被其他 BR crate 依赖的库。
2. 提供稳定的根级导入面。调用方可以直接写 `use astersql_br_pkg_logutil::{Field, ShortError, log}`，无需知道符号实际位于 `logging.rs`；`br/pkg/utils`、`br/pkg/conn/util`、`br/pkg/rtree` 和 `br/pkg/metautil` 已采用这种方式。
3. 保留模块路径作为扩展面。四个生产模块都是 `pub mod`，所以未被根级列表重导出的公开符号仍可通过 `context::...`、`logging::...`、`rate::...` 或 `stubs::...` 访问；例如测试直接使用 `crate::logging::ArrayEncoder` 和 `crate::rate::RateTracer`。
4. 明确当前迁移边界。`Cargo.toml` 注释说明该 crate 为 darwin arm64 瘦身，未直接接入完整 `kv`、`domain`、`kvproto`、`grpcio` 和 `util-redact`，而由 `stubs.rs` 提供所需形状；因此根级 `kvproto`、`KeyRange`、`KvKey` 是本地 stand-in，不是完整上游协议实现。
5. 把 Go/Rust 公开契约测试纳入 crate 测试构建。`parity_test.rs::go_rust_public_contract_matches` 从根级 API 同时覆盖字段格式、上下文、速率、脱敏和备份元数据；`logging_test.rs::test_root_exports_cover_go_public_helpers` 专门检查 Go 公共 helper 能从 crate 根调用。

## 主要符号

`lib.rs` 没有定义常量、结构体、trait、函数或 `impl`；其主要符号是模块声明和选择性重导出：

- `pub mod context` 与 `pub use context::{CL, Context, ContextWithField, LoggerFromContext, ResetGlobalLogger}`：公开轻量上下文及 logger 的绑定、查找和全局回退控制。实际三级查找顺序是 Context 内 logger、`GLOBAL_LOGGER`、`default_logger()`（`context.rs::LoggerFromContext`）。
- `pub mod logging` 与显式 `pub use logging::{...}`：根级暴露 `Field`、`Logger`、`Level`、`log`，对象/数组 marshaler trait，File/Region/SST/RewriteRule 字段构造器，错误、key、区间、直方图和脱敏 helper。`EncodedValue`、`ObjectEncoder`、`ArrayEncoder`、`CapturedLog`、`LevelGuard`、`default_logger` 等公开实现细节没有被根级列表提升，但仍可沿 `logging` 模块访问。
- `pub mod rate` 与 `pub use rate::{RateTracer, TraceRateOver}`：公开基于 Prometheus `Counter` 的平均速率追踪器及构造函数。`RateTracer::{Inc, Add, Rate, RateAt, L}` 由结构体固有方法自然随类型可用。
- `pub mod stubs`、`pub use stubs::kvproto`、`pub use stubs::{KeyRange, KvKey}`：既公开完整 `stubs` 模块路径，又为最常用的协议命名空间与 key range stand-in 提供根级短路径。
- `parity_test`、`logging_test`：`cfg(test)` 私有模块，不属于依赖方 API。它们从根导入符号，因而能够捕获漏掉根级重导出的兼容回归。

根级导出是显式白名单而非 `pub use ...::*`。新增子模块公开项不会自动成为根级 API；这能控制兼容面，但要求维护者同步判断是否应更新 `lib.rs` 和根导出测试。

## 执行流程

`lib.rs` 没有 `main`、初始化函数或运行时控制流；它在编译期形成以下典型调用链：

1. 上游 crate 在 manifest 中依赖 `astersql-br-pkg-logutil`，并从根导入所需符号。例如 `br/pkg/utils/worker.rs` 导入 `Field`、`ShortError`、`log`，`br/pkg/rtree/logging.rs` 导入 `AbbreviatedStringers` 与 `Field`。
2. 普通日志路径调用 `log::L()` 或上下文路径 `CL`/`LoggerFromContext` 得到 `Logger`，以 `Field` 及 File/Region/Key 等 helper 构造结构化字段；`logging.rs::Logger::log` 根据全局 `LOG_LEVEL` 过滤，再发送到 `tracing` 或测试 Capture 后端。
3. 上下文路径中，`ContextWithField` 先通过 `LoggerFromContext` 解析当前 logger，再调用 `Logger::With` 合并字段并返回持有派生 logger 的新 `Context`。旧 Context 不会因之后的 `ResetGlobalLogger` 而改变。
4. 元数据字段路径由 `File`、`Files`、`Region`、`SSTMeta`、`RewriteRule` 等构造 `Field`；对象/数组 marshaler 将本地 kvproto stand-in 的 getter 值编码成有序 JSON 形状。key 与敏感值先经过 `stubs::{Key, Value, NeedRedact}`。
5. 速率路径由 `TraceRateOver` 读取 counter 当前值作为基线并记录 `Instant`；调用方用 `Inc`/`Add` 更新同一 Prometheus counter，`RateAt` 计算 `(current - base) / elapsed_seconds`，`L` 生成带 `speed` 字段的 logger。
6. 测试构建额外编译两份独立测试模块。`parity_test` 走根级 API 的综合链，`logging_test` 逐项核对 Go 金标准及边界；生产构建不包含这两个模块。

## 数据与状态

crate 根不持有运行时状态。被它公开的实现中有三类共享状态：

- `context.rs::GLOBAL_LOGGER` 是 `LazyLock<RwLock<Option<Logger>>>`。`ResetGlobalLogger` 替换无 logger Context 的回退目标；`Context` 自身只保存一个可选 `Logger`，克隆时复制 logger 句柄。
- `logging.rs::LOG_LEVEL` 与 `LOGGER_TO_TERM` 分别以 `RwLock` 保存全局级别和 `WarnTerm` 的可选终端 logger。Capture `Logger` 以 `Arc<Mutex<Vec<CapturedLog>>>` 保存日志，派生 `Logger::With` 共享 backend 但复制预置字段列表。
- `stubs.rs::NEED_REDACT` 是 `AtomicBool`，测试 helper 以 `SeqCst` 切换并让 `Redact`、`RedactAny`、key/value 编码读取。该开关是进程级状态，不是 Context 局部策略。

`RateTracer` 持有创建时刻 `start`、浮点基线 `base` 和 `Option<Counter>`。Counter 由 `prometheus` 类型内部共享；克隆 tracer 会指向同一 counter。`counter == None` 时 `Inc`/`Add` 静默无操作、`RateAt` 返回 `NaN`。`Field`/`EncodedValue` 则是拥有数据的值类型，对象字段保存有序键值列表，以维持与 Go 测试期望一致的输出顺序。

根级 `kvproto`、`KeyRange`、`KvKey` 所承载的数据仅覆盖日志格式化需要的字段和 getter/setter 形状。它们没有 protobuf 编解码、网络传输或完整 TiDB key 行为；跨 crate 再导出（如 `br/pkg/utils/stubs.rs`）不会改变这一限制。

## 依赖与调用关系

`br/pkg/logutil/Cargo.toml` 的直接依赖为：`astersql-br-pkg-errors` 和 `astersql-errors`（错误文本测试/兼容）、`astersql-lightning-metric`（读取 Prometheus counter/histogram）、`prometheus`（`Counter`、`Histogram`）、`tracing`（生产日志后端）及 `uuid`（SST UUID 格式化）。manifest 没有 feature 声明；`uuid` 仅启用 `v4` feature。`astersql-br-pkg-errors` 同时列为 dev-dependency，供独立测试构造 BR 错误。

仓库内直接 Cargo 上游至少包括：

- `br/pkg/utils/Cargo.toml`：多数 Rust 工具文件使用根级 `Field`、`log`、`ShortError`、`StringifyRange`/`StringifyKeys`，并再导出 `KeyRange`、`KvKey`。
- `br/pkg/conn/util/Cargo.toml`：`util.rs` 使用 `Field`、`Level`、`ShortError`、`log`。
- `br/pkg/rtree/Cargo.toml`：`logging.rs` 使用 `AbbreviatedStringers` 与 `Field`。
- `br/pkg/metautil/Cargo.toml`：`metafile.rs` 使用 `Field` 与 `log`。
- `br/pkg/summary/Cargo.toml`：声明对本 crate 的路径依赖；本次精确 Rust 导入搜索未发现该目录直接写出 crate 名，故不能据 manifest 单独断言具体符号调用链。

RustCodeGraph 对 `lib.rs` 的文件节点只报告 `tools/tazel/parity_test.rs` 直接使用，这反映文件级/build 元数据关系，不能完整表达 Rust `use` 的 crate 根消费。实际运行侧证据来自上述 Cargo 依赖和 `rg` 找到的根级导入。反向看，`lib.rs` 本身没有函数 caller/callee；它通过模块装配间接把 `context.rs`、`logging.rs`、`rate.rs`、`stubs.rs` 纳入依赖图。

## 错误处理与边界

- crate 根不包装、吞掉或转换错误。日志 API 多为无返回值的观察性操作；锁中毒时 `context.rs`/`logging.rs` 的 `expect(...)` 会 panic，而不是返回业务错误。
- `ShortError(None)`/`AShortError(..., None)` 返回 skip 字段，编码时不输出 `null`；有错误时只使用 `Display` 文本，不展开额外 cause 结构。相关契约由 `logging_test.rs::test_short_error` 和 Go `logging_test.go::TestShortError` 覆盖。
- `RateTracer::RateAt` 故意保留 Go 浮点边界：空 counter 返回 `NaN`；时间差为零且增量非零会得到无穷；传入早于 `start` 的时刻可得到负速率。`logging_test.rs::test_rater_go_time_boundaries` 明确验证零时长与负时长，扩展时不能“修正”为错误或 clamp 而不评估兼容性。
- `RedactAny` 在脱敏开启时统一输出字符串 `"?"`，关闭时保留 bool、数字、数组等 JSON 类型；`Redact` 则保留原 key 并替换整个值。脱敏由本地进程级桩控制，不应描述成已经接入完整 `util-redact` 配置链。
- `MarshalHistogram(None)` 或无法读到 histogram 数据时写出空对象内容而不报错。UUID 字节不合法时 SST 格式化输出可诊断文本，而不是让日志路径失败。
- `stubs::kvproto` 是边界最大的兼容风险：它只模拟日志读取所需的协议字段，不可用于持久化、RPC 或 wire compatibility。未来替换真实依赖必须修改实现和 Cargo 接线，不能只调整根级 re-export。
- 显式重导出列表是 API 边界。删除、改名或漏导出符号会让现有根级 `use` 编译失败；仅新增 `pub mod` 内符号不会自动满足 Go 包级 API 对齐要求。

## 并发与资源生命周期

`lib.rs` 在编译期接线，不启动线程、任务、通道或事务，也不拥有文件和网络资源。其公共能力的生命周期约束集中在共享 logger 状态与指标句柄：

- `GLOBAL_LOGGER`、`LOG_LEVEL`、`LOGGER_TO_TERM` 使用读写锁串行化更新；读取时克隆 `Logger` 后释放锁。锁中毒是 panic 边界。`Logger::With` 不修改原 logger，因此 Context 派生字段不会反向污染父 Context。
- Capture backend 的 `Arc<Mutex<Vec<CapturedLog>>>` 可在 logger 克隆之间共享；每次写入持锁追加。高频测试/诊断捕获可能产生锁竞争和无界 Vec 增长，生产默认 backend 不使用该缓冲。
- `OverrideLevelForTest` 返回 `LevelGuard`，其 `Drop` 恢复旧级别；这是必须保持的 RAII 清理路径。测试也在结束时调用 `ResetGlobalLogger(None)`，避免进程级状态污染其他用例。
- `NEED_REDACT` 用 `SeqCst` 保证跨线程可见，但开关仍是全局的；并行测试切换它时必须串行协调或使用可恢复 guard，否则会影响其他日志编码。
- `RateTracer` 不持有后台采样任务；速率只在 `Rate`/`RateAt` 调用时读取 counter。它的时间基线与 counter 生命周期随值/内部共享句柄存在，不需要显式关闭。

## 与 Go 版本的对应关系

Go 包没有与 Rust `lib.rs` 一一对应的入口文件；Rust crate 根覆盖同目录多个 Go 文件的包级命名空间：

- `context.rs` 对应 `context.go`：`ResetGlobalLogger`、`ContextWithField`、`LoggerFromContext`、`CL` 的回退和字段继承意图一致。Rust 使用自定义轻量 `Context { logger: Option<Logger> }`，不具备 Go `context.Context` 的取消、deadline 或任意 value 传播。
- `logging.rs` 对应 `logging.go`：公开 helper 集合覆盖缩略数组、File/Files、StreamBackupTaskInfo、RewriteRule、Region/Peer、SST、key、短错误、WarnTerm、脱敏、区间、HexBytes 和 Histogram。Rust 用本地 `Field`/`EncodedValue` 加 `tracing` 模拟 Go 的 zap 字段与 logger，并非 zap 的二进制或扩展接口兼容实现。
- `rate.rs` 对应 `rate.go`：构造时忽略 counter 已有值，随后按当前 counter 增量除以经过秒数。Rust 额外用 `Option<Counter>` 表达 Go nil interface，并允许测试直接构造 `RateTracer` 注入 `Instant`。
- `stubs.rs` 没有等价 Go 生产文件；它替代 Go 直接依赖的 kvproto、`kv.KeyRange` 与 redact 包，是当前 arm64/迁移期局部接线。其存在说明 Rust 实现尚未使用完整 Go 依赖图。

Go `logging_test.go` 的 `TestRater`、`TestFile(s)`、`TestKey(s)`、`TestRewriteRule`、`TestRegion`、`TestLeader`、`TestSSTMeta`、`TestShortError`、`TestContextual` 已在独立 `logging_test.rs` 中有相应测试意图。Rust 还增加零/负时长、`RedactAny` 类型保持、Histogram 键格式和根导出覆盖。`parity_test.rs::go_rust_public_contract_matches` 以一条综合测试再次穿过根门面，但它不能替代各模块细粒度测试。

## 扩展指南

- 新增生产模块时，在本文件增加清晰的 `#[path = "..."] pub mod ...`；新增测试放在独立 `*_test.rs`，以 `#[cfg(test)]` 私有挂载，不能把测试逻辑嵌入 `lib.rs` 或其他生产文件。
- 新增 Go 包级公共 helper 时，先在对应实现文件完成真实语义，再判断是否加入本文件的显式 `pub use` 列表；同步扩展 `logging_test.rs::test_root_exports_cover_go_public_helpers` 或 `parity_test.rs`，防止实现存在但根 API 缺失。
- 修改根级导出前，用 Cargo manifest 和全仓 `use astersql_br_pkg_logutil` 搜索评估兼容面。尤其不要随意移除 `Field`、`log`、`ShortError`、`KeyRange`、`KvKey`、`StringifyRange` 等已有直接消费者的符号。
- 修改 Context/global logger 行为时，同步对照 `context.go`、`logging_test.go::TestContextual`、`logging_test.rs::test_contextual` 与 parity 测试，覆盖新 Context、已绑定 Context、全局 reset、字段顺序和测试清理。
- 修改速率公式时，同步 `rate.go`、Go `TestRater`、Rust `test_rater` 与 `test_rater_go_time_boundaries`；兼容风险包括 nil/NaN、零时长无穷、负时间和 counter 当前值语义，性能风险主要是高频 metric 读取。
- 修改字段/脱敏格式时同步 Go 金标准和两份 Rust 测试，重点检查 JSON 类型、字段顺序、十六进制大小写、长数组缩略阈值、空列表聚合及敏感信息泄漏。
- 若要替换 `stubs` 为真实 kvproto/kv/redact 依赖，应按仓库规则在对应上游 Rust 依赖完成移植并使用发布 tag 接入，同时验证所有 manifest 使用同一 tag；不能把依赖复制到 vendor/third_party，也不能把本地 stand-in 的 getter 形状误当 wire compatibility。该变化超出只改根门面的范围。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/logutil` 列出四个生产实现、crate 根、两份 Rust 测试及三个 Go 对照文件。
- RustCodeGraph：`node --file br/pkg/logutil/lib.rs --offset 1 --limit 180` 核对 57 行 crate 根、四个公开模块、显式重导出和两个 `cfg(test)` 测试模块；文件节点报告 `tools/tazel/parity_test.rs` 的 build/parity 使用关系。
- RustCodeGraph：`query RateTracer --kind struct`、`query TraceRateOver --kind function`、`query LoggerFromContext --kind function`、`query ShortError --kind function`、`query RedactAny --kind function` 同时定位 Go/Rust 对照符号，确认实现分别位于 `rate.rs`、`context.rs`、`logging.rs`。
- Cargo/上游：读取 `br/pkg/logutil/Cargo.toml`、根 `Cargo.toml` 及 `br/pkg/{summary,utils,rtree,metautil}/Cargo.toml`、`br/pkg/conn/util/Cargo.toml`；用精确搜索核对 `utils`、`conn/util`、`rtree`、`metautil` 的根级 Rust 导入。
- 实现证据：读取 `context.rs`、`rate.rs`、`logging.rs` 与 `stubs.rs` 的模块说明、状态定义和关键路径，核对 logger 回退、锁/原子状态、字段编码、脱敏、指标读取及本地协议桩限制。
- Go 对照：读取 `context.go`、`rate.go`、`logging.go`，并以公开符号搜索核对 Go 包级 helper；读取 `logging_test.go` 的测试清单与断言意图。
- 独立 Rust 测试：读取 `parity_test.rs` 与 `logging_test.rs`；确认根 API 综合测试、逐 helper 金标准、Context 生命周期、速率边界、脱敏值类型、Histogram 格式和根级导出均在生产文件外验证。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证命令及退出码在交付时单独报告。
