# 任务 1: int2到int4转换资格

批次：【批次 1】 无前置批次

状态：已完成，待回归

目的：只处理int2→int4有向对：前向E/A/I均允许；E为显式转换，A为目标列赋值，I为表达式隐式转换。数值舍入、溢出22003及NULL沿目标族，不能通过text串接。

来源任务：大计划6，`.plans/2026-10-24-postgresql18-后续整体实施/6-转换推导参数与元数据扩展.md`；路线图第3章显式隐式转换契约、准备描述执行元数据契约。

预计会话范围：一个有向类型对或一个推导/描述行为，复用相应类型族。只改该行为的分析、求值和必要协议接线及独立测试；不建立完整子系统。单次会话完成失败/通过证据及交接。

## 文件

候选修改：`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/expression/builtin_cast.rs`；协议必要接线在`pkg/server/runtime.rs`、`pkg/server/pg_extended.rs`、`pkg/server/conn.rs`、`pkg/server/pg_result.rs`。这些是候选落点，不要求全部修改。

测试：在已注册的`pkg/server/pg_extended_test.rs`追加待新增测试`pg06_task_001`，复用该文件read/send/query/parse/bind/execute和PgService、CreateAnalyzeSession真实入口。测试函数不得写入生产文件；不新增测试模块或Cargo feature。必要内核边界断言扩展`pkg/expression/builtin_cast_test.rs`，不得以其替代TCP行为验证。

## 上下文与接口依赖

大计划1、2及相应类型族的局部接口。执行前核对该最小接口存在且可消费；缺失时只在本任务记录精确签名、缺证及依赖，不递归扩建。`indexed_parameter_contract`负责参数映射，`prepare_statement_with_parameter_types`为准备入口；保留源类型，不靠Bind值或结果别名猜测。E/A/I表示显式/赋值/隐式资格，typmod表示声明长度或精度，OID表示PG类型或目录对象身份。

先读根AGENTS.md、pkg/server/AGENTS.md及存在的目标包doc.go（本轮三个包均无该文件），按rustcodegraph技能核查最新源码。大计划3–5拥有值模型，本任务只新增该有向差异或不可缺少的局部接线。每项生产修改在本文件记录其来源契约条目或接线依据。

## Cargo 共享槽位规则

1. 同一仓库的所有计划和任务共用仓库根目录下的 `target/rust-slot-1` 至 `target/rust-slot-10`，不得为任务、会话或计划另建构建目录，也不得按任务编号取模固定分配。
2. 每次构建/验证阶段开始前，创建仓库根目录下的 `target/rust-slot-locks`；依次用原子命令 `mkdir target/rust-slot-locks/slot-N.lock` 尝试领取槽位，`N` 为 1–10。仅 `mkdir` 成功者获得该槽位；这些相对路径命令从仓库根目录执行。
3. 领取成功后，在自己的锁目录中记录计划、任务、会话标识和持有进程 PID。同一槽位同时只允许一个持有者；已占用时尝试下一个，仅全部 10 个槽位都占用时等待并重试。
4. 将 `CARGO_TARGET_DIR` 设置为领取槽位的绝对路径，例如领取槽位 3 后，从仓库根目录执行 `export CARGO_TARGET_DIR="$PWD/target/rust-slot-3"`。示例编号不代表固定分配。
5. 所有 Cargo 构建、检查、测试以及调用 Cargo 的脚本和子进程均继承该设置，不得通过 `--target-dir` 或其他配置绕过槽位；生成的二进制也从该槽位定位。
6. 该阶段的 Cargo、脚本以及使用槽位产物的子进程全部结束后，删除自己写入的占用记录，并用 `rmdir` 释放自己的锁目录；保留槽位编译缓存供后续任务复用。
7. 正常结束、失败和中断均须清理自己的锁；在后台进程或其他使用槽位产物的进程仍运行时不得提前释放。规划清理步骤时覆盖这三种退出路径。
8. 遗留锁须先核实持有会话及相关进程已结束；无法确认则保持占用，不得删除其他活跃任务的锁、进程或缓存。
9. 生成或更新计划时，统一移除与这些规则冲突的单一 Rust 共享构建空间、逐任务独立目录和 Cargo 缓存全局独占窗口要求；仍须遵守源码文件占用、真实业务依赖和其他共享验证资源的互斥，不能仅因缓存分槽就把任务判定为可并行。
10. 最终验证记录包含实际槽位、绝对路径 `CARGO_TARGET_DIR`、确切命令、退出码和有效测试数量；Rust 代码修改后先运行 `cargo fmt --all`，再运行相应验证。编译成功或零测试不能替代行为验收证据。

