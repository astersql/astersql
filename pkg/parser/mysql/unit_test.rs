// Copyright 2026 AsterSQL.

// `parser/mysql` 包级冒烟测试：串联字符集、错误码、权限、常量与工具函数的公开入口。
//
// 这些用例验证各子模块在迁移后仍能互相协作，不启动服务器、不访问磁盘。

use super::{charset, r#const, errcode, errname, error, locale_format, privs, state, r#type, util};

/// 校验默认字符集到校对规则 ID 的映射，以及 UTF8 族判断。
#[test]
fn parser_mysql_charset_maps_default_collations() {
    // utf8mb4 / gb18030 应映射到各自默认 collation ID；未知名返回 0。
    assert_eq!(
        charset::CharsetNameToID("utf8mb4"),
        charset::UTF8MB4DefaultCollationID
    );
    assert_eq!(
        charset::CharsetNameToID("gb18030"),
        charset::GB18030DefaultCollationID
    );
    assert_eq!(charset::CharsetNameToID("unknown_charset"), 0);

    assert!(charset::IsUTF8Charset("utf8"));
    assert!(charset::IsUTF8Charset("utf8mb4"));
    assert!(!charset::IsUTF8Charset("latin1"));
}

/// 校验 collation ID 与名称的双向查找，以及未知值返回 None。
#[test]
fn parser_mysql_charset_collation_round_trips_known_values() {
    assert_eq!(charset::GetCollationNameByID(46), Some("utf8mb4_bin"));
    assert_eq!(charset::GetCollationIDByName("utf8mb4_bin"), Some(46));

    assert_eq!(charset::GetCollationNameByID(309), Some("utf8mb4_0900_bin"));
    assert_eq!(charset::GetCollationIDByName("utf8mb4_0900_bin"), Some(309));

    assert_eq!(charset::GetCollationNameByID(u16::MAX), None);
    assert_eq!(charset::GetCollationIDByName("unknown_collation"), None);
}

/// 抽样确认错误码、类型标志与权限列名等核心导出仍可用。
#[test]
fn parser_mysql_minimal_target_exports_protocol_flags() {
    assert_eq!(errcode::ErrNoDB, 1046);
    assert!(r#type::HasNotNullFlag(r#type::NotNullFlag));
    assert_eq!(
        privs::NewPrivFromColumn("Select_priv"),
        Some(privs::SelectPriv)
    );
    assert_eq!(privs::NewPrivFromColumn("Unknown_priv"), None);
}

/// 校验 SQLSTATE / 错误名表对已知码有条目，对未知码无条目。
#[test]
fn parser_mysql_error_metadata_maps_known_codes() {
    assert_eq!(state::MySQLState().get(&errcode::ErrNoDB), Some(&"3D000"));
    assert!(
        errname::MySQLErrName()[&errcode::ErrDupEntry]
            .Raw
            .contains("Duplicate entry")
    );

    let unknown_error_code = u16::MAX;
    assert!(!state::MySQLState().contains_key(&unknown_error_code));
    assert!(!errname::MySQLErrName().contains_key(&unknown_error_code));
}

/// 校验 NewErr / NewErrf 构造默认与自定义错误消息，以及 Error() 展示格式。
#[test]
fn parser_mysql_sql_error_builds_default_and_custom_messages() {
    // NewErr 按错误码查模板；无参时仍应生成非空 Message。
    let default_error = error::NewErr(errcode::ErrNoDB, vec![]);
    assert_eq!(default_error.Code, errcode::ErrNoDB);
    assert_eq!(default_error.State, "3D000");
    assert!(!default_error.Message.is_empty());

    // NewErrf 使用调用方格式串；码为 0 时回退 DefaultMySQLState。
    let custom_error = error::NewErrf(0, "customized error", &[], vec![]);
    assert_eq!(custom_error.Code, 0);
    assert_eq!(custom_error.State, state::DefaultMySQLState);
    assert_eq!(custom_error.Message, "customized error");

    let display = default_error.Error();
    assert!(!display.is_empty());
    assert!(display.contains(&errcode::ErrNoDB.to_string()));
    assert!(display.contains("3D000"));
}

/// 校验按 locale 格式化数字：en_US 千分位、en_IN 印度分组、未知 locale 回退。
#[test]
fn parser_mysql_locale_format_handles_grouping_and_fallback() {
    let (formatted, found, result) = locale_format::FormatByLocale("1234567.8", "2", "en_US");
    assert_eq!(formatted, "1,234,567.80");
    assert!(found);
    assert!(result.is_ok());

    let (formatted, found, result) = locale_format::FormatByLocale("1234567890.1", "3", "en_IN");
    assert_eq!(formatted, "1,23,45,67,890.100");
    assert!(found);
    assert!(result.is_ok());

    // 未知 locale：found=false，但仍按默认规则产出可读字符串。
    let (formatted, found, result) =
        locale_format::FormatByLocale("1234567.8", "2", "unknown_locale");
    assert_eq!(formatted, "1,234,567.80");
    assert!(!found);
    assert!(result.is_ok());
}

/// 校验服务器版本串、默认认证插件、SQL Mode 解析与 TiDBX 版本转换辅助函数。
#[test]
fn parser_mysql_const_exposes_protocol_sqlmode_and_versions() {
    assert!(r#const::ServerVersion().contains(r#const::VersionSeparator));
    assert!(r#const::DefaultAuthPlugins.contains(&r#const::AuthNativePassword));

    // FormatSQLModeStr 展开 ansi 等别名中的具体 mode 标志。
    let formatted = r#const::FormatSQLModeStr("ansi");
    assert!(formatted.split(',').any(|mode| mode == "REAL_AS_FLOAT"));
    assert!(
        formatted
            .split(',')
            .any(|mode| mode == "ONLY_FULL_GROUP_BY")
    );

    let Ok(mode) = r#const::GetSQLMode("STRICT_TRANS_TABLES,NO_ZERO_DATE") else {
        panic!("known SQL modes should parse");
    };
    assert!(mode.HasStrictMode());
    assert!(mode.HasNoZeroDateMode());

    // TiDBX release / server 版本字符串的合法与非法样例。
    for (release, expected) in [
        ("v26.3.0", "CLOUD.202603.0"),
        ("v26.3.0-xxx", "CLOUD.202603.0-xxx"),
    ] {
        let Ok(version) = r#const::BuildTiDBXReleaseVersion(release) else {
            panic!("valid TiDBX release version should parse");
        };
        assert_eq!(version, expected);
    }

    for (release, expected) in [
        ("v26.3.0", "8.0.11-TiDB-CLOUD.202603.0"),
        ("v26.3.0-xxx", "8.0.11-TiDB-CLOUD.202603.0-xxx"),
    ] {
        let Ok(version) = r#const::BuildTiDBXServerVersion(release) else {
            panic!("valid TiDBX server version should build");
        };
        assert_eq!(version, expected);
    }

    for release in ["26.1.1", "v26xxxx", "v24.1.1", "v26.0.1", "v26.13.1"] {
        assert!(r#const::BuildTiDBXReleaseVersion(release).is_err());
    }

    assert_eq!(
        r#const::NormalizeTiDBReleaseVersionForNextGen("v8.4.0-this-is-a-placeholder"),
        "v26.3.0-this-is-a-placeholder"
    );
    assert_eq!(
        r#const::NormalizeTiDBReleaseVersionForNextGen("v26.3.0"),
        "v26.3.0"
    );
}

/// 校验整数类型判断、DDL/CAST 默认长度小数位，以及明文认证插件识别。
#[test]
fn parser_mysql_util_exposes_type_defaults_and_auth_helpers() {
    assert!(util::IsIntegerType(r#type::TypeLonglong));
    assert!(!util::IsIntegerType(r#type::TypeJSON));

    assert_eq!(
        util::GetDefaultFieldLengthAndDecimal(r#type::TypeLong),
        (11, 0)
    );
    assert_eq!(util::GetDefaultFieldLengthAndDecimal(u8::MAX), (-1, -1));
    assert_eq!(
        util::GetDefaultFieldLengthAndDecimalForCast(r#type::TypeJSON),
        (4_194_304, 0)
    );

    assert!(util::IsAuthPluginClearText(r#const::AuthNativePassword));
    assert!(!util::IsAuthPluginClearText(
        r#const::AuthMySQLClearPassword
    ));
}
