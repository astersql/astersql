# `pkg/keyspace/username_policy.rs`

源码：[username_policy.rs](username_policy.rs)；Go 对照：[username_policy.go](username_policy.go)。

## 文件定位

本文件属于 [`astersql-keyspace`](Cargo.toml) crate。crate 入口 [`lib.rs`](lib.rs) 以私有 `mod username_policy` 装载它，再通过 `pub use username_policy::*` 将公开的 `UsernamePolicy` trait 和 `GetUsernamePolicy` 工厂提升到 crate 根。它位于 keyspace 多租户边界：根据部署模式为用户名选择“直接放行”或“必须携带当前 keyspace 前缀”的策略。

该文件只定义策略及字符串转换，不负责读取登录报文、查询 `mysql.user`、执行 `CREATE/ALTER/GRANT`，也不修改全局配置。Rust 全仓引用搜索目前只找到独立测试调用这些公开 API，未找到 Rust 生产调用点；因此当前可确认的是策略实现和测试已经迁移，不能据此声称 Rust 登录或执行器主链已完成接线。Go 生产版本的实际入口位于 `pkg/server/conn.go`、`pkg/executor/grant.go` 与 `pkg/executor/simple.go`。

## 核心职责

- `GetUsernamePolicy` 在调用时读取 `deploymode::IsStarter()`；Starter 返回捕获当前 keyspace 名称的 `PrefixPolicy`，其他模式返回 `DefaultUsernamePolicy`。
- `DefaultUsernamePolicy` 保持传统部署兼容：任意用户名均校验成功、任意格式均视为有效，并且不生成别名、不执行去前缀转换。
- `PrefixPolicy` 为 Starter 多租户部署提供四项能力：前缀校验、点号格式判定、为无前缀名字生成候选、从正确前缀名字恢复原用户名。
- 前缀校验失败使用共享的 DDL/MySQL 标准错误 `ErrUserNameNeedPrefix`，保留 Go 侧错误类别、错误码和消息参数语义。
- 空 keyspace 名称是显式的宽松边界：前缀策略仍可被构造，但校验放行且不生成或移除前缀，适配配置尚未完成的 bootstrap/测试阶段。

## 主要符号

- `pub trait UsernamePolicy`：公开策略契约，包含四个对象安全方法；调用者通过 `Box<dyn UsernamePolicy>` 动态分派，不需要知道实际策略类型。
  - `ValidateUsername(&str) -> Result<(), SharedError>`：检查当前策略要求；只有非空 `PrefixPolicy` 且输入未以期望前缀开头时返回错误。
  - `ValidateUsernameFormat(&str) -> bool`：默认策略总为 `true`；前缀策略只检查整个字符串恰好包含一个 `.`，不核对点号前内容是否等于当前 keyspace。
  - `GetUsernameVariants(&str) -> Vec<String>`：默认策略返回空向量；前缀策略只在前缀非空且输入尚未带正确前缀时返回一个 `{keyspace}.{username}` 候选。
  - `GetOriginalUsername(&str) -> String`：默认策略返回空串；前缀策略仅剥离精确的 `{keyspace}.` 起始片段，其他输入返回空串。
- `pub fn GetUsernamePolicy() -> Box<dyn UsernamePolicy>`：唯一公开构造入口。它依据调用瞬间的部署模式选择实现，并在 Starter 分支通过 [`GetKeyspaceNameBySettings`](keyspace.rs) 快照 keyspace 名称。
- `DefaultUsernamePolicy`：私有零字段类型，承担非 Starter 的兼容放行行为。
- `PrefixPolicy { user_prefix: String }`：私有前缀策略；拥有构造时取得的 keyspace 名称，后续全局配置变化不会更新已有实例。
- `PrefixPolicy::expected_prefix() -> String`：私有助手，每次调用构造带尾点的前缀，例如 `ks.`。

文件没有模块级常量、宏、条件编译项或内嵌测试；测试按仓库要求位于独立文件。

## 执行流程

