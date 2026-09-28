// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 跨模块回归：SQLError、locale 格式化、权限元数据、SQLSTATE 与类型默认值。
//
// 对照 Go 行为核对错误展示/脱敏、FORMAT 分组、权限列名往返，以及标志位与
// 字段长度工具函数，确保机械迁移后协议侧元数据不被改写。

use crate::r#const::{AuthCachingSha2Password, AuthNativePassword, AuthTiDBSM3Password};
use crate::error::{ErrBadConn, ErrMalformPacket, NewErr, NewErrf};
use crate::errors::{RedactLogDisable, RedactLogEnable, RedactLogEnabled, RedactLogMarker};
use crate::locale_format::FormatByLocale;
use crate::privs::*;
use crate::state::{DefaultMySQLState, MySQLState};
use crate::r#type::*;
use crate::util::{
    GetDefaultFieldLengthAndDecimal, GetDefaultFieldLengthAndDecimalForCast, IsAuthPluginClearText,
    IsIntegerType,
};

/// 覆盖连接错误、已知/自定义模板、三种脱敏模式与 Unicode 精度截断。
#[test]
fn sql_error_matches_go_state_formatting_and_redaction() {
    // 保存并在结束时恢复全局脱敏开关，避免污染其它测试。
    let original_redact_mode = RedactLogEnabled.Load();
    RedactLogEnabled.Store(RedactLogDisable);
    assert_eq!(ErrBadConn().to_string(), "connection was bad");
    assert_eq!(ErrMalformPacket().to_string(), "malform packet error");

    // ErrNoDB：查默认消息与 SQLSTATE 3D000，Display 形状为 ERROR code (state): message。
    let known = NewErr(crate::errcode::ErrNoDB, vec![]);
    assert_eq!(known.State, "3D000");
    assert_eq!(known.Message, "No database selected");
    assert_eq!(
        known.to_string(),
        "ERROR 1046 (3D000): No database selected"
    );

    // 未知错误码回退 DefaultMySQLState；关闭脱敏时参数原样写入。
    let custom = NewErrf(
        0,
        "user %s has %d grants and value %v",
        &[0],
        vec!["alice".into(), 2_i32.into(), "secret".into()],
    );
    assert_eq!(custom.State, DefaultMySQLState);
    assert_eq!(custom.Message, "user alice has 2 grants and value secret");

    // Enable 用 `?` 替换；Marker 用 ‹› 包裹，且精度截断发生在脱敏之前。
    RedactLogEnabled.Store(RedactLogEnable);
    assert_eq!(
        NewErrf(0, "user %s", &[0], vec!["alice".into()]).Message,
        "user ?"
    );
    RedactLogEnabled.Store(RedactLogMarker);
    assert_eq!(
        NewErrf(0, "user %s", &[0], vec!["alice".into()]).Message,
        "user ‹alice›"
    );
    assert_eq!(
        NewErrf(0, "value %.3s", &[0], vec!["abcdef".into()]).Message,
        "value ‹abc›"
    );
    RedactLogEnabled.Store(RedactLogDisable);

    // `%-.3s` 按 Unicode 标量截断；三字符中文整词保留。
    let truncated = NewErrf(0, "name=%-.3s", &[], vec!["数据库".into()]);
    assert_eq!(truncated.Message, "name=数据库");
    RedactLogEnabled.Store(original_redact_mode);
}

/// 校验 en_US / de_DE / en_IN 分组、未知 locale 回退与非数字输入零填充。
#[test]
fn locale_formatting_matches_go_grouping_and_fallbacks() {
    assert_eq!(
        FormatByLocale("1234567.8", "2", "en_US"),
        ("1,234,567.80".into(), true, Ok(()))
    );
    assert_eq!(
        FormatByLocale("-1234567.8", "2", "de_DE"),
        ("-1.234.567,80".into(), true, Ok(()))
    );
    // 印度分组：最右三位，左侧两位一组。
    assert_eq!(
        FormatByLocale("1234567890.1234", "3", "en_IN"),
        ("1,23,45,67,890.123".into(), true, Ok(()))
    );
    // 精度串只取开头数字；未知 locale 仍格式化但 found=false。
    assert_eq!(
        FormatByLocale("12.3", "2junk", "missing"),
        ("12.30".into(), false, Ok(()))
    );
    assert_eq!(
        FormatByLocale("nonnumeric", "2", "fr_FR"),
        ("0,00".into(), true, Ok(()))
    );
    assert_eq!(
        FormatByLocale("12.3.4", "2", "en_US"),
        ("12.00".into(), true, Ok(())),
        "Go keeps repeated decimal points, then treats the split as lacking one fraction"
    );
}

