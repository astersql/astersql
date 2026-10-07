# `br/pkg/streamhelper/prefix_scanner.rs`

## 文件定位

源文件：[prefix_scanner.rs](prefix_scanner.rs) 属于 `astersql-br-pkg-streamhelper` library crate。该 crate 的 [Cargo.toml](Cargo.toml) 以 `lib.rs` 为库入口；[lib.rs](lib.rs) 通过 `pub mod prefix_scanner` 装载本模块，并以 `pub use prefix_scanner::*` 把公开符号提升为 crate 级 API。文件没有 feature gate 或条件编译分支，独立测试由 `lib.rs` 在 `cfg(test)` 下挂载 [prefix_scanner_test.rs](prefix_scanner_test.rs) 和 [parity_test.rs](parity_test.rs)。

本文件负责一个窄而完整的能力：把有序键值源抽象成按前缀限定的分页扫描器。它自行计算前缀的半开区间上界、保存跨页游标、判断扫描结束，并在聚合扫描失败时保留此前成功页。当前 Rust 版本不包含 Go 同文件里的 etcd 适配器，仓库中 `Source` 的实现仅见于独立测试，因此它是已公开并经过契约测试的通用扫描组件，但尚不能据此认定 Rust 生产路径已经接入真实 etcd。

## 核心职责

1. `PrefixNextKey` 把任意字节前缀转换成可用于范围扫描的排他上界，从而把“键以 prefix 开头”表达为 `[prefix, PrefixNextKey(prefix))`。
2. `Source` 隔离实际存储：扫描器只要求下层按 `[from, to)` 返回最多一页有序 `Entry`，并用 `more` 表示是否还有数据。
3. `PrefixScanner` 维护下一页起点、固定上界和完成标志；`Page` 完成一次读取并推进游标，`AllPages` 循环聚合全部页。
4. `ScanAllPagesError` 在后续页失败时同时携带底层字符串错误和已经成功收集的条目，对齐 Go `AllPages` 返回“部分结果加 error”的契约。

## 主要符号

- `pub fn PrefixNextKey(key: &[u8]) -> Vec<u8>`：从尾到头对字节做带进位加一。遇到首个加一后非零的字节便截断其后后缀并返回；空输入或全 `0xff` 输入溢出为 `Vec::new()`，按 TiKV 约定表示无穷远上界。
- `pub trait Source: Send + Sync`：底层范围扫描接口。`Scan(&self, from, to, limit)` 返回 `Result<(Vec<Entry>, bool), String>`；元组第二项是 `more`。`Send + Sync` 约束允许线程安全来源被共享，但本文件自身不会启动并发任务。
- `pub struct PrefixScanner<'a>`：借用一个 `dyn Source`，内部保存 `next: Vec<u8>`、`end: Vec<u8>` 和 `done: bool`。字段私有，外部只能通过构造函数与方法维护状态。
- `pub struct ScanAllPagesError`：包含公开的 `entries: Vec<Entry>` 与 `source: String`。`Display` 只展示 `source`，同时实现标准 `Error`；调用方若要恢复部分结果必须读取 `entries`。
- `pub fn scanPrefix(src, prefix)`：以 UTF-8 字符串的原始字节初始化 `next`，以 `PrefixNextKey` 计算固定 `end`，并把 `done` 初始化为 `false`。
- `PrefixScanner::Page(size)`：向 `Source::Scan` 请求一页。`more == false` 时设置完成；`more == true` 时取最后一个返回键并追加 `0x00` 作为下一页的含式起点。
- `PrefixScanner::AllPages(size)`：持续调用 `Page` 直至 `Done`；成功时返回全部条目，失败时返回 `ScanAllPagesError` 和此前完整成功页。
- `PrefixScanner::Done()`：只读返回内部完成标志。

## 执行流程