1. 调用者从 crate 根调用 `GetUsernamePolicy`。
2. 工厂调用 `deploymode::IsStarter()`。该判定只有在 NextGen 内核且全局模式为 `Starter` 时为真；否则返回装箱的 `DefaultUsernamePolicy`。
3. Starter 分支调用 `GetKeyspaceNameBySettings()`，把全局配置中的 keyspace 名称复制到新建 `PrefixPolicy.user_prefix`，再装箱为 trait object。
4. 校验用户名时，`PrefixPolicy::ValidateUsername` 先处理空前缀宽松分支；非空时构造 `expected_prefix` 并用 `starts_with` 检查。失败则以 `user_prefix`、`user_prefix`、原用户名三个参数生成 `ErrUserNameNeedPrefix`，成功返回 `Ok(())`。
5. 格式检查与前缀校验是两个独立判断：`ValidateUsernameFormat` 只计数点号。因而 `other.user` 格式有效但可能前缀无效；`ks.user.extra` 前缀有效但格式无效。调用方若需要完整判定，必须按自己的流程组合二者。
6. 名称查找兼容路径可先调用 `GetUsernameVariants`：未带当前前缀时得到唯一候选 `ks.<原输入>`；已经带前缀或前缀为空时没有候选。
7. 需要展示或还原逻辑用户名时调用 `GetOriginalUsername`；只有精确以 `expected_prefix` 开头的输入会返回其余后缀。

## 数据与状态

本文件没有静态可变状态。每次 `GetUsernamePolicy` 都返回新的堆分配 trait object；`DefaultUsernamePolicy` 无字段，`PrefixPolicy` 只持有一个拥有所有权的 `String`。因此策略实例是创建时配置的快照：部署模式决定实现类型，keyspace 名称决定 Starter 前缀，二者之后的全局变化都不会改写已返回实例。

关键输入输出不变量如下：

| 策略/条件 | 校验 | 格式 | 变体 | 去前缀 |
| --- | --- | --- | --- | --- |
| 默认策略 | 始终成功 | 始终 `true` | 空向量 | 空串 |
| 前缀为空 | 始终成功 | 恰好一个点号 | 空向量 | 空串 |
| 前缀 `ks`，输入 `ks.user` | 成功 | `true` | 空向量 | `user` |
| 前缀 `ks`，输入 `user` | `ErrUserNameNeedPrefix` | `false` | `ks.user` | 空串 |
| 前缀 `ks`，输入 `other.user.extra` | `ErrUserNameNeedPrefix` | `false` | `ks.other.user.extra` | 空串 |

所有比较都是 Rust `str` 的精确、区分大小写字节前缀/字符匹配；文件不做 Unicode 归一化、大小写折叠、空白裁剪、用户名长度或合法字符检查。

## 依赖与调用关系

- 模块装配：[`lib.rs`](lib.rs) `mod username_policy` → `pub use username_policy::*`，使外部 crate 可从 `astersql_keyspace` 根访问公开 trait 与工厂；两个具体策略保持模块私有。
- 下游部署判定：`GetUsernamePolicy -> deploymode::IsStarter`。[`pkg/config/deploymode/mode.rs`](../config/deploymode/mode.rs) 显示该函数组合 `kerneltype::IsNextGen()` 与原子读取的当前模式，所以 Classic 构建不会选择前缀策略。
- 下游配置读取：Starter 分支 `GetUsernamePolicy -> GetKeyspaceNameBySettings -> config::get_global_keyspace_name`，返回拥有所有权的字符串快照。
- 下游错误构造：`PrefixPolicy::ValidateUsername -> ErrUserNameNeedPrefix.GenWithStackByArgs`；错误原型位于 [`pkg/util/dbterror/exeerrors/errors.rs`](../util/dbterror/exeerrors/errors.rs)，类别为 DDL，MySQL 错误码为 `ErrUsername`。
- Cargo 边界：[`Cargo.toml`](Cargo.toml) 直接依赖 `astersql-config`、`astersql-config-deploymode`、`astersql-util-dbterror-exeerrors` 和 `astersql-config-kerneltype`；`nextgen` feature 同时转发到 kerneltype 与 deploymode，默认 feature 为空。
- Rust 直接调用者：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 和 [`keyspace_test.rs`](keyspace_test.rs) 调用工厂与四个方法。全仓 `rg` 未发现 Rust 生产文件直接调用这些符号；多个 crate 依赖 `astersql-keyspace` 不能视为使用了本策略。
- Go 生产主链：`pkg/server/conn.go` 用变体重试身份匹配，并组合校验与格式检查报告 keyspace 不匹配；`pkg/executor/simple.go`/`grant.go` 在用户创建、重命名、授权及存在性查询时校验或尝试变体。这些是 Go 对照行为证据，不是 Rust 已接线证据。

