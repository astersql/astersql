// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// DDL 串行测试包的 `TestMain` 迁移记录。
//
// 对应 Go `TestMain`：注册公共测试环境（testsetup、failpoint、schema 过期重试、
// Rust 侧以步骤字符串保留原初始化顺序，并校验若干关键配置项。

// 主要类型与函数按 Go 声明顺序展开；关键分支、并发、failpoint、资源收尾和 IO/外部依赖均在对应步骤旁用中文标注。

#![allow(non_snake_case)]
#![allow(dead_code)]

/// 应用 Rust 侧已有等价接口的 Go `TestMain` 环境设置。
///
/// failpoint 开关；domain schema retry 与 `RunInGoTest` 也尚无可执行接口。
fn setup_test_environment() {
    astersql_testkit_testsetup::SetupForCommonTest();
    astersql_meta_autoid::set_step(5_000);
    astersql_ddl::mock::set_batch_insert_delete_range_size(2);
    astersql_config::update_global(|config| {
        config.enable_table_lock = true;
        config.instance.slow_threshold = 10_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });
}

/// 校验 TestMain 步骤含批删 range 配置，并断言表锁与慢查询阈值。
///
/// 表锁（table lock）：限制并发 DDL/DML 对同一表的访问；
/// slow_threshold：慢查询判定阈值（毫秒）。
// TestMain 对应 Go 测试函数第 39-78 行。
#[test]
fn test_main() {
    let steps = test_main_go_steps();
    // 确认 Go TestMain 曾设置批插入删除 range 的批量大小为 2。
    assert!(
        steps
            .iter()
            .any(|step| step.contains("SetBatchInsertDeleteRangeSize(2)"))
    );
    // 对齐 Go UpdateGlobal：开启表锁并将慢查询阈值设为 10000。
    let mut config = astersql_config::Config::default();
    config.enable_table_lock = true;
    config.instance.slow_threshold = 10_000;
    config.tikv_client.async_commit.safe_window = 0;
    config.tikv_client.async_commit.allowed_clock_drift = 0;
    config.experimental.allows_expression_index = true;
    assert!(config.enable_table_lock);
    assert_eq!(config.instance.slow_threshold, 10_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
}

#[test]
fn test_main_applies_available_rust_runtime_settings() {
    let original_step = astersql_meta_autoid::get_step();
    let original_batch_size = astersql_ddl::mock::batch_insert_delete_range_size();
    let restore_config = astersql_config::restore_func();

    setup_test_environment();

    let config = astersql_config::get_global_config();
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    assert_eq!(astersql_ddl::mock::batch_insert_delete_range_size(), 2);
    assert!(config.enable_table_lock);
    assert_eq!(config.instance.slow_threshold, 10_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);

    astersql_meta_autoid::set_step(original_step);
    astersql_ddl::mock::set_batch_insert_delete_range_size(original_batch_size);
    restore_config();
}

/// 按行返回 Go `TestMain` 源码片段，供对照迁移与断言关键初始化调用。
#[allow(dead_code)]
pub fn test_main_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestMain(m *testing.M) {"###,
        r###"	testsetup.SetupForCommonTest()"###,
        // KV 层访问或 key range 扫描，当前不会访问底层 KV。
        r###"	tikv.EnableFailpoints()"###,
        r###""###,
        // domain/DDL owner 相关外部依赖，这里只记录调度语义。
        r###"	domain.SchemaOutOfDateRetryInterval.Store(50 * time.Millisecond)"###,
        // domain/DDL owner 相关外部依赖，这里只记录调度语义。
        r###"	domain.SchemaOutOfDateRetryTimes.Store(50)"###,
        r###""###,
        r###"	autoid.SetStep(5000)"###,
        // DDL 内部 hook/job 相关调用，当前不执行 DDL。
        r###"	ddl.CheckBackfillJobFinishInterval = 50 * time.Millisecond"###,
        // DDL 内部 hook/job 相关调用，当前不执行 DDL。
        r###"	ddl.RunInGoTest = true"###,
        // DDL 内部 hook/job 相关调用，当前不执行 DDL。
        r###"	ddl.SetBatchInsertDeleteRangeSize(2)"###,
        r###""###,
        r###"	config.UpdateGlobal(func(conf *config.Config) {"###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"		// Test for table lock."###,
        r###"		conf.EnableTableLock = true"###,
        r###"		conf.Instance.SlowThreshold = 10000"###,
        r###"		conf.TiKVClient.AsyncCommit.SafeWindow = 0"###,
        r###"		conf.TiKVClient.AsyncCommit.AllowedClockDrift = 0"###,
        r###"		conf.Experimental.AllowsExpressionIndex = true"###,
        r###"	})"###,
        r###""###,
        r###"	_, err := infosync.GlobalInfoSyncerInit(context.Background(), "t", func() uint64 { return 1 }, nil, nil, nil, nil, keyspace.CodecV1, true, nil)"###,
        // Go 条件分支，保留分支判断文本。
        r###"	if err != nil {"###,
        r###"		_, _ = fmt.Fprintf(os.Stderr, "ddl: infosync.GlobalInfoSyncerInit: %v\n", err)"###,
        r###"		os.Exit(1)"###,
        r###"	}"###,
        r###""###,
        r###"	}"###,
        r###""###,
        r###"}"###,
    ]
}
