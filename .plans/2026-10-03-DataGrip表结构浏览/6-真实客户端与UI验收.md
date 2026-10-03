# 任务 6: 真实客户端与UI验收

批次：【批次 6】 依赖全部前置批次 1 至 5

状态：已阻塞

目的：全部完整 SQL 通过真实 JDBC/PG listener，真实 DataGrip 表树显示表与结构；Ready 检查及手册更新。

来源任务：用户授权“扩展范围，继续修复 DataGrip 表和结构浏览”；2026-10-03 21:10:20 session 1533977259。

预计会话范围：仅 PG 适配模块，按一个验证阶段推进；后续查询差异不吞入当前阶段。

## 文件

pkg/server/pg_client_integration_test.rs、docs/postgresql-protocol-first-phase.md。测试统一在独立 Rust 测试文件，新增注册位于 pkg/server/lib.rs。

## 上下文

执行前读取根 AGENTS.md、PLANS.md、docs/agents/testing-flow.md 和本目录 plan.md。pkg/server/doc.go 不存在。PG 独立 CatalogQuery 执行器已提供参数绑定、JOIN、CTE、快照和有界工作量。

## 测试计划

行为：全部完整 SQL 通过真实 JDBC/PG listener，真实 DataGrip 表树显示表与结构；Ready 检查及手册更新。

先在 pkg/server/pg_datagrip_test.rs 或既有真实客户端回归加入测试；用完整日志 SQL 与真实元数据断言复现失败。

失败和通过验证命令（从仓库根执行、领取槽位并设置 CARGO_TARGET_DIR 后）：

    PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc cargo test -p astersql-server pg_datagrip --lib -- --test-threads=1

预期失败原因：当前受限语法、提供器或协议格式尚未覆盖来源查询。真实 session/listener 优先，不 mock 系统目录。

## 步骤与里程碑

先冻结完整来源并编写失败回归；再实现最小 AST、元数据或格式接线。运行聚焦测试并核对真实对象行与类型。新增语法必须保留原有界限及错误恢复，完成后自审差异、运行适用 Ready 检查，按 git-commit 技能提交本任务。

## 验证和完成

记录确切命令、实际构建槽位、预期失败与通过数量。所有生产修改对应原始日志 SQL 或不可缺少的局部接线。编译成功和零测试不是完成证据。完整 UI 验收仅在批次 6 声明；当前阶段不能吞并其余任务。

## Progress

- [x] 来源与失败回归：27 条完整 SQL、两版真实 JDBC；3 项生产缺口均真实复现。
- [x] 修复及聚焦验证：最终 19/19 通过，两版 JDBC 各 27/27 完整 SQL 通过；已同步并核对最终源码逐字节一致。
- [x] Ready 适用检查；本阶段生产、测试、手册和编号记录独立提交。
- [ ] 真实 DataGrip UI 验收：已阻塞，缺少 macOS Accessibility 和 Screen Recording 系统权限。

## Surprises & Discoveries

待执行阶段记录实际发现。

## Decision Log

2026-10-03：所有阶段串行，复用现有 PG 边界且保留 MySQL 隔离。

## Outcomes & Retrospective

尚未获得本阶段完成证据。

## 批次 6 执行记录（2026-10-04）

先读取 plan.md（只读）、本任务文件、PLANS.md、testing-flow.md；pkg/server/doc.go 不存在。使用仓库 skills/rustcodegraph/SKILL.md 搜索 PG 解析/执行器，并读取 git-commit 技能。仓库 .agents/skills/tidb-verify-profile/SKILL.md 不存在，按 Ready 规则选检查，不声称使用缺失技能。无 Go/Bazel/依赖变更，无 bazel_prepare 触发项。

共享窗口先暂停 Cargo/全局格式化，随后暂停共享 Rust 写入。独立验证采用 git archive 的完整 SHA 05d04a93ac17c81f6295b102dfe22ec82bfb67aa（批次 5 交付）到 /tmp/astersql-dg6-verify，叠加本任务文件；不修改 Cargo manifest、不使用本地 patch、不修改任何上游 Git 依赖。原子 mkdir target/rust-slot-locks/slot-8.lock；启动 shell PID=83572，首次 Cargo PID=84643，后续 Cargo PID=42611、49363。CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-8。仍在使用自有锁；结束后释放锁、保留缓存。

