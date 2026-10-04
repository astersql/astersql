// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// `config` crate 的集成测试：验证 TiDB 配置文件（TOML 格式）的解析、
// 校验（`Config::valid`）与全局配置读写逻辑。
//
// 覆盖的主要场景：
// - 原子布尔（`AtomicBool`）与三态布尔（`NullableBool`）的序列化/反序列化；
// - 日志、部署模式（deploy-mode）、keyspace（键空间，多租户数据隔离单元）、
//   安全加密、外部负载（external-workload）等配置节的合法性校验；
// - 已废弃/迁移到实例作用域（instance）的配置项映射检查；
// - 各类数值上限（索引长度、索引数量、列数量、统计信息加载并发等）的边界测试。

use astersql_config::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

/// 全局测试互斥锁：部分测试会读写进程级全局配置（`store_global_config` 等），
/// 通过该锁串行化这些测试，避免并行执行时相互污染全局状态。
static GLOBAL_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 测试辅助函数：把给定的 TOML 文本写入临时目录中的 `config.toml`，
/// 再通过 `Config::load` 加载，模拟从磁盘读取配置文件的完整流程。
fn load_config(input: &str) -> Result<Config, ConfigError> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    fs::write(&path, input).unwrap();
    let mut config = new_config();
    config.load(path)?;
    Ok(config)
}

#[test]
fn go_merge_2_storage_and_starter_options() {
    let config = new_config();
    assert!(!config.enable_storage_class);
    assert!(!config.hosted_embedding.enabled);
    assert_eq!(config.starter_params.max_import_data_size, 0);
    let config = load_config("enable-storage-class = true\ndeploy-mode = 'starter'\n[hosted-embedding]\nenabled = true\napi-endpoint = 'https://example.com'\napi-key-path = '/tmp/key'\n[starter-params]\nbootstrap-file = '/tmp/bootstrap.json'").unwrap();
    assert!(config.enable_storage_class);
    assert!(config.hosted_embedding.enabled);
    assert_eq!(
        config.starter_params.max_import_data_size,
        DEF_STARTER_MAX_IMPORT_DATA_SIZE
    );
    assert_eq!(config.starter_params.bootstrap_file, "/tmp/bootstrap.json");
    assert!(load_config("[hosted-embedding]").is_err());
    assert!(load_config("[starter-params]\nbootstrap-file = '/tmp/bootstrap.json'").is_err());
    assert!(load_config("[starter-params]\nbootstrap-file = ''").is_ok());
    assert!(load_config("[hosted-embedding]\nenabled = false").is_err());
    let mut configured = new_config();
    configured.hosted_embedding.api_endpoint = "https://example.com".into();
    assert!(configured.valid().is_err());
    let config =
        load_config("[experimental]\nallow-enable-foreign-key-check-in-shared-lock = true")
            .unwrap();
    assert!(
        config
            .experimental
            .allow_enable_foreign_key_check_in_shared_lock
    );
    assert_eq!(
        serde_json::to_value(&config).unwrap()["enable-storage-class"],
        false
    );
    assert_eq!(
        load_config("deploy-mode = 'starter'\n[starter-params]\nmax-import-data-size = '0B'")
            .unwrap()
            .starter_params
            .max_import_data_size,
        0
    );
}

#[test]
fn go_merge_2_ru_weights_and_modes() {
    let mut config = new_config();
    assert_eq!(config.ruv2.report_mode, RU_REPORT_MODE_RESULT);
    assert_eq!(config.ruv2.stmt_weights.cpu_work, 1.0);
    assert_eq!(config.ruv2.ddl_weights.txn_kv_bytes, 1.0);
    config.ruv2.report_mode = "FULL".into();
    assert!(config.valid().is_err());
    config.ruv2.report_mode = RU_REPORT_MODE_FULL.into();
    config.ruv2.stmt_weights.cpu_work = -1.0;
    assert!(
        config
            .valid()
            .unwrap_err()
            .to_string()
            .contains("ru-v2.stmt-weights.cpu-work")
    );
    config.ruv2.stmt_weights.cpu_work = 1.0;
    config.ruv2.ddl_weights.ingest_kv_bytes = f64::NAN;
    assert!(
        config
            .valid()
            .unwrap_err()
            .to_string()
            .contains("ru-v2.ddl-weights.ingest-kv-bytes")
    );
    let config = load_config("[ru-v2]\nreport-mode = 'full'\n[ru-v2.stmt-weights]\ncpu-work = 2\n[ru-v2.ddl-weights]\ntxn-kv-bytes = 3").unwrap();
    assert_eq!(config.ruv2.stmt_weights.cpu_work, 2.0);
    assert_eq!(config.ruv2.ddl_weights.txn_kv_bytes, 3.0);
    let weights: StmtWeights = toml::from_str("CrossAZNetByte = 2\ncross-az-net-byte = 2").unwrap();
    assert_eq!(weights.cross_az_net_byte, 0.0);
    let encoded = serde_json::to_string(&weights).unwrap();
    assert!(!encoded.contains("CrossAZ"));
    assert!(!encoded.contains("cross-az"));
    let mut config = new_config();
    config.ruv2.ddl_weights.txn_kv_bytes = f64::INFINITY;
    assert!(
        config
            .valid()
            .unwrap_err()
            .to_string()
            .contains("ru-v2.ddl-weights.txn-kv-bytes")
    );
}

