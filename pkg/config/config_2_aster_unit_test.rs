// Copyright 2026 AsterSQL.
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

// config 模块的单元测试（第 2 部分，AsterSQL 迁移版）。
//
// 本文件针对配置模块中的若干独立能力进行验证：
// - `AtomicBool` 的 TOML/JSON 序列化与反序列化行为（对齐 Go 版本的
//   encoding.TextUnmarshaler / json.Marshaler 语义）；
// - `max_allowed_packet`（MySQL 协议中单个数据包的最大字节数）的取值校验；
// - 临时存储目录名的编码规则；
// - 错误信息扩展（error-msg-extension）的正则编译与匹配；
// - 隐藏配置项的大小写不敏感匹配；
// - 服务器连接相关字段与默认值、TOML 键名的一致性。

use astersql_config::{
    AtomicBool, Config, ErrorMessageExtension, MAX_ALLOWED_PACKET_UNIT, MAX_OF_MAX_ALLOWED_PACKET,
    MIN_MAX_ALLOWED_PACKET, contain_hidden_config, encode_def_temp_storage_dir,
    prepare_error_message_extensions, valid_max_allowed_packet,
};

/// 验证 `AtomicBool` 与 Go 版本的文本/JSON 编解码行为一致：
/// 可以从 TOML 布尔值反序列化，并以裸 `true`/`false` 形式做 JSON 序列化。
#[test]
fn atomic_bool_matches_go_text_and_json_behavior() {
    // TOML 顶层不能直接是标量，因此借助 BoolWrapper 包装后再取出字段。
    let enabled: AtomicBool = toml::from_str("value = true")
        .map(|wrapper: BoolWrapper| wrapper.value)
        .unwrap();
    assert!(enabled.load());
    assert_eq!(serde_json::to_string(&enabled).unwrap(), "true");
    assert_eq!(
        serde_json::from_str::<AtomicBool>("false").unwrap().load(),
        false
    );
}

/// 测试辅助结构：把 `AtomicBool` 包装成一个 TOML 表字段，
/// 以便通过 `toml::from_str` 触发其反序列化逻辑。
#[derive(serde::Deserialize)]
struct BoolWrapper {
    value: AtomicBool,
}

/// 验证 `max_allowed_packet` 的合法性检查遵循 MySQL 规则：
/// 取值必须落在最小/最大边界内，且按 `MAX_ALLOWED_PACKET_UNIT`（1024 字节）对齐。
#[test]
fn max_allowed_packet_uses_mysql_bounds_and_alignment() {
    assert!(valid_max_allowed_packet(MIN_MAX_ALLOWED_PACKET));
    assert!(valid_max_allowed_packet(MAX_OF_MAX_ALLOWED_PACKET));
    assert!(!valid_max_allowed_packet(MIN_MAX_ALLOWED_PACKET - 1));
    assert!(!valid_max_allowed_packet(
        MAX_OF_MAX_ALLOWED_PACKET + MAX_ALLOWED_PACKET_UNIT
    ));
    assert!(!valid_max_allowed_packet(MIN_MAX_ALLOWED_PACKET + 1));
}

/// 验证默认临时存储目录名的编码结果稳定且 URL 安全：
/// 由 "host:port/status_host:status_port" 经 Base64 编码得到，
/// 保证同一实例总是映射到同一目录，且不含路径非法字符。
#[test]
fn temp_storage_name_is_stable_and_url_safe() {
    let path = encode_def_temp_storage_dir("/tmp", "0.0.0.0", "0.0.0.0", 4000, 10080);
    assert!(path.starts_with("/tmp/"));
    assert!(path.ends_with("/MC4wLjAuMDo0MDAwLzAuMC4wLjA6MTAwODA=/tmp-storage"));
}

/// 验证错误信息扩展的准备逻辑：正则模式会被编译并复制到结果中，
/// 原始配置保持不变；非法正则应返回带有说明的错误。
/// 错误信息扩展用于给匹配到的服务端错误信息追加自定义后缀（例如文档链接）。
#[test]
fn error_message_extensions_are_compiled_and_copied() {
    let configured = vec![ErrorMessageExtension::new("^Access denied", " docs ")];
    let (prepared, error) = prepare_error_message_extensions(&configured, true);
    assert!(error.is_none());
    assert_eq!(prepared[0].pattern, "^Access denied");
    assert_eq!(prepared[0].suffix, " docs ");
    assert!(prepared[0].matches("Access denied for user"));
    assert_eq!(configured[0].pattern, "^Access denied");

    // "[" 是非法正则，编译应失败并返回可读的错误信息。
    let (prepared, error) =
        prepare_error_message_extensions(&[ErrorMessageExtension::new("[", "bad")], true);
    assert!(prepared.is_empty());
    let error = error.unwrap();
    assert!(
        error
            .to_string()
            .contains("invalid error-msg-extension regexp")
    );
}

/// Go 在 `ignoreInvalid` 为 true 时仍会返回遇到的首个错误，即使同一批配置里
/// 还有可成功编译的规则；调用方据此记录配置问题，不能静默吞掉无效规则。
#[test]
fn error_message_extensions_report_invalid_entry_in_mixed_input() {
    let configured = [
        ErrorMessageExtension::new("^Access denied", " docs "),
        ErrorMessageExtension::new("[", "bad"),
    ];

    let (prepared, error) = prepare_error_message_extensions(&configured, true);
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].pattern, "^Access denied");
    let error = error.unwrap();
    assert!(
        error
            .to_string()
            .contains("invalid error-msg-extension regexp")
    );
}

/// 验证隐藏配置项的匹配不区分大小写。
/// 隐藏配置指不对外公开、仅在内部使用的配置键（如已废弃或实验性选项），
/// 在导出配置时需要被识别并过滤。
#[test]
fn hidden_config_matching_is_case_insensitive() {
    assert!(contain_hidden_config(
        "PERFORMANCE.INDEX-USAGE-SYNC-LEASE = '1s'"
    ));
    assert!(contain_hidden_config("prepared-plan-cache.capacity = 100"));
    assert!(!contain_hidden_config("performance.stats-lease = '3s'"));
}

/// 验证服务器连接相关字段与 Go 版本的默认值和 TOML 键名保持一致：
/// `cors`（跨域来源白名单）、`socket`（Unix 域套接字路径）、
/// `security.auto-tls`（是否自动生成 TLS 证书）。
#[test]
fn server_connection_fields_match_go_defaults_and_names() {
    let config = Config::default();
    assert_eq!(config.cors, "");
    assert_eq!(config.socket, "/tmp/tidb-{Port}.sock");
    assert!(!config.security.auto_tls);

    let parsed: Config = toml::from_str(
        "cors = 'https://example.com'\nsocket = '/tmp/tidb.sock'\n[security]\nauto-tls = true\n",
    )
    .unwrap();
    assert_eq!(parsed.cors, "https://example.com");
    assert_eq!(parsed.socket, "/tmp/tidb.sock");
    assert!(parsed.security.auto_tls);
}

/// 验证 `performance.force-init-stats`（启动时强制等待统计信息初始化完成，
/// 统计信息供优化器估算执行计划代价）的默认值为 true，且 TOML 键名正确。
#[test]
fn force_init_stats_matches_go_default_and_toml_name() {
    assert!(Config::default().performance.force_init_stats);

    let parsed: Config = toml::from_str("[performance]\nforce-init-stats = false\n").unwrap();
    assert!(!parsed.performance.force_init_stats);
}
