# `pkg/config/external_workload.rs`

## 文件定位

本文件属于 `astersql-config` crate（见 `pkg/config/Cargo.toml`），定义 TiDB 主配置中 `[external-workload]` 段的 Rust 数据模型和字段级校验。`pkg/config/lib.rs` 以 `mod external_workload` 装入模块并通过 `pub use external_workload::*` 重导出，因此其他 crate 可直接使用 `astersql_config::ExternalWorkload` 和各角色常量。

它不是外部工作负载控制器或任务调度器的实现。配置经 `pkg/config/config.rs` 的 `Config::valid` 校验后，才由 `pkg/session/runtime/session.rs` 读取并转换成 `astersql_extworkload::config::ExternalWorkload`，随后交给 `NewManagerWithTLS` 建立运行时管理器。

## 核心职责

1. 用 `ExternalWorkload` 表示启用开关、节点角色、服务池名称和控制器地址。
2. 用 `RoleMaster`、`RoleGCV2Worker`、`RoleTTLTaskWorker`、`RoleAutoAnalyzeWorker` 固定允许的角色文本。
3. 在启用配置时由 `ExternalWorkload::Valid` 就地规范化字段并按确定顺序检查必填项和角色集合。
4. 用 `ExternalWorkload::isConfigured` 区分“保持全零值”与“用户写过任一相关配置”，供顶层配置实施仅 Starter 部署模式可用的约束。
5. 通过 serde 的 `default` 与 `rename_all = "kebab-case"` 对应 TOML/JSON 中的 `enable`、`role`、`tidb-pool` 和 `controller-addr`。

本文件只负责配置表示、规范化和校验，不负责网络连接、TLS、控制器 RPC、任务注册或资源调度。

## 主要符号

