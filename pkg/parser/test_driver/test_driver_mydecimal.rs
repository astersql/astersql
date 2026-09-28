// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 测试驱动精简版 MyDecimal：十进制字面量的解析与格式化。
//
// 对照 Go `test_driver_mydecimal.go`：用定长 word 数组（每 word 9 位十进制）
// 保存 DECIMAL 值；不支持的 TiDB 专属分支以 panic 拒绝，避免静默偏离。
// MyDecimal 是 MySQL DECIMAL/NUMERIC 在内存中的定点数表示。

// 这段逻辑只解析、保存和格式化内存中的十进制字面量。
// 原 Go 明确不支持的 TiDB 专属分支仍以 panic 拒绝，避免用大段占位掩盖行为差异。
use crate::{isDigit, isSpace, myMin, pow10};

/// 未实现 TiDB 专属分支时的 panic 提示文案。
pub const panicInfo: &str = "This branch is not implemented. This is because you are trying to test something specific to TiDB's MyDecimal implementation. It is recommended to do this in TiDB repository.";

// MyDecimal 固定持有 9 个 word，每个 word 保存 9 位十进制数。
/// word 缓冲区最大长度（9）。
pub const maxWordBufLen: usize = 9;
/// 每个 word 容纳的十进制位数。
pub const digitsPerWord: i32 = 9;
/// 提取 word 最高位数字的掩码（10^8）。
pub const digMask: i32 = 100_000_000;
/// 运行时使用的 word 缓冲区长度（与 maxWordBufLen 相同）。
pub static wordBufLen: usize = 9;

// fixWordCntError 限制整数和小数 word 总数；原简化驱动不做截断，而是直接拒绝超长输入。
/// 校验整数+小数 word 总数不超缓冲；超限则 panic。
pub fn fixWordCntError(words_int: i32, words_frac: i32) -> Result<(i32, i32), String> {
    if words_int + words_frac > wordBufLen as i32 {
        panic!("{panicInfo}");
    }
    Ok((words_int, words_frac))
}

// countLeadingZeroes 从指定十进制位宽向下比较 10 的幂，计算首个 word 可删除的前导零。
/// 计算指定位宽下 word 中可去掉的前导零个数。
pub fn countLeadingZeroes(mut index: i32, word: i32) -> i32 {
    let mut leading = 0;
    while word < pow10(index) {
        index -= 1;
        leading += 1;
    }
    leading
}

// digitsToWords 对十进制位数做向上整除。
/// 将十进制位数向上取整为所需 word 数。
pub fn digitsToWords(digits: i32) -> i32 {
    (digits + digitsPerWord - 1) / digitsPerWord
}

// MyDecimal 对应 Go 的十进制值：位数元数据与定长 word 数组保持原字段顺序。
/// 定点数：整数/小数位数、符号与定长 wordBuf。
#[derive(Clone, Default)]
pub struct MyDecimal {
    pub digitsInt: i8,
    pub digitsFrac: i8,
    pub resultFrac: i8,
    pub negative: bool,
    // 每个 word 的合法范围为 0 <= word < 10^9。
    pub wordBuf: [i32; maxWordBufLen],
}

impl MyDecimal {
    // String 复制接收者后调用 ToString，保留 Go 值副本避免格式化改变原值的约定。
    /// 克隆后格式化为十进制字符串（不修改 self）。
    pub fn String(&self) -> String {
        let mut temporary = self.clone();
        String::from_utf8(temporary.ToString()).expect("decimal output contains ASCII only")
    }

    // stringSize 为符号、整数零和小数点各预留一个字符。
    /// 估算格式化输出所需最大字符缓冲。
    pub fn stringSize(&self) -> usize {
        (self.digitsInt as i32 + self.digitsFrac as i32 + 3) as usize
    }

    // removeLeadingZeros 跳过全零整数 word，并修正首个非零 word 的有效位数。
    /// 跳过整数区前导零 word，返回起始下标与有效整数位数。
    pub fn removeLeadingZeros(&self) -> (usize, i32) {
        let mut word_index = 0usize;
        let mut digits_int = self.digitsInt as i32;
        let mut width = ((digits_int - 1) % digitsPerWord) + 1;
        while digits_int > 0 && self.wordBuf[word_index] == 0 {
            digits_int -= width;
            width = digitsPerWord;
            word_index += 1;
        }
        if digits_int > 0 {
            digits_int -=
                countLeadingZeroes((digits_int - 1) % digitsPerWord, self.wordBuf[word_index]);
        } else {
            digits_int = 0;
        }
        (word_index, digits_int)
    }

