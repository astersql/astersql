// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

use crate::{charset, charset_switch::switchDefaultCollation};

#[test]
fn switch_default_collation_updates_both_charset_catalogs() {
    for (enabled, expected_gbk, expected_gb18030) in [
        (
            true,
            charset::CollationGBKChineseCI,
            charset::CollationGB18030ChineseCI,
        ),
        (
            false,
            charset::CollationGBKBin,
            charset::CollationGB18030Bin,
        ),
    ] {
        switchDefaultCollation(enabled);

        let infos = charset::CharacterSetInfos.read().unwrap();
        for (charset_name, binary, chinese, expected) in [
            (
                charset::CharsetGBK,
                charset::CollationGBKBin,
                charset::CollationGBKChineseCI,
                expected_gbk,
            ),
            (
                charset::CharsetGB18030,
                charset::CollationGB18030Bin,
                charset::CollationGB18030ChineseCI,
                expected_gb18030,
            ),
        ] {
            let info = infos.get(charset_name).unwrap();
            assert_eq!(info.DefaultCollation, expected);
            assert_eq!(info.Collations.get(binary).unwrap().IsDefault, !enabled);
            assert_eq!(info.Collations.get(chinese).unwrap().IsDefault, enabled);
        }
    }
}
