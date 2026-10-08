# `pkg/store/mockstore/unistore/config/config.rs`

## 文件定位

该文件是 `astersql-store-mockstore-unistore-config` crate 的核心实现；crate 入口 `pkg/store/mockstore/unistore/config/lib.rs` 以 `pub mod config` 加载它，并通过 `pub use config::*` 对外重导出。`pkg/store/mockstore/unistore/config/Cargo.toml` 的 `package.metadata.porting.go-package` 指向同目录 Go 包，表明这里是 `pkg/store/mockstore/unistore/config/config.go` 的 Rust 移植边界。

它位于 mockstore/unistore 的配置层，不自行启动服务、打开数据库或创建后台任务。其产物由上层组装代码消费：`pkg/store/mockstore/unistore/mock.rs::New` 克隆 `DefaultConf` 并按测试存储路径调整引擎参数，`pkg/store/mockstore/unistore/server/server.rs::{create_db,get_region_options,new_mock,new}` 将配置映射为数据库、Region 和服务启动参数，`pkg/store/mockstore/unistore/util/lockwaiter/lockwaiter.rs::NewManager` 读取悲观事务的唤醒延迟。

## 核心职责

1. 用 `Config` 聚合 `Server`、`Engine`、`RaftStore`、`Coprocessor`、`PessimisticTxn` 五个配置段，并用 Serde 的 `rename` 保持 Go TOML 键名。
2. 通过 `DefaultConf: LazyLock<Config>` 提供一份可克隆的默认配置，默认值与 Go 的包级 `DefaultConf` 对齐。
3. 用 `CompressionType` 和 `ParseCompression` 把配置字符串收敛为引擎可消费的三态压缩枚举。
4. 用 `ParseDuration` 及三个私有辅助函数复刻 Go `time.ParseDuration` 所需的有符号纳秒解析、无单位秒回退和非法值终止语义。

该文件只定义和解析配置，不校验全部跨字段约束。例如压缩层数至少为 7 的要求是在 `server.rs::create_db` 中检查，而不是在 `Engine` 反序列化时检查。

## 主要符号

- `pub struct Config`：顶层配置对象；五个公开字段分别对应 TOML 的 `server`、`engine`、`raftstore`、`coprocessor`、`pessimistic-txn`。
- `pub struct Server`：PD、Store、状态服务地址，日志级别/文件、平均 Region 大小、CPU 上限和 Raft 开关。字段名称保留 Go 风格，借助 `#[allow(non_snake_case)]` 编译。
- `pub struct Engine`：数据库路径、value 阈值、MemTable/SST/L1 大小、L0 compact/stall 阈值、value log、缓存、压缩、同步写和关闭时 compact 等参数。`VolatileMode` 标有 `#[serde(skip)]`，不进入序列化格式，仅供测试/临时存储路径调整。
- `pub struct RaftStore`：保存 PD 心跳、leader lease、基础 tick 的字符串，以及心跳/选举 tick 数和自定义 Raft log 开关；本文件不把三个 duration 字符串转换为 `Duration`。
- `pub struct Coprocessor`：Region 最大键数与拆分阈值。
- `pub struct PessimisticTxn`：锁等待超时和唤醒延迟，单位均为毫秒。
- `pub enum CompressionType { None, Snappy, Zstd }` 与 `pub fn ParseCompression(&str)`：仅精确接受小写 `snappy`、`zstd`，其他输入均映射为 `None`，因此大小写错误不会返回显式错误。
- `pub const MB: i64`：`1024 * 1024`，供默认容量值计算。
- `pub static DefaultConf: LazyLock<Config>`：首次解引用时构造默认配置；`Config: Clone` 使调用者能取得独立可变副本。
- `pub fn ParseDuration(&str) -> Duration`：公开 duration 入口；先按原字符串解析，失败后追加 `s` 再试，最终拒绝负数和非法输入。
- `fn parse_go_duration`、`fn parse_duration_leading_int`、`fn parse_duration_leading_fraction`：私有解析器，分别负责复合 duration、整数部分和小数部分；中间量以纳秒和 `u64` 表示，并显式处理 `i64` 边界。

## 执行流程

默认配置路径如下：调用者首次访问 `DefaultConf` 时，`LazyLock` 构造五个配置段；此后调用者通常执行 `.clone()`，再修改与当前实例相关的字段。实际示例是 `mock.rs::New`：它覆盖 `Engine.DBPath`、关闭 `Server.Raft`，并在临时目录模式下启用 `VolatileMode`、缩小 MemTable/value log、减少 compactor。修改后的 `Config` 传给 `server.rs::new_mock`，后者再把 `Engine` 交给 `create_db`，并用 `Server.RegionSize`、`Server.StoreAddr` 建立初始 Region 元数据。

