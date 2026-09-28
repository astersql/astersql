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

// collate 包功能测试：Compare/Key、新 collation 开关、ID 重写与非法 UTF-8 处理。
//
// 对齐 Go `collate_test.go`；全局开关测试用互斥锁隔离，避免并行污染。

// 这些测试覆盖 collator 比较、key 生成、全局新 collation 开关、ID 重写和非法 UTF-8 输入处理。

use std::sync::Mutex;
use util_collate::*;

/// 串行化依赖全局新 collation 开关的测试，防止并发互相覆盖。
pub(crate) static COLLATION_TEST_LOCK: Mutex<()> = Mutex::new(());

// CompareTable 对应 Go 的 compareTable，Expect 按 collations 顺序保存每个 collator 的比较结果。
/// 单条比较用例：左右串与各 collation 期望的 Compare 结果。
struct CompareTable {
    left: &'static str,
    right: &'static str,
    expect: &'static [i32],
}

// KeyTable 对应 Go 的 keyTable，expect 中每个字节切片对应同下标 collation 的 Key/ImmutableKey。
struct KeyTable {
    str_: &'static str,
    expect: &'static [&'static [u8]],
}

// test_compare_table 对应 Go 的 testCompareTable。
// 外层遍历 collation 名称，内层逐条执行 Compare，保留 Go 中 fmt.Sprintf 的失败上下文。
fn test_compare_table(collations: &[&str], tests: &[CompareTable]) {
    for (i, c) in collations.iter().enumerate() {
        let collator = GetCollator(c);
        for table in tests {
            let comment = format!(
                "Compare Left: {} Right: {}, Using {}",
                table.left, table.right, c
            );
            assert_eq!(
                table.expect[i],
                collator.Compare(table.left, table.right),
                "{comment}"
            );
        }
    }
}

// test_key_table 对应 Go 的 testKeyTable。
// Go 同时检查 Key 和 ImmutableKey；保留两次断言，强调不可变 key 也应等价。
fn test_key_table(collations: &[&str], tests: &[KeyTable]) {
    for (i, c) in collations.iter().enumerate() {
        let collator = GetCollator(c);
        for test in tests {
            let comment = format!("key {}, using {}", test.str_, c);
            assert_eq!(
                test.expect[i],
                collator.Key(test.str_).as_slice(),
                "{comment}"
            );
            assert_eq!(
                test.expect[i],
                collator.ImmutableKey(test.str_).as_slice(),
                "{comment}"
            );
        }
    }
}

// test_utf8_collator_compare 对应 Go 的 TestUTF8CollatorCompare。
// Go 用 defer 在测试结束时关闭新 collation；这里用显式收尾调用记录相同的全局状态恢复语义。
#[test]
fn test_utf8_collator_compare() {
    let _guard = COLLATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    SetNewCollationEnabledForTest(true);
    let collations = [
        "binary",
        "utf8mb4_bin",
        "utf8mb4_general_ci",
        "utf8mb4_unicode_ci",
        "utf8mb4_0900_ai_ci",
        "utf8mb4_0900_bin",
        "gbk_bin",
        "gbk_chinese_ci",
    ];
    let tests = [
        CompareTable {
            left: "a",
            right: "b",
            expect: &[-1, -1, -1, -1, -1, -1, -1, -1],
        },
        CompareTable {
            left: "a",
            right: "A",
            expect: &[1, 1, 0, 0, 0, 1, 1, 0],
        },
        CompareTable {
            left: "À",
            right: "A",
            expect: &[1, 1, 0, 0, 0, 1, -1, -1],
        },
        CompareTable {
            left: "abc",
            right: "abc",
            expect: &[0, 0, 0, 0, 0, 0, 0, 0],
        },
        CompareTable {
            left: "abc",
            right: "ab",
            expect: &[1, 1, 1, 1, 1, 1, 1, 1],
        },
        CompareTable {
            left: "😜",
            right: "😃",
            expect: &[1, 1, 0, 0, 1, 1, 0, 0],
        },
        CompareTable {
            left: "a",
            right: "a ",
            expect: &[-1, 0, 0, 0, -1, -1, 0, 0],
        },
        CompareTable {
            left: "a ",
            right: "a  ",
            expect: &[-1, 0, 0, 0, -1, -1, 0, 0],
        },
        CompareTable {
            left: "a\t",
            right: "a",
            expect: &[1, 1, 1, 1, 1, 1, 1, 1],
        },
        CompareTable {
            left: "ß",
            right: "s",
            expect: &[1, 1, 0, 1, 1, 1, -1, -1],
        },
        CompareTable {
            left: "ß",
            right: "ss",
            expect: &[1, 1, -1, 0, 0, 1, -1, -1],
        },
        CompareTable {
            left: "啊",
            right: "吧",
            expect: &[1, 1, 1, 1, 1, 1, -1, -1],
        },
        CompareTable {
            left: "中文",
            right: "汉字",
            expect: &[-1, -1, -1, -1, -1, -1, 1, 1],
        },
        CompareTable {
            left: "æ",
            right: "ae",
            expect: &[1, 1, 1, 1, 0, 1, -1, -1],
        },
        CompareTable {
            left: "Å",
            right: "A",
            expect: &[1, 1, 1, 0, 0, 1, 1, 1],
        },
        CompareTable {
            left: "Å",
            right: "A",
            expect: &[1, 1, 0, 0, 0, 1, -1, -1],
        },
        CompareTable {
            left: "\u{1730F}",
            right: "啊",
            expect: &[1, 1, 1, 1, -1, 1, -1, -1],
        },
        CompareTable {
            left: "가",
            right: "㉡",
            expect: &[1, 1, 1, 1, -1, 1, 0, 0],
        },
        CompareTable {
            left: "갟",
            right: "감1",
            expect: &[1, 1, 1, 1, 1, 1, -1, -1],
        },
        CompareTable {
            left: "\u{FFFFE}",
            right: "\u{FFFFF}",
            expect: &[-1, -1, 0, 0, -1, -1, 0, 0],
        },
    ];
    test_compare_table(&collations, &tests);
    SetNewCollationEnabledForTest(false);
}

