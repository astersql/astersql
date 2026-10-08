# `pkg/util/timeutil/time_zone.rs`

## 文件定位

本文件是 `astersql-util-timeutil` crate 的时区实现，模块入口由 `pkg/util/timeutil/lib.rs` 的 `pub mod time_zone` 暴露。crate 在 `pkg/util/timeutil/Cargo.toml` 中声明为 `astersql-util-timeutil`，时区实现直接依赖 `chrono = 0.4.45` 和 `chrono-tz = 0.10`；`tokio`/`tokio-util` 属于同 crate 的时间辅助功能，并非本文件的执行依赖。

它位于 SQL 会话与后台调度共同使用的基础工具层：生产代码中，`pkg/session/runtime.rs::parse_stale_datetime_micros` 通过 `SystemLocation` 和 `Zone` 把无时区 SQL 时间解释为系统墙钟时间；`pkg/statistics/handle/autoanalyze/exec/exec.rs::CheckAutoAnalyzeWindow` 以及 `pkg/session/runtime/ttl_runtime.rs` 的 TTL 窗口判断通过 `WithinDayTimePeriod` 复用日内窗口语义。RustCodeGraph 显示该文件被 64 个文件引用；其中也包含测试和注释引用，因此具体生产链以这些已核对调用点为准。

对应的 Go 基准实现是 `pkg/util/timeutil/time_zone.go`，独立 Rust 测试位于 `pkg/util/timeutil/time_zone_test.rs`，迁移补充测试位于 `pkg/util/timeutil/migration_aster_unit_test.rs`，Go 对照测试位于 `pkg/util/timeutil/time_zone_test.go`。

## 核心职责

- 用 `Location` 统一表示 IANA 命名时区、固定 UTC 偏移和进程本地时区，并提供名称、指定 UTC 时刻偏移和本地墙钟时间转 UTC 的操作。
- 推断、保存和读取进程级系统时区：`InferSystemTZ` 从 `TZ` 或 `/etc/localtime` 推断名称，`SetSystemTZ` 只允许首次写入，`SystemLocation` 将保存的名称解析为可用位置。
- 通过 `location_cache` 缓存 IANA 时区解析结果，避免重复解析 `chrono_tz::Tz`。
- 在 SQL/MySQL 边界解析 `SYSTEM`、IANA 名和 `+/-HH:MM[:SS]` 固定偏移，并产生兼容的未知时区错误。
- 为调度器提供忽略日期和秒、支持跨午夜的闭区间判断 `WithinDayTimePeriod`。
- 为 zoneinfo 软链接提供操作系统探测辅助：单步读取软链接并从路径中提取 IANA 名称。

## 主要符号

