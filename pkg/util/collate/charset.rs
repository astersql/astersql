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

// 字符集默认排序规则切换：在 GBK/GB18030 的 bin 与 chinese_ci 之间切换默认项。
//
// 对应 Go `pkg/util/collate/charset.go`。`flag=true` 时默认改为 chinese_ci，
// 并更新 Collations 上的 IsDefault 标记；`false` 时切回 bin。

use crate::charset;

/// 按 flag 切换 GBK/GB18030 的 DefaultCollation 及其 IsDefault 标志。
pub fn switchDefaultCollation(flag: bool) {
    // Go 在包 init 阶段已把静态 collation 表挂到 CharacterSetInfos；Rust 的
    // 对应目录是惰性初始化的，因此切换前必须先完成同一初始化。
    charset::init();
    let mut infos = charset::CharacterSetInfos
        .write()
        .expect("charset metadata lock poisoned");
    // 仅处理带 chinese_ci / bin 成对默认值的东亚字符集
    for (charset_name, binary, chinese) in [
        (
            charset::CharsetGBK,
            charset::CollationGBKBin,
            charset::CollationGBKChineseCI,
        ),
        (
            charset::CharsetGB18030,
            charset::CollationGB18030Bin,
            charset::CollationGB18030ChineseCI,
        ),
    ] {
        let info = infos
            .get_mut(charset_name)
            .expect("built-in charset metadata missing");
        info.DefaultCollation = if flag { chinese } else { binary }.to_owned();
        info.Collations
            .get_mut(binary)
            .expect("built-in binary collation metadata missing")
            .IsDefault = !flag;
        info.Collations
            .get_mut(chinese)
            .expect("built-in chinese_ci collation metadata missing")
            .IsDefault = flag;
    }
}