    // ToString 不做舍入，按 wordBuf 中的整数区和小数区生成可打印 ASCII。
    /// 按 wordBuf 生成可打印 ASCII（可能调整临时位数元数据语义由调用约定保证）。
    pub fn ToString(&mut self) -> Vec<u8> {
        let mut output = vec![0u8; self.stringSize()];
        let mut digits_frac = self.digitsFrac as i32;
        let (mut word_start_index, mut digits_int) = self.removeLeadingZeros();
        if digits_int + digits_frac == 0 {
            // 零值仍需输出一个整数位。
            digits_int = 1;
            word_start_index = 0;
        }

        let mut digits_int_len = digits_int;
        if digits_int_len == 0 {
            digits_int_len = 1;
        }
        let digits_frac_len = digits_frac;
        let mut length = digits_int_len + digits_frac_len;
        if self.negative {
            length += 1;
        }
        if digits_frac > 0 {
            length += 1;
        }
        output.truncate(length as usize);

        let mut string_index = 0usize;
        if self.negative {
            output[string_index] = b'-';
            string_index += 1;
        }

        let mut fill;
        if digits_frac > 0 {
            let mut fraction_index = string_index + digits_int_len as usize;
            fill = digits_frac_len - digits_frac;
            let mut word_index = word_start_index + digitsToWords(digits_int) as usize;
            output[fraction_index] = b'.';
            fraction_index += 1;
            while digits_frac > 0 {
                let mut word = self.wordBuf[word_index];
                word_index += 1;
                // 每个小数 word 从最高位向最低位输出，末个 word 只取声明的小数位数。
                for _ in 0..myMin(digits_frac, digitsPerWord) {
                    let digit = word / digMask;
                    output[fraction_index] = digit as u8 + b'0';
                    fraction_index += 1;
                    word -= digit * digMask;
                    word *= 10;
                }
                digits_frac -= digitsPerWord;
            }
            while fill > 0 {
                output[fraction_index] = b'0';
                fraction_index += 1;
                fill -= 1;
            }
        }

        fill = digits_int_len - digits_int;
        if digits_int == 0 {
            fill -= 1; // 为小数点前的单个 0 留位。
        }
        while fill > 0 {
            output[string_index] = b'0';
            string_index += 1;
            fill -= 1;
        }

        if digits_int > 0 {
            string_index += digits_int as usize;
            let mut word_index = word_start_index + digitsToWords(digits_int) as usize;
            while digits_int > 0 {
                word_index -= 1;
                let mut word = self.wordBuf[word_index];
                // 整数区从低位 word 反向取模写入，最终字符顺序仍为高位到低位。
                for _ in 0..myMin(digits_int, digitsPerWord) {
                    let quotient = word / 10;
                    string_index -= 1;
                    output[string_index] = b'0' + (word - quotient * 10) as u8;
                    word = quotient;
                }
                digits_int -= digitsPerWord;
            }
        } else {
            output[string_index] = b'0';
        }
        output
    }

    // FromString 解析可选空白、符号、整数和小数部分，并按每 9 位一个 word 写入缓冲区。
    /// 从字节串解析十进制字面量写入 self；科学计数法等未实现分支会 panic。
    pub fn FromString(&mut self, input: &[u8]) -> Result<(), String> {
        let mut text = input;
        for (index, byte) in input.iter().enumerate() {
            if !isSpace(*byte) {
                text = &input[index..];
                break;
            }
        }
        if text.is_empty() {
            panic!("{panicInfo}");
        }

        match text[0] {
            b'-' => {
                self.negative = true;
                text = &text[1..];
            }
            b'+' => text = &text[1..],
            _ => {}
        }

        let mut string_index = 0usize;
        while string_index < text.len() && isDigit(text[string_index]) {
            string_index += 1;
        }
        let mut digits_int = string_index as i32;
        let mut digits_frac;
        let end_index;
        if string_index < text.len() && text[string_index] == b'.' {
            let mut end = string_index + 1;
            while end < text.len() && isDigit(text[end]) {
                end += 1;
            }
            digits_frac = (end - string_index - 1) as i32;
            end_index = end;
        } else {
            digits_frac = 0;
            end_index = string_index;
        }
        if digits_int + digits_frac == 0 {
            panic!("{panicInfo}");
        }

        let words_int = digitsToWords(digits_int);
        let words_frac = digitsToWords(digits_frac);
        let (words_int, _) = fixWordCntError(words_int, words_frac)?;
        self.digitsInt = digits_int as i8;
        self.digitsFrac = digits_frac as i8;

        // 整数部分从小数点向左读取，每 9 位形成一个 word，并从整数区末端反向存放。
        let mut word_index = words_int as usize;
        let integer_end = string_index;
        let mut word = 0i32;
        let mut inner_index = 0i32;
        while digits_int > 0 {
            digits_int -= 1;
            string_index -= 1;
            word += (text[string_index] - b'0') as i32 * pow10(inner_index);
            inner_index += 1;
            if inner_index == digitsPerWord {
                word_index -= 1;
                self.wordBuf[word_index] = word;
                word = 0;
                inner_index = 0;
            }
        }
        if inner_index != 0 {
            word_index -= 1;
            self.wordBuf[word_index] = word;
        }

        // 小数部分从小数点向右读取，不足 9 位的末 word 在右侧补零。
        word_index = words_int as usize;
        string_index = integer_end;
        word = 0;
        inner_index = 0;
        while digits_frac > 0 {
            digits_frac -= 1;
            string_index += 1;
            word = (text[string_index] - b'0') as i32 + word * 10;
            inner_index += 1;
            if inner_index == digitsPerWord {
                self.wordBuf[word_index] = word;
                word_index += 1;
                word = 0;
                inner_index = 0;
            }
        }
        if inner_index != 0 {
            self.wordBuf[word_index] = word * pow10(digitsPerWord - inner_index);
        }

        // 科学计数法属于精简测试驱动未实现的 TiDB 专属分支。
        if end_index < text.len() && (text[end_index] == b'e' || text[end_index] == b'E') {
            panic!("{panicInfo}");
        }

        // 规范化负零：所有 word 为零时清除负号。
        if self.wordBuf[..wordBufLen].iter().all(|word| *word == 0) {
            self.negative = false;
        }
        self.resultFrac = self.digitsFrac;
        Ok(())
    }
}
