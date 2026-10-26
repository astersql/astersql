# PostgreSQL interval 字段范围与精度

任务34（IV-PRECISION-FIELDS）在任务33的三分量载体和样式入口上增加声明修饰符。
`PgInterval::apply_typmod(fields, precision)` 先按末字段截断，再按微秒精度舍入；
精度0–5的半值向远离零方向舍入，精度6不改值。月、日、时间不相互折算。
YEAR截去不足整年的月份并清零日/时间；HOUR TO MINUTE保留已有月/日，只截去秒。
三分量共同构成的正负无穷哨兵不参与舍入，有限时间分量舍入溢出返回22008。

用户明确授权补齐本项必要的前置语法，不等待大计划2。整体plan.md保持只读。

## 生产修改与来源

- `pkg/parser/grammar/main.astergram` 和生成的 `generated/main_tables.rs`：
  增加interval(p)、字段范围、末字段SECOND(p)、typed literal的正式语法；
  generated/lexer_tokens.rs同步新增专用PG token。
- `pkg/parser/lexer.rs`：INTERVAL由专用语法消费，替代旧的identifier/TIMESTAMP转接。
- `pkg/parser/parser_actions/expression.rs`：将PG字段mask写入interval专用类型的Flen，
  p写入Decimal；不是MySQL TimeUnit/Fsp，类型码仍为TypePgInterval。
  未声明修饰符保持-1/-1；p>6归6并记录警告，负p按任务契约拒绝。
- `pkg/parser/types/field_type.rs`：AST恢复CAST SQL时保留字段声明及精度0，
  使预处理重解析与实际执行保持同一声明。
- `pkg/types/pg_temporal.rs`：在既有PgInterval上增加apply_typmod，不另建总时长模型。
- `pkg/session/runtime/pg_temporal.rs`、`query.rs`：分别在准备期常量与执行期cast
  消费同一个apply_typmod，保留已有IntervalStyle和NULL路径。
- `pkg/server/pg_result.rs`：将1186/16的typmod输出为 `(fields << 16) | p`，
  无精度声明的低16位为65535，无字段限制时mask为32767，完全未声明仍为-1。
  Query/Parse的精度错误/告警沿现有诊断入口输出22023。

对应独立测试在parser_test、parser/types/migration_aster_unit_test、
types/pg_temporal_test、session/runtime/pg_temporal_test及server/pg_temporal_test中。
生产文件不包含测试函数。

## 参考与兼容性边界

本机真实参考为 PostgreSQL 18.4 (Homebrew)，aarch64-apple-darwin25.4.0，
Apple clang 21.0.0。临时独立listener端口56434，IntervalStyle=postgres。
首轮初始化默认SQL_ASCII，随后另建UTF8数据库复核关键值；输入均为ASCII。
`target/iv34-reference.log`记录值、interval_send十六进制和pg_attribute元数据。
没有宣称18.6或完整libpq差分通过；完整客户端差分属于任务39。

参考观测包括：±1.2345秒到p3为±1.235秒、±0.5秒到p0为±1秒、
1.1234567秒到p6为1.123457秒、59.9999997秒到微秒进位一分钟，
以及HOUR TO MINUTE和YEAR的独立分量截断。
精度3/0/6的typmod为2147418115/2147418112/2147418118；
YEAR为327679，HOUR TO MINUTE为201392127，DAY TO SECOND(3)为470286339。

已知契约差异：本任务要求负p返回22023，参考18.4的负p语法返回42601；
本实现遵循任务要求，不将这一错误分类声称为参考端等价。
p=7的参考告警为22023并收敛到6。

## 验证记录

真实TCP红灯：`target/iv34-red.log`，测试编译成功并执行1项，interval(3)
收到42601 unsupported PostgreSQL type modifier，退出101。
初次夹具关闭API错误和一次Rhs访问方法编译错误均已修正，不计作行为红灯。
调试构建的解析表生成器因耗时改用release构建，主动终止的生成进程退出143；
释放其槽位后重新生成，未清理其他会话进程或锁。

语法迭代中保留的反例还包括：禁止interval(p)后再接字段、禁止非SECOND末字段的精度、
保留interval作为非保留表名/列名/别名、以及原MySQL interval函数和日期算术。
专用PG token通过非保留名称规则承接名称使用，普通::interval与带修饰符声明
分别规约，避免将声明解析与通用名称解析互相替代。