RustCodeGraph 的 `explore` 将 Rust `GetUsernamePolicy` 的调用者识别为 `migration_aster_unit_test.rs::username_policies_match_default_and_starter_go_behavior`，并识别该测试对四个 trait 方法的调用。图中同名 Go 符号另有 executor 调用者；精确 `callers/callees` 命令在本次查询中未完成返回，因此同名消歧与其余静态边使用 `query --json`、源码及全仓引用搜索复核。

## 错误处理与边界

唯一业务错误来自 `PrefixPolicy::ValidateUsername`。当 `user_prefix` 非空且输入不以 `{user_prefix}.` 开头时，它通过 `ErrUserNameNeedPrefix.GenWithStackByArgs` 返回 `SharedError`；三个格式化参数会生成类似 `User name must start with \`ks.\` (use \`ks.user\` instead)` 的标准消息。独立测试同时验证错误原型相等、DDL RFC code 与 MySQL `ErrUsername` code，说明调用方可以按标准错误体系识别它，而不应匹配文本。

需要特别保留以下边界：

- `ValidateUsername` 只检查起始前缀，不检查点号总数；`ks.user.extra` 会通过前缀校验。
- `ValidateUsernameFormat` 只检查恰好一个点号，不检查正确前缀；`other.user` 会返回 `true`。
- 空 keyspace 名称不会形成 `"."` 强制前缀，而是使校验、变体和去前缀保持宽松；格式检查仍按点号数量执行。
- `GetUsernameVariants` 会原样保留输入并在前面拼接前缀，包括已有错误前缀或多个点号的输入；它不承担格式净化。
- `GetOriginalUsername("ks.")` 返回空串，与“不匹配/未转换”也返回空串相同；返回类型本身不能区分这两种情况。
- 本文件不限制空用户名、长度、保留字符或 host 部分，这些校验属于调用方或其他认证/执行逻辑。

函数中没有显式 panic、I/O 或可重试错误。字符串分配失败遵循 Rust 进程级内存失败行为，不通过 `Result` 报告。

## 并发与资源生命周期

策略对象不包含锁、原子、引用计数、通道、异步任务、事务或外部资源。每个对象拥有自己的前缀字符串，`&self` 方法只读访问，因此并发安全风险主要来自对象创建之前读取的外部全局状态，而不是本文件内部状态。

部署模式由 deploymode 模块的 `AtomicI32` 管理，配置由 config crate 管理；`GetUsernamePolicy` 对它们进行瞬时读取，没有为两次读取建立联合快照锁。标准生命周期假定这些值在启动阶段配置稳定。若部署模式或 keyspace 名称随后改变，旧策略仍保持旧类型/旧前缀，新调用得到新策略；长期缓存策略对象的调用方需要明确接受这一快照语义。

`Box<dyn UsernamePolicy>` 在所有者离开作用域时正常释放；没有全局缓存或显式清理。trait 未声明 `Send + Sync` 上界，因此公开返回类型不保证能跨线程移动或共享，即使当前两个具体实现的字段本身具备这些自动 trait。若未来需要把策略存入跨线程长期状态，应先以真实调用场景和独立测试定义所需边界。

## 与 Go 版本的对应关系

直接对照 [`username_policy.go`](username_policy.go) 后，Rust 保留了 Go 的接口方法、工厂分支、两个私有策略、空前缀宽松规则、点号计数规则、候选拼接、前缀剥离和标准错误参数顺序。主要语言映射为：

- Go `UsernamePolicy` interface → Rust 对象安全 trait；Go 接口返回值 → `Box<dyn UsernamePolicy>`。
- Go `error`/`nil` → Rust `Result<(), SharedError>` 的 `Err`/`Ok(())`。
- Go `nil` 或空 `[]string` → Rust 空 `Vec<String>`；对当前调用者的迭代与 `Empty` 判断语义一致，但 nil 身份不保留。
- Go 值类型 `prefixPolicy{userPrefix: ...}` → Rust 拥有 `String` 的 `PrefixPolicy`。
- Go `strings.HasPrefix`、`strings.Count` 和切片去前缀 → Rust `starts_with`、`matches('.').count()` 和 `strip_prefix`。

