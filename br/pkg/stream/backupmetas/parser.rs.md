# `br/pkg/stream/backupmetas/parser.rs`

## 文件定位

[`parser.rs`](./parser.rs) 是 `astersql-br-pkg-stream-backupmetas` crate 的实际解析实现。crate 根 [`lib.rs`](./lib.rs) 通过 `#[path = "parser.rs"] pub mod parser` 装入本文件并 `pub use parser::*`，因此这里的公开类型、常量和函数同时构成 crate 的公开接口。该 crate 的 [`Cargo.toml`](./Cargo.toml) 将 Go 包映射到 `br/pkg/stream/backupmetas`，库入口为 `lib.rs`，运行时仅直接依赖 `regex`。

本文件位于 BR 日志备份/PiTR 链路：它不读取对象存储或 protobuf 内容，只把 backupmeta 对象的文件名转换为时间范围、store 标识和 flags。上层再据此筛选 checkpoint 元文件、计算恢复窗口的 shift TS、跳过空 meta，以及判断是否需要加载 DDL 文件。它不是门面、生成文件或桩；对应门面是同目录 `lib.rs`。

## 核心职责

1. `ParseName` 自动识别并解析两代文件名协议：旧格式 `flushTs-minBegin-minTs-maxTs`，以及 tagged 格式 `flushTs||storeID-(tag||value)+`。
2. `TryParseTaggedBackupMetaFileName` 为只允许新格式的调用链提供严格入口，防止 legacy 名称被最新元数据路径误接收。
3. `ParsedName::CalculateShiftTS` 根据文件覆盖区间与恢复窗口判断是否可采用 DefaultCF 的最小 begin TS。
4. `ParsedName::IsEmpty` 和 `HasDDLFiles` 解释可选 flags；必须区分“没有 `p` 标签”和“存在且值为 0”，所以 `HasFlags` 不能由 `Flags == 0` 推导。
5. tagged 后缀拒绝重复标签、缺失必需标签及非法编码，同时忽略合法但未知的字母数字标签，保留协议前向扩展能力。

本文件只负责纯解析和派生判断，不验证 `FlushTS`、`MinTS`、`MaxTS` 之间所有业务次序，也不访问网络、磁盘、全局配置或备份内容。

## 主要符号

- `LEGACY_BACKUP_META_PART_COUNT = 4`：legacy 格式的固定字段数。
- `TAGGED_META_TAG_VALUE_LEN = 17`：一个 tagged 段由 1 字节 ASCII tag 和 16 位十六进制值组成。
- `NAME_MIN_BEGIN_TS_IN_DEFAULT_CF_TAG`（`d`）、`NAME_MIN_TS_TAG`（`l`）、`NAME_MAX_TS_TAG`（`u`）、`NAME_FLAGS_TAG`（`p`）：公开协议标签。前三者在 tagged 名称中必需，`p` 可选。
- `FLAG_NO_DDL_FILES`、`FLAG_EMPTY`：flags 的第 0、1 位；前者表示不含 DDL，后者表示空 meta。
- `LEGACY_BACKUP_META_PATTERN`、`TAGGED_BACKUP_META_PATTERN`：`LazyLock<regex::Regex>`。第一次调用解析入口时编译；固定字面量若编译失败会因 `expect` panic，这与 Go 的 `regexp.MustCompile` 初始化失败语义一致。
- `ParsedName`：解析结果。`FlushTS`、`StoreID`、`MinBeginTsInDefaultCf`、`MinTS`、`MaxTS`、`Flags` 均为 `u64`；`HasFlags` 记录 `p` 标签是否实际出现。该类型实现 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`。
- `ShiftTSStatus`：`#[repr(u8)]` 的三态枚举，数值依次为 `ShiftTSFound = 0`、`ShiftTSNotFound = 1`、`ShiftTSInvalidStats = 2`，与 Go `iota` 顺序一致。
- `ParseName(&str) -> Result<ParsedName, String>`：公共自动识别入口，优先匹配 tagged，再匹配 legacy。
- `TryParseTaggedBackupMetaFileName(&str) -> Result<ParsedName, String>`：公共 tagged-only 入口。
- `ParsedName::CalculateShiftTS(&self, startTS, restoreTS)`：返回 `(shift_ts, status)`；失败状态的 TS 固定为 0。
- `ParsedName::IsEmpty`、`ParsedName::HasDDLFiles`：flags 解释器。
- `parseLegacyBackupMetaFileName`、`parseTaggedBackupMetaFileName`、`parseBackupMetaHexU64`、`isASCIIAlphanumeric`：私有解析与校验辅助函数。

