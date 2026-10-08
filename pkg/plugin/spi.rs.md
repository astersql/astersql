# `pkg/plugin/spi.rs`

## 文件定位

[`spi.rs`](spi.rs) 是 `astersql-plugin` crate 的服务提供方接口（SPI）基础层。crate 入口 [`lib.rs`](lib.rs) 将它声明为公开模块并重导出全部公开项，因此插件加载器、内置插件和 crate 外调用方都可以直接使用这里的 `Context`、`Manifest`、专用清单及导出接口。crate 的 [`Cargo.toml`](Cargo.toml) 指定 `lib.rs` 为库入口，并用 `package.metadata.porting.go-package = "pkg/plugin"` 标明它对应 Go 的 `pkg/plugin` 包；本文件本身只依赖标准库以及同 crate 的 `Kind`、`PluginError`。

在完整插件链路中，本文件定义“插件向框架交付什么数据和回调”，而 [`plugin.rs`](plugin.rs) 负责“怎样加载、校验、初始化、刷新和关闭插件”，[`helper.rs`](helper.rs) 负责把通用 `Manifest` 恢复为具体插件类别的清单。它不是动态库加载实现，也不保存全局插件注册状态。

## 核心职责

- 以 `LIBRARY_SUFFIX` 和 `MANIFEST_SYMBOL` 固定动态插件的文件后缀与入口符号约定；[`plugin.rs`](plugin.rs) 的 `load_one` 用二者拼出 `{插件 ID}.so` 并请求 `PluginLoader::load_manifest(..., "PluginManifest")`。
- 用 `Context` 为插件回调提供可跨线程共享的取消标志、重试标志和类型安全值；`ContextKey` 把键类型与值类型在编译期绑定。
- 用 `Manifest` 汇总插件身份、版本依赖、许可证/构建信息以及四个生命周期回调，并由 `LifecycleCallback` 统一回调签名。
- 用 `ExportManifest`、自由函数 `export_manifest` 和 `Manifest.extension` 将认证、Schema、Daemon 等专用清单转换为加载器认识的通用清单。认证回调会以类型擦除值保留，供 [`helper.rs`](helper.rs) 的 `declare_authentication_manifest` 恢复。
- 提供稳定的空值构造：`Manifest::new` 构造指定种类/名称/版本且无钩子的清单，各 `Default` 实现供测试和渐进式组装使用。

## 主要符号

- `LIBRARY_SUFFIX: &str = ".so"`、`MANIFEST_SYMBOL: &str = "PluginManifest"`：动态加载协议常量。
- `ContextKey`：公开 trait，要求键为 `Send + Sync + 'static`，并把 `Value` 约束为 `Any + Send + Sync`。键的 Rust `TypeId` 是实际索引，因此不同键类型即使值类型相同也不会冲突。
- `Context`：可克隆上下文。`cancel`/`is_cancelled` 管理取消；`set_retrying`/`is_retrying` 管理快速重试状态；`with_value<K>` 派生值快照；`value<K>` 返回 `Option<Arc<K::Value>>`。
- `LifecycleCallback`：`Arc<dyn Fn(&Context, &Manifest) -> Result<(), PluginError> + Send + Sync>`。`Manifest.validate`、`on_init`、`on_shutdown`、`on_flush` 都使用该类型。
- `Manifest`：通用插件描述。关键字段包括 `name`、`version`、`kind`、`require_version`、四个生命周期钩子，以及唯一的类型擦除扩展槽 `extension`。
- `Manifest::new`：显式设置 `kind`、`name`、`version`，其余字符串、映射、回调和扩展为空。`Manifest::default` 使用 `Kind::Audit`、空名称和版本 `0`，这是为 Rust 测试往返提供的稳定占位，不等同于 Go 的数值零值 `Kind=0`。
- `ExportManifest` 与 `export_manifest`：前者是专用清单转换协议，后者是泛型调用门面。`Manifest`、`AuthenticationManifest`、`SchemaManifest`、`DaemonManifest` 在本文件实现该 trait；[`audit.rs`](audit.rs) 还为 `AuditManifest` 实现它。
- `AuthenticationManifest` 与 `AuthenticationCallbacks`：前者是公开认证清单，后者是放入 `Manifest.extension` 的回调载荷；`authentication_callbacks` 用 `Any::downcast_ref` 尝试恢复它。
- `SchemaManifest`、`DaemonManifest`：当前仅包裹通用 `Manifest`，导出时直接克隆内层清单。