完整来源为 testdata/pg_datagrip 的 25 条日志 SQL，另从既有手册冻结 RetrieveColumns 和 RetrieveIndexColumns（只将 $1 转为 JDBC ?）。测试保持 27 条来源不简化。每条执行 namespace 字面量与绑定参数，每种模式重复两次并读所有列；断言真实表、主键索引/约束、列类型和索引列身份。首次漏绑重复 namespace 参数的问题已修正为绑定 ParameterMetaData 宣告的全部参数，旧轮次中的“参数 2 未设定”不属于服务器缺口。逐条收集错误，测试最终仍失败，且两版驱动都会运行；保留既有 CRUD、MySQL 隔离、libpq、SQLSTATE/恢复断言。

修复前最终有效红灯：/tmp/datagrip6-complete-clients.log；JDBC 42.7.13/42.7.3 各 24/27 查询通过，1869280137.sql 在 ::varchar::bigint 被原生解析拒绝、1869280139.sql 使用 usesuper 被原生解析拒绝、RetrieveIndexColumns.sql 报 expected from。Rust 客户端组 2 通过/1 失败。原有批次 1—4 目录回归（基线二进制）5/5 通过，/tmp/datagrip6-catalog.log，仅是前置证据，不替代新增 SQL 的失败/通过。

生产修改与完整来源依据：pg_catalog_query.rs 的标量 PG 探针路由、无 FROM 单行标量、余数用于 1869280137.sql；current_user/pg_user 路由用于 1869280139.sql；数组下标、减法、int 标量转换、chr/连接、多 WHEN CASE、CROSS JOIN、unnest WITH ORDINALITY 与标量表函数用于完整 RetrieveIndexColumns.sql。pg_catalog.rs 对应表达式类型/执行、前缀作用域与逐行表函数物化；补齐 indcollation 的每列无 PG 身份 OID 0，未知 PG 访问方法 can_order 为 NULL。pg_user 来自原生 mysql.user 的 Super_priv，不伪造权限；txid_current 来自 @@tidb_current_ts，未活跃事务为 0，是原生 TSO 而非 PostgreSQL wraparound XID。保持查询深度、谓词/分支数、行数、共享工作量与取消限制。

修复后的 /tmp/datagrip6-fix2-clients.log：两版 JDBC 各 27/27 完整 SQL 通过，客户端组 3/3 通过，未忽略；libpq 18 协议 3.0/3.2、MySQL 隔离与存储视图源仍通过。新增复合索引、向量下界 0/数组下界 1、NULL/越界、算术错误、非法函数参数/作用域和取消测试在隔离快照；初次新测试断言使用错误的第 14 列（真实结果 13 列）已在快照修正，不记为生产红灯。

UI：通过 cua.getApp("DataGrip") 连续三次读取，工具均返回 Computer Use permissions are still pending，Accessibility 和 Screen Recording 未授予。没有 UI 状态、截图、表树成功证据；不绕过系统权限、不使用 shell/AppleScript 控制 UI，不宣称 UI 验收通过。

Ready 初步：make lint 退出 0（/tmp/datagrip6-lint.log）；cargo fmt --all 仅在隔离快照退出 0（/tmp/datagrip6-fmt.log）；已有 diff check 退出 0。最新边界测试/最终格式尚待窗口恢复同步；不得提交共享源码旧版本冒充快照最终绿色。

