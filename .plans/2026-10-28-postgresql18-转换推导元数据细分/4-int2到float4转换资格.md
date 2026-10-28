# 任务 4: int2到float4转换资格

批次：【批次 4】 依赖批次 3（共享源码与验证资源串行交接）

状态：已完成，待回归

目的：只处理int2→float4有向对：前向E/A/I均允许；E为显式转换，A为目标列赋值，I为表达式隐式转换。数值舍入、溢出22003及NULL沿目标族，不能通过text串接。

来源任务：大计划6，`.plans/2026-10-24-postgresql18-后续整体实施/6-转换推导参数与元数据扩展.md`；路线图第3章显式隐式转换契约、准备描述执行元数据契约。

预计会话范围：一个有向类型对或一个推导/描述行为，复用相应类型族。只改该行为的分析、求值和必要协议接线及独立测试；不建立完整子系统。单次会话完成失败/通过证据及交接。

## 文件

候选修改：`pkg/session/runtime/planning.rs`、`pkg/session/runtime/query.rs`、`pkg/expression/builtin_cast.rs`；协议必要接线在`pkg/server/runtime.rs`、`pkg/server/pg_extended.rs`、`pkg/server/conn.rs`、`pkg/server/pg_result.rs`。这些是候选落点，不要求全部修改。

测试：在已注册的`pkg/server/pg_extended_test.rs`追加待新增测试`pg06_task_004`，复用该文件read/send/query/parse/bind/execute和PgService、CreateAnalyzeSession真实入口。测试函数不得写入生产文件；不新增测试模块或Cargo feature。必要内核边界断言扩展`pkg/expression/builtin_cast_test.rs`，不得以其替代TCP行为验证。

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

行为：只处理int2→float4有向对：前向E/A/I均允许；E为显式转换，A为目标列赋值，I为表达式隐式转换。数值舍入、溢出22003及NULL沿目标族，不能通过text串接。

真实输入：SELECT (1::int2)::float4；以float4目标列INSERT/UPDATE同源值；在仅接受float4的上下文验证I。覆盖目标边界两侧、舍入临界及typed NULL。

失败验证测试：待新增`pkg/server/pg_extended_test.rs::pg06_task_004`。失败验证命令（仓库根目录，默认features）：`env -u LDFLAGS cargo test --locked -p astersql-server --lib pg06_task_004 -- --nocapture`。新增测试必须先可编译，预期运行期在上述值、资格、参数身份或描述断言失败；先确认实际缺口。若当前行为已正确，只补覆盖并保存当前通过证据，不人为造红灯。

通过验证使用完全相同命令及输入。至少覆盖正常、边界、非法输入、NULL；错误比较SQLSTATE、发生阶段、无部分结果/副作用，Sync后显式`SELECT 1::int4`恢复。成功比较值、NULL、OID、typlen、typmod和格式；无结果字段明确不适用，未采集字段不能认定相等。模拟策略：真实共享内核、测试存储、临时TCP端口；不mock转换结果或元数据。参考PostgreSQL18.6采集准确预期，18.4仅单独报告，不自动替代18.6；缺固定参考构建时保留待回归及缺证。DSN由环境读取且不得输出密钥。

## 步骤与里程碑

1. 确认局部依赖，阅读契约对应方向与现有实现；在本文件登记实际受影响符号及来源。
2. 编写上述独立测试，领取槽位后运行失败验证并保存有效测试数量、具体断言与退出码。编译失败、零测试不算行为失败。
3. 仅实现本行为所需最小改动，保持Go/MySQL语义，保留PingCAP头；修复可用Rust文件顶部增加或保留`// Copyright 2026 AsterSQL.`。
4. 在槽位内先运行`cargo fmt --all`，再运行同一聚焦测试，验收该输入闭环。按该任务输入扩展真实参考夹具前，先核对现有libpq测试注册及接口，不假定新脚本已存在。
5. Ready交付运行`make lint`，审查diff；若修改Go导入/新增Go测试等触发根清单则先`make bazel_prepare`。不运行`make bazel_lint_changed`。使用Go测试时按testing-flow启用并最终禁用failpoints；Rust默认features，无新增开关。