压缩路径是 `Engine.Compression` 字符串数组 → `server.rs::create_db` 检查长度至少为 `BADGER_LEVEL_COUNT` → 对前七层逐项调用 `ParseCompression` → 得到数据库 `TableBuilderOptions.compression_per_level`。超过七层的槽位保留为 `CompressionType::None`；未知算法名同样静默成为 `None`。

`ParseDuration` 的步骤为：

1. `parse_go_duration` 读取可选正负号；仅字符串 `"0"` 可在没有单位时直接成功。
2. 循环解析一个或多个“整数/小数 + 单位”片段，支持 `ns`、`us`、`µs`、`μs`、`ms`、`s`、`m`、`h`。
3. 每段换算为纳秒并累加；整数、单段和值都受 `1 << 63` 边界约束。累加使用 `wrapping_add`，用于复刻 Go 先在 `uint64` 中累加再检查有符号范围的行为。
4. 小数只保留不会溢出内部整数的有效前缀，并按单位比例截断到纳秒。
5. 原字符串解析失败时，公开函数追加 `s`，所以 `"2"` 按两秒处理；两次均失败或结果为负数时 panic，否则用 `Duration::from_nanos` 返回。

## 数据与状态

所有配置结构体都实现 `Clone`、`Debug`、`Deserialize`、`Serialize`，字段本身是拥有所有权的 `String`、`Vec<String>` 和标量，没有借用生命周期。除 `Engine.VolatileMode` 外，字段通过显式 `serde(rename = ...)` 固定外部键名；没有 `Default` 派生或 `#[serde(default)]`，因此缺失普通字段时不能依靠本文件自动补上 `DefaultConf`。

`DefaultConf` 是进程级只读懒初始化值；初始化完成后本文件不提供内部可变性。调用者通过克隆获得独立状态，因此 `mock.rs::New` 的覆盖不会修改全局默认值。`Compression` 默认含七个空字符串，对齐 Go `make([]string, 7)`；空字符串经 `ParseCompression` 成为 `None`。

duration 解析的内部状态仅存在于栈上：字节索引、符号位、当前片段、十进制小数比例和累计纳秒。返回的 `std::time::Duration` 不能表达负数，所以负值在边界处被拒绝。

## 依赖与调用关系

直接 Rust 依赖只有源码实际使用的 `serde::{Deserialize, Serialize}` 与标准库 `Duration`、`LazyLock`；Cargo manifest 还声明了 `go-parse-duration = "0.1.1"`，但当前 `config.rs` 没有调用它，duration 逻辑由本文件私有函数实现。

已核对的上游关系：

- `config/lib.rs` 重导出本文件全部公开符号。
- `unistore/mock.rs::New` 使用并定制 `DefaultConf`，随后调用 server 层。
- `server/server.rs::create_db` 消费 `Engine` 并调用 `ParseCompression`；`get_region_options`、`new_mock`、`new` 消费 `Config.Server` 与 `Config.Engine`。
- `util/lockwaiter/lockwaiter.rs::NewManager` 读取 `Config.PessimisticTxn.WakeUpDelayDuration`。
- `unistore/config`、`unistore/server`、`unistore/tikv` 和 `unistore/util/lockwaiter` 的 Cargo manifest 均直接或间接声明此 crate；根 `Cargo.toml` 还提供 `facade_store_mockstore_unistore_config` 路径别名。

RustCodeGraph 将目标文件识别为含 20 个符号的已索引文件；精确 `query` 定位了本文件的 `ParseDuration`、`ParseCompression` 和 `DefaultConf`。图的精确 `callers/callees` 查询未输出可用边，因此上述调用关系由局部 Rust 引用搜索及调用点源码复核，不把 RustCodeGraph 的宽泛“used by”文件列表当作函数级调用证据。

## 错误处理与边界

- `ParseCompression` 没有 `Result`；未知值、空串和大小写不同的值全部降级为 `None`。扩展算法时必须同步引擎枚举和映射测试，避免无声关闭压缩。
- `server.rs::create_db` 才检查 `Compression.len() >= 7` 并返回 `ServerError::InvalidConfig`；本文件允许构造任意长度数组。
- `ParseDuration` 对非法单位、缺单位的非零复合形式、空串、只有符号、数值溢出和负结果执行 panic。它与 Go 的 `log.S().Fatalf` 都是不可恢复边界，但 Rust panic 与 Go 进程日志退出并非完全相同的运行时机制。
- 无单位纯数字通过追加 `s` 成为秒；显式 `+` 号可被内部解析器接受；`-0` 首次解析失败后追加 `s` 可得到零，而真正负值被拒绝。
- 单位匹配代码接受 `us`、`µs`、`μs`，不接受裸 `µ` 或 `μ`。`config_test.rs` 当前包含裸微秒符号应成功的断言，这与目标函数的可见匹配分支不一致；本任务按要求不运行 Cargo，也不修改实现或测试，因此不把该断言描述为已通过行为。
- 反序列化不会调用业务校验：地址格式、路径可写性、阈值正负关系、Raft tick 合法性等必须由消费者负责。