#[test]
fn go_merge_2_ru_serialization_and_boundaries() {
    let configured: RUV2Config = toml::from_str(
        "report-mode = 'full'\n[stmt-weights]\ncpu-work = 2\nscan-byte = 3\nnet-byte = 5\nfrontend-compile-byte = 7\nhash-state-row = 11\njoin-output-row = 13\nwrite-statement = 17\noperator-num = 19\nwrite-key = 23\nwrite-byte = 29\n[ddl-weights]\ntxn-kv-bytes = 31\ningest-kv-bytes = 37",
    )
    .unwrap();
    assert_eq!(configured.report_mode, RU_REPORT_MODE_FULL);
    assert_eq!(
        [
            configured.stmt_weights.cpu_work,
            configured.stmt_weights.scan_byte,
            configured.stmt_weights.net_byte,
            configured.stmt_weights.frontend_compile_byte,
            configured.stmt_weights.hash_state_row,
            configured.stmt_weights.join_output_row,
            configured.stmt_weights.write_statement,
            configured.stmt_weights.operator_num,
            configured.stmt_weights.write_key,
            configured.stmt_weights.write_byte,
        ],
        [2.0, 3.0, 5.0, 7.0, 11.0, 13.0, 17.0, 19.0, 23.0, 29.0]
    );
    assert_eq!(configured.ddl_weights.txn_kv_bytes, 31.0);
    assert_eq!(configured.ddl_weights.ingest_kv_bytes, 37.0);
    let json = serde_json::to_value(&configured).unwrap();
    assert_eq!(json["stmt-weights"]["cpu-work"], 2.0);
    assert_eq!(json["ddl-weights"]["ingest-kv-bytes"], 37.0);
    assert!(json["stmt-weights"].get("cross-az-net-byte").is_none());

    for invalid in [-1.0, f64::NAN, f64::INFINITY] {
        let mut config = new_config();
        config.ruv2.stmt_weights.write_byte = invalid;
        assert!(
            config
                .valid()
                .unwrap_err()
                .to_string()
                .contains("ru-v2.stmt-weights.write-byte")
        );
        let mut config = new_config();
        config.ruv2.ddl_weights.ingest_kv_bytes = invalid;
        assert!(
            config
                .valid()
                .unwrap_err()
                .to_string()
                .contains("ru-v2.ddl-weights.ingest-kv-bytes")
        );
    }
    let mut zero_weights = new_config();
    zero_weights.ruv2.stmt_weights.cpu_work = 0.0;
    zero_weights.ruv2.ddl_weights.txn_kv_bytes = 0.0;
    assert!(zero_weights.valid().is_ok());
}

/// 测试辅助函数：断言结果为错误并返回错误消息文本，便于做子串匹配。
fn error_text(result: Result<(), ConfigError>) -> String {
    result.unwrap_err().to_string()
}

/// 包装 `AtomicBool` 的测试结构体，用于验证原子布尔类型的
/// TOML/JSON 序列化与反序列化行为。
#[derive(Debug, Serialize, Deserialize)]
struct AtomicWrapper {
    value: AtomicBool,
}

/// 验证 `AtomicBool` 的反序列化：只接受 true/false 布尔字面量，
/// 数字（如 1）应报类型错误；序列化为 JSON 时输出普通布尔值。
#[test]
fn test_atomic_bool_unmarshal() {
    let enabled: AtomicWrapper = toml::from_str("value = true").unwrap();
    assert!(enabled.value.load());
    let disabled: AtomicWrapper = toml::from_str("value = false").unwrap();
    assert!(!disabled.value.load());
    assert_eq!(serde_json::to_string(&enabled.value).unwrap(), "true");
    assert!(
        toml::from_str::<AtomicWrapper>("value = 1")
            .unwrap_err()
            .to_string()
            .contains("bool")
    );
}

/// 验证三态布尔 `NullableBool`（UNSET/FALSE/TRUE）的 JSON 往返序列化，
/// 以及日志配置中 `enable-error-stack` 字段对空字符串（视为未设置）
/// 和非法类型（数字）的处理。
#[test]
fn test_nullable_bool_unmarshal() {
    for (encoded, expected) in [
        ("null", NullableBool::UNSET),
        ("false", NullableBool::FALSE),
        ("true", NullableBool::TRUE),
    ] {
        let value: NullableBool = serde_json::from_str(encoded).unwrap();
        assert_eq!(value, expected);
        assert_eq!(
            serde_json::from_str::<NullableBool>(&serde_json::to_string(&value).unwrap()).unwrap(),
            expected
        );
    }
    let log: Log = toml::from_str("enable-error-stack = true").unwrap();
    assert_eq!(log.enable_error_stack, NullableBool::TRUE);
    let log: Log = toml::from_str("enable-error-stack = ''").unwrap();
    assert_eq!(log.enable_error_stack, NullableBool::UNSET);
    assert!(toml::from_str::<Log>("enable-error-stack = 1").is_err());
}