// test_utf8_collator_key 对应 Go 的 TestUTF8CollatorKey。
// 每个 expect 子切片仍按 collations 顺序排列，避免丢失 Go 表驱动测试的索引语义。
#[test]
fn test_utf8_collator_key() {
    let _guard = COLLATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    SetNewCollationEnabledForTest(true);
    let collations = [
        "binary",
        "utf8mb4_bin",
        "utf8mb4_general_ci",
        "utf8mb4_unicode_ci",
        "utf8mb4_0900_ai_ci",
        "utf8mb4_0900_bin",
        "gbk_bin",
        "gbk_chinese_ci",
    ];
    let tests = [
        KeyTable {
            str_: "a",
            expect: &[
                &[0x61],
                &[0x61],
                &[0x0, 0x41],
                &[0x0E, 0x33],
                &[0x1C, 0x47],
                &[0x61],
                &[0x61],
                &[0x41],
            ],
        },
        KeyTable {
            str_: "A",
            expect: &[
                &[0x41],
                &[0x41],
                &[0x0, 0x41],
                &[0x0E, 0x33],
                &[0x1C, 0x47],
                &[0x41],
                &[0x41],
                &[0x41],
            ],
        },
        KeyTable {
            str_: "Foo © bar 𝌆 baz ☃ qux",
            expect: &[
                &[
                    0x46, 0x6f, 0x6f, 0x20, 0xc2, 0xa9, 0x20, 0x62, 0x61, 0x72, 0x20, 0xf0, 0x9d,
                    0x8c, 0x86, 0x20, 0x62, 0x61, 0x7a, 0x20, 0xe2, 0x98, 0x83, 0x20, 0x71, 0x75,
                    0x78,
                ],
                &[
                    0x46, 0x6f, 0x6f, 0x20, 0xc2, 0xa9, 0x20, 0x62, 0x61, 0x72, 0x20, 0xf0, 0x9d,
                    0x8c, 0x86, 0x20, 0x62, 0x61, 0x7a, 0x20, 0xe2, 0x98, 0x83, 0x20, 0x71, 0x75,
                    0x78,
                ],
                &[
                    0x0, 0x46, 0x0, 0x4f, 0x0, 0x4f, 0x0, 0x20, 0x0, 0xa9, 0x0, 0x20, 0x0, 0x42,
                    0x0, 0x41, 0x0, 0x52, 0x0, 0x20, 0xff, 0xfd, 0x0, 0x20, 0x0, 0x42, 0x0, 0x41,
                    0x0, 0x5a, 0x0, 0x20, 0x26, 0x3, 0x0, 0x20, 0x0, 0x51, 0x0, 0x55, 0x0, 0x58,
                ],
                &[
                    0x0E, 0xB9, 0x0F, 0x82, 0x0F, 0x82, 0x02, 0x09, 0x02, 0xC5, 0x02, 0x09, 0x0E,
                    0x4A, 0x0E, 0x33, 0x0F, 0xC0, 0x02, 0x09, 0xFF, 0xFD, 0x02, 0x09, 0x0E, 0x4A,
                    0x0E, 0x33, 0x10, 0x6A, 0x02, 0x09, 0x06, 0xFF, 0x02, 0x09, 0x0F, 0xB4, 0x10,
                    0x1F, 0x10, 0x5A,
                ],
                &[
                    0x1c, 0xe5, 0x1d, 0xdd, 0x1d, 0xdd, 0x2, 0x9, 0x5, 0x84, 0x2, 0x9, 0x1c, 0x60,
                    0x1c, 0x47, 0x1e, 0x33, 0x2, 0x9, 0xe, 0xf0, 0x2, 0x9, 0x1c, 0x60, 0x1c, 0x47,
                    0x1f, 0x21, 0x2, 0x9, 0x9, 0x1b, 0x2, 0x9, 0x1e, 0x21, 0x1e, 0xb5, 0x1e, 0xff,
                ],
                &[
                    0x46, 0x6f, 0x6f, 0x20, 0xc2, 0xa9, 0x20, 0x62, 0x61, 0x72, 0x20, 0xf0, 0x9d,
                    0x8c, 0x86, 0x20, 0x62, 0x61, 0x7a, 0x20, 0xe2, 0x98, 0x83, 0x20, 0x71, 0x75,
                    0x78,
                ],
                &[
                    0x46, 0x6f, 0x6f, 0x20, 0x3f, 0x20, 0x62, 0x61, 0x72, 0x20, 0x3f, 0x20, 0x62,
                    0x61, 0x7a, 0x20, 0x3f, 0x20, 0x71, 0x75, 0x78,
                ],
                &[
                    0x46, 0x4f, 0x4f, 0x20, 0x3f, 0x20, 0x42, 0x41, 0x52, 0x20, 0x3f, 0x20, 0x42,
                    0x41, 0x5a, 0x20, 0x3f, 0x20, 0x51, 0x55, 0x58,
                ],
            ],
        },
        KeyTable {
            str_: "a ",
            expect: &[
                &[0x61, 0x20],
                &[0x61],
                &[0x0, 0x41],
                &[0x0E, 0x33],
                &[0x1c, 0x47, 0x2, 0x9],
                &[0x61, 0x20],
                &[0x61],
                &[0x41],
            ],
        },
        KeyTable {
            str_: "ﷻ",
            expect: &[
                &[0xEF, 0xB7, 0xBB],
                &[0xEF, 0xB7, 0xBB],
                &[0xFD, 0xFB],
                &[
                    0x13, 0x5E, 0x13, 0xAB, 0x02, 0x09, 0x13, 0x5E, 0x13, 0xAB, 0x13, 0x50, 0x13,
                    0xAB, 0x13, 0xB7,
                ],
                &[
                    0x23, 0x25, 0x23, 0x9c, 0x2, 0x9, 0x23, 0x25, 0x23, 0x9c, 0x23, 0xb, 0x23,
                    0x9c, 0x23, 0xb1,
                ],
                &[0xEF, 0xB7, 0xBB],
                &[0x3f],
                &[0x3F],
            ],
        },
        KeyTable {
            str_: "中文",
            expect: &[
                &[0xE4, 0xB8, 0xAD, 0xE6, 0x96, 0x87],
                &[0xE4, 0xB8, 0xAD, 0xE6, 0x96, 0x87],
                &[0x4E, 0x2D, 0x65, 0x87],
                &[0xFB, 0x40, 0xCE, 0x2D, 0xFB, 0x40, 0xE5, 0x87],
                &[0xFB, 0x40, 0xCE, 0x2D, 0xfB, 0x40, 0xE5, 0x87],
                &[0xE4, 0xB8, 0xAD, 0xE6, 0x96, 0x87],
                &[0xD6, 0xD0, 0xCE, 0xC4],
                &[0xD3, 0x21, 0xC1, 0xAD],
            ],
        },
        KeyTable {
            str_: "갟감1",
            expect: &[
                &[0xea, 0xb0, 0x9f, 0xea, 0xb0, 0x90, 0x31],
                &[0xea, 0xb0, 0x9f, 0xea, 0xb0, 0x90, 0x31],
                &[0xac, 0x1f, 0xac, 0x10, 0x0, 0x31],
                &[0xfb, 0xc1, 0xac, 0x1f, 0xfb, 0xc1, 0xac, 0x10, 0xe, 0x2a],
                &[
                    0x3b, 0xf5, 0x3c, 0x74, 0x3c, 0xd3, 0x3b, 0xf5, 0x3c, 0x73, 0x3c, 0xe0, 0x1c,
                    0x3e,
                ],
                &[0xea, 0xb0, 0x9f, 0xea, 0xb0, 0x90, 0x31],
                &[0x3f, 0x3f, 0x31],
                &[0x3f, 0x3f, 0x31],
            ],
        },
        KeyTable {
            str_: "\u{FFFFE}\u{FFFFF}",
            expect: &[
                &[0xf3, 0xbf, 0xbf, 0xbe, 0xf3, 0xbf, 0xbf, 0xbf],
                &[0xf3, 0xbf, 0xbf, 0xbe, 0xf3, 0xbf, 0xbf, 0xbf],
                &[0xff, 0xfd, 0xff, 0xfd],
                &[0xff, 0xfd, 0xff, 0xfd],
                &[0xfb, 0xdf, 0xff, 0xfe, 0xfb, 0xdf, 0xff, 0xff],
                &[0xf3, 0xbf, 0xbf, 0xbe, 0xf3, 0xbf, 0xbf, 0xbf],
                &[0x3f, 0x3f],
                &[0x3f, 0x3f],
            ],
        },
    ];
    test_key_table(&collations, &tests);
    SetNewCollationEnabledForTest(false);
}