构造扫描器时，`scanPrefix` 把字符串前缀复制到 `next`。`PrefixNextKey` 从前缀末尾开始加一：例如 `abc` 变为 `abd`，`[0x01, 0xff]` 进位并截断为 `[0x02]`。于是正常前缀的扫描区间固定为 `[next, end)`；空前缀或全 `0xff` 前缀的 `end` 为空，其“空上界等于正无穷”语义必须由 `Source` 实现遵守。

调用 `Page(size)` 时，扫描器把当前 `next`、固定 `end` 和页大小原样传给 `Source::Scan`。底层错误通过 `?` 立即返回，此时 `next` 与 `done` 都不变。若返回 `more == false`，本页仍正常返回，同时将 `done` 设为真。若返回 `more == true`，实现要求 `kvs` 非空，取最后条目的 `Key` 副本并追加零字节，将其写回 `next`。对于按字节字典序排列的键，`last_key + 0x00` 严格大于 `last_key`，又小于任何以 `last_key` 为真前缀且还有后续字节的键，因此不会重复上一条记录，也不会跳过合法的紧邻后继。

`AllPages(size)` 先以 `size as usize` 作为结果向量预容量，然后在 `Done()` 为假时调用 `Page`。每个成功页按原顺序追加；最后一页令 `done = true`，循环结束。若中间页失败，已追加的页被移入 `ScanAllPagesError.entries`，底层字符串放入 `source`；失败页没有部分条目接口，也不会把扫描器标为完成。

## 数据与状态

`PrefixScanner` 的权威可变状态只有 `next` 和 `done`；`end` 在构造后保持不变，`src` 是生命周期为 `'a` 的共享借用。扫描器不拥有或关闭数据源。`next` 始终表示下一次 `Source::Scan` 的含式起点，而 `done` 只在一次成功扫描报告 `more == false` 后变为真，不会复位。

条目类型是 [stubs.rs](stubs.rs) 中的 `Entry { Key: Vec<u8>, Value: Vec<u8> }`。`Page` 只检查最后条目的键来推进游标，不解释 value，也不自行过滤返回条目；范围正确性、键排序、页大小和 `more` 的真实性都属于 `Source` 契约。`parity_test.rs::MemSource::Scan` 通过半开区间过滤、按键排序、截断到 limit 并计算 `more`，展示了预期实现方式。

状态不是快照一致性保证。每页都是一次独立 `Scan`，页间底层数据可以变化；Go 原实现明确说明这种分页可能读取到不同 revision。Rust trait 没有 revision 或事务参数，因此也不能在本层弥补该限制。

## 依赖与调用关系

直接下游仅有 `crate::stubs::Entry` 和标准库的 `fmt`/`Error`/`Vec` 能力；本文件不直接依赖 `Cargo.toml` 中的 `serde`、`regex`、`uuid` 或两个 streamhelper 子 crate，也不连接网络 SDK。

模块入口 [lib.rs](lib.rs) 同时公开模块和所有公开符号。RustCodeGraph 对目标文件的索引显示它被 `prefix_scanner_test.rs`、`parity_test.rs`、`integration_test.rs` 以及两个宽泛聚合文件引用；结合精确源码检索，真正构造 `PrefixScanner` 的 Rust 调用点位于前两个独立测试，`integration_test.rs` 仅触达 `PrefixNextKey`。仓库内没有生产 Rust `impl Source`，也没有与 Go `scanEtcdPrefix` 对应的真实 etcd 适配器，因此当前生产上游调用关系尚未建立。

Go 侧的实际生产链更完整：[prefix_scanner.go](prefix_scanner.go) 的 `etcdSource::Scan` 调用 etcd `Get`，`scanEtcdPrefix` 负责适配客户端；[client.go](client.go) 中 `GetAllTasksWithRevision`、`Task::Ranges`、`NextBackupTSList` 与 `GetStorageCheckpoint` 等路径以不同页大小调用 `AllPages`。这些调用说明本抽象在完整 BR streamhelper 中应处于元数据前缀读取边界，但只能作为 Rust 后续接线的语义依据，不能当成 Rust 已接线证据。

## 错误处理与边界