/// 验证日志配置中新旧字段的合并语义：`enable-timestamp` 与已废弃的
/// `disable-timestamp`（以及对应的 error-stack 字段）同时出现时，
/// 以 enable 系列为准计算最终生效值。
#[test]
fn test_log_config() {
    // 元组含义：(TOML 输入, 期望 disable_timestamp, 期望 disable_error_stack)
    let cases = [
        ("[log]\n", false, true),
        ("[log]\nenable-timestamp = false\n", true, true),
        (
            "[log]\nenable-timestamp = true\ndisable-timestamp = false\n",
            false,
            true,
        ),
        (
            "[log]\nenable-timestamp = false\ndisable-timestamp = true\n",
            true,
            true,
        ),
        (
            "[log]\nenable-error-stack = true\ndisable-error-stack = true\n",
            false,
            false,
        ),
    ];
    for (input, disable_timestamp, disable_error_stack) in cases {
        let mut config = load_config(input).unwrap();
        config.valid().unwrap();
        assert_eq!(config.log.disable_timestamp(), disable_timestamp);
        assert_eq!(config.log.disable_error_stack(), disable_error_stack);
    }
}

/// 验证错误消息扩展（error-msg-extension）：按正则匹配错误消息并追加后缀。
/// 该功能仅允许在 starter 部署模式下配置；同时验证
/// `get_error_message_extensions` 返回的是副本，修改副本不影响全局配置。
#[test]
fn test_error_message_extension_config() {
    let _guard = GLOBAL_TEST_LOCK.lock().unwrap();
    let restore = restore_func();
    let mut config = new_config();
    config.deploy_mode = DeployMode::Starter;
    config.error_msg_extension = vec![
        ErrorMessageExtension::new("^Access denied", " docs"),
        ErrorMessageExtension::new("^Access denied for user", " user docs"),
    ];
    config.valid().unwrap();
    store_global_config(config.clone());
    let mut prepared = get_error_message_extensions();
    assert_eq!(prepared.len(), 2);
    assert!(prepared[0].matches("Access denied for user root"));
    prepared[0].suffix.clear();
    assert!(!get_error_message_extensions()[0].suffix.is_empty());
    assert!(new_config().error_msg_extension.is_empty());
    restore();
}

/// 验证错误消息扩展的非法配置：无效正则、空白模式串，
/// 以及在非 starter 部署模式下配置时应报错。
#[test]
fn test_error_message_extension_invalid_regexp() {
    let mut config = new_config();
    config.deploy_mode = DeployMode::Starter;
    config.error_msg_extension = vec![ErrorMessageExtension::new("[", "bad")];
    assert!(error_text(config.valid()).contains("invalid error-msg-extension regexp"));
    config.error_msg_extension = vec![ErrorMessageExtension::new(" \t", "missing")];
    assert!(error_text(config.valid()).contains("empty error-msg-extension pattern"));
    config = new_config();
    config.error_msg_extension = vec![ErrorMessageExtension::new(".*", "not allowed")];
    assert!(error_text(config.valid()).contains("only be configured"));
    assert!(
        load_config("error-msg-extension = [{ pattern = '.*', suffix = 'x' }]")
            .unwrap_err()
            .to_string()
            .contains("only be configured")
    );
}

/// 验证 keyspace 可观测性配置：把 keyspace 元数据字段映射到监控指标标签
/// （metric-label）、慢日志字段（slow-log-field）与语句日志字段
/// （stmt-log-field），并验证缺失 required 元数据时的解析失败。
#[test]
fn test_keyspace_observability() {
    let mut config: Config = toml::from_str(
        "deploy-mode='starter'\n[[keyspace-observability.fields]]\nsource='meta_a'\nmetric-label='keyspace_meta_label_a'\nslow-log-field='Keyspace_meta_slow_a'\nstmt-log-field='stmt_meta_a'\nrequired=true\n[[keyspace-observability.fields]]\nsource='meta_b'\nmetric-label='keyspace_meta_label_b'\nslow-log-field='Keyspace_meta_slow_b'",
    )
    .unwrap();
    config.keyspace_observability.Valid().unwrap();
    config
        .ResolveKeyspaceObservability(HashMap::from([
            ("meta_a".into(), "value_a".into()),
            ("meta_b".into(), "value_b".into()),
        ]))
        .unwrap();
    assert_eq!(config.GetKeyspaceObservabilityMetricLabels().len(), 2);
    assert_eq!(
        config.GetKeyspaceObservabilitySlowLogFields()[0].Name,
        "Keyspace_meta_slow_a"
    );
    assert_eq!(
        config.GetKeyspaceObservabilityStmtLogFields()["stmt_meta_a"],
        "value_a"
    );
    assert!(
        config
            .ResolveKeyspaceObservability(HashMap::from([("meta_b".into(), "value_b".into())]))
            .unwrap_err()
            .contains("missing required")
    );
}

/// 验证 keyspace 可观测性配置的各类非法输入：空 source、无任何输出字段、
/// 指标标签命名不合法或与内置字段冲突，以及非 starter 模式下不允许配置。
#[test]
fn test_keyspace_observability_invalid() {
    // 元组含义：(fields 配置片段, 期望的错误消息子串)
    let cases = [
        (
            "source=''\nmetric-label='keyspace_meta_a'",
            "source cannot be empty",
        ),
        ("source='meta_a'", "at least one output"),
        (
            "source='meta_a'\nmetric-label='1_label'",
            "invalid metric-label",
        ),
        ("source='meta_a'\nmetric-label='KEYSPACE_ID'", "must start"),
        ("source='meta_a'\nslow-log-field='Digest'", "must start"),
    ];
    for (field, expected) in cases {
        let input = format!("[[keyspace-observability.fields]]\n{field}");
        let config: Config = toml::from_str(&input).unwrap();
        assert!(
            config
                .keyspace_observability
                .Valid()
                .unwrap_err()
                .contains(expected)
        );
    }
    let mut config: Config = toml::from_str(
        "[[keyspace-observability.fields]]\nsource='meta_a'\nmetric-label='keyspace_meta_a'",
    )
    .unwrap();
    assert!(error_text(config.valid()).contains("only be configured"));
}