## 测试计划

行为：只处理int2→int4有向对：前向E/A/I均允许；E为显式转换，A为目标列赋值，I为表达式隐式转换。数值舍入、溢出22003及NULL沿目标族，不能通过text串接。

真实输入：SELECT (1::int2)::int4；以int4目标列INSERT/UPDATE同源值；在仅接受int4的上下文验证I。覆盖目标边界两侧、舍入临界及typed NULL。

失败验证测试：待新增`pkg/server/pg_extended_test.rs::pg06_task_001`。失败验证命令（仓库根目录，默认features）：`env -u LDFLAGS cargo test --locked -p astersql-server --lib pg06_task_001 -- --nocapture`。新增测试必须先可编译，预期运行期在上述值、资格、参数身份或描述断言失败；先确认实际缺口。若当前行为已正确，只补覆盖并保存当前通过证据，不人为造红灯。

通过验证使用完全相同命令及输入。至少覆盖正常、边界、非法输入、NULL；错误比较SQLSTATE、发生阶段、无部分结果/副作用，Sync后显式`SELECT 1::int4`恢复。成功比较值、NULL、OID、typlen、typmod和格式；无结果字段明确不适用，未采集字段不能认定相等。模拟策略：真实共享内核、测试存储、临时TCP端口；不mock转换结果或元数据。参考PostgreSQL18.6采集准确预期，18.4仅单独报告，不自动替代18.6；缺固定参考构建时保留待回归及缺证。DSN由环境读取且不得输出密钥。

## 步骤与里程碑

1. 确认局部依赖，阅读契约对应方向与现有实现；在本文件登记实际受影响符号及来源。
2. 编写上述独立测试，领取槽位后运行失败验证并保存有效测试数量、具体断言与退出码。编译失败、零测试不算行为失败。
3. 仅实现本行为所需最小改动，保持Go/MySQL语义，保留PingCAP头；修复可用Rust文件顶部增加或保留`// Copyright 2026 AsterSQL.`。
4. 在槽位内先运行`cargo fmt --all`，再运行同一聚焦测试，验收该输入闭环。按该任务输入扩展真实参考夹具前，先核对现有libpq测试注册及接口，不假定新脚本已存在。
5. Ready交付运行`make lint`，审查diff；若修改Go导入/新增Go测试等触发根清单则先`make bazel_prepare`。不运行`make bazel_lint_changed`。使用Go测试时按testing-flow启用并最终禁用failpoints；Rust默认features，无新增开关。

## 验证与完成

命令：`cargo fmt --all`；`env -u LDFLAGS cargo test --locked -p astersql-server --lib pg06_task_001 -- --nocapture`；`make lint`；`git diff --check`。WIP用于聚焦迭代，Ready用于交付；verify-profile技能当前缺失，不宣称已使用该技能。新增有效测试至少1个，所有适用断言通过，保留失败/通过输出、真实参考版本与未验证项。同Server MySQL会话在PG流程前后验证原有整数值/CAST行为，避免方言泄漏。

涉及真实TiKV才按runbook后台启动专用playground、验证并清理自己服务和数据；不删除用户资源。外部Rust依赖不得vendor或本地patch，确有移植另走上游提交/tag流程。本任务不授权扩展依赖移植。

按Agent Output Contract报告文件、验证profile与理由、正确性/兼容性/性能风险、确切命令、退出码、测试数及未验证项。实际变更完成后使用$git-commit只提交本任务；仅状态更新/删除不单独提交。证据齐全才标记已完成并删除本编号文件；无关环境阻止验证可标记已完成，待回归并保留文件和缺证；依赖缺失标记已阻塞。不得修改plan.md。

## Progress

- [x] 核对依赖及生产差异依据。
- [x] 取得行为失败或既有正确行为的当前证据。
- [x] 完成实现、聚焦验证和Ready检查。
- [x] 完成交接、资源清理及适用提交。

## Surprises & Discoveries

