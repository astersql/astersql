# 任务 19: CTE嵌套与作用域释放


批次：【批次 19】依赖：批次18（任务18）。这是拆分批次2的子实施批次，不能等同于大计划编号。

状态：未开始

目的：交付“CTE嵌套与作用域释放”这一行为的生产保真或增量回归证据。

来源任务：大计划2；路线图第2章「公共表表达式遮蔽规则」。原蓝图工作流2；首批只增量承接。

预计会话范围：限定一组相关输入的解析/绑定行为、必要局部接线与独立测试，复用正常共享准备链及真实客户端夹具；不补建外部依赖。接口不满足时在本文件留证阻塞，不能把跨系统工作吞进当前会话。

## 文件


- 候选修改：`pkg/server/pg_name.rs`（先核符号及消费者，只改本行为必要部分）。
- 候选修改：`pkg/planner/core/resolve/lib.rs`（先核符号及消费者，只改本行为必要部分）。
- 候选修改：`pkg/parser/ast/lib.rs`（先核符号及消费者，只改本行为必要部分）。
- 新增测试函数：`pkg/server/pg_name_test.rs::pg_dialect_19_behavior`，名称为本计划待新增，不声称已存在；沿现有lib.rs已注册独立测试文件添加。
- 新增真实客户端测试函数：`pkg/server/pg_client_integration_test.rs::pg_dialect_19_libpq`，复用现有模块注册、start_client_server / CreateAnalyzeSession / MySQL握手模式，不调用mock TiDBContext代替真实内核。
- 增量扩展任务1新建的：`pkg/server/testdata/pg_dialect_libpq.py`：仅添加本行为场景。该文件尚未存在；任务1建立仅ILIKE所需的libpq入口与参数读取，后继不得假设支持尚未定义的运行模式。Rust客户端测试用当前Python解释器启动并传本任务编号、临时PG端口及库名；DSN只走环境，不进日志。它与首批pg_core_libpq.py并列，复用pg_core_evidence.py的适用函数，不能import会自动连接的首批脚本。
- 相邻回归：`pkg/server/pg_query_test.rs`、`pkg/server/pg_extended_test.rs`；先读相邻测试，不把测试函数放进生产文件。若确有新生产Rust文件须在所属lib.rs显式挂载；本任务未授权无关新模块。

## 上下文


静态HEAD `0fdee3a372d61bc4dd98a2649b43f49bdb084d83`；不是当前通过证据。Parser::with_dialect/ParseSQL与普通Q、Extended::process→adapt→markers→prepare_statement是入口；共享执行内核不变。实现前用rustcodegraph核最新源码/提交、目标doc.go（存在时）和server AGENTS，再读路线图对应完整小节、验收/后续细分及只读来源文档。

## 单一行为与接线


外层t=1，派生表内t=2，主查询返回(1,2)；退出内层恢复外层；下一条语句读真实t=9；移除真实t后下一语句42P01；quoted CTE与未引用名；Q/扩展Parse分别验证生命周期。

从所属连接/Parser::with_dialect取得身份，通过实际Rust语义动作、共享AST及现存消费者传递。每项生产修改必须在本文件记录对应的上述契约或不可缺少的局部接线理由。不能仅增加无人消费字段、只测手工构造AST、做通用SQL文本替换或旁路执行。输入保留原SQL/字符跨度、quoted及$n；字符串/注释不被重写。需要额外消费者时只接到正常准备/解析/绑定边界，不用执行写入换取Describe。

复用已取证的载体，但为本语法重新检查能否保留全部信息；Cast/ILIKE的结论不是其他语法的自动选型。共享AST足够时补Rust动作/消费者，不能表达时只扩展本行为字段；受控入口候选必须仍进入共享内核，不能递归扩建。

## 依赖与边界


不改变CTE执行模型；仅嵌套绑定、显式列名列表与生命周期的必要局部接线。

所有任务都消费大计划1的当前分端归档/来源/比较门槛；目前CaseRegistry仍限定首批输入，compare_rejection仅接受22P02/22003/25P02，不能把新42601/0A000直接交它并声称已验收。新错误仅按本任务契约局部扩展适用比较，并为扩展加负向自检；必要时保留独立观测与断言，未满足完整能力门槛保持未验证，不为其放宽首批注册。外部接口实际不存在时，先记录具体符号、文件、调用证据和最小交接需求，只在本任务写已阻塞；禁止无限扩大范围。未开始是当前计划状态，不预先声称所有依赖可执行。

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