/// 验证已删除配置项与隐藏配置项的识别：
/// `is_all_removed_config_items` 判断一组配置项是否全部为已删除项，
/// `contain_hidden_config` 判断路径是否命中隐藏配置（不区分大小写）。
#[test]
fn test_removed_variable_check() {
    assert!(is_all_removed_config_items(&[
        "enable-batch-dml".into(),
        "performance.committer-concurrency".into(),
        "log.slow-threshold".into(),
    ]));
    assert!(!is_all_removed_config_items(&[
        "unrecognized-option-test".into()
    ]));
    assert!(contain_hidden_config(
        "PERFORMANCE.INDEX-USAGE-SYNC-LEASE='1s'"
    ));
}

/// 综合配置加载测试：未知配置项应报错；正常配置各字段被正确解析；
/// 加密方法名会被规范化为小写；默认隔离读引擎为 tikv/tiflash/tidb。
#[test]
fn test_config() {
    assert!(
        load_config("unrecognized-option-test = true")
            .unwrap_err()
            .to_string()
            .contains("invalid configuration option")
    );
    let mut config = load_config(
        "store='unistore'\nport=2333\nmax-index-length=4096\n[performance]\ntxn-total-size-limit=2000\ntcp-no-delay=false\n[security]\nspilled-file-encryption-method='AES128-CTR'",
    )
    .unwrap();
    config.valid().unwrap();
    assert_eq!(config.store, "unistore");
    assert_eq!(config.port, 2333);
    assert_eq!(config.performance.txn_total_size_limit, 2000);
    assert!(!config.performance.tcp_no_delay);
    assert_eq!(config.security.spilled_file_encryption_method, "aes128-ctr");
    assert_eq!(
        Config::default().isolation_read.engines,
        ["tikv", "tiflash", "tidb"]
    );
}

/// 验证事务总大小上限（txn-total-size-limit）的边界：最大允许 1TiB（1<<40）。
/// 该值限制单个事务写入数据的总字节数。
#[test]
fn test_txn_total_size_limit_valid() {
    for (value, valid) in [(1_u64 << 40, true), ((1_u64 << 40) + 1, false)] {
        let mut config = new_config();
        config.performance.txn_total_size_limit = value;
        assert_eq!(config.valid().is_ok(), valid);
    }
}

/// 验证部署模式（deploy-mode）解析与 `dxf-resource-limit`
/// （DXF：分布式执行框架的资源上限）的组合约束：
/// premium 模式不允许自定义资源上限，未知模式解析失败。
#[test]
fn test_deploy_mode_config() {
    // 元组含义：(TOML 输入, 期望部署模式, 期望资源上限, 校验是否通过)
    for (input, mode, limit, valid) in [
        (
            "deploy-mode='premium_reserved'",
            DeployMode::PremiumReserved,
            100,
            true,
        ),
        (
            "deploy-mode='premium_reserved'\ndxf-resource-limit=30",
            DeployMode::PremiumReserved,
            30,
            true,
        ),
        (
            "deploy-mode='premium'\ndxf-resource-limit=30",
            DeployMode::Premium,
            30,
            false,
        ),
        ("deploy-mode='starter'", DeployMode::Starter, 100, true),
    ] {
        match load_config(input) {
            Ok(mut config) => {
                assert_eq!(config.deploy_mode, mode);
                assert_eq!(config.dxf_resource_limit, limit);
                assert_eq!(config.valid().is_ok(), valid);
            }
            Err(_) => assert!(!valid),
        }
    }
    assert!(load_config("deploy-mode='unknown'").is_err());
}

/// 验证 keyspace 激活模式的约束：仅 starter 部署模式可启用，
/// 且不能与备用（standby）模式同时开启。
#[test]
fn test_keyspace_activate_mode_config() {
    let mut config = new_config();
    config.keyspace_activate_mode = true;
    assert!(error_text(config.valid()).contains("starter"));
    config.deploy_mode = DeployMode::Starter;
    config.valid().unwrap();
    config.standby.standby_mode = true;
    assert!(error_text(config.valid()).contains("standby"));
}

/// 验证顶层（无 section）配置项到实例作用域系统变量的迁移映射，
/// 如 `run-ddl` -> `tidb_enable_ddl`。
#[test]
fn test_conflict_instance_config() {
    let sections = section_moved_to_instance();
    let root = sections
        .iter()
        .find(|section| section.section_name.is_empty())
        .unwrap();
    assert_eq!(root.name_mappings["run-ddl"], "tidb_enable_ddl");
    assert_eq!(
        root.name_mappings["max-server-connections"],
        "max_connections"
    );
}

/// 验证 log、performance 等配置节中废弃配置项到系统变量的迁移映射，
/// 如 `log.slow-threshold` -> `tidb_slow_log_threshold`。
#[test]
fn test_deprecated_config() {
    let sections = section_moved_to_instance();
    let log = sections
        .iter()
        .find(|section| section.section_name == "log")
        .unwrap();
    assert_eq!(
        log.name_mappings["slow-threshold"],
        "tidb_slow_log_threshold"
    );
    let performance = sections
        .iter()
        .find(|section| section.section_name == "performance")
        .unwrap();
    assert_eq!(
        performance.name_mappings["memory-usage-alarm-ratio"],
        "tidb_memory_usage_alarm_ratio"
    );
}