## 执行流程

`ParseName` 的主流程如下：

1. 先用 `TAGGED_BACKUP_META_PATTERN` 检查整个输入。该格式要求 32 位十六进制前缀、一个连字符，以及至少一个连续的 `tag + 16 hex` 段。
2. tagged 匹配时调用 `parseTaggedBackupMetaFileName`：以第一个 `-` 分离前后缀；前 16 位解析为 `FlushTS`，后 16 位解析为 `StoreID`。
3. 后缀按 17 字节步长扫描。每段验证 tag 是 ASCII 字母或数字、值恰为 16 位十六进制、同一 tag 未出现过；`d/l/u/p` 写入对应字段，其他合法 tag 只登记为已见而不改变结果。
4. 扫描完成后确认 `d`、`l`、`u` 均出现；`p` 出现时同时设置 `HasFlags = true`。成功后返回完整 `ParsedName`。
5. 若 tagged 不匹配，则尝试 `LEGACY_BACKUP_META_PATTERN`。`parseLegacyBackupMetaFileName` 再防御性检查四段，然后按次序解析 `FlushTS`、`MinBeginTsInDefaultCf`、`MinTS`、`MaxTS`；`StoreID`、`Flags`、`HasFlags` 采用默认值。
6. 两种正则均不匹配时返回带原文件名的格式错误。

`CalculateShiftTS` 先做窗口相交检查：当 `MinTS > restoreTS` 或 `MaxTS < startTS` 时返回 `ShiftTSNotFound`。窗口相交后，若 `MinBeginTsInDefaultCf == 0` 或大于 `MinTS`，返回 `ShiftTSInvalidStats`；否则返回该 begin TS 和 `ShiftTSFound`。边界使用严格大于/小于，因此端点相等视为相交。

## 数据与状态

解析过程没有可变全局业务状态。两个正则由 `LazyLock` 缓存，首次使用后只读共享；每次解析的字段、`seenTags: [bool; 256]` 和扫描位置均为栈上局部状态。

tagged 名称的关键不变量是：前缀恰好编码两个 `u64`；每个后缀段长度恰为 17；任意 tag 最多出现一次；`d/l/u` 必须出现；`p` 可缺省。未知 tag 仍进入 `seenTags`，所以同一个未知扩展 tag 重复也会报错。正则先保证后缀总形状，函数内的长度、字符和十六进制检查则提供防御性错误上下文。

`HasFlags` 是协议状态而不是冗余缓存：legacy 或无 `p` 的 tagged 名称会得到 `Flags = 0, HasFlags = false`；显式 `p000...000` 会得到 `Flags = 0, HasFlags = true`。当前 `HasDDLFiles` 对这两种情况都返回 true，但 `IsEmpty` 要求标签确实存在后才解释空标志位。

## 依赖与调用关系

下游依赖很小：`regex::Regex` 负责整体格式预筛选，标准库 `u64::from_str_radix` 负责十六进制转换，`LazyLock` 负责正则的一次性初始化。错误使用 `String`，没有依赖 BR 的统一错误类型。

RustCodeGraph 对 `parser.rs::ParseName` 给出的直接被调入口包括：

