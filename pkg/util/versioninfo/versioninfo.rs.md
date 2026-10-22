# `pkg/util/versioninfo/versioninfo.rs`

## 文件定位

为 Rust 上层提供同步读取边界。

## 核心职责

1. 以 `CommunityEdition` 给出默认发行版名称 `"Community"`。
2. 以五个公开 `RwLock<&'static str>` 保存可读取、可覆写的进程级版本元数据：`TiDBBuildTS`、`TiDBGitHash`、`TiDBGitBranch`、`TiDBEdition`、`TiDBEnterpriseExtensionGitHash`。
3. 使默认值与 `pkg/util/versioninfo/versioninfo.go` 完全对应：前三项为 `"None"`，发行版为 `CommunityEdition`，企业扩展哈希为空串。
4. 为 Rust 上层提供同步读取边界。当前仓库中，`br/pkg/version/build/info.rs` 读取构建时间和 Git 分支/哈希并缓存为展示信息；`pkg/server/internal/testserverclient/server_client.rs::run_test_status_api` 读取 Git 哈希，用于校验 `/status` 响应。

本文件不是完整的构建注入系统。`Makefile.common` 的 `-X github.com/pingcap/tidb/pkg/util/versioninfo...` 参数只会覆写 Go 包变量；在已检索的 Rust 代码中，没有发现等价的链接期注入接线，只有通过 `RwLock::write` 进行的进程内覆写和测试写入。因而“发布构建可覆盖”是存储模型提供的能力，不能据此推断当前 Rust 构建已自动注入真实值。

## 主要符号

- `pub const CommunityEdition: &str = "Community"`：不可变的默认发行版字面量，同时用于初始化 `TiDBEdition`。
- `pub static TiDBBuildTS: RwLock<&str>`：UTC 构建时间字符串，默认 `"None"`。
- `pub static TiDBGitHash: RwLock<&str>`：当前二进制对应的 Git commit hash，默认 `"None"`。它是当前 Rust 仓库中直接使用范围最广的本文件符号。
- `pub static TiDBGitBranch: RwLock<&str>`：Git 分支名，默认 `"None"`。
- `pub static TiDBEdition: RwLock<&str>`：发行版名称，默认指向 `CommunityEdition`。
- `pub static TiDBEnterpriseExtensionGitHash: RwLock<&str>`：企业扩展 commit hash，社区默认值为空串。

五个静态量的内部元素都是 `&'static str`，不是拥有所有权的 `String`。写入者只能存放静态生命周期字符串；运行时生成的临时 `String` 不能直接借用后写入。文件没有类型、trait、函数、`impl` 或条件编译项，全部 API 都是公开常量/静态量。

## 执行流程

1. 进程加载 crate 时，五个 `RwLock` 用源码默认值完成静态初始化，不执行动态探测、文件读取或 Git 命令。
2. 使用方通过 crate 根导出的同名符号获取锁，例如 `TiDBGitHash.read()`；成功后解引用读守卫获得 `&'static str`，需要拥有值时再转为 `String`。
3. 若有进程内注入者，则通过对应静态量的 `write()` 获取独占写守卫并替换内部引用。当前目标 crate 的回归测试只演示 `TiDBGitHash` 的这一能力；没有生产写入者证据。
4. 上层可能进一步快照值。`br/pkg/version/build/info.rs::{BuildTS, GitHash, GitBranch}` 使用各自的 `OnceLock<String>`，第一次读取后缓存；后续修改本文件静态量不会改变这些 BR 快照，`br/pkg/version/build/info_test.rs::version_metadata_is_snapshotted_once` 固定了这一语义。
5. 另一些调用点按需读取。`pkg/server/internal/testserverclient/server_client.rs::run_test_status_api` 每次校验状态接口时读取 `TiDBGitHash`，并在锁中毒时返回带字段名的错误。

## 数据与状态

状态作用域是单进程、单份 crate 静态实例。每个字段有独立的 `RwLock`，因此单字段读写具备同步保护，但多个字段的组合不构成原子快照：读者依次读取构建时间、哈希和分支时，理论上可能跨越其他线程的更新。当前代码没有版本号、事务或批量更新 API 来保证五个字段同时切换。

默认值 `"None"` 和 `""` 都是有效数据哨兵，而不是 `Option`。本文件不解释或验证字符串格式，也不区分“未注入”“未知”和真实内容；调用者必须保留这些哨兵或在自己的展示层处理。企业扩展哈希的空串表达社区构建没有该组件。

`RwLock<&'static str>` 允许并发读取并串行化写入。它也意味着字段只能引用贯穿进程生命周期的数据；若未来需要从配置文件、环境变量或运行时命令生成值，应重新评估是否改为 `RwLock<String>`、使用一次初始化容器，或提供持有所有权的设置 API。

## 依赖与调用关系

下游依赖只有 Rust 标准库的 `std::sync::RwLock`；`Cargo.toml` 未声明第三方依赖。直接装配入口是 `pkg/util/versioninfo/lib.rs`，它公开模块并通配再导出全部符号。

已验证的 Rust 上游关系包括：