/// 验证最大索引长度（max-index-length）的取值区间：
/// [DEF_MAX_INDEX_LENGTH, DEF_MAX_OF_MAX_INDEX_LENGTH]，越界即校验失败。
#[test]
fn test_max_index_length() {
    for (value, valid) in [
        (DEF_MAX_INDEX_LENGTH, true),
        (DEF_MAX_OF_MAX_INDEX_LENGTH, true),
        (DEF_MAX_INDEX_LENGTH - 1, false),
        (DEF_MAX_OF_MAX_INDEX_LENGTH + 1, false),
    ] {
        let mut config = new_config();
        config.max_index_length = value;
        assert_eq!(config.valid().is_ok(), valid);
    }
}

/// 验证单表索引数量上限（index-limit）的取值区间边界。
#[test]
fn test_index_limit() {
    for (value, valid) in [
        (DEF_INDEX_LIMIT, true),
        (DEF_MAX_OF_INDEX_LIMIT, true),
        (DEF_INDEX_LIMIT - 1, false),
        (DEF_MAX_OF_INDEX_LIMIT + 1, false),
    ] {
        let mut config = new_config();
        config.index_limit = value;
        assert_eq!(config.valid().is_ok(), valid);
    }
}

/// 验证单表列数量上限（table-column-count-limit）的取值区间边界。
#[test]
fn test_table_column_count_limit() {
    for (value, valid) in [
        (DEF_TABLE_COLUMN_COUNT_LIMIT, true),
        (DEF_MAX_OF_TABLE_COLUMN_COUNT_LIMIT, true),
        (DEF_TABLE_COLUMN_COUNT_LIMIT - 1, false),
        (DEF_MAX_OF_TABLE_COLUMN_COUNT_LIMIT + 1, false),
    ] {
        let mut config = new_config();
        config.table_column_count_limit = value;
        assert_eq!(config.valid().is_ok(), valid);
    }
}

/// 验证插件审计日志（audit log）的缓冲区大小与刷新间隔的合法范围：
/// 缓冲区不能为负且不超上限，刷新间隔必须为正。
#[test]
fn test_plugin_audit_log() {
    // 元组含义：(缓冲区大小, 刷新间隔, 校验是否通过)
    for (buffer, interval, valid) in [
        (0, 1, true),
        (
            MAX_PLUGIN_AUDIT_LOG_BUFFER_SIZE,
            MAX_PLUGIN_AUDIT_LOG_FLUSH_INTERVAL,
            true,
        ),
        (-1, 30, false),
        (0, 0, false),
        (MAX_PLUGIN_AUDIT_LOG_BUFFER_SIZE + 1, 30, false),
    ] {
        let mut config = new_config();
        config.instance.plugin_audit_log_buffer_size = buffer;
        config.instance.plugin_audit_log_flush_interval = interval;
        assert_eq!(config.valid().is_ok(), valid);
    }
}

/// 验证并发连接令牌上限（token-limit）的自动修正：
/// 0 回退为默认值 1000，超过最大值则被截断为最大值。
#[test]
fn test_token_limit() {
    for (input, expected) in [
        (0, 1000),
        (100, 100),
        (MAX_TOKEN_LIMIT + 1, MAX_TOKEN_LIMIT),
    ] {
        let config = load_config(&format!("token-limit={input}")).unwrap();
        assert_eq!(config.token_limit, expected);
    }
}

/// 验证默认临时存储目录的编码规则：把 "host:port/status_host:status_port"
/// 做 Base64 编码作为目录名，保证不同实例的临时目录互不冲突。
#[test]
fn test_encode_def_temp_storage_dir() {
    // 元组含义：(host, status_host, port, status_port, 期望的 Base64 编码)
    for (host, status_host, port, status_port, encoded) in [
        (
            "0.0.0.0",
            "0.0.0.0",
            4000,
            10080,
            "MC4wLjAuMDo0MDAwLzAuMC4wLjA6MTAwODA=",
        ),
        (
            "127.0.0.1",
            "127.16.5.1",
            4000,
            10080,
            "MTI3LjAuMC4xOjQwMDAvMTI3LjE2LjUuMToxMDA4MA==",
        ),
        (
            "127.0.0.1",
            "127.16.5.1",
            4000,
            15532,
            "MTI3LjAuMC4xOjQwMDAvMTI3LjE2LjUuMToxNTUzMg==",
        ),
    ] {
        let path =
            encode_def_temp_storage_dir(std::env::temp_dir(), host, status_host, port, status_port);
        assert!(path.ends_with(&format!("{encoded}/tmp-storage")));
    }
}

/// 验证通过链接期标志（对应 Go 的 ldflags 构建注入）初始化全局开关：
/// `CHECK_TABLE_BEFORE_DROP` 仅在 flag 为 "1" 时开启，且遥测始终关闭。
#[test]
fn test_modify_through_ldflags() {
    let _guard = GLOBAL_TEST_LOCK.lock().unwrap();
    let restore = restore_func();
    for (edition, flag, expected) in [
        ("Community", "None", false),
        ("Community", "1", true),
        ("Enterprise", "None", false),
        ("Enterprise", "1", true),
    ] {
        init_by_ld_flags(edition, flag);
        assert_eq!(CHECK_TABLE_BEFORE_DROP.load(Ordering::SeqCst), expected);
        assert!(!get_global_config().enable_telemetry);
    }
    restore();
}