- [`br/pkg/stream/crr/internal/checkpoint/storage.rs`](../crr/internal/checkpoint/storage.rs) 的 `collect_meta_files`：从路径取 basename 后解析，提取 flush/store/empty 信息；解析失败会进入该扫描路径的错误处理。
- [`br/pkg/stream/stream_mgr.rs`](../stream_mgr.rs)：解析 backupmeta 名称以获得元信息。
- [`parity_test.rs`](./parity_test.rs) 的五个契约测试，以及 [`stream_mgr_fuzz_test.rs`](../stream_mgr_fuzz_test.rs) 的确定性 fuzz 对齐入口。

tagged-only 路径由 [`stream_metas.rs`](../stream_metas.rs) 的 `TryParseTaggedBackupMetaFileNameWrapper` 去除目录后调用；`UpdateShiftTS` 再调用 `CalculateShiftTS`。该 wrapper 还被 [`br/pkg/restore/log_client/log_file_manager.rs`](../../restore/log_client/log_file_manager.rs) 使用，用于空 meta 过滤和恢复窗口 shift TS 计算。Go 同名链路还在 `log_file_manager.go` 中用 `HasDDLFiles` 决定是否加载 DDL 文件；Rust 当前直接证据显示 `log_file_manager.rs` 已使用 `IsEmpty` 和 `CalculateShiftTS`，不能仅据 Go 调用宣称所有 Go 消费点都已完整迁移。

## 错误处理与边界

所有可恢复解析失败均返回 `Result::Err(String)`，错误文本携带原文件名；具体分支区分总格式非法、latest/tagged 格式非法、legacy 段数不符、tagged 前缀长度不符、段不完整、tag 非 ASCII 字母数字、十六进制解析失败、重复 tag、缺少必需 tag。`TryParseTaggedBackupMetaFileName` 会明确拒绝格式正确的 legacy 名称。

输入只接受 ASCII 十六进制；大小写均可。tag 可为 ASCII 数字或大小写字母，协议保留 tag 仅匹配小写 `d/l/u/p`，因此大写同名字母属于未知扩展 tag。未知合法 tag 被忽略但不能重复，这一行为由 `tagged_validation_and_forward_compatibility_match_go` 覆盖。

本文件不会主动拒绝 `startTS > restoreTS`、`MinTS > MaxTS`、`FlushTS` 与区间不一致等更高层业务异常；`CalculateShiftTS` 只实现 Go 版本已有的窗口与 DefaultCF 统计条件。唯一 panic 风险是内置正则字面量无法编译，属于开发期常量错误而非用户输入错误。

## 并发与资源生命周期

所有公开操作都是同步、无 I/O、无锁的纯计算；`ParsedName` 不持有借用、句柄、任务、通道或事务。调用返回后，输入 `&str` 不被结果引用，结果可以独立移动或克隆。

两个 `LazyLock<Regex>` 的初始化由标准库保证线程安全：首次并发解析只会完成一次初始化，随后各线程共享不可变正则。除这次惰性编译外，每次调用仅分配错误字符串；legacy 路径还会为 `split('-').collect::<Vec<_>>()` 分配一个小向量，tagged 路径按后缀长度线性扫描。函数没有取消、超时或显式清理需求。

## 与 Go 版本的对应关系

直接对照文件是 [`parser.go`](./parser.go)。Rust 保留了 Go 的公开命名、字段顺序、两种正则、tag/flag 数值、解析优先级、`CalculateShiftTS` 分支、flags 缺省语义、必需标签检查和未知标签前向兼容行为。`ShiftTSStatus` 的 `repr(u8)` 与 Go 的 `type ShiftTSStatus uint8` 及 `iota` 数值对齐。

实现层面的语言差异包括：Go 返回 `errors.Errorf/Annotatef`，Rust 返回普通 `String`；Go 方法接收 `*ParsedName`，Rust 使用不可变 `&self`；Go 的包级 `regexp.MustCompile` 对应 Rust 的 `LazyLock<Regex>` 加 `expect`；Go 用 `strings.Cut`，Rust用 `split_once`；Go 的 `[256]bool` 以 byte 索引，Rust同样以 `u8 as usize` 索引。