- 行为：上文单一行为的树形/绑定、诊断及所属方言隔离；成功、边界、非法、NULL（适用时）各自登记，源码Span/参数不靠字符串相等代替。
- 失败验证测试：先新增 `pg_dialect_19_behavior` 和 `pg_dialect_19_libpq`，注册方式见文件节；使用已有公共入口及现存载体编写可编译的语义断言。缺新字段时先用访问者观察现有节点/拒绝结果，不能把编译错误当红灯。
- 失败验证命令（仓库根、已领取槽位、默认features，无新增feature）：`env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_dialect_19_behavior -- --nocapture`；真实边界命令：`env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_dialect_19_libpq -- --nocapture`。
- 预期失败原因：集合克隆虽保留文本却无法证明具体对象，内层或跨语句名称泄漏。
- 通过验证命令：同样运行 `env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_dialect_19_behavior -- --nocapture` 和 `env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_dialect_19_libpq -- --nocapture`，同一输入和预先冻结的断言不得降级。
- 已有行为例外：若当前实现正确，保留新增独立覆盖并取得当前通过证据，记录无需生产修改的源码理由；不改坏代码制造失败，不将既有成功当成新增能力。依赖准备失败不是预期行为红灯。
- 模拟策略：零生产依赖mock。Parser单元调用正式入口；绑定/协议由真实共享会话与测试存储验证；参考为真实PostgreSQL、客户端为libpq18。开发InsecureRootOnly只代表测试存储路径，不声称生产鉴权或RealTiKV通过。

## 真实参考、客户端与双方言断言


在 `pg_dialect_19_libpq` 中启动同Server临时双listener、PG A/B及MySQL M，执行本编号夹具。夹具入口是本任务待新增，先实现注册再跑过滤器；`env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_dialect_19_libpq -- --nocapture` 不能零命中。参考读取 `PG18_REFERENCE_DSN`，来源目标18.6，冻结完整version()/server_version_num与独立构建来源/产物摘要；沿用 `PG_CORE_REFERENCE_TARGET_VERSION`、`PG_CORE_REFERENCE_BUILD_FILE` 与 `PG_LIBPQ_LIBRARY` 的既有证据习惯，libpq实际路径/版本/摘要均采集。18.4只能单列补充，不能降级目标或声称18.6通过；无精确参考构建则阻塞该真实验收，不操作用户既有参考服务。

每端独立登记能力ID、case_id、原SQL、阶段Q或Parse/Bind/Describe/Execute、显式OID、参数值/NULL及text格式、编码UTF8、UTC、search_path、角色、初态I、对象映射和预期依据。保持原始观测与预期分离。解析表示在Rust单元树形断言；参考端可用PQprepare验证解析/分析而不执行DML。描述收集列名、OID、typlen/typmod、参数数目/类型、格式与适用来源信息；成功执行项比较值、NULL、行列数、命令标签和副作用。只解析项的值/OID按阶段明确不适用；未知字段保持缺证，不能填写默认相等。RETURNING/冲突/数组/record消费者未开放时本地合法输入副作用前0A000与参考成功分开保存，只验收语法/门禁层，完整执行交外部大计划，不能将该拒绝算完整语义成功。

MySQL使用自身合法SQL和已冻结sql_mode/字符集/collation，以同Server M在PG前/中/后核真实值、列类型及自身准备行为；PG专属输入按路线图目标拒绝，先采集本地旧扩展基线，不能把共享语法旧接受行为悄悄删除。MySQL精确参考版本/构建从执行环境采集：本任务待新增环境名 `MYSQL_REFERENCE_DSN`，只环境读取，缺失保持该层未验证。ROW/CTE等双方语法不得一律作为负例，PG public与MySQL database限定名各用自身夹具。MySQL错误码/位置独立断言，不套PG SQLSTATE；PG先后A→M→B→A与反向次序核身份/路径不污染。

本任务最少登记 6 个唯一主要case（来自上文输入，不能把日志行/循环/重试算case），同时记录实际有效断言数和每层有效数；参考或消费者门槛未满足的case计0有效成功，另记准备阻塞或预期能力门禁。仅两端适用断言、双方言旁路、来源完整且恢复/清理齐全的唯一case计入完成数；负向自检单列，不冒充业务场景。若合法输入需要类型/执行接口尚无，本任务不得删掉此case凑通过。

## 恢复与清理


每次错误保留发生阶段、SQLSTATE、适用P与事务状态；Simple消费ReadyForQuery，Extended错误先Sync，T/E则ROLLBACK，再在原连接执行显式 `SELECT 1::int4`，同时B和M各自合法探针继续。不能重连掩盖错误恢复；健康请求不能覆盖此前失败。用独立观察连接比对初态与后态，确认被拒DML/Describe没有改表，不能DROP后反推无副作用。

