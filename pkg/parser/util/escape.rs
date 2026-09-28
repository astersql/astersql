// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

// MySQL 字符串字面量反斜杠转义还原，对照 Go 的 `escape.go`。
//
// 词法分析在遇到 `\` 后调用本模块，把下一个字节按 MySQL 规则还原为实际字符；
// LIKE 通配符 `\%` / `\_` 需保留反斜杠，因此返回两字节。

// 本文件由 pkg/parser/util/escape.go 迁移而来，保留 MySQL 反斜杠转义分支。
// 该实现执行内存中字节到字节的转换。
// UnescapeChar 接收反斜杠后的单个字节，并返回 MySQL 规则下的替换字节。
// 大多数输入产生一个字节；百分号和下划线为 LIKE 模式保留反斜杠，因此产生两个字节。
// See https://dev.mysql.com/doc/refman/8.0/en/string-literals.html
// `\" \' \\ \n \0 \b \Z \r \t` 转义为一个字符。
// `\% \_` 保留两个字符；其他输入只移除反斜杠。
/// UnescapeChar 接收反斜杠后的单个字节，按 MySQL 规则返回替换后的字节序列。
pub fn UnescapeChar(b: u8) -> Vec<u8> {
    match b {
        b'n' => vec![b'\n'],
        b'0' => vec![0],
        b'b' => vec![8],
        // MySQL 的 \Z 表示 ASCII 26（Windows 文本 EOF）。
        b'Z' => vec![26],
        b'r' => vec![b'\r'],
        b't' => vec![b'\t'],
        // LIKE 通配符必须连同反斜杠保留，不能走默认的单字节分支。
        b'%' | b'_' => vec![b'\\', b],
        // 引号、反斜杠及未知转义均对应 Go 默认分支：丢弃前导反斜杠。
        _ => vec![b],
    }
}