// test_set_new_collate_enabled 对应 Go 的 TestSetNewCollateEnabled。
// 这里显式调用收尾，保留 Go defer 恢复全局开关的测试隔离语义。
#[test]
fn test_set_new_collate_enabled() {
    let _guard = COLLATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    SetNewCollationEnabledForTest(true);
    assert!(NewCollationEnabled());
    SetNewCollationEnabledForTest(false);
}

// test_rewrite_and_restore_collation_id 对应 Go 的 TestRewriteAndRestoreCollationID。
// 新 collation 开启时 ID 通过负数传播；关闭时正负号保持输入原样。
#[test]
fn test_rewrite_and_restore_collation_id() {
    let _guard = COLLATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    SetNewCollationEnabledForTest(true);
    assert_eq!(-5, RewriteNewCollationIDIfNeeded(5));
    assert_eq!(-5, RewriteNewCollationIDIfNeeded(-5));
    assert_eq!(5, RestoreCollationIDIfNeeded(-5));
    assert_eq!(5, RestoreCollationIDIfNeeded(5));
    // Go 的有符号整数取负按二进制补码回绕，MinInt32 保持自身且不会 panic。
    assert_eq!(i32::MIN, RewriteNewCollationIDIfNeeded(i32::MIN));
    assert_eq!(i32::MIN, RestoreCollationIDIfNeeded(i32::MIN));

    SetNewCollationEnabledForTest(false);
    assert_eq!(5, RewriteNewCollationIDIfNeeded(5));
    assert_eq!(-5, RewriteNewCollationIDIfNeeded(-5));
    assert_eq!(5, RestoreCollationIDIfNeeded(5));
    assert_eq!(-5, RestoreCollationIDIfNeeded(-5));
}

