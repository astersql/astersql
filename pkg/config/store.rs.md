# `pkg/config/store.rs`

## 文件定位

`pkg/config/store.rs` 属于 `astersql-config` crate（见 `pkg/config/Cargo.toml`），定义进程级“存储后端名称”的轻量值类型。模块在 `pkg/config/lib.rs` 中以私有 `mod store` 装配，再通过 `pub use store::*` 对外重导出，因此调用方通常从 `astersql_config` 根路径使用这些符号。

该类型表达的是启动、配置和驱动注册层面的后端名称，不应与 `pkg/kv/kv.rs`、`pkg/store/copr/coprocessor.rs` 等文件中表示请求目标或 coprocessor endpoint 的同名 `StoreType` 混淆。它保留在 config 包而非 store 包，是为了延续 Go 文件所注明的避免循环依赖约束（`pkg/config/store.rs:30-37`、`pkg/config/store.go:17-19`）。

## 核心职责

- 用 `StoreType` 包装任意存储类型字符串，同时让内置常量可以零分配地借用静态字面量（`pkg/config/store.rs:36-37`）。
- 声明三个受认可的后端名称：`tikv`、`unistore`、`mocktikv`（`pkg/config/store.rs:39-49`）。其中后两者沿用 Go 注释所定义的测试/内嵌后端语义；`tikv` 是生产分布式存储入口。
- 通过 `StoreType::String` 暴露原始名称，通过 `StoreType::Valid` 进行封闭集合校验，并通过 `StoreTypeList` 以稳定顺序枚举集合（`pkg/config/store.rs:51-65,81-86`）。
- 支持从拥有所有权的 `String` 和静态 `&'static str` 构造类型，以覆盖配置解析/URI scheme 与编译期常量两类来源（`pkg/config/store.rs:67-79`）。

## 主要符号

- `pub struct StoreType(pub Cow<'static, str>)`：字符串 newtype；派生 `Clone`、`Debug`、`Eq`、`PartialEq`、`Hash`，所以可作为 `HashMap` 键。元组字段公开，调用方也能直接构造，但通常应使用常量或 `From`。
- `StoreTypeTiKV`、`StoreTypeUniStore`、`StoreTypeMockTiKV`：三个 `Cow::Borrowed` 常量，依次对应 `"tikv"`、`"unistore"`、`"mocktikv"`。
- `StoreType::String(&self) -> &str`：借用返回内部文本，不分配，也不改变值。
- `StoreType::Valid(&self) -> bool`：使用派生的值相等语义，与三个常量逐一比较；大小写敏感，不会 trim 或规范化。
- `From<String>`：保存为 `Cow::Owned`，允许运行时产生的任意名称原样存在。
- `From<&'static str>`：保存为 `Cow::Borrowed`；签名只接受静态生命周期，不能借用短生命周期临时文本。
- `StoreTypeList() -> Vec<StoreType>`：每次创建一个新 `Vec`，元素顺序固定为 TiKV、UniStore、MockTiKV。

## 执行流程

典型启动链路如下：

1. `cmd/tidb-server/main.rs:318-320` 使用 `StoreTypeUniStore` 作为命令行默认值，并用 `StoreTypeList` 生成帮助文本中的合法值列表。
2. server 完成配置处理后，`cmd/tidb-server/main.rs:985-1004` 分别把三个常量交给 `pkg/store/store.rs::Register` 注册 TiKV 与本地驱动。
3. `Register` 首先调用 `StoreType::Valid`，非法名称立即返回错误；合法值再作为 `HashMap<StoreType, DriverRef>` 的键进入全局驱动表（`pkg/store/store.rs:316-347`）。
4. 打开形如 `<scheme>://...` 的存储路径时，`newStoreWithRetryAndInterval` 将 scheme 转为小写、通过 `StoreType::from(String)` 构造查找键，再由 `loadDriver` 查询驱动（`pkg/store/store.rs:365-386,410-413`）。

`StoreTypeList` 和 `Valid` 都不执行注册；“名称受认可”和“对应驱动已注册”是两个独立条件。

## 数据与状态

`StoreType` 唯一状态是一个 `Cow<'static, str>`。内置常量持有借用分支，不拥有堆字符串；`From<String>` 持有 owned 分支。相等和哈希只取决于字符串内容，因此 owned 的 `"tikv"` 与常量 `StoreTypeTiKV` 等价，可命中同一驱动表键。

本文件没有全局可变状态。`StoreTypeList` 返回拥有自身元素的新向量，调用方修改或丢弃该向量不会影响常量或后续调用。真正的全局驱动映射位于 `pkg/store/store.rs::store_drivers`，不在本文件中。

当前 Rust 顶层 `Config.store` 仍是 `String`，默认值为 `"unistore"`，`Config::Valid` 也用字符串模式独立校验三种名称（`pkg/config/config.rs:971-974,1090,1342-1349`）。因此 `StoreType` 主要承担跨 crate 的注册键与公开名称集合角色，而不是配置反序列化字段的直接类型；扩展名称时必须同步这两条校验路径。

## 依赖与调用关系