## 执行流程

1. 插件构造 `Manifest::new(kind, name, version)`，填充描述、依赖版本与所需生命周期回调；专用插件再把它放入相应的 `*Manifest`。
2. 调用 `export_manifest(&specialized)`。普通、Schema、Daemon 清单只克隆通用部分；认证清单先克隆通用部分，再把四个认证回调克隆进 `AuthenticationCallbacks`，以 `Arc<dyn Any + Send + Sync>` 写入 `extension`。审计清单在 [`audit.rs`](audit.rs) 中采用相同模式保存四类审计回调。
3. [`plugin.rs`](plugin.rs) 的 `load_one` 从静态工厂、测试钩子或 `PluginLoader` 获得通用 `Manifest`，检查清单名称和动态插件版本。`load` 随后检查 `require_version` 并调用 `validate`。
4. `init` 调用 `on_init`；若存在 `on_flush` 且配置了键值客户端，则创建 `FlushWatcher`，刷新禁用状态并启动监视线程。`shutdown` 先取消 watcher，再调用 `on_shutdown`。
5. 需要类别专有回调时，[`helper.rs`](helper.rs) 的 `declare_authentication_manifest` 调用 `authentication_callbacks`，克隆回调并重建 `AuthenticationManifest`；Schema/Daemon 只重新包装通用清单。若扩展为空或类型不匹配，认证回调均恢复为 `None`。
6. 回调所用 `Context` 可通过 `with_value` 派生。派生时复制当前 `HashMap`，插入/遮蔽对应键类型的值；父子上下文共享取消和重试原子量，因此任何克隆调用 `cancel` 或 `set_retrying` 后，其余克隆立即观察同一状态。

## 数据与状态

`Context` 的两个布尔状态分别存放在 `Arc<AtomicBool>` 中。`Clone` 只克隆 `Arc`，所以取消和重试状态属于整个克隆族。值表是 `Arc<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>`：读取不需要可变借用；派生先复制映射再包入新 `Arc`，所以父级和已有子级的值快照不会被后续派生修改。同一 `ContextKey` 再次插入会遮蔽旧值，但不会影响原上下文。

`Manifest` 是可克隆的拥有型快照：字符串、版本映射和 `Option<Arc<...>>` 回调一起复制，回调闭包与扩展载荷本身通过 `Arc` 共享。`extension` 只有一个槽，当前由具体类别的回调集合占用；它不携带运行时类型标签之外的业务协议，也不允许同时存放两个互不相关的扩展对象。

认证回调当前统一为 `AuthenticationCallback = Arc<dyn Fn() + Send + Sync>`，没有参数和返回值；这只是对 Go SPI 字段形状的当前迁移基线。Schema 与 Daemon 专用清单当前也没有额外字段。

## 依赖与调用关系

上游关系如下：

- [`lib.rs`](lib.rs) 公开并重导出本模块。
- [`plugin.rs`](plugin.rs) 使用 `Context`、`Manifest`、`LIBRARY_SUFFIX`、`MANIFEST_SYMBOL`；其 `Plugin` 持有清单，`load`/`init`/`shutdown` 驱动生命周期回调，`FlushWatcher` 使用上下文取消状态。
- [`helper.rs`](helper.rs) 使用 `export_manifest`、`authentication_callbacks` 及各专用清单，实现声明恢复和测试插件装配。
- [`audit.rs`](audit.rs) 实现 `AuditManifest: ExportManifest`，定义多个 `ContextKey`，并以与认证清单相同的方式在 `extension` 保存审计回调。
- `pkg/session/runtime/typed_adapter_bridge.rs` 以 `Context::with_value` 注入执行开始时间等审计上下文；`pkg/plugin/integration_test.rs` 使用 `set_retrying` 验证重试路径。

