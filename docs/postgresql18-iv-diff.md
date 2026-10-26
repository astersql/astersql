# PostgreSQL18 interval 真实客户端差分

2026-10-27，任务39基于 `a7215388e9` 消费任务32–38的规范载体、样式、
精度、极值、NULL、非法载荷和显式MySQL桥接。新增
`pg_client_integration_test::pg_temporal_iv_diff` 及 `pg_core_libpq.py` 的
`interval-differential` 模式，使用真实libpq、TCP PgService与CreateAnalyzeSession。
没有模拟SQL、解析器或客户端；没有改变只读plan.md。

## 发现和修复

`target/iv39-fourth.log` 编译成功并实际执行1项差分，目标将
`2147483647 months 1 month` 提前分类为22007，参考端返回累加溢出22015。
`target/iv39-types-red.log` 的独立回归同样实际执行1项失败，退出101。
`pkg/types/pg_temporal.rs::PgInterval::parse_postgres` 将重复单位拒绝移动到
分量累加溢出检查之后；普通重复单位仍为22007，溢出仍为22015，不接受重复单位。
相邻 `pg_temporal_test.rs` 覆盖月、日、微秒、顺序及边界未溢出反例。
依据为IV-RANGE-SPECIAL/IV-INVALID的解析阶段错误契约。

`target/iv39-seventh.log` 实际执行1项差分失败，退出101：有限时间分量极值
强制为interval(0)时两端均22008，但参考在Bind拒绝，目标到Execute才拒绝。
`pkg/server/pg_extended.rs::interval_parameter_modifiers` 从真实AST缓存直接参数
投影的字段/精度约束，在完整Bind帧和参数codec校验后调用同一个apply_typmod，
提前验证，保留原参数给真实Execute消费。NULL及其他OID仍走既有路径；
条件表达式和条件中的子查询不被提前求值。独立TCP回归位于
`pkg/server/pg_temporal_test.rs::pg_temporal_iv_typmod_overflow_at_bind`，覆盖
正负极值、文本/二进制入出、两种cast写法、BindComplete前拒绝和Sync后SELECT 1。
依据为本任务错误阶段差分和IV-PRECISION-FIELDS的舍入溢出约束。

新测试位于独立测试文件，生产文件没有测试函数，保留既有版权。
没有改写SQL、改变规范载体、通用转换资格、外部依赖或新增子系统。

## 真实环境和比较范围

参考 SQL version()/server_version_num 为 PostgreSQL **18.4 (Homebrew)** / 180004，
aarch64-apple-darwin25.4.0、Apple clang21、64-bit；实际libpq版本180000。
未运行18.6。基线DateStyle=ISO, MDY、TimeZone=UTC、IntervalStyle=postgres，
另核对postgres_verbose、sql_standard、iso_8601。参考通过SET TimeZone，
目标通过已有真实SET time_zone接口配置UTC。identity记录两端实际设置。

主机tzdata为2026d-rearguard；参考发行标签未暴露，identity保存五个zone文件
SHA256，不用主机标签代替参考端版本。identity还保存源码HEAD、相关源码diff、
Cargo.lock、Python夹具、实际Rust二进制SHA256及源码工作区状态。

每端独立采集608项观察并逐项比较，另有行字节、类型元数据和SQLSTATE三项
比较器扰动检查。608是配对观察数量，不是Rust测试数量。

| 能力 | 断言 |
| --- | --- |
| IV-COMPONENTS | 月、30日、1日、24小时分别保留维度，1.5月归(1,15,0)，ISO输入，原始16字节及规范分量。 |
| IV-SIGNS-STYLE | 混合/纯负/零及四种样式互读、短负号在postgres/sql_standard的不同分量；Parse冻结字面量、Bind冻结参数、改样式后Execute及新Bind重新解释。 |
| IV-PRECISION-FIELDS | 正负p3/p0半值离零，p6与七位小数、进位，HOUR TO MINUTE和YEAR字段截断，精确typmod、p7告警22023归6及舍入溢出22008的Bind阶段。 |
| IV-RANGE-SPECIAL | 月/日i32两端、时间i64两端及相邻混合有限极值，三分量全极值作为非NULL正负无限；四方向保留分量，解析/累加22015与最终年归月22008分开。 |
| IV-NULL-METADATA | -1/p0/p3/p6、typed NULL、首行NULL、全NULL、零行、多行、重复Execute和格式切换；参数1186，statement/portal描述1186/16/编码typmod，NULL长度-1与16字节全零分开。 |
| IV-INVALID | 空串/garbage及不适用的now/today/yesterday/tomorrow为22007；UTF8/NUL22021，空/15字节08P01，17字节22P03，帧声明错误08P01；错误阶段、Sync恢复SELECT 1，事务错误E经ROLLBACK回I。 |
| IV-BRIDGE | 显式PgInterval::from_mysql_duration/to_mysql_duration与真实MySQL TIME(6)值；-51小时含微秒往返，PG客户端返回binary再反向桥接回真实MySQL；月/日及839小时均0A000，拒绝后SELECT 1。 |