/// 验证落盘文件加密方法（spilled-file-encryption-method）的白名单：
/// 仅支持 plaintext 与 aes128-ctr（大小写不敏感）。
#[test]
fn test_security_valid() {
    // 元组含义：(加密方法名, 校验是否通过)
    for (method, valid) in [
        ("", false),
        ("Plaintext", true),
        ("plaintext123", false),
        ("aes256-ctr", false),
        ("aes128-ctr", true),
    ] {
        let mut config = new_config();
        config.security.spilled_file_encryption_method = method.into();
        assert_eq!(config.valid().is_ok(), valid);
    }
}

/// 验证 TCP_NODELAY（禁用 Nagle 算法以降低延迟）默认开启。
#[test]
fn test_tcp_no_delay() {
    assert!(new_config().performance.tcp_no_delay);
}

/// 验证 `get_json_config` 导出的 JSON 配置：隐藏/已删除的配置项不应出现，
/// 而正常配置项（如 stmt-count-limit）应保留。
#[test]
fn test_get_json_config() {
    let _guard = GLOBAL_TEST_LOCK.lock().unwrap();
    let value = get_json_config().unwrap();
    for hidden in [
        "index-usage-sync-lease",
        "enable-batch-dml",
        "mem-quota-query",
        "query-log-max-len",
        "oom-action",
    ] {
        assert!(!value.contains(hidden));
    }
    assert!(value.contains("stmt-count-limit"));
    assert!(value.contains("rpc-metrics"));
}

/// 验证随包发布的示例配置文件 `config.toml.example`：
/// 递归遍历其中所有配置路径，确保没有任何一项属于隐藏配置。
#[test]
fn test_config_example() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let input = fs::read_to_string(manifest_dir.join("config.toml.example")).unwrap();
    let value: toml::Value = toml::from_str(&input).unwrap();
    // 递归遍历 TOML 表，将嵌套键拼接成 "a.b.c" 形式的完整路径逐一检查。
    fn check(prefix: &str, value: &toml::Value) {
        if let toml::Value::Table(table) = value {
            for (key, child) in table {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                assert!(!contain_hidden_config(&path), "{path} should be hidden");
                check(&path, child);
            }
        }
    }
    check("", &value);
}

/// 验证统计信息（statistics，优化器用于估算执行计划代价的数据）
/// 同步加载的并发数与队列长度的取值区间边界。
#[test]
fn test_stats_load_limit() {
    for (value, valid) in [
        (DEF_STATS_LOAD_CONCURRENCY_LIMIT, true),
        (DEF_STATS_LOAD_CONCURRENCY_LIMIT - 1, false),
        (DEF_MAX_OF_STATS_LOAD_CONCURRENCY_LIMIT, true),
        (DEF_MAX_OF_STATS_LOAD_CONCURRENCY_LIMIT + 1, false),
    ] {
        let mut config = new_config();
        config.performance.stats_load_concurrency = value;
        assert_eq!(config.valid().is_ok(), valid);
    }
    for (value, valid) in [
        (DEF_STATS_LOAD_QUEUE_SIZE_LIMIT, true),
        (DEF_STATS_LOAD_QUEUE_SIZE_LIMIT - 1, false),
        (DEF_MAX_OF_STATS_LOAD_QUEUE_SIZE_LIMIT, true),
        (DEF_MAX_OF_STATS_LOAD_QUEUE_SIZE_LIMIT + 1, false),
    ] {
        let mut config = new_config();
        config.performance.stats_load_queue_size = value;
        assert_eq!(config.valid().is_ok(), valid);
    }
}

/// 验证外部负载（external-workload）配置：仅 starter 模式可启用，
/// controller-addr 与 tidb-pool 必填且会去除首尾空白，
/// 角色（Role）需在合法集合内并做规范化；配置文件中不允许出现该节。
#[test]
fn test_external_workload_valid() {
    let mut config = new_config();
    config.external_workload.Enable = true;
    assert!(error_text(config.valid()).contains("only be configured"));
    config.deploy_mode = DeployMode::Starter;
    assert!(error_text(config.valid()).contains("controller-addr"));
    config.external_workload.ControllerAddr = " http://127.0.0.1:1234 ".into();
    assert!(error_text(config.valid()).contains("tidb-pool"));
    config.external_workload.TidbPool = " pool-a ".into();
    config.external_workload.Role = "unknown".into();
    assert!(error_text(config.valid()).contains("invalid external-workload role"));
    config.external_workload.Role = " GCV2 ".into();
    config.valid().unwrap();
    assert_eq!(config.external_workload.Role, RoleGCV2Worker);
    assert_eq!(config.external_workload.TidbPool, "pool-a");
    assert!(load_config("[external-workload]\nenable=false").is_err());
}

/// 验证全局 keyspace 名称的读写：更新全局配置后可读到新值。
#[test]
fn test_get_global_keyspace_name() {
    let _guard = GLOBAL_TEST_LOCK.lock().unwrap();
    let restore = restore_func();
    update_global(|config| config.keyspace_name = "test".into());
    assert_eq!(get_global_keyspace_name(), "test");
    restore();
}

