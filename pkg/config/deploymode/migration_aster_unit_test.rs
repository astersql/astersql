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

// 部署模式（deploy mode）模块的迁移单元测试。
//
// 本文件对应 Go(TiDB) 版本 deploymode 包测试的机械迁移，用于验证 Rust 迁移
// 实现与 Go 原实现行为一致，覆盖以下三个方面：
// - 部署模式字符串的解析（`Parse`）与模式列表、字符串化、合法性判断；
// - JSON / TOML 序列化与反序列化的成功路径与错误路径；
// - 当前进程全局部署模式的读取（`Get`）与设置（`Set`），并区分是否启用
//   `nextgen`（下一代 TiDB 架构）编译特性。
//
// 术语说明：部署模式（deploy mode）用于区分数据库集群的运行形态，例如
// `premium`（标准高级模式）、`premium_reserved`（高级预留资源模式）、
// `starter`（入门/轻量模式），不同模式下内核可能启用不同的资源与功能策略。

// 引入被测的部署模式 API：全局读写函数、模式常量与解析函数。
use crate::deploymode::{
    Get, IsPremiumReserved, IsStarter, Mode, ModeList, Parse, Premium, PremiumReserved, Set,
    Starter,
};

/// 验证字符串解析、字符串化、合法性判断与模式列表的行为与 Go 版一致。
#[test]
fn migration_parse_string_valid_and_list_match_go() {
    // Parse 对输入大小写不敏感，但不会去除首尾空白，
    // 因此带空格的 " starter " 应返回错误。
    assert_eq!(Ok(Premium), Parse("PrEmIuM"));
    assert_eq!(Ok(PremiumReserved), Parse("PREMIUM_RESERVED"));
    assert_eq!(Ok(Starter), Parse("Starter"));
    assert_eq!(
        Err(r#"invalid deploy mode " starter ""#.to_string()),
        Parse(" starter ")
    );

    // String() 输出规范化的小写名称；未知数值模式输出 "unknown(N)"。
    // Valid() 仅对预定义模式返回 true；ModeList() 返回全部合法模式。
    assert_eq!("premium", Premium.String());
    assert_eq!("premium_reserved", PremiumReserved.String());
    assert_eq!("starter", Starter.String());
    assert_eq!("unknown(-7)", Mode(-7).String());
    assert!(Premium.Valid());
    assert!(!Mode(3).Valid());
    assert_eq!(vec![Premium, PremiumReserved, Starter], ModeList());
}

/// 验证 JSON 与 TOML 的序列化/反序列化行为与 Go 版一致，覆盖成功与错误路径。
#[test]
fn migration_json_and_toml_match_go_success_and_error_paths() {
    // 合法模式序列化为带引号的 JSON 字符串；非法数值模式序列化时报错。
    assert_eq!(
        br#""premium_reserved""#.to_vec(),
        PremiumReserved.MarshalJSON().unwrap()
    );
    assert_eq!(
        Err("invalid deploy mode 100".to_string()),
        Mode(100).MarshalJSON()
    );

    // JSON 反序列化：合法字符串（大小写不敏感）成功；
    // 非字符串（如数字 1）或未知模式名报错。
    let mut mode = Premium;
    mode.UnmarshalJSON(br#""Starter""#).unwrap();
    assert_eq!(Starter, mode);
    assert!(mode.UnmarshalJSON(b"1").is_err());
    assert_eq!(
        Err(r#"invalid deploy mode "unknown""#.to_string()),
        mode.UnmarshalJSON(br#""unknown""#)
    );

    // TOML 反序列化：仅接受字符串类型的值，整数等其他类型报错。
    mode.UnmarshalTOML(&toml::Value::String("Premium_Reserved".to_string()))
        .unwrap();
    assert_eq!(PremiumReserved, mode);
    assert_eq!(
        Err("invalid deploy mode 1".to_string()),
        mode.UnmarshalTOML(&toml::Value::Integer(1))
    );
}

/// 验证全局部署模式的读写行为与对应 Go 构建（是否 nextgen）一致。
///
/// 对应 Go 侧通过 build tag 区分构建产物：经典构建下部署模式固定为
/// premium 且不可修改；nextgen（下一代架构）构建下允许在合法模式间切换。
#[test]
fn migration_current_mode_matches_the_selected_go_build() {
    // 未启用 nextgen 特性：模式固定为 Premium，Set 一律报错且不改变状态。
    #[cfg(not(feature = "nextgen"))]
    {
        assert_eq!(Premium, Get());
        assert!(!IsPremiumReserved());
        assert!(!IsStarter());
        assert_eq!(
            Err("deploy mode can only be set for nextgen TiDB".to_string()),
            Set(PremiumReserved)
        );
        assert_eq!(Premium, Get());
    }

    // 启用 nextgen 特性：可在合法模式间切换，非法模式（如 Mode(100)）报错；
    // 测试结束前恢复为 Premium，避免影响其他测试的全局状态。
    #[cfg(feature = "nextgen")]
    {
        assert_eq!(Premium, Get());
        Set(PremiumReserved).unwrap();
        assert_eq!(PremiumReserved, Get());
        assert!(IsPremiumReserved());
        assert!(!IsStarter());
        Set(Starter).unwrap();
        assert!(!IsPremiumReserved());
        assert!(IsStarter());
        assert_eq!(Err("invalid deploy mode 100".to_string()), Set(Mode(100)));
        Set(Premium).unwrap();
    }
}