- `pub enum Location { Named(Tz), Fixed { name, offset }, Local }`：本模块的核心值类型。`Named` 保留 IANA/DST 规则，`Fixed` 保留显示名和固定秒偏移，`Local` 延迟使用 `chrono::Local` 的进程本地规则。
- `Location::utc()`、`Location::fixed(name, seconds_east_of_utc)`：分别构造 UTC 和固定偏移；后者通过 `FixedOffset::east_opt` 拒绝 chrono 无法表示的偏移。
- `Location::String()`、`offset_at_utc(instant)`：返回可传播的时区名称，并计算某一 UTC 时刻的实际偏移。命名时区的偏移会随 DST 规则和时刻变化。
- `Location::local_datetime_to_utc(local)` 与私有 `resolve_local`：把无时区墙钟时间映射到 UTC；唯一映射直接返回，DST 回拨造成两个映射时确定性选择较早者，DST 跳时造成无映射时返回 `InvalidLocalTime`。
- `location_cache()`：`OnceLock<RwLock<HashMap<String, Location>>>` 形式的进程级名称缓存。
- `system_timezone()` 与 `SET_SYSTEM_TIMEZONE_ONCE`：保存系统时区名称的全局锁，以及约束 `SetSystemTZ` 只生效一次的 `Once`。
- `init()`：主动触发两个全局存储初始化；即使不显式调用，访问器也会惰性初始化。
- `infer_one_step_link_for_path(path)`：以 `symlink_metadata` 判断路径，只对软链接调用一次 `read_link`，不递归解析。
- `InferSystemTZ()`：按环境和系统文件推断名称；`TZ` 为合法、非空、非 `UTC` 名时直接采用，未设置时尝试 `/etc/localtime`，其余情况回退 `UTC`。
- `infer_tz_name_from_file_name(path)`：从包含 `zoneinfo.default/` 或 `zoneinfo/` 的路径截取后缀，否则返回 `UnsupportedZoneInfoPath`。
- `SystemLocation()`、`SetSystemTZ(name)`、`GetSystemTZ()`：系统时区的读取位置、一次性设置与名称读取 API。未正确设置时 `GetSystemTZ` 报错，而 `SystemLocation` 在加载失败时降级为 `Location::Local`。
- 私有 `load_named_location(name)` 与公开 `LoadLocation(name)`：前者解析 `Local` 或 `chrono_tz::Tz`，后者处理特殊占位 `System` 并增加缓存。
- `Zone(location)`、`ZoneName(location)`：取当前时刻的名称和偏移；`Local` 对外改名为 `System`，无名固定时区格式化为 `+/-HH:MM`。
- `ConstructTimeZone(name, offset)`：名称非空时名称优先且忽略 `offset`；空名时构造固定偏移。
- `WithinDayTimePeriod(start, end, now)`：转换至 UTC 后只比较小时和分钟，普通区间与跨午夜区间都包含端点。
- 私有 `parse_duration_seconds(value)` 与 `ParseTimeZone(value)`：解析 2 或 3 段持续时间，随后执行 MySQL 偏移上下界校验；负向最大 `-12:59`，正向最大 `+14:00`。

## 执行流程

系统时区启动链通常是：启动阶段调用 `InferSystemTZ`；若 `TZ` 存在且是可加载的非空、非 `UTC` 名称则返回该名称，若 `TZ` 未设置则解析 `/etc/localtime` 的目标路径，若目标涉及 `posixrules` 则仅读取一层链接后再提取 `zoneinfo` 后缀；无法得到名称时返回 `UTC`。调用方把结果传给 `SetSystemTZ`，第一次调用写入全局名称，后续调用因 `Once` 不再修改。`SystemLocation` 读取该名称，经 `LoadLocation` 解析；若名称无效，则回退进程 `Local`。

`LoadLocation` 先把精确字符串 `System` 直接映射到 `Location::Local`，再在读锁下查缓存。未命中时调用 `load_named_location`：`Local` 也是专用分支，其他字符串由 `chrono_tz::Tz::from_str` 解析。成功后在写锁下写回克隆值；失败不进入缓存。

`ParseTimeZone` 的优先级为：大小写不敏感的 `SYSTEM`；命名时区或 `Local`；带正负号的固定偏移；最终未知时区错误。固定偏移主体允许 `HH:MM` 或 `HH:MM:SS`，分钟和秒必须在 `0..60`，算术使用 checked 操作防止溢出。范围校验具有正负不对称的 MySQL 规则，成功后以空名称构造 `Location::Fixed`。

调用 `ZoneName` 时先执行 `Zone`，后者在 `Utc::now()` 时刻计算实际偏移，并把 `Local` 显示名规范为 `System`。若名称非空则原样返回；只有空名称固定时区才根据偏移生成可被 `ParseTimeZone` 理解的 `+/-HH:MM` 字符串。

`WithinDayTimePeriod` 把三个输入都转换到 UTC，仅提取小时和分钟。若结束分钟不早于开始分钟，使用 `start <= now <= end`；否则把区间解释为跨午夜，使用 `now <= end || now >= start`。日期、秒和更细粒度都不参与结果。