Go 的 [`keyspace_test.go`](keyspace_test.go) 覆盖默认策略与 NextGen Starter 分支；Rust 的 [`keyspace_test.rs`](keyspace_test.rs) 对齐这些用例，并额外由 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 检查标准错误消息与错误分类。Rust 测试目前覆盖了正确前缀、缺失前缀、错误/多段名字、变体和去前缀；空 keyspace 下前缀策略的完整四方法组合没有在这两个直接测试中单独构造，因为工厂依赖全局模式与配置。

迁移状态上的关键差异不是策略算法，而是接线：Go server/executor 已使用该策略，Rust 生产源码目前没有直接引用。因此安全描述应是“Rust 策略实现与测试对齐 Go”，而不是“Rust 登录和用户管理链已应用该策略”。

## 扩展指南

- 新增部署模式或策略时，优先扩展 `UsernamePolicy` 与 `GetUsernamePolicy` 的选择规则，同时检查 `deploymode::IsStarter`/feature 传播；具体实现应继续保持私有，通过 trait 暴露稳定契约。
- 修改前缀语法时必须成对审查四个方法。校验、格式、变体与去前缀当前刻意分工，单改一个方法可能让登录变体查找、错误报告和用户 DDL 得到矛盾结论。
- 若要接入 Rust 生产链，应分别找到 Rust server 身份匹配、executor 用户创建/重命名/授权和用户存在性查询的真实实现，再以 Go 调用点为语义基准做最小接线；不能仅因 crate 已依赖 `astersql-keyspace` 就假定入口存在。
- 若允许运行期间改变 keyspace 配置，需要决定策略应保持创建时快照还是每次方法调用重读配置；这是兼容性和并发语义变化，不能只替换字段类型。
- 若需要区分“合法空原用户名”和“未匹配”，应评估把 `GetOriginalUsername` 改为 `Option<String>` 的 API 兼容影响；当前空串哨兵与 Go 一致。
- 性能敏感路径应注意 `expected_prefix` 每次分配新 `String`，错误路径还会克隆两次前缀。只有性能证据表明必要时才缓存带点前缀，并保持空前缀行为不变。
- 测试必须继续放在独立文件。优先扩展 [`keyspace_test.rs`](keyspace_test.rs) 或 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，同步核对 [`keyspace_test.go`](keyspace_test.go)；建议覆盖空 keyspace、大小写、`ks.`、多点用户名，以及任何新增生产接线的调用级行为。

主要兼容风险是改变用户名解析后影响现有账户查找与错误码；正确性风险是四方法语义不一致或把 Go 接线误认为 Rust 已接线；性能风险局限于热路径上的装箱与字符串构造，本文件没有网络、锁竞争或数据库访问。

## 验证依据

- RustCodeGraph：`status` 显示项目索引包含 11,467 个文件、307,296 个节点与 1,848,419 条边；`explore "pkg/keyspace/username_policy.rs UsernamePolicy GetUsernamePolicy PrefixPolicy DefaultUsernamePolicy"` 返回目标 Rust/Go 全文及调用概览；`query GetUsernamePolicy --json` 精确区分 `username_policy.rs:40` 与 `username_policy.go:38`，`query expected_prefix --json` 定位 Rust 私有助手。精确 `callers/callees` 查询本次未完成，未据此虚构边。
- 已读 Rust 与 crate 边界：[`username_policy.rs`](username_policy.rs)、[`lib.rs`](lib.rs)、[`keyspace.rs`](keyspace.rs)、[`Cargo.toml`](Cargo.toml)、[`pkg/config/deploymode/mode.rs`](../config/deploymode/mode.rs)、[`pkg/util/dbterror/exeerrors/errors.rs`](../util/dbterror/exeerrors/errors.rs)。修改行为前已读取包契约 [`doc.go`](doc.go)。
- 已读独立测试：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 与 [`keyspace_test.rs`](keyspace_test.rs)，分别验证策略/错误对齐和 Go `TestUsernamePolicy` 迁移行为；测试未嵌入生产文件。
- 已读 Go 对照与生产调用：[`username_policy.go`](username_policy.go)、[`keyspace_test.go`](keyspace_test.go)、`pkg/server/conn.go`、`pkg/executor/grant.go`、`pkg/executor/simple.go`。全仓 `rg` 同时确认 Rust 直接调用只出现在上述 keyspace 测试，而 Go 生产调用覆盖认证和用户管理路径。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前使用任务指定命令验证恰有 11 个固定二级标题，并人工检查源码链接、迁移状态、边界与扩展建议均有直接证据。