下游依赖仅有标准库的 `Any`/`TypeId`、`HashMap`、`Arc`、`AtomicBool`，以及同 crate 的 `Kind` 和 `PluginError`。`Cargo.toml` 没有声明普通外部依赖；唯一列出的 `serial_test` 是开发依赖，与本文件生产逻辑无直接关系。

RustCodeGraph 对目标文件给出的文件级结果显示它被 `plugin.rs`、`helper.rs`、相关测试及其他调用方使用；对同名/重载的 `export_manifest` 未生成可用的精确调用边，因此上述函数级关系还用定向源码搜索核验。

## 错误处理与边界

`Context` 操作本身不返回错误。`value<K>` 在键不存在或类型擦除值不能向下转换为 `K::Value` 时返回 `None`；正常通过 `with_value<K>` 写入时，trait 的关联类型保证类型匹配。取消是协作式标志，不会主动中断线程或回调。

生命周期回调通过 `Result<(), PluginError>` 报错，但具体策略在 [`plugin.rs`](plugin.rs)：`validate` 或 `on_init` 的错误通常终止加载/初始化，`skip_when_fail` 可将插件禁用并继续；`FlushWatcher` 会忽略刷新回调错误以继续处理后续事件；`shutdown` 忽略单个 `on_shutdown` 错误以继续清理其他插件。`spi.rs` 只定义错误通道，不实施这些策略。

`authentication_callbacks` 对缺失或错误类型的 `extension` 静默返回 `None`，声明函数随后生成无认证回调的清单；因此扩展类型错误不会在此处变成 `PluginError`。新增扩展时必须避免覆盖既有类别载荷，否则回调会在导出—声明往返中丢失。

`Manifest::default` 的 `Kind::Audit` 是 Rust 特有占位选择，不应作为有效插件身份依赖。动态加载仍由 `load_one` 核对真实名称和版本。常量 `.so` 也意味着当前动态协议以 Go 插件共享库约定为准，没有在本文件为其他平台选择后缀。

## 并发与资源生命周期

所有存入上下文、回调和扩展的对象都要求 `Send + Sync`，并由 `Arc` 共享，因此 `Context` 和 `Manifest` 可安全克隆到 watcher 线程。`cancelled` 与 `is_retrying` 的写使用 `Ordering::Release`、读使用 `Ordering::Acquire`，保证线程间观察相应标志；它们互相独立，取消不会自动清除重试状态。

值表采用不可变快照，没有锁竞争；代价是每次 `with_value` 都克隆整个 `HashMap`，适合少量上下文值，不适合作为高频、大容量可变存储。值和闭包在最后一个 `Arc` 被释放时自动销毁，本文件没有显式 `Drop`、后台任务、文件句柄或网络资源。

真正的 watcher 生命周期位于 [`plugin.rs`](plugin.rs)：`init` 可把含 `on_flush` 的 `Manifest` 克隆进 `FlushWatcher` 并启动线程，`shutdown` 调用 watcher 的 `cancel`。因为取消是轮询式的，是否及时退出取决于 watcher 检查上下文标志的频率，而不是 `Context::cancel` 等待线程完成。

## 与 Go 版本的对应关系

直接对照文件是 [`spi.go`](spi.go)，独立 Go 回归是 [`spi_test.go`](spi_test.go)。两边的 `LibrarySuffix`/`ManifestSymbol`、通用清单字段、四类生命周期钩子以及 Authentication/Schema/Daemon 清单角色一致；Rust 的 [`spi_test.rs`](spi_test.rs) 对齐 Go `TestExportManifest`，验证 `OnInit` 与审计事件回调在导出—声明后仍可触发。

关键实现差异：

- Go `ExportManifest(any)` 通过 `reflect` 与 `unsafe.Pointer` 把“首字段为 `Manifest`”的专用结构体指针重解释为 `*Manifest`；Rust 禁止这种布局假设，改用 `ExportManifest` trait、克隆通用字段，并在类型擦除的 `extension` 中保留专用回调。
- Go 使用标准 `context.Context`，取消和值形成上下文链；Rust `Context` 是本地最小实现：克隆族共享取消/重试状态，值使用写时复制快照，没有截止时间、取消原因或父上下文链。
- Go `Manifest` 内部还持有 `flushWatcher`；Rust 将 watcher 放在 [`plugin.rs`](plugin.rs) 的 `Plugin` 中，SPI 清单保持为可公开克隆的数据/回调描述。
- Go 的 `Kind` 零值是 `0`；Rust `Manifest::default` 明确选 `Kind::Audit` 作为测试占位。
- Go 认证清单字段也是无参数函数，但 Rust 用 `Option<Arc<dyn Fn() + Send + Sync>>` 表达可选、共享且线程安全的回调。