## 验证与完成

命令：`cargo fmt --all`；`env -u LDFLAGS cargo test --locked -p astersql-server --lib pg06_task_004 -- --nocapture`；`make lint`；`git diff --check`。WIP用于聚焦迭代，Ready用于交付；verify-profile技能当前缺失，不宣称已使用该技能。新增有效测试至少1个，所有适用断言通过，保留失败/通过输出、真实参考版本与未验证项。同Server MySQL会话在PG流程前后验证原有整数值/CAST行为，避免方言泄漏。

涉及真实TiKV才按runbook后台启动专用playground、验证并清理自己服务和数据；不删除用户资源。外部Rust依赖不得vendor或本地patch，确有移植另走上游提交/tag流程。本任务不授权扩展依赖移植。

按Agent Output Contract报告文件、验证profile与理由、正确性/兼容性/性能风险、确切命令、退出码、测试数及未验证项。实际变更完成后使用$git-commit只提交本任务；仅状态更新/删除不单独提交。证据齐全才标记已完成并删除本编号文件；无关环境阻止验证可标记已完成，待回归并保留文件和缺证；依赖缺失标记已阻塞。不得修改plan.md。

## Progress

- [x] 核对依赖及生产差异依据。
- [x] 取得行为失败或既有正确行为的当前证据。
- [x] 完成实现、聚焦验证和Ready检查。
- [x] 完成交接、资源清理及适用提交。

## Surprises & Discoveries

独立TCP测试证实混合int2/float4算术被描述为int8（OID20）；局部PG类型推导修复后同一命令通过。int2源域完全可由float4精确表示，目标舍入/溢出边界对本方向不适用。

## Decision Log

2026-10-28：选择单行为最小接线与真实TCP验收，避免类型族重建；本编号文件按PLANS.md作为活动ExecPlan维护。

## Outcomes & Retrospective

本地行为闭环及Ready检查通过；固定PostgreSQL18.6参考差分缺证，保留任务供回归。

## 重试与恢复

测试使用唯一临时表及系统分配端口；成功、失败及中断均关闭连接/listener、删除自己夹具，待子进程结束才释放槽位。可重复运行；保留缓存，不重置其他任务变更。

## 执行记录（2026-10-28）

批次3提交92175b985b已交付局部接口并通过TCP测试，可消费indexed_parameter_contract和prepare_statement_with_parameter_types；18.6参考差分的环境缺证独立保留。已读根/server规则、PLANS.md、执行/RustCodeGraph技能及路线图第3章对应契约；三个目标包无doc.go，verify-profile技能不存在。RustCodeGraph已核对query.rs::relational_query_expression_type_from_sources：PG混合int2/numeric已有定型分支，int2/float4尚待TCP证实。新增pg06_task_004覆盖E/A/I、源域上下界、NULL、参数OID21和结果700/4/-1、文本格式0、Bind错误及Sync恢复、同Server MySQL前后隔离。int2完整域[-32768,32767]均能精确表示为float4，因此目标溢出/舍入临界不在该有向转换的可达输入域内；不借反向转换或text串接制造伪边界。源边界外32768/-32769在Bind验22003，非法源值验22P02。

PG18_REFERENCE_DSN未配置；本机postgres仅18.4 (Homebrew)，不能代替18.6参考。先取得本地行为证据，最后保留参考差分待回归。