沿pg_core_libpq_integer_differential的守卫模式：finally释放PQresult、关闭statement/portal、恢复会话设置/结束事务、关闭连接和子进程；Rust清理自己的唯一库/表并关闭双listener，检查端口拒绝连接。成功、失败、SIGINT/异常均走本任务清理；子进程及使用槽位二进制的进程退出后才释放自己的槽锁，保留缓存。参考仅清理本轮登记对象，不能停用户数据库或删除他人锁/数据。清理失败单列且不计有效，日志不含DSN、口令或认证载荷。

## 步骤与里程碑


1. 重核HEAD、前置任务当前证据与外部局部接口，记录消费者链及单项方案；接口缺失仅在本文件记阻塞，不修改任何plan.md。
2. 新增独立失败测试和该编号libpq场景，先领取槽位，Rust修改后先 `cargo fmt --all`。运行失败命令，核确切测试名和非零有效断言，保留实际语义失败；若旧实现已正确走当前绿灯例外。
3. 只修本行为生产/工厂/消费者接线，必要语法表用 `env -u LDFLAGS cargo run --locked -p astersql-parsergen -- generate` 更新，审查RuleId/action映射与生成差异。没有grammar变化则不生成；不照旧build.rs文档运行不存在的入口。
4. 先 `cargo fmt --all`，再同输入通过验证。需要语法表时运行 `env -u LDFLAGS cargo run --locked -p astersql-parsergen -- check`。不得把编译成功当行为通过。
5. 按Ready跑适用检查，核恢复/清理/有效计数和diff。完成或阻塞的证据都只维护本编号文件，依下节处理交付。

线性执行位置为子批次19；前置为批次18（任务18）。本目录保守串行，每批一个任务，不授权并行。若后续确需调整批次，只能在编号文件与prompt一致更新，并重新核同源文件、生成表、客户端夹具、参考schema/listener、验证资源和局部依赖；槽位容量不授权并行。

## 验证与完成


WIP：`cargo fmt --all` → `env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_dialect_19_behavior -- --nocapture` → `env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_dialect_19_libpq -- --nocapture`。Ready：沿用本轮仍有效聚焦证据，补 `make lint` 与 `git diff --check`；Rust grammar改动补上述parsergen check。修改pkg/parser/**时按根矩阵执行 `make parser`、`make parser_yacc`、`make parser_fmt`、`make parser_unit_test` 及Rust聚焦验证；执行前先核Bazel完整触发清单、Go failpoints并按文档清理，审查生成差异。Go既有回归失败据实记录，依根规则不阻止本任务成功；不能省略本次行为验证或用Go产物代替Rust证据。不要默认运行 `make bazel_lint_changed`。verify-profile技能缺失据实报告，不能伪造技能通过。

交付记录：确切变更文件/符号、每项生产变更的契约依据，失败前和通过后同命令输出/退出码、命中测试数（每个新增过滤器至少1）、实际唯一有效case与断言数、参考/客户端/双方言原始证据路径、运行构建SHA与工作区输入摘要、实际槽位及绝对CARGO_TARGET_DIR、清理结果。源与测试分离、保留PingCAP版权，已修复可用的Rust顶部加/保留AsterSQL版权；外部依赖仅独立上游tag方式，本任务默认不改依赖。

标为已完成必须本任务承诺的适用证据齐全；仅上游外部原因导致无法回归可用已完成，待回归，必须保留文件并列确切待补命令，不能让缺行为本身的实现冒充待回归。阻塞状态只能写本文件，范围外缺口给出归属，不扩写。报告Agent Output Contract：文件、WIP/Ready理由、正确性/兼容性/性能风险、精确命令、未验证项。测试存储证据不代表RealTiKV或性能通过。

有实际代码/测试/配置/说明变更时按 `$git-commit` 检查staged/unstaged/untracked并只提交本任务，排除其他工作区变更。只有编号状态/删除不提交。完成报告新证据后按prompt删除本编号文件；已阻塞或已完成，待回归保留。不能删除来源大计划，不能改来源plan、当前plan或原路线图。

## Progress


- [ ] 接口与消费者取证（尚未执行）
- [ ] 独立失败验证或旧实现当前绿灯例外证据（尚未执行）
- [ ] 最小实现及同输入通过验证（尚未执行）
- [ ] Ready、真实观测、恢复清理与提交交接（尚未执行）

## Surprises & Discoveries


本轮仅静态规划，未执行。当前发现：集合克隆虽保留文本却无法证明具体对象，内层或跨语句名称泄漏。 这是待验证假设，不是测试结果；执行时用实际输出替换/补充。

## Decision Log


2026-10-24：仅围绕本行为增量复用共享内核。不改变CTE执行模型；仅嵌套绑定、显式列名列表与生命周期的必要局部接线。 活动决策在本文件维护，plan.md只读。

## Outcomes & Retrospective


尚无实施结果。本轮有效运行数0、有效行为断言数0；不得引用历史94场景作为本任务完成证据。