/// 验证全局 TiKV worker URL 的读写（TiKV 为分布式 KV 存储引擎）。
#[test]
fn test_get_global_tikv_worker_url() {
    let _guard = GLOBAL_TEST_LOCK.lock().unwrap();
    let restore = restore_func();
    update_global(|config| config.tikv_worker_url = "tikv-worker-0:10080".into());
    assert_eq!(get_global_config().tikv_worker_url, "tikv-worker-0:10080");
    restore();
}

/// 验证自动扩缩容（AutoScaler）开关的默认值与全局更新，
/// 以及 `IsValidAutoScalerConfig` 对合法/非法配置字符串的判定。
#[test]
fn test_auto_scaler_config() {
    let _guard = GLOBAL_TEST_LOCK.lock().unwrap();
    let restore = restore_func();
    assert!(!new_config().use_auto_scaler);
    update_global(|config| config.use_auto_scaler = true);
    assert!(get_global_config().use_auto_scaler);
    restore();
    assert!(IsValidAutoScalerConfig(MockASStr));
    assert!(!IsValidAutoScalerConfig(TestASStr));
}

/// 验证同时存在废弃项与类型错误项时的报错：错误消息应指向真正的
/// 类型问题（enforce-mpp 需要布尔值），而非被废弃项掩盖。
#[test]
fn test_invalid_config_with_deprecated_config() {
    let error = load_config("[log]\nslow-threshold=1000\n[performance]\nenforce-mpp=1")
        .unwrap_err()
        .to_string();
    assert!(error.contains("enforce-mpp"));
    assert!(error.contains("boolean"));
}

/// 验证 keyspace 名称的合法性规则：只允许特定字符集，
/// 且不能是"字母开头 + 超出 u64 范围的纯数字"这类歧义形式。
#[test]
fn test_keyspace_name() {
    for (name, valid) in [
        ("#!", false),
        ("abc", true),
        ("18446744073709551615", true),
        ("a18446744073709551615", false),
    ] {
        let mut config = new_config();
        config.keyspace_name = name.into();
        assert_eq!(config.valid().is_ok(), valid);
    }
}

/// 验证计量（metering，按用量计费的数据上报）存储 URI 的解析：
/// 支持 s3 与 azure 两种对象存储协议，校验通过后可提取
/// scheme、bucket 与路径前缀。
#[test]
fn test_metering() {
    // 元组含义：(存储 URI, 期望协议, 期望 bucket, 期望路径前缀)
    for (uri, scheme, bucket, prefix) in [
        (
            "s3://test-bucket/test-prefix?region-id=test-region",
            "s3",
            "test-bucket",
            "/test-prefix",
        ),
        (
            "azure://metering-data/test-prefix?account-name=test-account&account-key=test-key",
            "azure",
            "metering-data",
            "/test-prefix",
        ),
    ] {
        let mut config = new_config();
        config.metering_storage_uri = uri.into();
        config.valid().unwrap();
        let parsed = url::Url::parse(&config.metering_storage_uri).unwrap();
        assert_eq!(parsed.scheme(), scheme);
        assert_eq!(parsed.host_str().unwrap(), bucket);
        assert_eq!(parsed.path(), prefix);
    }
}

/// 验证 `get_tikv_config` 保留 TiKV 客户端自己的 RU 系数。
/// 显式设置的 0 值（RU：Request Unit，资源计量单位）。
#[test]
fn test_get_tikv_config_keeps_zero_ruv2_ru_scale() {
    let mut config = new_config();
    config.tikv_client.ruv2.ru_scale = 0.0;
    assert_eq!(get_tikv_config(&config).tikv_client.ruv2.ru_scale, 0.0);
}

/// Async Commit 与实验功能默认值应和 Go 配置一致。
#[test]
fn test_async_commit_and_experimental_defaults() {
    let config = new_config();
    assert_eq!(config.tikv_client.async_commit.keys_limit, 256);
    assert_eq!(
        config.tikv_client.async_commit.total_key_size_limit,
        4 * 1024
    );
    assert_eq!(config.tikv_client.async_commit.safe_window, 2_000_000_000);
    assert_eq!(
        config.tikv_client.async_commit.allowed_clock_drift,
        500_000_000
    );
    assert!(!config.experimental.allows_expression_index);
    assert!(!config.experimental.enable_new_charset);
}

/// Go `defaultConf` 的连接与执行默认值必须原样保留，避免 Rust 端在未提供
/// 配置文件时选择不同的存储实现、socket 或笛卡尔积策略。
#[test]
fn test_go_default_config_parity() {
    let config = new_config();
    assert_eq!(config.store, "unistore");
    assert_eq!(config.socket, "/tmp/tidb-{Port}.sock");
    assert!(config.performance.cross_join);
}