- `pub type ExternalWorkloadRole = String`：角色的公开类型别名。它保留 Go `type ExternalWorkloadRole string` 可承载字符串的语义，但 Rust 类型系统不会阻止调用方构造未知角色；合法性由 `valid`/`Valid` 在运行时确认。
- `RoleMaster = "master"`：协调普通 TiDB 与外部任务的主角色；空角色在启用校验时回退到该值。
- `RoleGCV2Worker = "gcv2"`、`RoleTTLTaskWorker = "ttl"`、`RoleAutoAnalyzeWorker = "auto-analyze"`：GC v2、TTL、自动分析三类专职 worker 角色。
- `pub struct ExternalWorkload`：公开配置结构。`Enable` 决定本文件是否执行启用态校验；`Role` 表示节点角色；`TidbPool` 是控制器识别的服务池名；`ControllerAddr` 是控制器地址。结构体派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq` 和 serde 编解码能力。
- `pub fn defaultExternalWorkload() -> ExternalWorkload`：返回结构体零值，即关闭且三个字符串为空。当前 `pkg/config/config.rs` 的 Rust 默认实现直接调用 `ExternalWorkload::default()`；该函数仍作为 Go 同名函数的公开迁移接口，并由独立测试直接覆盖。
- `ExternalWorkload::Valid(&mut self) -> Result<(), String>`：启用态规范化与校验入口。它会修改接收者，所以调用成功后配置可直接供运行时消费。
- `ExternalWorkload::isConfigured(&self) -> bool`：只判断是否出现非空语义值，不验证组合是否合法，也不修改字段。它在 crate 外不可直接调用，但供同 crate 的顶层 `Config::valid` 使用。
- `pub fn normalized(&ExternalWorkloadRole) -> ExternalWorkloadRole`：去除首尾空白并转为小写。
- `pub fn valid(&ExternalWorkloadRole) -> bool`：仅接受四个角色常量的精确文本。

## 执行流程

顶层配置链路如下：

1. serde 按 kebab-case 字段名解析 `Config.external_workload`；没有该段时由 `Default` 提供关闭的零值（`pkg/config/config.rs` 的 `Config` 定义与默认实现）。
2. `Config::valid` 先检查部署模式。非 Starter 模式下，只要 `isConfigured()` 为真就返回 `external-workload can only be configured when deploy-mode is starter`；Starter 模式下才调用 `ExternalWorkload::Valid`。
3. `Valid` 在 `Enable == false` 时立即成功返回，既不检查未知角色，也不修剪或改写任何字符串。
4. 启用时，`Role` 先经 `normalized` 去空白并小写；结果为空则写入 `RoleMaster`。
5. `ControllerAddr` 与 `TidbPool` 随后被就地 `trim`。
6. 校验按固定顺序执行：控制器地址非空、角色属于允许集合、服务池非空。遇到首个错误即返回，之前已经完成的规范化不会回滚。
7. `pkg/session/runtime/session.rs` 的启动流程克隆全局 `external_workload`。仅当部署模式是 Starter 且 `Enable` 为真时，读取 keyspace 元数据，把四个字段复制到 `astersql_extworkload` 的运行时配置并调用 `NewManagerWithTLS`。

因此，安全的正常路径要求先完成顶层 `Config::valid`，再把配置交给运行时；直接构造结构体并绕过校验会把合法性责任留给调用方。

## 数据与状态

`ExternalWorkload` 全部是实例内拥有的数据：一个布尔值和三个 `String`，没有全局变量、内部缓存或隐藏句柄。`Default` 状态为 `Enable == false` 且字符串均为空。

`Valid` 是有状态的就地转换。启用时，它把角色写为规范形式，把空白角色写为 `master`，并修剪地址与池名；即使后续返回错误，这些较早发生的字段修改仍然保留。`isConfigured` 则只基于规范化/修剪后的观察结果判断：`Enable` 为真，或角色、地址、池名任一去空白后非空，即视为已配置；仅包含空白的字符串不算已配置。

角色常量是 `&str`，而配置字段是拥有所有权的 `String`。运行时接线会克隆这些字符串，配置对象与管理器之间没有共享可变引用。

## 依赖与调用关系

- crate 边界：`pkg/config/Cargo.toml` 声明 crate 名为 `astersql-config`，本文件直接使用的外部能力只有 `serde` 派生；`serde` 以 `derive` feature 引入。
- 模块出口：`pkg/config/lib.rs` 私有声明模块并公开重导出所有符号。
- 上游配置调用：`pkg/config/config.rs::Config::valid` 调用 `ExternalWorkload::isConfigured` 和 `ExternalWorkload::Valid`；`Config::default` 构造其零值。
- 文件内部调用：RustCodeGraph 显示 `Valid` 调用 `normalized` 与 `valid`，`isConfigured` 调用 `normalized`。其余操作来自 `String`/`str` 的标准库方法。
- 下游运行时：`pkg/session/runtime/session.rs` 读取并克隆配置，在 Starter 启用态转换为 `astersql_extworkload::config::ExternalWorkload`，然后创建外部工作负载管理器。
- 下游策略读取：`pkg/domain/domain.rs::ttl_external_workload_role` 在没有已安装管理器时读取全局配置；启用且角色为空时仍防御性地回退到 `RoleMaster`，并据此决定 TTL 本地调度条件。
- 角色常量还被 `pkg/extworkload`、`pkg/domain`、`pkg/session/runtime` 和 `pkg/store/gcworker` 的管理、判别与任务路径使用；这些消费者依赖字符串值保持兼容。

配置模块与 `pkg/extworkload` 当前各自定义了形状相同的配置类型，二者通过启动代码显式逐字段转换，并非同一个 Rust 类型。

## 错误处理与边界

`Valid` 使用 `Result<(), String>`，不定义专用错误枚举。错误文本分别为：启用但控制器地址为空、角色不在允许集合、启用但服务池为空。顶层 `Config::valid` 会把这些字符串映射为配置错误；部署模式错误由顶层先行产生，不在本文件内判断。

重要边界包括：

- 关闭状态是完全短路：即使角色非法或其他字段非空，`Valid` 本身也成功且不修改字段；但顶层非 Starter 检查仍可能因 `isConfigured` 为真而拒绝配置。
- 空白角色在启用态不是错误，而是默认成 `master`。
- 大小写混合且带首尾空白的合法角色会被接受并规范化，例如 `" GCV2 "` 变为 `"gcv2"`。
- 地址与池名只检查去除首尾空白后是否为空；本文件不解析 URL、主机端口或池名格式。
- 校验有顺序性：同时存在多个问题时只返回最先命中的错误，顺序为地址、角色、池名。
- serde 的 `#[serde(default)]` 允许字段缺省；启用后缺失的必填字符串会在 `Valid` 阶段报错。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、网络连接或文件句柄，也没有 `Drop` 清理逻辑。所有操作都在调用线程内同步完成。