确切验证命令：

    target/rust-slot-8/debug/deps/astersql_server-ae1da1502f93adcd pg_datagrip --test-threads=1
    CARGO_TARGET_DIR=$PWD/target/rust-slot-8 PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc cargo test --manifest-path /tmp/astersql-dg6-verify/Cargo.toml -p astersql-server pg_introspection_clients --lib -- --test-threads=1
    CARGO_TARGET_DIR=$PWD/target/rust-slot-8 PROTOC=/opt/homebrew/opt/protobuf@21/bin/protoc cargo test --manifest-path /tmp/astersql-dg6-verify/Cargo.toml -p astersql-server pg_datagrip_ui_metadata_core --lib -- --test-threads=1
    cargo fmt --all --manifest-path /tmp/astersql-dg6-verify/Cargo.toml
    make lint
    git diff --check -- pkg/server/pg_client_integration_test.rs docs/postgresql-protocol-first-phase.md pkg/server/testdata/pg_datagrip

阶段仍未完成：需最终边界/相邻回归、窗口恢复后同步、提交，以及有权限环境中的真实 DataGrip 表树验收。只修改本阶段差异；plan.md 的既有他人变更未覆盖、未暂存。

2026-10-04 最终隔离聚焦回归：/tmp/datagrip6-ready-focused.log，19 通过、0 失败、0 忽略（解析器 10、DataGrip 目录/新增边界 6、真实客户端 3）。该最终命令是 cargo test --manifest-path /tmp/astersql-dg6-verify/Cargo.toml -p astersql-server --lib -- pg_catalog_query_test:: pg_datagrip pg_introspection_clients --test-threads=1 --nocapture；PROTOC 与 CARGO_TARGET_DIR 同上。最后 Cargo PID=54320，已自然结束；已验证锁内 pid=83572/cargo-pid=54320 并仅删除自有 slot-8.lock，缓存保留。随后暂停一切新增 Cargo。根和 server manifest 检查没有共享工作区绝对路径；原有 crates.io patch 为 Git rev，无新增本地 patch。测试源码来自独立快照而非共享工作区。

新增边界回归曾错误假设 root 的 Super_priv 为 Y；真实原生测试库为 N，现改为核对原生字段并更新为 Y 后验证新 Execute 刷新，不修改生产映射以迎合测试。最后既有语法错误 SQLSTATE 回归发现 SELECT FROM 被可选 FROM 改为 0A000，修正为 42601；19 项最终回归全部绿色。

Ready：最后隔离 cargo fmt --all 与共享 make lint 均退出 0（/tmp/datagrip6-final-fmt.log、/tmp/datagrip6-ready-lint.log）。目前隔离的 pg_catalog.rs、pg_catalog_query.rs、pg_datagrip_test.rs、pg_catalog_query_test.rs 与共享 Rust 文件尚不完全相同，包含最终边界和语法修正/格式；需在窗口结束后同步这四个文件。pg_client_integration_test.rs 与两个 SQL 夹具已为最终测试来源。暂不提交未同步的旧版源码，不删除尚需同步的 /tmp/astersql-dg6-verify。

## 窗口恢复与最终交付（2026-10-04）

独占窗口结束后检查 Git：从验证基线 05d04a93ac17c81f6295b102dfe22ec82bfb67aa 到当前 HEAD 7d80e2e64c 的七项他人提交均未修改本任务五个 PG Rust 文件。暂停前哈希和当前共享文件比较：pg_catalog.rs、pg_catalog_query_test.rs、pg_client_integration_test.rs 未变化；pg_catalog_query.rs 与 pg_datagrip_test.rs 只有协调全局格式化变化，与最终快照的差异均为本任务尚未同步的错误类别/测试断言修正。未发现他人同路径生产修改。已同步五个 PG Rust 文件（包含原本一致的客户端文件），每个文件 SHA256 与最终格式化/19 项测试通过的快照逐字节一致。

遵照新协调指令，不再运行共享 cargo fmt --all，也不启动新的 Cargo；复用隔离最终格式化、聚焦验证及 Ready make lint 证据。交付追加检查：git diff --check、git diff --cached --check，核对暂存仅包含本任务五个 Rust 文件、两个 SQL 夹具、PG 手册与编号记录。编号文件最初已有的两个“独立会话”协调改动保留在工作区、不混入本提交；plan.md、任务 5、其余计划及源码不暂存。