/// 对齐 Go `Config` 字段的 TOML/JSON 标签；这些标签属于公开配置契约，
/// 不能由 Rust 字段名机械推导出不同的名称。
#[test]
fn test_go_config_field_names_parity() {
    let config: Config = toml::from_str(
        "use-autoscaler=true\nkeyspace-activate=true\n[ru-v2]\nreport-mode='full'\n[transaction-summary]\ntransaction-summary-capacity=321\n[experimental]\nallow-expression-index=true\n[instance]\ntidb_slow_log_threshold=123",
    )
    .unwrap();
    assert!(config.extra.is_empty());
    assert!(config.use_auto_scaler);
    assert!(config.keyspace_activate_mode);
    assert_eq!(config.ruv2.report_mode, RU_REPORT_MODE_FULL);
    assert_eq!(config.trx_summary.transaction_summary_capacity, 321);
    assert!(config.experimental.allows_expression_index);
    assert_eq!(config.instance.slow_threshold, 123);

    let json = serde_json::to_value(config).unwrap();
    let object = json.as_object().unwrap();
    for key in [
        "use-autoscaler",
        "keyspace-activate",
        "ru-v2",
        "transaction-summary",
    ] {
        assert!(object.contains_key(key), "missing Go config key {key}");
    }
    for key in [
        "use-auto-scaler",
        "keyspace-activate-mode",
        "ruv2",
        "trx-summary",
    ] {
        assert!(!object.contains_key(key), "unexpected Rust-only key {key}");
    }
    assert!(
        object["experimental"]
            .as_object()
            .unwrap()
            .contains_key("allow-expression-index")
    );
    assert!(
        object["instance"]
            .as_object()
            .unwrap()
            .contains_key("tidb_slow_log_threshold")
    );
}

/// 对齐 Go `Config.Valid` 中现有 Rust 数据模型可表达的校验分支。
#[test]
fn test_go_validation_parity() {
    let mut config = new_config();
    config.security.skip_grant_table = true;
    assert!(error_text(config.valid()).contains("need root privilege"));

    let mut config = new_config();
    config.store = "invalid".into();
    assert!(error_text(config.valid()).contains("invalid store"));

    let mut config = new_config();
    config.store = "mocktikv".into();
    config.instance.tidb_enable_ddl.store(false);
    assert!(error_text(config.valid()).contains("disable DDL on mocktikv"));

    let mut config = new_config();
    config.dxf_resource_limit = 30;
    assert!(error_text(config.valid()).contains("premium_reserved"));

    let mut config = new_config();
    config.trx_summary.transaction_summary_capacity = 5001;
    assert!(
        error_text(config.valid())
            .contains("transaction-summary-capacity should not be larger than 5000")
    );
}

/// Go 通过 TOML metadata 判断键是否显式配置，注释或普通字符串中的
/// 同名文本不应触发部署模式限制，也不应改变 starter 默认值。
#[test]
fn test_load_uses_toml_keys_not_text_matches() {
    let config = load_config(
        "deploy-mode='starter'\npath='dxf-resource-limit'\n# error-msg-extension\n# [external-workload]\n# enable-zero-backend",
    )
    .unwrap();
    assert!(config.standby.enable_zero_backend);

    let config =
        load_config("path='error-msg-extension and dxf-resource-limit'\n# [external-workload]")
            .unwrap();
    assert_eq!(config.path, "error-msg-extension and dxf-resource-limit");
}

#[test]
fn pessimistic_transaction_configuration_preserves_go_defaults_and_roundtrips() {
    let config = crate::PessimisticTxn::default();
    assert_eq!(config.max_retry_count, 256);
    assert_eq!(config.deadlock_history_capacity, 10);
    assert!(!config.deadlock_history_collect_retryable);
    assert!(config.constraint_check_in_place_pessimistic);
    assert_eq!(
        config.pessimistic_auto_commit.load(),
        astersql_config_kerneltype::IsNextGen()
    );
    config.pessimistic_auto_commit.store(true);
    let json = serde_json::to_string(&config).unwrap();
    let restored: crate::PessimisticTxn = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, config);
    restored.pessimistic_auto_commit.store(false);
    assert!(
        config.pessimistic_auto_commit.load(),
        "config clones must not share mutable atomic storage"
    );
}
#[test]
fn starter_import_size_limit_requires_starter_mode_and_round_trips() {
    let mut config = crate::Config::default();
    config.starter_params.max_import_data_size = 1024;
    assert!(
        config
            .valid()
            .unwrap_err()
            .to_string()
            .contains("starter-params.max-import-data-size")
    );
    config.deploy_mode = crate::DeployMode::Starter;
    config.valid().unwrap();
    let encoded = serde_json::to_string(&config).unwrap();
    assert!(encoded.contains("\"max-import-data-size\":\"1KiB\""));
    let document: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(document["starter-params"].as_object().unwrap().len(), 1);
    let restored: crate::Config = serde_json::from_str(&encoded).unwrap();
    assert_eq!(restored.starter_params.max_import_data_size, 1024);
    let toml = toml::to_string(&config).unwrap();
    assert!(toml.contains("max-import-data-size = \"1KiB\""));
    let restored_toml: crate::Config = toml::from_str(&toml).unwrap();
    assert_eq!(restored_toml.starter_params.max_import_data_size, 1024);
}

#[test]
fn starter_rg_fallback_is_cli_only_and_defaults_to_disabled() {
    let mut params = super::config::StarterParams::default();
    assert!(!params.enable_rg_fallback);
    params.enable_rg_fallback = true;
    let value = serde_json::to_value(&params).unwrap();
    assert!(value.get("enable-rg-fallback").is_none());
    let decoded: super::config::StarterParams =
        serde_json::from_str("{\"enable-rg-fallback\":true}").unwrap();
    assert!(!decoded.enable_rg_fallback);
    let decoded: super::config::StarterParams = toml::from_str("enable-rg-fallback=true").unwrap();
    assert!(!decoded.enable_rg_fallback);
    assert!(
        !toml::to_string(&params)
            .unwrap()
            .contains("enable-rg-fallback")
    );
}