## 数据与状态

`Location` 是可克隆、可比较的拥有型值，不保存动态句柄。`Named(Tz)` 使用编译进程序的 chrono-tz 数据；`Fixed` 保存用户给定名称及 `FixedOffset`；`Local` 在求偏移或解析墙钟时间时读取进程本地规则。因此同一个 `Local` 值在不同宿主环境中可能产生不同结果。

名称缓存和系统时区名称均为进程级静态状态。缓存初始为空，生命周期与进程一致，没有淘汰策略；其键为调用者提供的原字符串，值为解析后的 `Location`。系统时区名称初值为 `System`，这表示尚未用真实名称完成初始化，而不是 IANA 时区名。

`SetSystemTZ` 的一次性状态同样持续整个进程，不能由普通 API 重置。这影响并行测试和需要不同 `TZ` 的场景，所以 `time_zone_test.rs::test_local` 通过子进程隔离每组环境；迁移测试还使用互斥锁串行化环境变量修改。

## 依赖与调用关系

向下依赖方面，`chrono` 提供 `DateTime`、`FixedOffset`、`LocalResult`、UTC/本地转换和小时分钟提取，`chrono-tz` 提供 IANA 名称解析与历史/DST 规则；标准库提供文件系统、环境变量、`HashMap`、`OnceLock`、`Once` 和 `RwLock`。错误统一来自 `crate::errors::{TimeUtilError, ErrUnknownTimeZone}`：普通解析/IO 错误使用具体枚举，SQL 时区输入最终失败使用 MySQL 错误类标记。

文件内部关键调用边包括：`ParseTimeZone -> SystemLocation -> system_timezone/LoadLocation`，`ParseTimeZone -> load_named_location/parse_duration_seconds/Location::fixed`，`SystemLocation -> LoadLocation`，`LoadLocation -> location_cache/load_named_location`，`ZoneName -> Zone -> Location::offset_at_utc`，以及 `Location::local_datetime_to_utc -> resolve_local`。RustCodeGraph 的 flow 查询直接确认了 `ParseTimeZone -> SystemLocation -> system_timezone` 链；其余边由同一索引中的文件节点源码核对。

向上调用方面，`pkg/session/runtime.rs::parse_stale_datetime_micros` 读取 `SystemLocation` 当前偏移修正 stale-read 时间；`pkg/statistics/handle/autoanalyze/exec/exec.rs::CheckAutoAnalyzeWindow` 调用 `WithinDayTimePeriod` 判断自动分析窗口；`pkg/session/runtime/ttl_runtime.rs` 的 TTL 调度窗口也导入该函数。`pkg/util/timeutil/lib.rs` 把模块公开给整个 workspace，测试模块通过独立 `*_test.rs` 文件装配，符合源码与测试分离要求。

## 错误处理与边界

- 文件系统错误由 `infer_one_step_link_for_path` 转成带操作名和路径的 `TimeUtilError::Io`；`InferSystemTZ` 多数探测失败被视为可恢复并回退 `UTC`，但 `posixrules` 分支的一层链接读取失败会返回空字符串，这一点与一般回退路径不同。
- `infer_tz_name_from_file_name` 仅识别 `zoneinfo.default` 和 `zoneinfo` 标记，不验证截取结果是否真是已知 IANA 名；后续加载阶段才负责名称有效性。
- `GetSystemTZ` 将空串或占位 `System` 视为未初始化错误；`SystemLocation` 则面向运行路径容错，任何加载失败都回退 `Local`。
- `LoadLocation` 对未知名称返回 `InvalidTimeZoneName`，而 `ParseTimeZone` 在命名解析和固定偏移解析均失败后返回 `UnknownTimeZone`。调用方若依赖 MySQL 错误分类，应使用后者。
- `ParseTimeZone` 接受 `-6:00` 这类非两位小时；拒绝缺字段、空字段、非法数字、分钟/秒达到 60、算术溢出以及超出 `[-12:59,+14:00]` 的值。正向 `+14:00:01` 也因总秒数越界失败。
- `ZoneName` 只把空名称固定偏移保证格式化为可解析形式；显式名称会原样返回，调用者不应假设任意自定义固定时区名称都能被 `ParseTimeZone` 再解析。
- `local_datetime_to_utc` 对 DST 回拨歧义取较早实例，但对 DST 跳时的不存在墙钟时间报 `InvalidLocalTime`。当前 `errors.rs` 的说明把歧义也概括进该错误文本，但本实现不会在 `Ambiguous` 分支报错，应以代码行为为准。
- `WithinDayTimePeriod` 是分钟精度的闭区间；相同起止分钟只匹配该分钟，并不表示全天窗口。