// test_get_collator 对应 Go 的 TestGetCollator。
// Go 的 require.IsType 检查返回 trait object 的具体实现；用辅助函数表达同一断言意图。
#[test]
fn test_get_collator() {
    let _guard = COLLATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    SetNewCollationEnabledForTest(true);
    assert_collator_type::<binCollator>(GetCollator("binary"));
    assert_collator_type::<binPaddingCollator>(GetCollator("utf8mb4_bin"));
    assert_collator_type::<binPaddingCollator>(GetCollator("utf8_bin"));
    assert_collator_type::<generalCICollator>(GetCollator("utf8mb4_general_ci"));
    assert_collator_type::<generalCICollator>(GetCollator("utf8_general_ci"));
    assert_collator_type::<unicodeCICollator>(GetCollator("utf8mb4_unicode_ci"));
    assert_collator_type::<unicodeCICollator>(GetCollator("utf8_unicode_ci"));
    assert_collator_type::<zhPinyinTiDBASCSCollator>(GetCollator("utf8mb4_zh_pinyin_tidb_as_cs"));
    assert_collator_type::<unicode0900AICICollator>(GetCollator("utf8mb4_0900_ai_ci"));
    assert_collator_type::<derivedBinCollator>(GetCollator("utf8mb4_0900_bin"));
    assert_collator_type::<binPaddingCollator>(GetCollator("default_test"));
    assert_collator_type::<binCollator>(GetCollatorByID(63));
    assert_collator_type::<binPaddingCollator>(GetCollatorByID(46));
    assert_collator_type::<binPaddingCollator>(GetCollatorByID(83));
    assert_collator_type::<generalCICollator>(GetCollatorByID(45));
    assert_collator_type::<generalCICollator>(GetCollatorByID(33));
    assert_collator_type::<unicodeCICollator>(GetCollatorByID(224));
    assert_collator_type::<unicodeCICollator>(GetCollatorByID(192));
    assert_collator_type::<unicode0900AICICollator>(GetCollatorByID(255));
    assert_collator_type::<zhPinyinTiDBASCSCollator>(GetCollatorByID(2048));
    assert_collator_type::<binPaddingCollator>(GetCollatorByID(9999));
    assert!(
        GetSupportedCollations()
            .iter()
            .all(|collation| collation.Name != "utf8mb4_zh_pinyin_tidb_as_cs"),
        "developing pinyin collation must stay hidden from users"
    );

    SetNewCollationEnabledForTest(false);
    assert_collator_type::<derivedBinCollator>(GetCollator("binary"));
    assert_collator_type::<derivedBinCollator>(GetCollator("utf8mb4_bin"));
    assert_collator_type::<derivedBinCollator>(GetCollator("utf8_bin"));
    assert_collator_type::<derivedBinCollator>(GetCollator("utf8mb4_general_ci"));
    assert_collator_type::<derivedBinCollator>(GetCollator("utf8_general_ci"));
    assert_collator_type::<derivedBinCollator>(GetCollator("utf8mb4_unicode_ci"));
    assert_collator_type::<derivedBinCollator>(GetCollator("utf8_unicode_ci"));
    assert_collator_type::<derivedBinCollator>(GetCollator("utf8mb4_zh_pinyin_tidb_as_cs"));
    assert_collator_type::<derivedBinCollator>(GetCollator("utf8mb4_0900_ai_ci"));
    assert_collator_type::<derivedBinCollator>(GetCollator("default_test"));
    assert_collator_type::<derivedBinCollator>(GetCollatorByID(63));
    assert_collator_type::<derivedBinCollator>(GetCollatorByID(46));
    assert_collator_type::<derivedBinCollator>(GetCollatorByID(83));
    assert_collator_type::<derivedBinCollator>(GetCollatorByID(45));
    assert_collator_type::<derivedBinCollator>(GetCollatorByID(33));
    assert_collator_type::<derivedBinCollator>(GetCollatorByID(224));
    assert_collator_type::<derivedBinCollator>(GetCollatorByID(255));
    assert_collator_type::<derivedBinCollator>(GetCollatorByID(309));
    assert_collator_type::<derivedBinCollator>(GetCollatorByID(192));
    assert_collator_type::<derivedBinCollator>(GetCollatorByID(2048));
    assert_collator_type::<derivedBinCollator>(GetCollatorByID(9999));

    SetNewCollationEnabledForTest(true);
    assert_collator_type::<gbkBinCollator>(GetCollator("gbk_bin"));
    assert_collator_type::<gbkBinCollator>(GetCollatorByID(87));
    SetNewCollationEnabledForTest(false);
}