## 并发与资源生命周期

本文件不开线程、不持锁、不创建通道、不执行 I/O，也不拥有数据库或网络资源。唯一的共享生命周期机制是 `LazyLock`：首次并发访问由标准库保证只初始化一次，之后共享不可变 `Config`。实例级变更应先克隆；公开字段允许调用者直接修改，因此跨线程共享可变实例时的同步责任在调用者。

配置对资源的影响发生在下游：`NumCompactors` 控制 compact 并发，缓存/表大小影响内存与磁盘，`SyncWrite` 影响持久性与写入开销，`CompactL0WhenClose` 影响关闭阶段工作，`WakeUpDelayDuration` 被锁等待管理器保存。它们在这里都只是数据，不代表相应资源已经创建或行为已经启动。

## 与 Go 版本的对应关系

`config.go` 与本文件具有相同的五段顶层结构、TOML 名称和核心默认值；Rust 的 `i32` 对应若干 Go `int` 字段，容量和等待时间继续使用 `i64`。Go `options.CompressionType` 在 Rust 中被本地 `CompressionType` 取代，但 switch 语义保持为仅识别小写 `snappy`、`zstd`。

Go 的可变包级变量 `DefaultConf` 在 Rust 中变为 `LazyLock<Config>`；七个空压缩槽位、地址、容量、tick、Region 键阈值及悲观锁参数与 Go 对照文件一致。Rust 还显式填入 Go 零值所隐含的 `SyncWrite = false`、`IngestCompression = ""`、`VolatileMode = false`。

Go `ParseDuration` 先调用 `time.ParseDuration`，失败后追加 `s`，随后对错误或负值 `Fatalf`。Rust 以本地解析器复刻纳秒、复合单位、小数与溢出语义，最终以 panic 表示致命配置错误。`config_test.rs` 特别覆盖 Go 微秒拼写、小数精度和无符号累加；`migration_aster_unit_test.rs` 覆盖默认值、压缩 switch、常见 duration、秒回退及非法/负数。

## 扩展指南

新增配置字段时，应同时修改对应 Rust 子结构、Serde 外部键、`DefaultConf`、同路径 Go 对照（若迁移目标发生变化）及 `migration_aster_unit_test.rs::default_conf_matches_go_values`。若字段驱动真实行为，还必须接到直接消费者，例如引擎项接入 `server.rs::create_db`、Region 项接入 `get_region_options/new_mock/new`、锁等待项接入 `lockwaiter.rs::NewManager`；只增加字段并不等于功能已生效。

新增压缩算法时，修改 `CompressionType` 和 `ParseCompression`，再同步 server/engine 接受的算法类型与 `parse_compression_matches_go_switch`。需要决定未知字符串继续静默降级还是改为显式错误；后者会改变 Go 兼容行为。

修改 duration 时，优先保持 `parse_go_duration` 的复合片段、微秒别名、截断和边界语义，并把测试放在独立的 `config_test.rs` 或 `migration_aster_unit_test.rs`，不要嵌入生产文件。需特别覆盖 `i64::{MIN,MAX}` 纳秒附近、累加回绕、长小数、无单位秒回退、负数和非法单位。若改用 manifest 中的 `go-parse-duration` 依赖，必须先用现有边界用例证明语义等价。

性能风险主要来自改变容量、缓存、compact 和同步写默认值；兼容风险主要来自 TOML 键名、字段必填性、duration 致命错误方式及未知压缩算法的降级行为。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/store/mockstore/unistore/config` 确认目标、Go 对照和测试均在索引中；`node --file .../config.rs --offset 1 --limit 500` 读取目标文件 440 行及 20 个符号；`query ParseDuration`、`query ParseCompression`、`query DefaultConf` 精确定位目标符号。函数级 `callers/callees` 未产生可用输出，已如实回退到局部引用搜索。
- 实现与边界：`pkg/store/mockstore/unistore/config/config.rs`。
- crate 边界：`pkg/store/mockstore/unistore/config/Cargo.toml`、`pkg/store/mockstore/unistore/config/lib.rs`，以及直接消费者的 Cargo manifests。
- Go 语义：`pkg/store/mockstore/unistore/config/config.go`。
- 独立测试：`pkg/store/mockstore/unistore/config/config_test.rs`、`pkg/store/mockstore/unistore/config/migration_aster_unit_test.rs`。
- 真实调用点：`pkg/store/mockstore/unistore/mock.rs::New`、`pkg/store/mockstore/unistore/server/server.rs::{create_db,get_region_options,new_mock,new}`、`pkg/store/mockstore/unistore/util/lockwaiter/lockwaiter.rs::NewManager`。
- 按任务约束未运行 Cargo；结构验收仅检查目标文档存在且恰有十一个固定二级标题。