## 并发与资源生命周期

`OnceLock` 保证两个全局锁只初始化一次；`RwLock` 允许已缓存位置和系统名称的并发读取。锁中毒时所有读写路径都通过 `poisoned.into_inner()` 继续使用底层值，选择可用性而非传播 panic。

`LoadLocation` 采用先读后写，但没有在获得写锁后再次检查键；并发冷启动可能重复解析同一名称并依次覆盖为等价值，结果正确但会有少量重复工作。缓存永不淘汰，理论内存占用与进程生命周期内不同有效名称的数量线性相关；IANA 名称集合有限，通常风险较低。

`SET_SYSTEM_TIMEZONE_ONCE` 与 `system_timezone` 分离：`Once` 保护写入时序，`RwLock` 保护字符串存取。第一次 `SetSystemTZ` 获胜，随后调用静默无效；启动代码必须保证首次值可信。环境变量和 `/etc/localtime` 只在 `InferSystemTZ` 调用时读取，没有后台刷新任务、线程、通道或事务资源。

所有 `Location` 操作都是同步计算；文件句柄由标准库调用内部短暂持有，函数返回前释放。本文件不启动异步任务，也不持有跨调用借用。

## 与 Go 版本的对应关系

整体结构对应 `pkg/util/timeutil/time_zone.go`：Rust `location_cache` 对应 Go `locCache`，`RwLock<HashMap<...>>` 对应 `sync.RWMutex + map`；`system_timezone` 和 `SET_SYSTEM_TIMEZONE_ONCE` 对应 `atomic.String` 与 `sync.Once`；`LoadLocation`、`Zone`、`ZoneName`、`ConstructTimeZone`、`WithinDayTimePeriod` 和 `ParseTimeZone` 保留相同分支意图。

重要一致点包括：`System` 映射进程本地时区；输出时把 `Local` 改名为 `System`；有名称时 `ConstructTimeZone` 忽略偏移以保留 DST；无名时固定偏移；日内窗口转 UTC 后按分钟、闭区间和跨午夜规则判断；MySQL 固定偏移范围为 `-12:59` 到 `+14:00`；最终解析错误使用 `ErrUnknownTimeZone` 类。

Rust 为承载 Go `*time.Location` 增加了显式 `Location` 枚举和 `local_datetime_to_utc`，并把 Go `time.LoadLocation` 替换为编译期 chrono-tz 数据。Go 固定偏移构造对任意 `int` 直接创建位置，Rust `Location::fixed` 还受 `FixedOffset::east_opt` 范围限制并可能返回 `InvalidOffset`。Go `types.ParseDuration` 的完整语义由 Rust 的局部 `parse_duration_seconds` 复刻为本任务所需格式；扩展格式时必须重新逐项核对，而不能假设两者天然等价。

系统探测的错误可观测性也有差异：Go 会写日志后回退或返回空串，Rust 本文件不依赖日志组件，只通过返回值表达结果。Go 测试可直接重写包内原子变量，而 Rust 一次性设置无法重置，因此采用子进程测试隔离。`pkg/util/timeutil/time_zone_test.rs` 覆盖原 Go 用例主干，`migration_aster_unit_test.rs` 补充非法路径、偏移边界、缓存和跨午夜窗口。