#[test]
fn unsupported_collation_preserves_dbterror_contract() {
    let _guard = COLLATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    SetNewCollationEnabledForTest(true);

    let error = GetCollationByName("utf8mb4_unicode_520_ci").unwrap_err();
    assert_eq!(
        error.to_string(),
        "[ddl:1273]Unsupported collation when new collation is enabled: 'utf8mb4_unicode_520_ci'"
    );
    let cause = dbterror::errors::Cause(Some(&error)).expect("generated error must have a cause");
    let normalized = cause
        .downcast_ref::<dbterror::terror::Error>()
        .expect("unsupported collation must remain a normalized terror error");
    assert_eq!(
        normalized.Code(),
        dbterror::errno::ErrUnknownCollation as i32
    );
    assert_eq!(normalized.RFCCode(), "ddl:1273");

    assert_eq!(
        ErrIllegalMixCollation.Code(),
        dbterror::errno::ErrCantAggregateNcollations as i32
    );
    assert_eq!(ErrIllegalMixCollation.RFCCode(), "expression:1271");
    assert_eq!(
        ErrIllegalMix2Collation.Code(),
        dbterror::errno::ErrCantAggregate2collations as i32
    );
    assert_eq!(ErrIllegalMix2Collation.RFCCode(), "expression:1267");
    assert_eq!(
        ErrIllegalMix3Collation.Code(),
        dbterror::errno::ErrCantAggregate3collations as i32
    );
    assert_eq!(ErrIllegalMix3Collation.RFCCode(), "expression:1270");
    assert_eq!(
        dbterror::terror::ToSQLError(&ErrIllegalMixCollation).Code,
        dbterror::errno::ErrCantAggregateNcollations
    );
    assert_eq!(
        dbterror::terror::ToSQLError(&ErrIllegalMix2Collation).Code,
        dbterror::errno::ErrCantAggregate2collations
    );
    assert_eq!(
        dbterror::terror::ToSQLError(&ErrIllegalMix3Collation).Code,
        dbterror::errno::ErrCantAggregate3collations
    );

    SetNewCollationEnabledForTest(false);
}

