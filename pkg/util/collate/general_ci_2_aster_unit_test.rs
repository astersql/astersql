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

// general_ci / unicode_ci / 0900_ai_ci / 拼音 Collator 的迁移期单元测试。
//
// 对齐 Go：大小写与口音折叠、PAD SPACE、排序 key、通配符权重，以及拼音未实现契约。

use util_collate::general_ci::{ciPattern, convertRuneGeneralCI, generalCICollator};
use util_collate::pinyin_tidb_as_cs::zhPinyinTiDBASCSCollator;
use util_collate::unicode_0400_ci_generated::unicodeCICollator;
use util_collate::unicode_0900_ai_ci_generated::unicode0900AICICollator;

/// 验证 general_ci：大小写/口音等价、PAD SPACE、key 字节与非 BMP 映射为 0xFFFD。
#[test]
fn general_ci_matches_go_case_accent_padding_and_keys() {
    let collator = generalCICollator::default();
    assert_eq!(collator.Compare("a", "A "), 0);
    assert_eq!(collator.Compare("À", "a"), 0);
    assert!(collator.Compare("a", "b") < 0);
    assert_eq!(collator.Key("a  "), vec![0x00, 0x41]);
    assert_eq!(
        collator.KeyWithoutTrimRightSpace("a "),
        vec![0x00, 0x41, 0x00, 0x20]
    );
    assert_eq!(collator.MaxKeyLen("a界"), 4);
    assert_eq!(convertRuneGeneralCI('😀'), 0xFFFD);
}

/// 验证 LIKE pattern 使用 general_ci 权重（À 与 a 等价）。
#[test]
fn general_ci_wildcard_uses_collation_weights() {
    let mut pattern = ciPattern::default();
    pattern.Compile("À_%", b'\\');
    assert!(pattern.DoMatch("a界suffix"));
    assert!(!pattern.DoMatch("b界suffix"));
}

/// 验证 unicode_ci（UCA 4.0）：权重、PAD SPACE 与通配符匹配。
#[test]
fn unicode_0400_matches_go_weights_padding_and_pattern() {
    let collator = unicodeCICollator::default();
    assert_eq!(collator.Compare("a", "A "), 0);
    assert_eq!(collator.Key("a "), collator.Key("A"));
    assert_eq!(collator.MaxKeyLen("a界"), 32);
    let mut pattern = collator.Pattern();
    pattern.Compile("À%", b'\\');
    assert!(pattern.DoMatch("asuffix"));
}

/// 验证 utf8mb4_0900_ai_ci：大小写不敏感但不做尾空格预处理。
#[test]
fn unicode_0900_matches_go_weights_without_space_preprocessing() {
    let collator = unicode0900AICICollator::default();
    assert_eq!(collator.Compare("a", "A"), 0);
    assert_ne!(collator.Compare("a", "a "), 0);
    assert_ne!(collator.Key("a"), collator.Key("a "));
    assert_eq!(collator.MaxKeyLen("a界"), 32);
    let mut pattern = collator.Pattern();
    pattern.Compile("À%", b'\\');
    assert!(pattern.DoMatch("asuffix"));
}

/// 拼音 collator 仍保持 Go 的未实现 panic 契约。
#[test]
#[should_panic(expected = "implement me")]
fn pinyin_collator_preserves_go_unimplemented_contract() {
    zhPinyinTiDBASCSCollator::default().Compare("a", "b");
}
