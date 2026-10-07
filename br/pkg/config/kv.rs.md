# `br/pkg/config/kv.rs`

## 文件定位

该文件属于 `astersql-br-pkg-config` library crate，是 Go 包 `br/pkg/config` 中 `kv.go` 的 Rust 移植文件。crate 入口 `br/pkg/config/lib.rs` 通过 `pub mod kv` 挂载本模块，并以 `pub use kv::*` 将类型和解析函数提升到 crate 根；`br/pkg/config/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认了这一边界。

它面向 TiKV `/config` 返回的 JSON，只抽取 BR 恢复流程关心的少量字段，不负责请求 TiKV、遍历 store 或聚合多节点结果。需要特别注意当前接线状态：仓库内没有其他 Cargo manifest 依赖 `astersql-br-pkg-config`；Rust 的连接主链 `br/pkg/conn/conn.rs` 当前使用自己的私有 `parse_*` 函数。因此，本文件目前是可独立验证的移植 API 和配置数据模型，而不是 Rust `Mgr::ProcessTiKVConfigs`/`Mgr::IsLogBackupEnabled` 的实际解析实现。Go 主链则在 `br/pkg/conn/conn.go` 中直接调用同路径 Go 包的三个公开解析函数。

## 核心职责

本文件有三组职责：

1. 用 `ConfigTerm<T>` 表达“配置值 + 是否由外部显式修改”，并用 `KVConfig` 聚合导入并发、region 尺寸和 region 键数三个 BR 参数。
2. 用临时的 `serde::Deserialize` 结构从 TiKV JSON 中分别解析 `import.num-threads`、`coprocessor.region-split-size`、`coprocessor.region-split-keys` 和 `log-backup.enable`。
3. 在本地 `units` 模块中移植 docker/go-units v0.5.0 的 `RAMInBytes`/`parseSize` 行为，使容量字符串按 1024 倍率转换，避免为一个小型兼容函数引入 Go 依赖或额外 Rust crate。

本文件刻意不做跨节点取最小值、默认值回退、HTTP 状态处理或恢复并发的 `threads + 4` 调整；这些在 Go 中属于 `br/pkg/conn/conn.go`，Rust 当前对应逻辑位于 `br/pkg/conn/conn.rs::Mgr::ProcessTiKVConfigs`。

## 主要符号

- `ConfigTerm<T> { Value, Modified }`：公开泛型结构。`Value` 保存实际配置，`Modified` 区分默认/推断值与用户显式值。它派生 `Clone`、`Debug`、`Default`，但未对 `T` 写显式 trait bound；派生实现仅在调用相应能力时要求 `T` 满足约束。与 Go 的 `ConfigTerm[T uint | uint64]` 相比，Rust 类型本身更宽，实际字段将其约束到整数。
- `KVConfig`：公开聚合结构。`ImportGoroutines` 为 `ConfigTerm<usize>`，另外两项为 `ConfigTerm<u64>`。它只是数据容器，不在本文件内修改 `Modified` 或合并多个节点的结果。
- `ParseImportThreadsFromConfig(&[u8]) -> Result<usize, serde_json::Error>`：读取可选 `import` 对象中的 `num-threads`；字段缺失、对象缺失或显式 `null` 均得到零。
- `ParseMergeRegionSizeFromConfig(&[u8]) -> Result<(u64, u64), Box<dyn Error + Send + Sync>>`：读取 `coprocessor` 对象，调用 `units::RAMInBytes` 转换尺寸，并返回 `(字节数, 键数)`。其错误类型必须同时容纳 JSON 错误、浮点解析错误和手工构造的单位错误。
- `ParseLogBackupEnableFromConfig(&[u8]) -> Result<bool, serde_json::Error>`：读取可选 `log-backup` 对象中的 `enable`；缺失或 `null` 为 `false`。
- `units::RAMInBytes(&str) -> Result<i64, Box<dyn Error + Send + Sync>>`：公开兼容入口，直接委托私有 `parse_size`。
- `units::parse_size`：私有容量解析器。它分离数值和后缀、拒绝负数和非有限浮点值、校验后缀，再乘二进制倍率并转换为 `i64`。

本文件没有常量、trait、`impl` 块或条件编译项；解析用的 `Importer`、`Coprocessor`、`LogBackup` 和各个 `Config` 均定义在函数内部，不构成公共 API。

## 执行流程

三个 JSON 入口都接收已经读入内存的响应字节：调用方选择目标解析器，`serde_json::from_slice` 反序列化到只包含所需字段的局部结构，未知 JSON 字段由 serde 默认忽略，然后返回单个值或二元组。这样 TiKV 完整配置的其他字段不会进入 BR 的数据模型。

`ParseImportThreadsFromConfig` 的流程是：反序列化顶层 `Config` → 将 `Option<Importer>` 的 `None` 变为默认对象 → 返回 `threads`。`ParseLogBackupEnableFromConfig` 对 `Option<LogBackup>` 使用同样模式并返回 `enable`。

`ParseMergeRegionSizeFromConfig` 的流程是：反序列化顶层 `Config` → 取得默认或实际 `Coprocessor` → 把 `region_split_size` 交给 `RAMInBytes` → 将成功的 `i64` 转为 `u64` → 与 `region_split_keys` 一起返回。这里 JSON 缺少 `coprocessor` 时尺寸字符串为空，随后单位解析失败；它不同于另外两个入口的“缺失即零值成功”。

`parse_size` 先从末尾寻找数字、点或空格作为数值/后缀分界：分界是空格时跳过恰好一个空格，否则在分界后一位切分。数值部分按 `f64` 解析；无后缀或 `b` 直接转为字节。其他后缀转小写后只接受 `k/m/g/t/p`、`kb/mb/...`、`kib/mib/...` 三类形式，且全部使用 `1024^n`。最终乘法结果转换为 `i64`，因此小数字节会截去小数部分。

在完整应用的 Go 链路中，`Mgr.ProcessTiKVConfigs` 从各存活 TiKV 拉取 `/config`，调用尺寸和线程解析器，再保留保守阈值并调整恢复并发；`Mgr.IsLogBackupEnabled` 调用布尔解析器并对所有节点做 AND。Rust 当前主链在 `br/pkg/conn/conn.rs:792-849` 复现该流程，但调用的是该文件底部的私有解析实现，而非本文件的公开函数。

## 数据与状态

所有解析函数都是纯计算：输入为借用字节切片或字符串，输出为拥有的标量，没有全局变量、缓存或可变静态状态。局部 serde 结构只在一次调用期间存在。

`ConfigTerm::Modified` 是本文件定义的唯一控制状态，但这里不解释或改变它。Go 调用方 `ProcessTiKVConfigs` 以它决定是否读取/覆盖集群配置；三项全为 `true` 时完全跳过远程拉取。`KVConfig::default()` 会把整数值置零并把三个 `Modified` 置为 `false`，这只是 Rust 派生默认值，不等同于 BR 业务默认值（例如连接模块的 96 MiB、960000 和导入并发 36）。

容量解析以 `f64` 作为中间表示、以 `i64` 返回，再在 region 解析器中转为 `u64`。负值和 NaN/无穷显式被拒绝；合法小数会在整数转换时截断。调用方若扩展到极大容量，需要同时审视浮点精度与整数转换边界。

## 依赖与调用关系

- 上游模块：`br/pkg/config/lib.rs` 声明并 re-export 本模块；`br/pkg/config/parity_test.rs::kv_config_parsers_match_go` 是当前直接 Rust 调用者，覆盖三个解析器和 `units::RAMInBytes`。
- 下游依赖：`serde` derive 为函数内结构生成反序列化实现，`serde_json::from_slice` 负责 JSON 解析；错误边界使用标准库 `std::error::Error`。`br/pkg/config/Cargo.toml` 声明了 `serde` 与 `serde_json`，没有 docker/go-units 的 Rust 依赖，因为单位逻辑在本文件内移植。
- Go 应用调用边：`br/pkg/conn/conn.go::ProcessTiKVConfigs` → `kvconfig.ParseMergeRegionSizeFromConfig`/`ParseImportThreadsFromConfig`；`Mgr.IsLogBackupEnabled` → `ParseLogBackupEnableFromConfig`。
- Rust 当前应用调用边：`br/pkg/conn/conn.rs::Mgr::ProcessTiKVConfigs` → 私有 `parse_merge_region_size_from_config`/`parse_import_threads_from_config`，`Mgr::IsLogBackupEnabled` → 私有 `parse_log_backup_enable_from_config`。这条链是语义对照证据，同时也是尚未复用本 crate 的接线差异。
- `RustCodeGraph` 将 `RAMInBytes → parse_size`、`ParseMergeRegionSizeFromConfig → RAMInBytes` 识别为直接调用边；对三个公开 JSON 解析器未识别出生产 Rust caller，与 Cargo manifest 搜索到“只有本 crate 自身声明、没有依赖方”一致。

## 错误处理与边界

非法 JSON 会由三个入口立即返回 `serde_json::Error`；不会吞错或提供业务默认值。JSON 中字段类型错误同样失败，例如字符串形式的线程数不能反序列化为 `usize`。未知字段不影响解析。

线程和日志备份解析器将缺失对象、显式 `null` 对象和缺失内部字段视为 Go 零值；`parity_test.rs` 分别验证了这些情况。region 解析器的 `coprocessor` 本身不是 `Option`，缺失时得到默认空字符串和零键数，但空字符串无法通过 `RAMInBytes`，所以整体报错。该差异来自后续单位转换，不应误写成所有缺失字段都会成功返回零。

单位解析接受大小写不敏感的 K/M/G/T/P、可选 `B` 或 `iB`，纯数字按字节处理；所有这些单位都使用 1024 而不是 SI 1000。它拒绝无数值输入、未知或过长后缀、负值、非有限值以及不符合切分规则的空白。`parity_test.rs` 已锁定 `32`、`1KB`、`1.5GiB`、`32b`、`32 B`、`32.3`、`1e3MB`、负数和首尾空白等代表性边界。

`ParseMergeRegionSizeFromConfig` 返回 boxed `Send + Sync` 错误而非具体枚举，便于统一传播不同来源的错误，但调用方难以按错误种类做稳定匹配；扩展错误语义时应避免依赖当前字符串。该函数在 `RAMInBytes` 成功后使用 `ram as u64`，其安全性依赖解析器已经拒绝负值。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、网络连接或文件句柄。所有状态均位于调用栈，公开结构只包含可复制语义的整数/布尔状态；多个线程可独立调用解析函数，不共享可变资源。

响应体的读取和 HTTP body 生命周期由上游负责。本文件只借用 `&[u8]`，在返回前完成解析，不保存输入引用。Go 主链负责逐 store 关闭/消费响应；Rust 当前 `conn.rs` 的 `HttpResponse` 已持有 body 字节。若未来将本 crate 接入异步请求链，应继续保持“网络资源由调用方管理、解析器只处理完整字节”的边界，除非另行设计流式解析 API。

## 与 Go 版本的对应关系

`ConfigTerm`、`KVConfig` 和三个 PascalCase 解析函数逐项对应 `br/pkg/config/kv.go`。Rust 保留 Go 风格公开命名，并由 crate 根 re-export，以降低迁移期间的符号差异。主要类型映射为 Go `uint` ↔ Rust `usize`、Go `uint64` ↔ Rust `u64`；在仓库支持的 64 位目标上，`parity_test.rs` 用 `4294967296` 验证线程数没有被收窄到 32 位。

Go 通过 `encoding/json.Unmarshal` 将缺失嵌套结构保留为零值。Rust 对 `import` 和 `log-backup` 使用 `Option<T> + unwrap_or_default()`，额外接受显式 JSON `null` 并仍返回零值；测试明确将其作为当前兼容契约。`coprocessor` 使用非可选默认结构，与 Go 零值结构一致，随后两端都会把空尺寸交给单位解析并报错。

Go 直接调用 `github.com/docker/go-units.RAMInBytes`；Rust 的 `units` 模块移植其 v0.5.0 核心规则。Rust parity 测试对二进制倍率、小数、指数、空格和负值做了定点核验，但并非上游 go-units 的穷举测试集合。

最大的迁移差异不是单个解析函数，而是接线：Go `conn.go` 直接依赖 `br/pkg/config`；Rust `conn.rs` 当前拥有 `u32` 线程字段和私有解析副本，且其 crate manifest 未依赖 `astersql-br-pkg-config`。因此不能仅凭本文件存在就断言生产 Rust 路径使用了这里的实现。

## 扩展指南

新增 TiKV 配置字段时，优先在最窄的解析函数内增加局部 serde 字段；只有该值需要跨调用方传递时才扩展 `KVConfig`。同步检查 `br/pkg/config/kv.go` 的公开契约、JSON key 的连字符重命名、缺失与 `null` 的预期、数值位宽及业务默认值。测试逻辑必须放在独立的 `br/pkg/config/parity_test.rs`（或新的独立 `*_test.rs`）中，不要内嵌进生产文件。

修改单位语法时，应以 docker/go-units 对应版本为依据，并为大小写、空格、小数、指数、负数、非有限值、未知后缀和整数边界补独立测试。改变 `f64 → i64 → u64` 链路可能影响兼容性和超大值精度，需要明确风险而不能只让常见样例通过。

若要消除 Rust 连接层的重复实现，应作为单独接线工作：给 `br/pkg/conn` 增加带 tag/工作区规范允许的 crate 依赖，统一 `usize` 与其当前 `u32` 的线程类型和错误类型，再让 `Mgr` 调用本文件的公开解析器。必须同步 `br/pkg/conn/conn_test.rs` 中的多 store 聚合、默认回退和日志备份测试，且不能只删除私有函数而忽略错误包装或行为差异。

对 `ConfigTerm` 或 `KVConfig` 的结构性修改会影响序列化以外的调用者数据模型；当前它们未派生 `PartialEq`/serde traits。除非确有跨 crate 契约需要，不应随意扩大派生能力或把业务默认值塞进通用 `Default`，以免混淆“语言零值”和“BR 默认配置”。

## 验证依据

- 源文件与符号：`br/pkg/config/kv.rs`（`ConfigTerm`、`KVConfig`、三个 `Parse*` 函数、`units::RAMInBytes`、`parse_size`）。
- crate 边界：`br/pkg/config/lib.rs`（模块声明及 re-export）、`br/pkg/config/Cargo.toml`（library 名称、入口、porting metadata、serde 依赖）。
- Go 对照：`br/pkg/config/kv.go`（数据结构和三个解析器）；`br/pkg/conn/conn.go`（生产调用、跨节点聚合与默认回退）。
- Rust 当前接线：`br/pkg/conn/conn.rs:792-849`（`ProcessTiKVConfigs`、`IsLogBackupEnabled`）及 `br/pkg/conn/conn.rs:1047-1100`（私有解析副本）；`br/pkg/task/stream.rs:941-973` 使用的是 `astersql_br_pkg_conn::{ConfigTerm, KVConfig}`，不是本 crate 类型。
- 独立测试：`br/pkg/config/parity_test.rs:162-224` 直接覆盖本文件公开解析 API；`br/pkg/conn/conn_test.rs:793-894` 与其后日志备份用例覆盖当前 Rust 连接层的聚合行为；Go 对照测试位于 `br/pkg/conn/conn_test.go::TestGetMergeRegionSizeAndCount` 等用例。
- RustCodeGraph：索引状态为 7032 个 Rust 文件；文件查询确认 `kv.rs` 有 14 个符号；调用图确认 `ParseMergeRegionSizeFromConfig → RAMInBytes → parse_size`，并显示本 crate 的公开 JSON 解析器没有生产 Rust 调用边。原始 `callers` 命令在本索引上超时无输出，因此又以精确 `explore`、文件节点、Cargo manifest 搜索和调用点搜索交叉核验。
- 人工复核结论：该文件存在是为了保留 Go `br/pkg/config/kv.go` 的公开数据模型、JSON 解析和容量单位语义；运行时接线、跨节点决策及安全扩展边界已分别说明，没有把未接线 API 表述为生产主链。