规划时未运行测试；预期缺口须由实施前测试确认。

## Decision Log

2026-10-28：选择单行为最小接线与真实TCP验收，避免类型族重建；本编号文件按PLANS.md作为活动ExecPlan维护。

## Outcomes & Retrospective

实现与本地聚焦验收完成，保留18.6参考差分待回归；具体证据见执行记录。

## 重试与恢复

测试使用唯一临时表及系统分配端口；成功、失败及中断均关闭连接/listener、删除自己夹具，待子进程结束才释放槽位。可重复运行；保留缓存，不重置其他任务变更。

## 执行记录（2026-10-28）

已读取执行技能、根协议、server协议和PLANS.md，无前置批次。RustCodeGraph确认 pg_extended.rs::indexed_parameter_contract、runtime.rs::prepare_statement_with_parameter_types 可消费。verify-profile技能目录缺失，按任务规定的WIP聚焦检查与Ready交付命令执行，未声称调用该技能。

生产修改及契约依据：pkg/session/runtime/query.rs::relational_query_expression_type_from_sources 在PG分支仅为int2/int4混合加减乘复用既有pg_returning_expression_type整数提升规则，修正普通SELECT中误用MySQL int8默认类型的问题。对应本任务I方向int2→int4转换资格及OID身份契约，不建立新类型系统、不改变其他类型对及MySQL分支。原有求值器、E显式和A赋值路径均可复用，无其他生产改动。文件已保留AsterSQL及PingCAP版权。

新增并保留 pkg/server/pg_extended_test.rs::pg06_task_001：真实同Server TCP连接，检查E方向1、-32768、32767、typed NULL，numeric正负1.5先转int2的舍入，再转int4；A方向INSERT/UPDATE及最终列值；I方向int2/int4混合加法及NULL。检查值、OID23、typlen4、typmod-1和文本格式0；Prepare描述保留参数OID21，结果OID23；Bind边界输入成功，32768/-32769于Bind阶段返回22003、非法整数返回22P02，不发送Execute，因此无执行副作用/部分结果；每次错误后Sync返回Ready并明确SELECT 1::int4返回1。MySQL同Server前后CAST整数结果一致。无结果的DML描述字段不适用。

TDD证据：恢复追加测试后、生产修复前，完全相同聚焦命令实际运行1个测试，退出101，混合加法OID断言实际20/预期23。修复后同命令退出0，1 passed、0 failed、437 filtered out。初次INT4 DDL别名0A000与数组下标42601仅属夹具/范围外语法证据，未把它们计作转换红灯；改用既有INT列身份和混合整数算术，没有扩建这些子系统。曾将其判断为阻塞；用户明确授权继续本方向最小接线后，已消除该阻塞。

实际动态领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1。所有Cargo命令均继承该变量。验证命令及结果：

- cargo fmt --all：退出0；在测试前运行。
- env -u LDFLAGS cargo test --locked -p astersql-server --lib pg06_task_001 -- --nocapture：修复前101（1个有效测试失败）；修复后0（1个有效测试通过）。
- make lint：退出0；生产修改后再次执行亦退出0。
- git diff --check：退出0。
- /opt/homebrew/opt/postgresql@18/bin/postgres --version：退出0，PostgreSQL 18.4 (Homebrew)。

日志：/tmp/pg06-001-red.log、/tmp/pg06-001-green.log、/tmp/pg06-001-fmt.log、/tmp/pg06-001-lint-final.log（本地临时证据，不入库）。Cargo及使用槽位产物的测试子进程全部结束后，释放自己的slot-1.lock，保留缓存。没有Go源码/import/测试或Bazel变动，不触发bazel_prepare。未运行禁止的bazel_lint_changed。

交付结论：实现及聚焦验证完成，状态为已完成，待回归；按用户最新指示保留本文件供18.6参考采集，不修改plan.md。正确性风险仅限新增PG混合整数表达式类型分支；同Server MySQL回归已覆盖基本CAST，未执行完整双协议回归。性能仅增加目标PG表达式的局部类型检查，无性能基准。未验证PG18.6参考差分、扩展结果二进制格式、全部算术运算符及全量Rust套件；现有18.4不能代替18.6证据。下一步提供固定18.6参考后回归本任务输入与元数据，再删除本任务文件。