`Source::Scan` 只提供 `String` 错误，`Page` 原样传播；Rust 版本不像 Go `etcdSource::Scan` 那样为错误附加扫描范围并对键做脱敏。`AllPages` 会保留此前成功页，但 `ScanAllPagesError::Display` 只输出底层消息。调用方既要记录错误，也要明确决定是否使用 `entries`，以免把不完整扫描误当成完整结果。

`PrefixNextKey` 对空输入和全 `0xff` 返回空向量，而不是追加零字节；这与 TiKV client-go 契约一致。该结果只有在 `Source` 把空 `to` 解释为正无穷时才正确。普通内存实现若直接使用 `key < to`，会令空上界匹配不到任何键；`parity_test.rs::MemSource` 只覆盖普通非空上界，未验证该特殊适配责任。

`Page` 隐含要求 `more == true` 时 `kvs` 必须非空，否则 `kvs[kvs.len() - 1]` 会 panic。它也信任最后一条是本页最大键；无序或重复页可能造成回退、重复、遗漏或死循环。`done == true` 后直接再次调用 `Page` 仍会访问数据源，因为方法本身不做短路，正常调用者应通过 `Done` 或 `AllPages` 控制生命周期。

页大小没有显式校验。`AllPages` 将 `i32` 直接转为 `usize` 创建容量，负值可能变成巨大容量并导致分配失败；零值或负值传给 `Source` 的行为也未定义。安全调用应使用正数，并要求来源在 `more == true` 时至少返回一个条目。现有测试覆盖前缀进位/溢出、正常多页聚合和第二页错误保留结果，但尚未覆盖空 `more` 页、无序来源、非正页大小、完成后重复 `Page` 或空上界来源语义。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、channel、事务或网络连接。`Source: Send + Sync` 使来源对象具备跨线程共享资格；`PrefixScanner` 只持有共享引用，但分页方法需要 `&mut self`，因此同一个扫描器的游标不能被多个调用者同时推进，除非外层另行加锁。这个独占可变借用保证单个实例内 `next` 和 `done` 的更新顺序化。

扫描器生命周期受借用的 `Source` 约束，离开作用域即释放自己的三个小状态字段，不负责清理来源。`Page` 成功返回的 `Vec<Entry>` 和 `AllPages` 聚合结果拥有条目副本；游标也克隆最后一个键，因此来源后续修改不会悬垂引用。另一方面，`AllPages` 会把所有匹配条目保留在内存中，数据量大时内存随总结果线性增长；分页只限制单次来源响应，不限制最终聚合内存。

Rust API 没有 Go 版本的 `context.Context`，因此本层没有取消、deadline 或跨页一致性 revision。若真实网络来源未来接入，取消与超时必须加入 `Source` 契约或由来源使用外部共享状态实现，不能假设现有方法会响应取消。

## 与 Go 版本的对应关系

Rust 的 `Source`、`PrefixScanner`、`scanPrefix`、`Page`、`AllPages` 和 `Done` 逐项对应 [prefix_scanner.go](prefix_scanner.go) 中的 `source`、`prefixScanner` 及其同名逻辑。两边都使用 `PrefixNextKey(prefix)` 构造排他上界；`more` 为真时都以最后键追加 `0x00` 推进；`more` 为假时都把 `done` 置真；聚合出错时都返回此前已完成页。

Rust 自行实现 `PrefixNextKey`，并由 `prefix_scanner_test.rs::prefix_next_key_matches_tikv_overflow_contract` 固定空输入、全 `0xff` 和进位截断行为；Go 直接调用 `github.com/tikv/client-go/v2/kv.PrefixNextKey`。Rust 的 `ScanAllPagesError` 是为表达 Go 的 `([]kv.Entry, error)` 双返回而引入的结构体，错误页之前的结果不会丢失。