## 扩展指南

若新增时区表示形式，应首先修改 `Location`，并同步审查 `String`、`offset_at_utc`、`local_datetime_to_utc`、`Zone` 和 `ZoneName` 的穷举分支；测试应放在独立的 `pkg/util/timeutil/time_zone_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌到生产文件。

若改变 SQL 时区字符串语法，应从 `ParseTimeZone` 和 `parse_duration_seconds` 接入，保持命名时区优先级、MySQL 正负边界和 `ErrUnknownTimeZone` 分类，并同步 Go `time_zone.go::ParseTimeZone` / `time_zone_test.go::TestParseTimeZone` 的语义证据。尤其要加入最小/最大合法值、刚越界值、分钟秒边界、大小写 `SYSTEM` 和 `Local` 用例。

若改变系统时区初始化，应同时审查 `InferSystemTZ`、`SetSystemTZ`、`GetSystemTZ` 和 `SystemLocation`，以及首次写入发生在启动流程中的位置。由于 `SetSystemTZ` 不可重置，新增测试应继续使用子进程或其他进程级隔离；不要依赖测试执行顺序。

若改变缓存策略，应保持错误不缓存、并发读安全和 `Location` 值语义；引入淘汰或写锁二次检查前，应评估启动热路径和长期不同名称数量。若改变 `WithinDayTimePeriod`，需同步检查自动分析和 TTL 两条生产链，并明确相同起止点、秒精度、时区转换和跨午夜含义，避免把分钟窗口误改成瞬时时间比较。

兼容风险主要来自 MySQL 错误类型和偏移范围、`Local`/`System` 名称映射、DST 歧义选择及 Go 的分钟窗口语义；性能风险集中在缓存锁竞争、重复冷加载和无界键增长。任何行为变更都应先增加对应独立回归测试，再修改生产实现。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被索引为 25 个符号并显示被 64 个文件使用。
- RustCodeGraph 文件节点：完整读取 `pkg/util/timeutil/time_zone.rs`、`pkg/util/timeutil/time_zone_test.rs`、`pkg/util/timeutil/time_zone.go`、`pkg/util/timeutil/time_zone_test.go`、`pkg/util/timeutil/lib.rs`、`pkg/util/timeutil/errors.rs`，以及生产调用片段 `pkg/session/runtime.rs`、`pkg/session/runtime/ttl_runtime.rs`、`pkg/statistics/handle/autoanalyze/exec/exec.rs`。
- RustCodeGraph 查询：`query ParseTimeZone`、`query SystemLocation`、`query GetSystemTZ`、`query ConstructTimeZone` 消除 Go/Rust 同名歧义；`explore` 确认 `ParseTimeZone -> SystemLocation -> system_timezone` 调用链及目标文件使用范围。直接 `callers/callees` 批量查询未稳定返回输出，因此没有把该轮空输出当作无调用边证据，具体边由已索引源码节点和精确 `rg` 调用点交叉核对。
- 配置与模块证据：`pkg/util/timeutil/Cargo.toml` 确认 crate 名、`lib.rs` 路径、chrono/chrono-tz 依赖和 `go-package = "pkg/util/timeutil"`；`pkg/util/timeutil/lib.rs` 确认公开模块及独立测试装配。
- 测试证据：`time_zone_test.rs` 覆盖 zoneinfo 路径、单步软链、子进程 `TZ`、SYSTEM/IANA/固定偏移、`Local`、名称格式与构造优先级；`migration_aster_unit_test.rs` 补充非法路径、非法/边界偏移、跨午夜窗口和缓存；`time_zone_test.go` 提供原 Go 行为基线。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付前另运行任务指定的 11 章节结构命令，并人工检查文档只描述已由上述源码、调用点、配置或测试支持的事实。