并发安全由所有权边界自然限定：`Valid` 需要 `&mut self`，同一配置值不能在安全 Rust 中被并发修改；`isConfigured`、`normalized` 和 `valid` 只读输入。进入运行时后，启动代码克隆字符串并把独立配置值传给管理器，管理器的连接与关闭生命周期属于 `pkg/extworkload` 和 session/domain 接线，不由本文件管理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/config/external_workload.go`。Rust 保留了 Go 的四个角色文本、四个配置字段、零值默认、启用短路、规范化顺序、默认 master、校验顺序和错误文本语义。

主要语言映射为：Go 的命名字符串类型对应 Rust `String` 类型别名；Go 的 TOML/JSON tag 对应 serde kebab-case；Go 的指针接收者 `Valid` 对应 Rust `&mut self`；Go `error` 对应当前 Rust 的 `Result<(), String>`；Go 的值接收者 `isConfigured` 对应 Rust `&self`。

已确认的接线差异是 Rust `Config::default` 直接使用派生的 `ExternalWorkload::default()`，而 Go `defaultConf` 调用 `defaultExternalWorkload()`，两者结果相同。Rust 另保留公开的同名函数供迁移接口和测试使用。Rust 顶层加载/校验位于 `pkg/config/config.rs`，Go 对应逻辑位于 `pkg/config/config.go`；两者都实施“仅 Starter 可配置”的外层约束。

Go 回归 `pkg/config/config_test.go::TestExternalWorkloadValid` 覆盖部署模式、必填字段、非法角色和 `GCV2` 规范化。Rust 的 `pkg/config/config_test.rs::test_external_workload_valid` 覆盖相同主链，`pkg/config/const_3_aster_unit_test.rs::external_workload_normalizes_and_validates_like_go` 进一步直接覆盖关闭态不修改、空角色默认 master、空白修剪和错误文本。

## 扩展指南

- 新增角色时，应同时修改本文件的公开常量和 `valid` 匹配集合，并同步 Go 对照文件；还必须检查 `pkg/extworkload` 中重复的角色常量/配置类型以及所有按角色分支的 domain、session、GC/TTL/auto-analyze 路径，避免配置接受了角色而运行时不认识。
- 新增配置字段时，应加入 `ExternalWorkload`、明确 serde 名称与默认值、决定 `isConfigured` 是否应感知该字段，并在 `Valid` 中规定规范化和错误优先级；启动时在 `pkg/session/runtime/session.rs` 的显式转换处同步传递，并同步 `pkg/extworkload` 的运行时配置结构。
- 收紧地址或池名语法前，需要评估现有配置兼容性。当前契约只做非空检查，直接增加 URL 或字符集限制会成为行为变化。
- 不应把测试内嵌到本源文件。直接单元测试应继续放在 `pkg/config/const_3_aster_unit_test.rs` 或 `pkg/config/config_test.rs`，顶层配置/加载行为同步 `pkg/config/config_test.go` 的 Go 用例；涉及真实管理器创建的行为应放在 `pkg/extworkload/manager_test.rs` 或 session/domain 的独立测试文件。
- 修改错误文本或校验顺序时，应同步断言字符串的 Rust/Go 测试，并检查顶层 `Config::valid` 的错误包装是否仍保持用户可诊断性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/config/external_workload.rs` 报告该文件 7 个符号；`node --file ... --offset 1 --limit 260` 读取了完整 132 行源码；精确查询确认 Rust/Go 同名结构、默认函数及规范化函数；调用图确认 `Valid → normalized/valid`、`isConfigured → normalized`。通用符号名的 callers 查询存在跨语言歧义，因此上游接线另由精确引用与源码核验。
- 目标源码：`pkg/config/external_workload.rs`，核对常量、结构、serde 属性、默认函数、两个方法与两个辅助函数。
- crate 与模块：`pkg/config/Cargo.toml`、`pkg/config/lib.rs`，核对 crate 名、serde 依赖、模块装入与公开重导出。
- Rust 上下游：`pkg/config/config.rs`、`pkg/session/runtime/session.rs`、`pkg/domain/domain.rs`、`pkg/extworkload/manager.rs`、`pkg/extworkload/lib.rs`，核对默认值、部署模式门禁、校验调用、运行时转换、角色消费和管理器边界。
- Go 对照：`pkg/config/external_workload.go`、`pkg/config/config.go`，核对字段、角色、规范化、校验及顶层接线。
- 测试证据：`pkg/config/config_test.rs::test_external_workload_valid`、`pkg/config/const_3_aster_unit_test.rs::external_workload_normalizes_and_validates_like_go`、`pkg/config/config_test.go::TestExternalWorkloadValid`。本任务是纯文档分析，按计划不运行 Cargo 或代码测试。