// Go 先由 charset 按大小写/utf8mb3 别名规范化名称，再按规范化后的 collation ID
// 判断实现是否受支持；合法别名不能被误报为 unsupported。
#[test]
fn supported_collation_aliases_follow_charset_lookup() {
    let _guard = COLLATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    SetNewCollationEnabledForTest(true);

    assert_eq!(
        GetCollationByName("UTF8MB4_BIN").unwrap().Name,
        "utf8mb4_bin"
    );
    assert_eq!(GetCollationByName("utf8mb3_bin").unwrap().Name, "utf8_bin");

    SetNewCollationEnabledForTest(false);
}

// assert_collator_type 对应 Go require.IsType，使用生产 Collator::as_any 做真实向下转型断言。
fn assert_collator_type<T: 'static>(collator: Box<dyn Collator>) {
    assert!(
        collator.as_any().is::<T>(),
        "expected collator type {}",
        std::any::type_name::<T>()
    );
}

// test_campare_invalid_utf8_rune 对应 Go 的 TestCampareInvalidUTF8Rune，保留原函数名中的拼写。
// Go string 可以携带无效 UTF-8 字节；Rust str 不能，因此这里把原始字节和转换点都显式标出。
#[test]
fn test_campare_invalid_utf8_rune() {
    let _guard = COLLATION_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    SetNewCollationEnabledForTest(true);
    let collaters = [
        "utf8mb4_general_ci",
        "utf8mb4_0900_ai_ci",
        "utf8mb4_unicode_ci",
        "gbk_chinese_ci",
        "gb18030_bin",
        "gbk_bin",
    ]
    .map(GetCollator);

    for (i, c) in collaters.iter().enumerate() {
        assert_eq!(c.CompareBytes(&[0xff], &[0xff]), 0);
        assert_eq!(c.CompareBytes(&[0xff], &[0xfe]), 0);
        // Go 注释：gbk_bin 和 gb18030_bin 会用 0x3f 替代 invalid utf8 rune，因此只在前四个 collator 检查 padding 行为。
        if i < 4 {
            assert_eq!(c.CompareBytes(&[0xff], &[0xff, 0x3e]), 0);
            assert_eq!(c.CompareBytes(&[0x3e, 0xff], &[0x3e, 0xff, 0x3e]), 0);
            assert!(c.KeyBytes(&[0xff]).is_empty());
        } else {
            assert_eq!(c.KeyBytes(&[0xff]), vec![b'?']);
        }
        assert!(!c.KeyBytes(&[0x3e, 0xff]).is_empty());
        assert!(!c.KeyBytes(&[0x3e, 0xff, 0x3e]).is_empty());
    }
    SetNewCollationEnabledForTest(false);
}