最终状态为已阻塞，阻塞范围仅真实 DataGrip UI：macOS 未授予 Computer Use Accessibility（辅助功能）和 Screen Recording（屏幕录制）权限，三次工具请求均返回 permissions are still pending，无可观察 UI 状态。无需新增权限的生产修复、完整 SQL 客户端、边界回归、Ready 与交付均已完成。应在系统权限具备后重新打开 DataGrip，刷新 PostgreSQL 数据源并核对 public 表、列、主键索引和约束树；这一步完成之前不得标记整阶段已完成。

正确性/兼容风险：仅支持冻结查询所需有界 PG 目录语法；txid_current 使用原生 TSO、未知访问方法属性为 NULL，不承诺 PG XID/复制/生产鉴权或任意 PG SQL。性能为带预算的逐行数组展开/目录连接，仍执行取消、行数和工作量检查。未本地验证完整最新共享工作区 Cargo 构建、持久重启、RealTiKV 或大 schema 性能；最终 PG 文件通过隔离基线与逐字节交付核对。自有槽位锁已释放，缓存保留；同步完成后清理仅本任务临时源码快照。

## UI 阻塞恢复判定（2026-10-04）

阻塞范围仅真实 DataGrip UI；生产修复、完整客户端与 Ready 检查已通过并在 4b44877fbf 提交。主状态严格为“已阻塞”，不以 JDBC 的 27/27 通过替代表树证据，也不以仅尝试打开应用认定成功。

复核本会话三次 cua.getApp("DataGrip") 的实际结果：每次都返回“Computer Use permissions are still pending”，说明用户尚未完成 ChatGPT Computer Use 窗口中的 Accessibility / Screen Recording 授权；三次均没有返回 DataGrip AX 树或截图。没有收到权限已开启的证据，本次不重复同一失败调用。允许的 Computer Use 原生 UI 入口均受此权限门控制；当前没有能操作 DataGrip 原生窗口的专用连接器、用户提供的 UI 截图或其他获授权的 UI 通道。浏览器页面、JDBC/SQL 回归和 DataGrip 日志不能观察 Database Explorer 表树；仅限语音会话的屏幕上下文工具不适用于本任务，shell/AppleScript 等替代 UI 控制被 Computer Use 协议禁止。因此本会话现有安全 UI 访问方案已穷尽，恢复依赖用户完成系统授权。

授权对象是 Computer Use 的宿主应用，不是 DataGrip，也不是数据库服务。只读进程检查显示本机运行宿主可执行文件为 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT；该应用 Info.plist 的 CFBundleDisplayName=ChatGPT、CFBundleIdentifier=com.openai.codex。/Applications/Codex.app 的 bundle ID 同为 com.openai.codex，因此系统权限条目可能显示 ChatGPT 或 Codex；应以正在运行的 /Applications/ChatGPT.app 和权限窗口指向的宿主为准，避免给同名旧安装副本授权。

用户恢复操作：打开 macOS“系统设置 → 隐私与安全性 → 辅助功能”，为上述宿主开启 Accessibility；再在“屏幕录制”或“屏幕与系统音频录制”中为同一宿主开启 Screen Recording。按系统提示退出并重新打开该宿主应用，然后在 Computer Use 权限窗口完成检查，并在本会话告知权限已开启。授予这些权限会允许宿主读取屏幕和操作原生 UI，需由用户亲自完成；代理没有修改系统权限或绕过权限门。

收到恢复证据后，先用 cua.getApp("DataGrip") 检查实际 AX 状态/截图可用，再连接测试 PG listener，刷新 PostgreSQL 数据源并展开 public 中真实表、列、主键索引与约束，核对名称、列类型和对象身份，记录成功或具体失败证据。只有真实 UI 验收通过后才把本阶段判为已完成并按调度协议删除编号文件；若仍无法读取窗口，保持已阻塞并记录新的实际原因。

本次仅更新编号记录，不改变生产代码、测试或系统设置；Ready 文档交付检查为 git diff --check 和 git diff --cached --check，无代码构建/测试触发项。编号文件原有两处“独立会话”协调差异继续保留在工作区并排除出本记录提交。
