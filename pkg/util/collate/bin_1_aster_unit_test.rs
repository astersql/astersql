// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 二进制与中文相关排序规则的单元测试：padding、通配、ID 改写与注册表。
//
// 对齐 Go collate 行为，覆盖 bin/padding 尾空格、byte/rune 通配、
// NewCollation 开关下的 ID 正负改写，以及 GBK/GB18030 键值与注册表回退。

use util_collate::*;

/// 校验 bin 保留尾空格、padding 截去尾空格及 KeyWithoutTrimRightSpace。
#[test]
fn binary_collators_match_go_padding_and_byte_rules() {
    let binary = binCollator::default();
    assert_eq!(binary.Compare("a", "a "), -1);
    assert_eq!(binary.Key("a "), b"a ");

    let padded = binPaddingCollator::default();
    assert_eq!(padded.Compare("a", "a  "), 0);
    assert_eq!(padded.Key("a  "), b"a");
    assert_eq!(padded.KeyWithoutTrimRightSpace("a  "), b"a  ");
}

/// 校验 binary 字节通配与 derived rune 通配对中文的匹配差异。
#[test]
fn binary_patterns_match_go_byte_and_rune_semantics() {
    let mut binary = binCollator::default().Pattern();
    binary.Compile(r"a\_%", b'\\');
    assert!(binary.DoMatch("a_中文"));
    assert!(!binary.DoMatch("ab中文"));

    let mut derived = derivedBinCollator::default().Pattern();
    derived.Compile("_文", b'\\');
    assert!(derived.DoMatch("中文"));
    assert!(!derived.DoMatch("ab文"));
}

/// NewCollation 开关下 ID 正负改写、恢复与 charset→bin 映射。
#[test]
fn collation_switch_and_protocol_ids_match_go() {
    let _guard = crate::collate_test::COLLATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    SetNewCollationEnabledForTest(true);
    assert!(NewCollationEnabled());
    assert_eq!(RewriteNewCollationIDIfNeeded(5), -5);
    assert_eq!(RewriteNewCollationIDIfNeeded(-5), -5);
    assert_eq!(RestoreCollationIDIfNeeded(-5), 5);
    assert_eq!(RestoreCollationIDIfNeeded(5), 5);
    assert_eq!(ConvertAndGetBinCollation("gbk_chinese_ci"), "gbk_bin");

    SetNewCollationEnabledForTest(false);
    assert_eq!(RewriteNewCollationIDIfNeeded(5), 5);
    assert_eq!(RestoreCollationIDIfNeeded(-5), -5);
}

/// GBK/GB18030 bin 键字节与 MaxKeyLen 对齐 Go 样例。
#[test]
fn gbk_and_gb18030_keys_match_go_examples() {
    let gbk = gbkBinCollator::default();
    assert_eq!(gbk.Key("中文"), vec![0xD6, 0xD0, 0xCE, 0xC4]);
    assert_eq!(gbk.Key("a "), b"a");
    assert_eq!(gbk.MaxKeyLen("中文"), 4);

    let gb18030 = gb18030BinCollator::default();
    assert_eq!(gb18030.Key("中文"), vec![0xD6, 0xD0, 0xCE, 0xC4]);
    assert_eq!(gb18030.MaxKeyLen("中文"), 8);
}

/// 中文 CI 权重键与大小写不敏感比较。
#[test]
fn chinese_ci_weights_and_keys_match_go_tables() {
    let gbk = gbkChineseCICollator::default();
    assert_eq!(gbk.Key("a"), vec![0x41]);
    assert_eq!(gbk.Compare("a", "A"), 0);
    assert_eq!(gbk.Key("中文"), vec![0xD3, 0x21, 0xC1, 0xAD]);

    let gb18030 = gb18030ChineseCICollator::default();
    assert_eq!(gb18030.Compare("a", "A"), 0);
    assert_eq!(gb18030.Key("a "), gb18030.KeyWithoutTrimRightSpace("a"));
    assert_eq!(gb18030.MaxKeyLen("中文"), 8);
}

/// 注册表按名称返回分组 collator，未知名回退 binary；开关影响 utf8mb4_bin padding。
#[test]
fn focused_registry_returns_group_collators_and_binary_fallback() {
    let _guard = crate::collate_test::COLLATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    SetNewCollationEnabledForTest(true);
    assert_eq!(GetCollator("binary").Key("a "), b"a ");
    assert_eq!(GetCollator("utf8mb4_bin").Key("a "), b"a");
    assert_eq!(
        GetCollator("gbk_bin").Key("中文"),
        vec![0xD6, 0xD0, 0xCE, 0xC4]
    );
    assert_eq!(GetCollator("missing").Key("a "), b"a");

    SetNewCollationEnabledForTest(false);
    assert_eq!(GetCollator("utf8mb4_bin").Key("a "), b"a ");
}