- `br/pkg/version/build/info.rs` 导入 `TiDBBuildTS`、`TiDBGitHash`、`TiDBGitBranch`；`BuildTS()`、`GitHash()`、`GitBranch()` 在首次调用时读取并缓存相应值，随后由 `LogInfo()` 和 `Info()` 输出。
- `pkg/server/internal/testserverclient/server_client.rs::run_test_status_api` 读取 `TiDBGitHash`，要求 `/status` JSON 的 `git_hash` 与它精确相等。
- `pkg/server/internal/testserverclient/server_client_test.rs::status_api_requires_exact_version_and_git_hash` 使用同一静态量构造回归测试响应。
- `pkg/util/versioninfo/migration_aster_unit_test.rs` 直接覆盖全部默认值契约，并验证 `TiDBGitHash` 可写后恢复。

Cargo 清单确认 `br/pkg/version/build`、`pkg/server`、`pkg/server/handler/tests` 和 `pkg/server/internal/testserverclient` 声明了到 `astersql-util-versioninfo` 的路径依赖；根 `Cargo.toml` 还以 `facade_util_versioninfo` 登记该包。文本检索没有发现 Rust 生产代码读取 `TiDBEdition` 或 `TiDBEnterpriseExtensionGitHash`；`pkg/util/printer/lib.rs` 内另有同名的本地 `versioninfo` 模块，不是本 crate 的调用者，分析时不可混同。

RustCodeGraph 已索引目标文件，但把整个文件只识别为一个符号，且 `node --file` 报告 `used by 0 files`；精确 `query/node/callers/callees` 无法为这些 `static` 建立可靠调用边。因此上述上游关系以 RustCodeGraph 的目标源码视图结合限定 `.rs`/Cargo 文本检索核实。

## 错误处理与边界

本文件没有返回 `Result`、输入校验或显式错误分支。实际失败边界来自 `std::sync::RwLock`：若持有写锁的线程 panic，锁会中毒，后续 `read()`/`write()` 返回 `PoisonError`。目标 crate 的迁移测试及 `br/pkg/version/build/info.rs` 都直接调用 `.unwrap()`，中毒时会 panic；`run_test_status_api` 则将中毒转换为 `"TiDBGitHash lock poisoned"` 错误。

任意字符串都可以写入，包含空串、`"None"` 或格式错误的时间/哈希。本层也不提供恢复默认值的方法。调用者若临时覆写全局值，必须自行保存旧值并恢复；`migration_aster_unit_test.rs` 展示了这一模式，但若断言在恢复前 panic，值仍可能污染同一测试进程。并行测试或生产写入需要额外的外部串行化策略。

不要把 Go 链接器 `-X` 的能力直接套用到 Rust：`Makefile.common` 中的注入目标是 Go import path，不能修改 Rust 的 `RwLock` 静态量。若交付物要求 Rust 二进制带真实构建信息，必须另行实现并验证 Rust 构建接线。

## 并发与资源生命周期

所有数据都使用字符串字面量引用且由静态量持有，生命周期贯穿整个进程；没有堆分配、文件句柄、网络连接、后台任务、通道或显式清理。读守卫/写守卫采用 RAII，在离开作用域时自动释放。

每个字段独立加锁使同一字段的并发访问免于数据竞争，但不能保证跨字段一致性。读者应尽快复制所需值并释放守卫，避免在持锁期间执行日志、网络或其他可能阻塞的操作。`br/pkg/version/build` 的 `OnceLock<String>` 将第一次读到的值保存到进程结束，因此注入若要影响该模块，必须发生在首次调用 `BuildTS()`、`GitHash()`、`GitBranch()` 之前。

测试中的可变全局状态同样跨线程共享。新增覆写测试应放在独立测试文件中，并用统一互斥锁或串行测试机制包围“保存—写入—断言—恢复”全过程；仅靠每个字段自己的 `RwLock` 不能防止其他测试观察中间值。

## 与 Go 版本的对应关系

Rust 用 `RwLock<&'static str>` 代替 Go 可直接读写的 `string` 变量，使共享可变静态状态满足 Rust 同步与安全要求；代价是所有读写都必须显式处理加锁结果，并且写入值受 `'static` 生命周期约束。

当前 Rust 代码没有经验证的等价链接期路径，所以默认值对齐和进程内可写性已经迁移，发布构建自动注入尚不能从本文件或已检索接线中确认。

不能因为 Go 有调用点就宣称对应 Rust 主链已经接通。

## 扩展指南

- 新增版本字段时，应在 `versioninfo.rs` 定义明确的默认值和同步容器，在 `lib.rs` 保持可见性，并同步 `versioninfo.go`、构建注入接线及 `migration_aster_unit_test.rs`；不要把测试写进生产源文件。
- 若新增真实 Rust 构建注入，优先设计一次初始化且不可在业务运行期任意更改的接口，并明确注入发生在任何 `OnceLock` 快照之前。需要同时更新构建脚本、相关 Cargo/Bazel 元数据及端到端展示测试，不能仅添加一个可写静态量。
- 若需要原子更新多字段，不能按顺序取得五把独立写锁；应考虑将元数据合并为一个结构体并由单锁保护，或发布不可变快照。改变公开静态量类型会影响直接读取者，属于兼容性变更。
- 若需要运行时生成的字符串，当前 `&'static str` 形状不合适。不要通过泄漏 `String` 来凑 `'static`；应评估拥有所有权的状态结构和清晰的 setter/getter API。
- 修改默认值或哨兵语义时，应同步核对 `br/pkg/version/build/info.rs` 的首次读取缓存、`run_test_status_api` 的精确相等校验，以及 Go 端所有直接变量使用处。
- 性能方面，版本信息通常只在启动、状态接口或日志路径读取，锁开销较低；若把它放入高频路径，应优先读取一次并持有不可变快照，而不是反复加锁。