独立 Rust 测试 [`parity_test.rs`](./parity_test.rs) 验证 public contract、窗口端点和非法统计、flags 存在性、重复/缺失/未知 tag 及空标志组合；[`stream_mgr_fuzz_test.rs`](../stream_mgr_fuzz_test.rs) 对照 Go [`stream_mgr_fuzz_test.go`](../stream_mgr_fuzz_test.go) 覆盖多组 `u64`、大小写十六进制、额外 tag、flags 和缺标签错误。checkpoint 的组合接线还由 [`crr/internal/checkpoint/storage_internal_test.rs`](../crr/internal/checkpoint/storage_internal_test.rs) 使用真实风格长文件名覆盖。

## 扩展指南

- 新增 tagged 字段时，应先分配不与 `d/l/u/p` 冲突的 ASCII 字母数字 tag，在 `ParsedName` 增加字段，并在 `parseTaggedBackupMetaFileName` 的 `match` 中赋值；若字段为可选，需明确“缺失”和“值为零”是否需要类似 `HasFlags` 的存在位。
- 若新增必需 tag，必须同步必需标签检查、Go `parser.go`、文件名生产方和兼容策略。直接将新 tag 设为必需会使旧文件无法解析，属于备份格式兼容性变更。
- 修改 flags 时应保留未知位，不要把完整 flags 收窄为布尔值；同步扩展 `IsEmpty`/`HasDDLFiles` 或新增独立查询方法，并覆盖“标签缺失、显式零、组合位、未知位”。
- 修改 shift TS 规则时，应同时检查 `stream_metas.rs::UpdateShiftTS`、restore `log_file_manager.rs` 的消费方式及 Go `stream_metas_test.go::TestCalculateShiftTS` 的意图，尤其关注闭区间端点和非法统计与“不相交”的优先级。
- 测试逻辑应继续放在独立文件，不嵌入 `parser.rs`。最直接的同步位置是 `parity_test.rs`；跨 crate 接线用 `stream_mgr_fuzz_test.rs`、`stream_misc_test.rs` 或 checkpoint 的 `storage_internal_test.rs`。
- 性能修改需保持整体校验与重复标签检测。该解析器可能用于对象列表扫描；应避免按 tag 数量做二次扫描或引入 I/O，同时用基准或代表性长后缀证明收益。

## 验证依据

- 源码与 crate 边界：`br/pkg/stream/backupmetas/parser.rs`、`lib.rs`、`Cargo.toml`。
- Go 语义对照：`br/pkg/stream/backupmetas/parser.go`；上层 Go 消费证据来自 `br/pkg/stream/stream_metas.go`、`br/pkg/restore/log_client/log_file_manager.go`、`br/pkg/stream/crr/internal/checkpoint/storage.go`。
- Rust 调用与测试证据：`br/pkg/stream/stream_metas.rs`、`br/pkg/stream/stream_mgr.rs`、`br/pkg/restore/log_client/log_file_manager.rs`、`br/pkg/stream/crr/internal/checkpoint/storage.rs`、`parity_test.rs`、`stream_mgr_fuzz_test.rs`、`crr/internal/checkpoint/storage_internal_test.rs`。
- RustCodeGraph：索引状态为 7032 个 Rust 文件；`files --filter br/pkg/stream/backupmetas` 确认 `lib.rs`、`parser.rs`、`parity_test.rs` 和 Go 对照文件；`query`/`node parser.rs::ParseName` 确认签名、对 `parseTaggedBackupMetaFileName` 与 `parseLegacyBackupMetaFileName` 的调用，以及 parity/fuzz 测试调用者；`explore` 还确认 checkpoint、stream wrapper、restore manager 的上游链路。单独 `callers` 查询在本机 30 秒内未返回输出，因此未把其作为唯一证据。
- 本任务为纯文档分析，按任务约束未运行 Cargo。交付前使用任务指定命令确认目标文档存在且恰有 11 个固定二级章节，并人工复核文档只陈述上述代码与测试能支持的当前事实。