本文件的唯一标准库依赖是 `std::borrow::Cow`，没有第三方依赖。crate 边界由 `pkg/config/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认，根模块在 `pkg/config/lib.rs` 重导出全部公开符号。

直接上游包括：

- `pkg/config/store_test.rs::test_store_type`：验证列表长度和列表中每项均合法。
- `pkg/config/const_3_aster_unit_test.rs::store_type_preserves_arbitrary_go_string_values`：验证列表顺序/文本，以及任意 owned 字符串可保留但不合法。
- `cmd/tidb-server/main.rs`：构造 CLI 默认值/帮助文本、判断当前后端并注册三个驱动。
- `pkg/store/store.rs::Register`、`newStoreWithRetryAndInterval`、`loadDriver`：分别消费合法性、运行时构造和哈希键能力。

下游只包含 `Cow` 的借用/拥有语义和派生的相等、哈希能力；本文件不调用配置加载、网络、磁盘或存储驱动 API。

## 错误处理与边界

本文件没有 `Result` 或 panic 分支。未知值并不会在构造时失败：例如 `StoreType::from("custom-engine".to_owned())` 会完整保留文本，仅由 `Valid` 返回 `false`（`pkg/config/const_3_aster_unit_test.rs:99-116`）。实际错误由消费方产生：`pkg/store/store.rs::Register` 对非法类型返回 `invalid storage type ...`，未注册 scheme 则在打开路径时返回 `storage ... is not registered`。

合法性严格区分大小写和全部字符；`"TiKV"`、前后带空白的名称以及空串均不合法。只有 `pkg/store/store.rs` 从 URI 读取 scheme 时显式调用 `to_ascii_lowercase`，不能据此推断所有 `StoreType` 构造都会规范化。

`StoreTypeList` 的顺序和长度已被测试当作契约。新增常量而未加入列表，或加入列表而未更新 `Valid`，都会造成枚举与校验不一致。

## 并发与资源生命周期

`StoreType` 自身不含锁、引用计数、线程、任务、通道或外部资源；借用分支仅引用 `'static` 字符串，owned 分支随值正常释放。只读方法可安全并发调用，本文件也没有初始化/关闭流程。

跨线程共享是否发生由调用方决定。当前重要消费方 `pkg/store/store.rs` 把 `StoreType` 作为受 `OnceLock<RwLock<...>>` 保护的驱动表键；锁生命周期和重复注册原子性属于该模块，而非 `store.rs`。`StoreTypeList` 每次分配小型向量，适合启动/校验路径，但不应误写成零分配 API。

## 与 Go 版本的对应关系

`pkg/config/store.go` 定义同名字符串类型、相同三个常量、`String`、`Valid` 和 `StoreTypeList`；Rust 保持了名称、合法集合、列表顺序和“任意字符串可先构造、再校验”的行为。`pkg/config/store_test.go::TestStoreType` 与 Rust 的 `pkg/config/store_test.rs::test_store_type` 都断言列表长度为 3 且每项合法。

语言映射差异是：Go 的 `type StoreType string` 可直接转换任意字符串；Rust 用 `Cow<'static, str>` newtype，并提供 `From<String>`/`From<&'static str>`。Go `String()` 按值返回新的 string header，Rust `String()` 借用 `&str`。Go 的 switch 校验在 Rust 中写成三个相等比较，当前结果一致。

还存在迁移层差异：Go `Config.Store` 直接使用 `StoreType`（`pkg/config/config.go:220`），Rust `Config.store` 使用普通 `String`。因此不能假设 Rust 已在 serde 配置边界自动获得 `StoreType::Valid`；目前配置校验由 `Config::Valid` 的独立字符串匹配承担。

## 扩展指南

新增后端名称时，最小一致性修改点是：

1. 在 `pkg/config/store.rs` 新增借用常量，并同时更新 `StoreType::Valid` 与 `StoreTypeList`，保持唯一、稳定的枚举顺序。
2. 同步 Go 对照 `pkg/config/store.go`，并更新独立测试 `pkg/config/store_test.rs`、`pkg/config/store_test.go`；Rust 侧还应扩展 `pkg/config/const_3_aster_unit_test.rs::store_type_preserves_arbitrary_go_string_values` 的顺序和非法值断言。
3. 同步 Rust `pkg/config/config.rs::Config::Valid` 的字符串集合和相关配置测试；否则新类型虽能注册，却会在配置校验阶段被拒绝。
4. 在 `cmd/tidb-server/main.rs` 注册真实驱动，并在 `pkg/store/store_test.rs` 或 server 的独立测试中覆盖合法注册、重复注册、URI scheme 查找和实际 driver 类型。不要把测试嵌入 `store.rs`。

兼容性风险主要是修改现有字符串值或列表顺序会影响配置文件、CLI 和 URI scheme；正确性风险是枚举、校验、配置校验和驱动注册漏改任一处；性能风险很低，但高频调用 `StoreTypeList` 会重复分配 `Vec`。若未来把 `Config.store` 改为 `StoreType`，需要单独评估 serde 表示、默认值以及现有字符串比较的迁移，不能只替换字段类型。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 11,467 个文件；`files --filter pkg/config/store.rs` 报告目标文件含 7 个符号。
- RustCodeGraph `node --file pkg/config/store.rs --offset 1 --limit 400`：读取完整 86 行源码，并报告该文件被 13 个文件使用。
- RustCodeGraph `query StoreType --kind struct --json` 与 `query StoreTypeList --kind function --json`：定位到 `pkg/config/store.rs::StoreType` 和 `pkg/config/store.rs::StoreTypeList`。精确 `callers`/`callees` 查询两次在约 30 秒内没有返回结果，因此直接调用证据由限定范围的 `rg` 和下列源码复核补齐。
- 已读实现/边界：`pkg/config/store.rs`、`pkg/config/lib.rs`、`pkg/config/Cargo.toml`、`pkg/config/config.rs`、`pkg/store/store.rs`、`cmd/tidb-server/main.rs`。
- 已读对照与测试：`pkg/config/store.go`、`pkg/config/store_test.rs`、`pkg/config/store_test.go`、`pkg/config/const_3_aster_unit_test.rs`；另以 `pkg/config/config.go` 和 `cmd/tidb-server/main.go` 的直接引用核对 Go 接线。
- 本任务为纯文档分析，按计划不运行 Cargo。结构验证命令及退出码在任务交付时记录。