合法值及typed NULL覆盖IT→OT、IT→OB、IB→OT、IB→OB；libpq准备/执行配合
经libpq认证的独立wire连接记录ParameterDescription、statement/portal Describe、
Execute行载荷、协议阶段及SQLSTATE。完整错误诊断另存，不比较构建路径/错误文案。
同一MySQL连接在PG流程前、中、后更新并读回TIME、DATETIME、TIMESTAMP及NULL，
保留sql_mode、type=11/12/7、Fsp=6、原始列定义和行字节；不只PING。
桥接API是显式选择，不把PG interval隐式映射到MySQL TIME。

复用规范载体 `pkg/types/pg_temporal.rs::PgInterval`，公开入口为from_components、
components、from_binary/to_binary、parse_with_style/format_with_style、apply_typmod，
以及上述显式桥接方法。保持(months:i32,days:i32,time_microseconds:i64)，
wire按网络序i64微秒/i32日/i32月，不折为总秒、总纳秒或text OID。

参考差异明确保留：负p任务契约目标22023，而18.4 SQL语法42601，分别断言并存identity。
参考18.4对自身有限i64最小时间分量的postgres文本输出回读返回22007，目标可回读；
此观测单列identity。该值的显式单位文本和binary四方向仍独立严格验收，未删除极值覆盖。
早期夹具路径错误及错误期望校准不算生产红灯。未声明18.6通过。

## 验证和资源

WIP用于定位两个实际差异，Ready按根协议用于交付；验证profile技能缺失，
未声称技能通过。已有工作区，没有Go源/import/Go顶层测试、依赖或Bazel元数据变化，
无Bazel错误，不触发bazel_prepare；没有执行bazel_lint_changed。
仅运行默认features的Rust --lib测试，没有运行Go包单测或改变Go failpoints。

所有Cargo阶段通过原子mkdir动态领取共享槽位 **1**：

    CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1

锁记录计划、任务、会话与持有PID；EXIT/INT/TERM清理自己锁，保留缓存。
独立initdb使用唯一临时目录和动态回环端口，PG18_REFERENCE_DSN缺失明确失败。
finally/Drop关闭连接、结果、子进程、自己的listener及唯一数据库，外层runner停止
自己的参考集群、检查reference_port_closed=true并删除临时目录。
不停止用户服务、不删除用户数据。

从仓库根运行，Cargo及其子进程继承上述目录，差分继承临时参考DSN：

    cargo fmt --all
    python3 -m py_compile pkg/server/testdata/pg_core_libpq.py
    env -u LDFLAGS cargo test --locked -p astersql-types --lib pg_interval_duplicate_field_overflow_precedes_syntax -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-types --lib pg_interval -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_iv_diff -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_iv_typmod_overflow_at_bind -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_iv -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_types_test -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_extended_test -- --nocapture
    make lint
    git diff --check
    git diff --cached --check

## 限制和风险

正确性/兼容性风险集中在未列出的输入及通用表达式Bind规划时机；本修复只覆盖本族
直接参数投影，不重写通用常量折叠。显式桥接保留既有限制，没有承诺任意SQL隐式桥接。
性能新增一次prepare期AST约束采集及带修饰符interval参数的Bind验证，没有基准测量。
未验证18.6、持久TiKV、全仓测试、完整GUC及时间运算。内存存储TCP通过不代表持久TiKV。

## 最终结果

`target/iv39-ready.log` 退出0：fmt、py_compile、值层5项、累计interval9项、
元数据7项、扩展协议34项，共55项有效Rust测试通过（去除重复运行）。
`target/iv39-lint.log` 的make lint退出0。自审修正一处Python行末空白后，
`target/iv39-final-diff.log` 以同一精确过滤器重新执行，退出0、1 passed、608配对通过。
前述未变动Rust回归证据继续适用，没有以编译或零测试代替行为验收。

最终原始证据：

    target/pg-iv-evidence/61654-1793035424805813000/

summary两端cleanup=true，child-exit code=0；rust-cleanup的database_dropped、
postgres_port_closed、mysql_port_closed均true。runner确认参考端口关闭，
临时参考数据已删除，共享槽位1锁已释放，缓存保留。
最终源码diff SHA256为
`2487bf0dea9c2d46085bca67cc8092e97daf6a503a1050aba56ee8c8c2e1c7ef`，
夹具SHA256为`8aae10dbdbbd3e2a5031f025edb6525821dd1e1af3d132b5acd1e758abc66085`，
已重新计算并核对identity一致。diff空白检查及暂存检查均通过。
下一步建议在可用的18.6参考环境复用相同差分入口补测指定补丁版本。