[`helper_test.rs`](helper_test.rs) 进一步验证 Rust 的 Audit/Authentication/Schema/Daemon 导出—声明往返，并用 `Arc::ptr_eq` 证明四个认证回调没有被替换；Go 的对应覆盖位于 [`helper_test.go`](helper_test.go)。

## 扩展指南

- 新增通用生命周期信息时，应修改 `Manifest`、`Manifest::new`，并检查 [`plugin.rs`](plugin.rs) 的 `load`/`init`/`shutdown`/flush 路径；同步更新独立的 [`spi_test.rs`](spi_test.rs) 或 [`plugin_test.rs`](plugin_test.rs)，不要把测试嵌入生产文件。
- 新增 `ContextKey` 时，在拥有该业务语义的模块中定义零大小键类型并实现 `ContextKey`，选择可安全跨线程的 `Value`；为父级不变、同键遮蔽、跨克隆取消传播等语义增加同目录独立测试。若值注入位于会话适配层，还应覆盖相应 `pkg/session/runtime/*_test.rs`。
- 扩展 Authentication/Schema/Daemon 专用字段时，必须同时更新专用清单、承载扩展的回调结构、`ExportManifest` 实现、[`helper.rs`](helper.rs) 的 `declare_*_manifest` 和 [`helper_test.rs`](helper_test.rs) 的往返断言。只给结构体加字段而不更新导出/声明链会静默丢失数据。
- 在复用 `Manifest.extension` 前先确认类别协议。该槽当前只能保存一个具体 `Any` 值；若需要组合多种扩展，应设计显式容器或枚举，并保持已有 downcast 兼容，不能简单覆盖。
- 修改动态库后缀、入口符号、名称或版本语义时，应同步检查 `PluginLoader`、`load_one` 和 Go `spi.go`/`plugin.go`；这些值是加载 ABI/协议边界，存在平台兼容风险。
- 性能上重点关注 `Context::with_value` 的映射复制和大量清单克隆；只有实测证明它们进入高频路径后再改变表示，并用并发与往返测试保证共享状态、值隔离和回调身份不退化。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/plugin` 确认目标源、Go 对照与测试均已索引；`node --file pkg/plugin/spi.rs --offset 1 --limit 500` 读取目标文件全部 267 行；另读取 `plugin.rs`、`helper.rs`、`audit.rs`、`spi_test.rs`、`helper_test.rs`、`lib.rs` 的相关节点。
- RustCodeGraph 符号查询：`query export_manifest --kind function` 返回本文件的 trait 实现/自由函数及 `audit.rs` 实现；`query authentication_callbacks --kind function` 定位恢复入口；文件级引用显示 `spi.rs` 被 `plugin.rs`、`helper.rs` 与相关测试使用。由于重载符号的 `callers`/`callees` 没有返回精确边，使用 `rg` 对 `export_manifest`、`authentication_callbacks`、`Context` 方法、两项加载常量和 `Manifest::new` 做了定向补充核验。
- 直接读取：[`Cargo.toml`](Cargo.toml)、[`spi.go`](spi.go)、[`spi_test.go`](spi_test.go)；它们分别证明 crate/Go 包边界、原始 SPI 语义与 Go 导出回归意图。
- 独立 Rust 测试证据：[`spi_test.rs`](spi_test.rs) 验证值派生不修改父级、同键遮蔽、取消向克隆传播及审计清单回调往返；[`helper_test.rs`](helper_test.rs) 验证四类清单往返，尤其认证回调保持同一 `Arc`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求文档存在且恰含“文件定位、核心职责、主要符号、执行流程、数据与状态、依赖与调用关系、错误处理与边界、并发与资源生命周期、与 Go 版本的对应关系、扩展指南、验证依据”十一个二级章节。
