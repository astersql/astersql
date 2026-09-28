// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 字符集编码的公共基座逻辑。
//
// 对应 Go `encoding_base.go`：封装字符遍历、转换缓冲区、错误生成与零拷贝字符串视图。
// 各具体编码（ASCII/GBK/UTF-8 等）通过 `self_encoding` 回指复用本基座的 `transform`/`foreach`。

// 负责字符遍历、转换缓冲区、错误生成和零拷贝字符串视图。
// x/text、parser/mysql 与 terror 的对象只保留接口形状，后续由同包模块和 Rust 编码库完成接线。
/// ErrInvalidCharacterString 对应 Go 的 parser 标准错误实例。
pub static ERR_INVALID_CHARACTER_STRING: std::sync::LazyLock<Box<terror::Error>> =
    std::sync::LazyLock::new(|| {
        terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrInvalidCharacterString as isize))
    });

/// EncodingBase 对应 Go encodingBase，保存底层编码器和对具体 Encoding 实现的回指。
pub struct EncodingBase {
    pub enc: Encoding,
    pub self_encoding: Option<EncodingRef>,
}

impl EncodingBase {
    /// new 仅建立底层编码依赖；具体实现会在各文件的 init 迁移函数中补上 self 回指。
    pub fn new(enc: Encoding) -> Self {
        Self {
            enc,
            self_encoding: None,
        }
    }

    /// 设置对具体 Encoding 实现的回指，供基座在遍历/转换时派发到子类型。
    pub fn set_self(&mut self, encoding: EncodingRef) {
        self.self_encoding = Some(encoding);
    }

    /// mb_len 对应 Go 默认实现；不识别多字节长度时返回 0。
    pub fn mb_len(&self, _src: &str) -> usize {
        0
    }

    /// to_upper 对应 strings.ToUpper，使用 Rust Unicode 大写映射。
    pub fn to_upper(&self, src: &str) -> String {
        src.to_uppercase()
    }

    /// to_lower 对应 strings.ToLower，使用 Rust Unicode 小写映射。
    pub fn to_lower(&self, src: &str) -> String {
        src.to_lowercase()
    }

    /// is_valid 对应 Go 实现：遍历时遇到首个无效字符便停止并返回 false。
    pub fn is_valid(&self, src: &[u8]) -> bool {
        let mut valid = true;
        let mut inspect = |_: &[u8], _: &[u8], ok| {
            valid = ok;
            ok
        };
        encoding_by_ref(self.self_encoding.expect("encodingBase.self must be initialized"))
            .foreach(src, OP_FROM_UTF8, &mut inspect);
        valid
    }

    /// transform 对应通用转换入口，根据 Op 决定错误、截断、替换以及收集转换前后的字节。
    pub fn transform<'a>(
        &self,
        dest: Option<&mut ByteBuffer>,
        src: &'a [u8],
        op: Op,
    ) -> Result<TransformResult<'a>, EncodingError> {
        // Go 允许 nil dest 并按输入长度预分配；Rust 用局部缓冲承接相同生命周期内的写入。
        let mut owned_dest = ByteBuffer::with_capacity(src.len());
        let dest = dest.unwrap_or(&mut owned_dest);
        dest.clear();
        let mut first_error = None;

        let mut collect = |from: &[u8], to: &[u8], ok: bool| {
            if !ok {
                // 与 Go 一致，只记录首个错误；opSkipError 会完全抑制错误对象。
                if first_error.is_none() && op & OP_SKIP_ERROR == 0 {
                    first_error = Some(generate_encoding_err(self.name(), from));
                }
                if op & OP_TRUNCATE_TRIM != 0 {
                    return false;
                }
                if op & OP_TRUNCATE_REPLACE != 0 {
                    dest.push(b'?');
                    return true;
                }
            }
            if op & OP_COLLECT_FROM != 0 {
                dest.extend_from_slice(from);
            } else if op & OP_COLLECT_TO != 0 {
                dest.extend_from_slice(to);
            }
            true
        };
        encoding_by_ref(self.self_encoding.expect("encodingBase.self must be initialized"))
            .foreach(src, op, &mut collect);

        match first_error {
            Some(error) => Err(error),
            None => Ok(TransformResult::Owned(dest.as_slice().to_vec())),
        }
    }

    /// foreach 对应 Go 的 transform.Transformer 驱动循环，每次只向回调暴露一个编码字符。
    pub fn foreach<F>(&self, src: &[u8], op: Op, mut callback: F)
    where
        F: FnMut(&[u8], &[u8], bool) -> bool,
    {
        let self_encoding = self.self_encoding.expect("encodingBase.self must be initialized");
        let from_utf8 = op & OP_FROM_UTF8 != 0;
        let mut transformer: Box<dyn Transformer> = if from_utf8 {
            self.enc.new_encoder()
        } else {
            self.enc.new_decoder()
        };

        let mut index = 0;
        let mut buf = [0_u8; 4];
        while index < src.len() {
            let width = if from_utf8 {
                encoding_utf8_impl().peek(&src[index..]).len()
            } else {
                encoding_by_ref(self_encoding).peek(&src[index..]).len()
            };
            let from = &src[index..index + width];
            let transformed = transformer.transform(&mut buf, from, false);
            let (written, transform_error, rune_error_is_input) = match transformed {
                Ok(written) => (written, false, transformer.rune_error_is_last_input()),
                Err(error) => (error.written(), true, false),
            };

            // 解码为 UTF-8 时，替换字符可能是合法输入 U+FFFD；只有不是原输入时才算转换失败。
            let replacement_error = op & OP_TO_UTF8 != 0
                && begin_with_replacement_char(&buf[..written])
                && !rune_error_is_input;
            if !callback(from, &buf[..written], !(transform_error || replacement_error)) {
                return;
            }
            index += width;
        }
    }

    /// 通过回指获取具体编码的名称，供错误信息使用。
    fn name(&self) -> &'static str {
        encoding_by_ref(self.self_encoding.expect("encodingBase.self must be initialized")).name()
    }
}

/// REPLACEMENT_BYTES 对应 Unicode 替换字符 U+FFFD 的 UTF-8 编码。
pub const REPLACEMENT_BYTES: &[u8; 3] = &[0xef, 0xbf, 0xbd];

/// begin_with_replacement_char 对应 bytes.HasPrefix 检查。
pub fn begin_with_replacement_char(dst: &[u8]) -> bool {
    dst.starts_with(REPLACEMENT_BYTES)
}

/// generate_encoding_err 保留 Go 的十六进制无效字节参数格式和 parser 错误类别。
pub fn generate_encoding_err(name: &str, invalid_bytes: &[u8]) -> EncodingError {
    let argument = invalid_bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    ERR_INVALID_CHARACTER_STRING.FastGenByArgs(&[
        errors::ErrorArg::String(name.to_owned()),
        errors::ErrorArg::String(argument),
    ])
}

/// hack_slice 对应 Go HackSlice，以只读借用表达无复制 string -> []byte 视图。
pub fn hack_slice(src: &str) -> &[u8] {
    src.as_bytes()
}

/// hack_string 对应 Go HackString；调用方必须保证输入是 UTF-8，等价于原实现“自行承担风险”的约束。
pub unsafe fn hack_string(bytes: &[u8]) -> &str {
    if bytes.is_empty() {
        return "";
    }
    // SAFETY: Go 原实现不会校验字节；把该前置条件明确交给调用者。
    unsafe { std::str::from_utf8_unchecked(bytes) }
}