/// 权限列名/集合枚举双向映射、Has 与 AllPrivMask 位数与 Go 一致。
#[test]
fn privilege_metadata_round_trips_like_go() {
    for privilege in AllGlobalPrivs().into_iter().chain(StaticGlobalOnlyPrivs()) {
        let column = privilege.ColumnString();
        assert!(
            !column.is_empty(),
            "missing column for {}",
            privilege.String()
        );
        assert_eq!(NewPrivFromColumn(column), Some(privilege));
    }
    for privilege in AllTablePrivs().into_iter().chain(AllColumnPrivs()) {
        let value = privilege.SetString();
        assert!(
            !value.is_empty(),
            "missing set value for {}",
            privilege.String()
        );
        assert_eq!(NewPrivFromSetEnum(value), Some(privilege));
    }
    assert!(vec![InsertPriv, SelectPriv].Has(SelectPriv));
    // AllPriv 是哨兵位，Privileges::Has 不做掩码展开。
    assert!(!vec![AllPriv].Has(InsertPriv));
    assert_eq!(AllPrivMask.0, AllPriv.0 - 1);
    assert_eq!(Priv2UserCol().len(), AllGlobalPrivs().len() + 1);
    assert_eq!(Priv2Str().len(), Priv2UserCol().len() + 2);
}

/// SQLSTATE 抽样、列标志谓词、整数类型判定与默认长度/鉴权插件分类。
#[test]
fn sql_states_flags_and_type_defaults_match_go() {
    let states = MySQLState();
    // 23000 完整性约束；42S02 表不存在；40001 死锁；22032 JSON 文本非法。
    assert_eq!(states[&crate::errcode::ErrDupEntry], "23000");
    assert_eq!(states[&crate::errcode::ErrNoSuchTable], "42S02");
    assert_eq!(states[&crate::errcode::ErrLockDeadlock], "40001");
    assert_eq!(states[&crate::errcode::ErrInvalidJSONText], "22032");

    assert!(HasNotNullFlag(NotNullFlag));
    assert!(HasUniKeyFlag(UniqueKeyFlag));
    assert!(HasNoDefaultValueFlag(NoDefaultValueFlag));
    assert!(HasAutoIncrementFlag(AutoIncrementFlag));
    assert!(HasUnsignedFlag(UnsignedFlag));
    assert!(HasZerofillFlag(ZerofillFlag));
    assert!(HasBinaryFlag(BinaryFlag));
    assert!(HasPriKeyFlag(PriKeyFlag));
    assert!(HasMultipleKeyFlag(MultipleKeyFlag));
    assert!(HasTimestampFlag(TimestampFlag));
    assert!(HasOnUpdateNowFlag(OnUpdateNowFlag));

    assert!(IsIntegerType(TypeLonglong));
    assert!(!IsIntegerType(TypeDouble));
    assert_eq!(GetDefaultFieldLengthAndDecimal(TypeLonglong), (20, 0));
    assert_eq!(
        GetDefaultFieldLengthAndDecimal(TypeJSON),
        (4_294_967_295, 0)
    );
    // 未知类型返回 (-1, -1)；CAST 路径对 CHAR/JSON 有独立默认值。
    assert_eq!(GetDefaultFieldLengthAndDecimal(0xff), (-1, -1));
    assert_eq!(GetDefaultFieldLengthAndDecimalForCast(TypeString), (0, -1));
    assert_eq!(
        GetDefaultFieldLengthAndDecimalForCast(TypeJSON),
        (4_194_304, 0)
    );
    // Clear-text 鉴权插件集合与 Go 一致；mysql_clear_password 名本身不在集合内。
    assert!(IsAuthPluginClearText(AuthNativePassword));
    assert!(IsAuthPluginClearText(AuthTiDBSM3Password));
    assert!(IsAuthPluginClearText(AuthCachingSha2Password));
    assert!(!IsAuthPluginClearText("mysql_clear_password"));
}