会话测试另覆盖准备常量与重复绑定不同text值，以及NULL。该会话API沿既有关系记录集
将NULL表示为`<nil>`；`pkg/server/runtime.rs::protocol_value`负责转成Value::Null。
服务端测试仍断言真正wire NULL的长度为-1，不把这个内部拼写作为协议输出。

另一个实际红灯来自真实TCP的NULL首行UNION：第二行未消费typmod，返回.2345。
system_query.rs的常量投影原仅转交time/timestamp/timestamptz；增加TypePgInterval
这一条必要局部接线，复用query.rs中的同一cast求值器。相邻system_query_test.rs
增加重复绑定正负半值的UNION回归，修复后该测试和原TCP断言均通过。

## 最终检查与命令

迭代采用WIP，交付采用根协议的Ready适用检查。
`.agents/skills/tidb-verify-profile`缺失，没有宣称该技能通过。
本项共18个文件：上述生产/生成文件及独立测试、本文档。
没有Go源、import、依赖或Bazel元数据变化，不触发bazel_prepare；没有执行
make bazel_lint_changed。没有运行Go包单测或启用额外Rust features/failpoints。
Rust parser修改以原生parsergen生成/检查及解析测试验证，未运行Go parser生成目标。

每个Cargo阶段均原子动态领取槽位，实际使用槽位1和2；绝对路径为：

    CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1
    CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-2

所有Cargo、生成器及测试子进程均继承相应目录；阶段结束后释放自己的锁，保留缓存。
Rust修改后先执行cargo fmt --all。下列命令从仓库根执行，最终适用结果全部退出0：

    cargo fmt --all
    env -u LDFLAGS cargo run --locked --release -p astersql-parsergen -- generate
    env -u LDFLAGS cargo run --locked --release -p astersql-parsergen -- check
    env -u LDFLAGS cargo test --locked -p astersql-parser --lib interval -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-parser --lib consistent_test -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-parser-types --lib interval_cast_restore_preserves_fields_and_zero_precision -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-types --lib pg_interval -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-session --lib interval_precision_prepared_constants_and_parameters -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-session --lib pg_interval_constant_union_precision -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_iv_precision_fields -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_iv_components -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_iv_signs_style -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_types_test -- --nocapture
    make lint
    git diff --check
    git diff --cached --check

有效Rust测试去除重复验证后共23项，逐测试命令数量分别为6、1、1、3、1、1、1、1、1、7。
值层/恢复测试的最终记录在target/iv34-value-final.log（3+1项，槽位2）；
最终解析器6项与关键字1项记录在target/iv34-green.log（槽位1）。该文件尾部保留
修复前UNION的TCP失败，不能将该整批当成通过；其失败由后续同一TCP测试替代。
UNION修复后target/iv34-union-green.log的会话1项/TCP1项通过（槽位1）；
target/iv34-ready.log的会话1项、服务端1+1+7项通过（槽位1）。
target/iv34-parser-check.log为最终生成表检查（槽位2），target/iv34-lint.log为lint。
生成表检查通过后没有再修改语法或生成器。

TCP的13组值各覆盖两种cast形式及IT→OT、IT→OB、IB→OT、IB→OB，
共104次非NULL往返及104次typed NULL往返，检查原始16字节、1186/16/typmod、
参数Describe、statement/portal Describe、重复Execute和格式切换；另覆盖零行、
首行NULL、多行、非法UTF-8/NUL/长度、p7告警、负p错误、Sync及ROLLBACK恢复。
有限极值舍入溢出保持22008。临时PG服务已停止，临时数据已删除。

## 风险与未验证范围

正确性风险集中在字段mask的消费者覆盖，已用普通cast、常量、参数、UNION和wire测试约束；
没有声称时间运算、通用interval输入全集或后续任务35–39全部通过。
兼容性差异为前述负p的任务契约22023与参考18.4的42601；未验证18.6、完整libpq差分、
持久TiKV、全仓测试。精度收敛使用有界整数运算，复用现有字符串解析/输出；未做性能基准。
后续建议继续任务35的范围与特殊值验收。