当前差异包括：Go `Scan` 接受 `context.Context` 且返回三个独立值，Rust 无 context 并用 `Result<(entries, more), String>`；Go 文件包含可用的 `etcdSource` 与 `scanEtcdPrefix`，Rust 没有；Go etcd 错误会附加脱敏后的范围上下文，Rust 原样返回字符串；Go 页大小为平台宽度 `int`，Rust 固定为 `i32`。两边都隐含“`more` 为真则页面非空”的前提，也都没有在扫描器层验证正页大小。

## 扩展指南

接入真实 Rust etcd 客户端时，最小扩展点是新增独立适配器实现 `Source`，而不是把网络逻辑塞入本文件。适配器必须保证键按字典序排列、范围为 `[from, to)`、空 `to` 表示正无穷、正 limit 下 `more == true` 必伴随非空页，并为错误增加脱敏后的范围上下文。若需要与 Go 等价的取消和 revision 一致性，应先设计 `Source` 的上下文/快照契约；这会影响公开 trait，不能只在某个实现中悄悄改变语义。

加强输入健壮性时，修改入口应是 `Page`/`AllPages` 的页大小校验和 `Page` 对矛盾 `(empty, more=true)` 的显式错误，而不是依靠索引 panic。任何错误类型调整都要保持 `AllPages` 可携带部分结果的能力，并评估现有 `String` 调用方兼容性。游标算法若改变，必须证明对二进制键的“不重复、不遗漏、单调前进”不变量，尤其要覆盖某键本身是另一键前缀的情况。

测试逻辑继续放在独立 [prefix_scanner_test.rs](prefix_scanner_test.rs)，不要内嵌到生产文件。建议补充：普通多页的精确游标参数、空最后页、矛盾 `more`、非正 size、全 `0xff`/空前缀的来源适配、来源错误后重试以及完成后调用行为；跨语言稳定契约同步扩展 [parity_test.rs](parity_test.rs)。若新增 etcd 适配器，还应增加独立集成测试验证 revision、取消、范围错误脱敏和数据在翻页期间变化的行为，并与 Go [prefix_scanner.go](prefix_scanner.go) 及 [client.go](client.go) 的调用语义对照。

## 验证依据

- 目标源码：RustCodeGraph `node --file br/pkg/streamhelper/prefix_scanner.rs --offset 1 --limit 260`，覆盖文件全部 105 行；确认一个函数、一个 trait、两个 struct、三个 impl/方法组，无条件编译项。
- crate 与模块边界：[Cargo.toml](Cargo.toml) 和 [lib.rs](lib.rs)；确认 crate 名、library 入口、公开模块、包级再导出及独立测试挂载。
- 数据模型：[stubs.rs](stubs.rs) 的 `Entry`；确认键和值都是拥有所有权的二进制向量。
- 调用图：RustCodeGraph `explore "br/pkg/streamhelper/prefix_scanner.rs PrefixScanner prefix scan next reset"`、`files --filter br/pkg/streamhelper` 及精确源码检索；确认 Rust 构造/聚合调用位于 `prefix_scanner_test.rs` 和 `parity_test.rs`，没有生产 `impl Source`。宽泛图查询会混入同名 Go/Rust 符号，因此最终调用结论以文件限定结果和源码引用交叉核对。
- Go 对照：[prefix_scanner.go](prefix_scanner.go) 全文件及 [client.go](client.go) 的 `scanEtcdPrefix`/`AllPages` 调用点；确认 etcd 适配、Context、错误注解、生产调用位置和分页语义。
- 独立 Rust 测试：[prefix_scanner_test.rs](prefix_scanner_test.rs) 覆盖上界溢出和第二页错误时保留成功结果；[parity_test.rs](parity_test.rs) 的 `MemSource` 与 `go_rust_public_contract_matches` 覆盖排序、范围过滤、页大小为 1 的多页聚合及前缀外键排除；[integration_test.rs](integration_test.rs) 仅验证符号可链接。
- 同路径不存在 `prefix_scanner_test.go`；Go 行为依据来自生产实现及 Rust 的 Go/Rust 契约测试。任务为纯文档分析，按计划未运行 Cargo；交付采用结构检查和人工事实复核。