首次聚焦验证：动态槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1；cargo fmt --all完成后，同一计划命令实际运行pg06_task_004 1个测试，退出101，OID断言实际20/预期700（日志/tmp/pg06-004-initial.log）。生产修改仅query.rs::relational_query_expression_type_from_sources的PG分支，为int2与float4混合加减乘返回TypeFloat；依据路线图第3章前向I资格及float4结果700/4/-1契约，复用既有值转换，不开放其他类型对。两方版权头已存在并保留。相同命令和输入将复验。

最终验收：pg06_task_004实际运行1个测试，1 passed、0 failed、440 filtered out，退出0；日志/tmp/pg06-004-final.log。显式E覆盖1、-32768、32767及typed NULL；A覆盖INSERT后的32767读回、UPDATE后的-32768和NULL；I覆盖int2与float4小数双侧加法、减法、乘法、两端值及NULL。简单查询和扩展两种Describe均核对结果700/4/-1及格式0；显式和隐式准备语句的ParameterDescription均固定源OID21，重绑定上下界与NULL不改变结果描述。无结果INSERT明确NoData，结果字段不适用。非法源值在Bind即返回22003/22P02且无DataRow，Sync后SELECT 1::int4恢复，id=2计数始终0；同Server MySQL前后CAST(32767 AS SIGNED)结果一致。临时表DROP成功，Cleanup关闭服务。

TDD证据：首次新增测试编译成功，实际运行1个失败（退出101，OID20/700）；8行PG类型身份修复后相同命令/输入1个通过（退出0）；补强扩展隐式描述及失败INSERT无副作用后最终同一命令1个通过（退出0）。生产修改只涉及本任务int2/float4加减乘的结果身份，MySQL路径及依赖均不变；版权头保留。

确切验证命令（仓库根目录）：

- cargo fmt --all：三阶段均完成，最终退出0；日志/tmp/pg06-004-fmt.log、/tmp/pg06-004-fmt-green.log、/tmp/pg06-004-fmt-final.log。
- env -u LDFLAGS cargo test --locked -p astersql-server --lib pg06_task_004 -- --nocapture：首次101（1 failed），修复后0（1 passed），补强后0（1 passed）；日志分别/tmp/pg06-004-initial.log、/tmp/pg06-004-green.log、/tmp/pg06-004-final.log。
- make lint：退出0；日志/tmp/pg06-004-lint.log；后续仅追加Rust测试断言，未改变Go检查输入。
- git diff --check：退出0；最终自审无越界变动。
- /opt/homebrew/opt/postgresql@18/bin/postgres --version：退出0，仅18.4 (Homebrew)，只检测版本，没有作为18.6参考实测。

三次Cargo阶段均动态原子领取槽位1，CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1，owner记录任务、会话及持有shell PID；全部Cargo与测试子进程退出后以EXIT trap unlink自己的owner并rmdir自己的锁，已核对锁目录为空，缓存保留。最初含rm -f的清理命令被工具规则拒绝（未执行），改为unlink后正常运行。

WIP用聚焦TCP回归证明缺口与修复；Ready使用fmt、同一聚焦测试、make lint及diff自审交付；.agents/skills/tidb-verify-profile缺失，不宣称调用该技能。未改Go/Bazel/依赖，不触发bazel_prepare，未运行bazel_lint_changed、Go单测或RealTiKV；本任务Rust默认features，未调用Go failpoint工具。变更文件为query.rs、pg_extended_test.rs和本任务文件；总plan.md保持只读。依git-commit技能只提交这三个文件，不push。

正确性及兼容性风险：PG混合算术只新增本有向对的结果身份，已有转换器复用，MySQL基本整数CAST隔离已通过；固定18.6参考差分、二进制结果格式、全量双协议测试未验证。性能只增加PG算术节点一次局部类型对判断，未做基准。PG18_REFERENCE_DSN未配置、固定18.6构建缺失属于无关环境缺证，故状态为已完成，待回归，保留任务文件。下一步在固定18.6参考端重放相同SQL与参数，采集值、NULL、OID/typlen/typmod/格式及错误阶段完成差分后再删除任务文件。
