// Copyright 2015 PingCAP, Inc.
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

// 字符串工具实现：SQL 字面量反引号解析、LIKE 模式编译/匹配、标识符转义等。
//
// 对应 Go `pkg/util/stringutil/string_util.go`。在本地内存处理字符串与字节切片；
// Go `string` 可含任意字节，而 Rust `&str` 须合法 UTF-8，相关函数旁标出语义差异。

// 本文件由 pkg/util/stringutil/string_util.go 迁移而来，保留 Go 实现结构。
// stringutil 包里的字符串反引号解析、LIKE 模式编译/匹配、SQL 标识符转义、
// 标签格式化、UTF-8 字节位置换算以及 ASCII 大小写工具。它只在本地内存中处理字符串和字节切片，
// 其中 errors.Trace、mysql.SQLMode、regexp.QuoteMeta 和 hack.Slice 尚未接通到真实 Rust 依赖；
// 下方用局部占位或标准库保留原 Go 分支结构，避免把误解为可运行实现。
// 本文件没有 goroutine、channel、async 或外部 IO；需要特别注意的是 Go string 可保存任意字节，
// 而 Rust &str 必须是合法 UTF-8，所以相关函数旁边会标出该语义差异。

#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;

// ErrSyntax indicates that a value does not have the right syntax for the target type.
// ErrSyntax 对应 Go 包级变量 errors.New("invalid syntax")。
// Rust 用静态字符串保留错误文本；Go 的 errors.Trace 调用在返回错误处用 StringUtilError 包一层说明。
/// 语法错误消息常量，对应 Go 的 `errors.New("invalid syntax")`。
pub const ErrSyntax: &str = "invalid syntax";

// StringUtilError 是这里用来表达 Go error 返回值的最小占位。
// 它不复刻 pingcap/errors 的堆栈信息，只保留错误消息以维持函数返回形状。
/// Go `error` 的最小占位：仅保留消息文本，不复刻堆栈。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StringUtilError {
    /// 错误消息文本。
    pub message: String,
}

impl StringUtilError {
    fn syntax() -> Self {
        // 对应 Go 的 errors.Trace(ErrSyntax)；真实迁移时应接入统一错误类型。
        Self {
            message: ErrSyntax.to_owned(),
        }
    }
}

impl std::fmt::Display for StringUtilError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StringUtilError {}

// UnquoteChar decodes the first character or byte in the escaped string
// or character literal represented by the string s.
// It returns four values:
// 1) value, the decoded Unicode code point or byte value;
// 2) multibyte, a boolean indicating whether the decoded character requires a multibyte UTF-8 representation;
// 3) tail, the remainder of the string after the character; and
// 4) an error that will be nil if the character is syntactically valid.
// The second argument, quote, specifies the type of literal being parsed
// and therefore which escaped quote character is permitted.
// If set to a single quote, it permits the sequence \' and disallows unescaped '.
// If set to a double quote, it permits \" and disallows unescaped ".
// If set to zero, it does not permit either escape and allows both quote characters to appear unescaped.
// Different with strconv.UnquoteChar, it permits unnecessary backslash.
// UnquoteChar 解码转义字符串中的第一个字符，并返回解码后的字节与剩余尾串。
// Go 注释仍提到 multibyte 返回值，但当前 Go 签名实际只返回 value、tail、err；这里按现有签名迁移。
pub fn UnquoteChar(s: &str, quote: u8) -> Result<(Vec<u8>, String), StringUtilError> {
    let (value, tail) = UnquoteCharBytes(s.as_bytes(), quote)?;
    Ok((value, String::from_utf8_lossy(tail).into_owned()))
}

// UnquoteCharBytes is the byte-preserving form of UnquoteChar. Go strings may
// contain arbitrary bytes, so this is the exact migration path for callers
// that need Go's invalid-UTF-8 behavior.
/// UnquoteChar 的按字节版本：保留非法 UTF-8，供需 Go 任意字节语义的调用方使用。
pub fn UnquoteCharBytes<'a>(
    s: &'a [u8],
    quote: u8,
) -> Result<(Vec<u8>, &'a [u8]), StringUtilError> {
    let Some(&c) = s.first() else {
        return Err(StringUtilError::syntax());
    };
    if c == quote {
        return Err(StringUtilError::syntax());
    }
    if c >= 0x80 {
        let width = Utf8Len(c);
        if width <= 4 && width <= s.len() && std::str::from_utf8(&s[..width]).is_ok() {
            return Ok((s[..width].to_vec(), &s[width..]));
        }
        return Ok((vec![c], &s[1..]));
    }
    if c != b'\\' {
        return Ok((vec![c], &s[1..]));
    }
    if s.len() <= 1 {
        return Err(StringUtilError::syntax());
    }

    let c = s[1];
    let tail = &s[2..];
    let mut value = Vec::with_capacity(2);
    match c {
        b'b' => value.push(b'\x08'),
        b'n' => value.push(b'\n'),
        b'r' => value.push(b'\r'),
        b't' => value.push(b'\t'),
        b'Z' => value.push(0o32),
        b'0' => value.push(0),
        b'_' | b'%' => {
            // LIKE 场景下保留转义符本身，表达 Go 对 \_ 与 \% 的特殊处理。
            value.push(b'\\');
            value.push(c);
        }
        b'\\' => value.push(b'\\'),
        b'\'' | b'"' => value.push(c),
        _ => {
            // TiDB 允许不必要的反斜杠；未知转义直接取反斜杠后的字节。
            value.push(c);
        }
    }
    Ok((value, tail))
}

// Unquote interprets s as a single-quoted, double-quoted,
// or backquoted Go string literal, returning the string value
// that s quotes. For example: test=`"\"\n"` (hex: 22 5c 22 5c 6e 22)
// should be converted to `"\n` (hex: 22 0a).
// Unquote 去掉单引号或双引号包裹，并逐段调用 UnquoteChar 解析反斜杠转义。
pub fn Unquote(s: &str) -> Result<String, StringUtilError> {
    let value = UnquoteBytes(s.as_bytes())?;
    // A valid UTF-8 input remains valid after TiDB's byte-oriented escape
    // removal. Keep the historical String API for existing Rust callers.
    Ok(String::from_utf8(value).expect("unquoting valid UTF-8 must remain valid UTF-8"))
}

// UnquoteBytes preserves the complete Go string behavior, including inputs
// and outputs containing invalid UTF-8 bytes.
/// Unquote 的按字节版本：输入输出均可含非法 UTF-8。
pub fn UnquoteBytes(s: &[u8]) -> Result<Vec<u8>, StringUtilError> {
    let n = s.len();
    if n < 2 {
        return Err(StringUtilError::syntax());
    }

    let quote = s[0];
    if quote != s[n - 1] {
        return Err(StringUtilError::syntax());
    }
    if quote != b'"' && quote != b'\'' {
        return Err(StringUtilError::syntax());
    }

    let mut inner = &s[1..n - 1];
    // Avoid allocation. No need to convert if there is no '\'
    // 无需反转义时直接返回原内容；contains 的字节检查对应 strings.IndexByte。
    if !inner.contains(&b'\\') && !inner.contains(&quote) {
        return Ok(inner.to_vec());
    }

    // Try to avoid more allocations.
    // Go 预分配 3*len/2 的 []byte；Rust Vec 只用容量表达同样的分配意图。
    let mut buf = Vec::with_capacity(3 * inner.len() / 2);
    while !inner.is_empty() {
        let (mb, tail) = UnquoteCharBytes(inner, quote)?;
        inner = tail;
        buf.extend_from_slice(&mb);
    }
    Ok(buf)
}

// PatMatch is the enumeration value for per-character match.
// PatMatch 表示 LIKE 模式里需要逐字符精确匹配的元素。
pub const PatMatch: u8 = 1;
// PatOne is the enumeration value for '_' match.
// PatOne 表示 SQL LIKE 中的 '_'，匹配一个字符或字节。
pub const PatOne: u8 = 2;
// PatAny is the enumeration value for '%' match.
// PatAny 表示 SQL LIKE 中的 '%'，匹配任意长度字符或字节。
pub const PatAny: u8 = 3;

// CompilePatternBinary is used for binary strings.
// CompilePatternBinary 是二进制字符串适配器，保持 Go 代码只转调 CompilePatternInnerBinary 的结构。
pub fn CompilePatternBinary(pattern: &str, escape: u8) -> (Vec<u8>, Vec<u8>) {
    CompilePatternInnerBinary(pattern, escape)
}

// CompilePattern is an adapter for `CompilePatternInner`, `pattern` can be any unicode string.
// CompilePattern 是 Unicode 字符串适配器，返回 rune/char 权重和对应模式类型。
pub fn CompilePattern(pattern: &str, escape: u8) -> (Vec<char>, Vec<u8>) {
    CompilePatternInner(pattern, escape)
}

// CompilePatternInner handles escapes and wild cards convert pattern characters and
// pattern types.
// Note: if anything changes in this method, please double-check CompilePatternInnerBytes
// CompilePatternInner 按 rune 处理 LIKE 模式，识别转义符、'_' 和 '%'。
// 这里的 patWeights/patTypes 与 Go 的两个切片一一对应，patLen 表示已经写入的有效长度。
pub fn CompilePatternInner(pattern: &str, escape: u8) -> (Vec<char>, Vec<u8>) {
    let runes: Vec<char> = pattern.chars().collect();
    let escape_rune = escape as char;
    let len_runes = runes.len();
    let mut pat_weights = vec!['\0'; len_runes];
    let mut pat_types = vec![0_u8; len_runes];
    let mut pat_len = 0_usize;
    let mut i = 0_usize;

    while i < len_runes {
        let mut r = runes[i];
        let tp;
        match r {
            _ if r == escape_rune => {
                // 转义符自身按普通匹配处理；如果后面还有字符，就消耗后一个字符作为字面量。
                tp = PatMatch;
                if i < len_runes - 1 {
                    i += 1;
                    r = runes[i];
                }
            }
            '_' => {
                // %_ => _%
                // Go 为了后续匹配效率，把 "%_" 重排成 "_%"，但保持语义等价。
                if pat_len > 0 && pat_types[pat_len - 1] == PatAny {
                    tp = PatAny;
                    r = '%';
                    pat_weights[pat_len - 1] = '_';
                    pat_types[pat_len - 1] = PatOne;
                } else {
                    tp = PatOne;
                }
            }
            '%' => {
                // %% => %
                // 连续多个 '%' 合并成一个 PatAny，避免匹配器重复回溯。
                if pat_len > 0 && pat_types[pat_len - 1] == PatAny {
                    i += 1;
                    continue;
                }
                tp = PatAny;
            }
            _ => {
                tp = PatMatch;
            }
        }
        pat_weights[pat_len] = r;
        pat_types[pat_len] = tp;
        pat_len += 1;
        i += 1;
    }

    pat_weights.truncate(pat_len);
    pat_types.truncate(pat_len);
    (pat_weights, pat_types)
}

// CompilePatternInnerBinary handles escapes and wild cards convert pattern characters and
// pattern types in bytes.
// The main algorithm is the same as CompilePatternInner. However, it's not easy to use interface/lambda to hide the different details here.
// Note: if anything changes in this method, please double-check CompilePatternInner
// CompilePatternInnerBinary 按字节处理 LIKE 模式，适用于 binary/ascii 字符串。
// 它刻意复制 CompilePatternInner 的控制流，因为 Go 原实现也避免用抽象隐藏字节与 rune 的差异。
pub fn CompilePatternInnerBinary(pattern: &str, escape: u8) -> (Vec<u8>, Vec<u8>) {
    let bytes = pattern.as_bytes();
    let len_bytes = bytes.len();
    let mut pat_weights = vec![0_u8; len_bytes];
    let mut pat_types = vec![0_u8; len_bytes];
    let mut pat_len = 0_usize;
    let mut i = 0_usize;

    while i < len_bytes {
        let mut b = bytes[i];
        let tp;
        match b {
            _ if b == escape => {
                // 字节版转义符逻辑：转义符后有字节时取后一字节作为普通匹配目标。
                tp = PatMatch;
                if i < len_bytes - 1 {
                    i += 1;
                    b = bytes[i];
                }
            }
            b'_' => {
                // %_ => _%
                if pat_len > 0 && pat_types[pat_len - 1] == PatAny {
                    tp = PatAny;
                    b = b'%';
                    pat_weights[pat_len - 1] = b'_';
                    pat_types[pat_len - 1] = PatOne;
                } else {
                    tp = PatOne;
                }
            }
            b'%' => {
                // %% => %
                if pat_len > 0 && pat_types[pat_len - 1] == PatAny {
                    i += 1;
                    continue;
                }
                tp = PatAny;
            }
            _ => {
                tp = PatMatch;
            }
        }
        pat_weights[pat_len] = b;
        pat_types[pat_len] = tp;
        pat_len += 1;
        i += 1;
    }

    pat_weights.truncate(pat_len);
    pat_types.truncate(pat_len);
    (pat_weights, pat_types)
}

// matchRune 保留 Go 的大小写敏感匹配行为。
// 下面原 Go 文件保留了一段未来可恢复大小写不敏感匹配的注释，这里不展开实现。
fn matchRune(a: char, b: char) -> bool {
    a == b
    // We may reuse below code block when like function go back to case insensitive.
    /*
        if a == b {
            return true
        }
        if a >= 'a' && a <= 'z' && a-caseDiff == b {
            return true
        }
        return a >= 'A' && a <= 'Z' && a+caseDiff == b
    */
}

// CompileLike2Regexp convert a like `lhs` to a regular expression
// CompileLike2Regexp 把 SQL LIKE 模式转换成正则表达式文本。
// Go 依赖 regexp.QuoteMeta；用局部 helper 表达同样的“普通字符转义”意图。
pub fn CompileLike2Regexp(str_: &str) -> String {
    let (pat_chars, pat_types) = CompilePattern(str_, b'\\');
    let mut result = String::with_capacity(pat_chars.len() * 2 + 2);
    result.push('^');
    for i in 0..pat_chars.len() {
        match pat_types[i] {
            PatMatch => result.push_str(&regexp_quote_meta(&pat_chars[i].to_string())),
            PatOne => result.push('.'),
            PatAny => result.push_str(".*"),
            _ => {}
        }
    }
    result.push('$');
    result
}

// regexp_quote_meta 是 regexp.QuoteMeta 的局部替代。
// 它只为本文件避免凭空声明 regexp crate；真实迁移时应统一接入正则库。
fn regexp_quote_meta(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// DoMatchBinary is an adapter for `DoMatchInner`, `str` is binary strings or ascii string.
// DoMatchBinary 是字节匹配适配器；matcher 闭包按索引读取 str 字节与 patChars。
pub fn DoMatchBinary(str_: &str, pat_chars: &[u8], pat_types: &[u8]) -> bool {
    let bytes = str_.as_bytes();
    let len_bytes = bytes.len();
    let len_pat_weights = pat_chars.len();
    doMatchInner(len_pat_weights, len_bytes, pat_types, |a, b| {
        bytes[a] == pat_chars[b]
    })
}

// DoMatch is an adapter for `DoMatchCustomized`, `str` can be any unicode string.
// DoMatch 使用默认 rune/char 相等匹配器，保持 Go 适配器层级。
pub fn DoMatch(str_: &str, pat_chars: &[char], pat_types: &[u8]) -> bool {
    DoMatchCustomized(str_, pat_chars, pat_types, matchRune)
}

// DoMatchCustomized is an adapter for `DoMatchInner`, `str` can be any unicode string.
// DoMatchCustomized 先把字符串整体拆成 rune/char，再把索引匹配委托给 doMatchInner。
pub fn DoMatchCustomized<F>(str_: &str, pat_weights: &[char], pat_types: &[u8], matcher: F) -> bool
where
    F: Fn(char, char) -> bool,
{
    // TODO(bb7133): it is possible to get the rune one by one to avoid the cost of get them as a whole.
    // 这里保留 Go 的整体 []rune 转换成本；后续真实 Rust 版本可考虑迭代器优化。
    let runes: Vec<char> = str_.chars().collect();
    let len_runes = runes.len();
    let len_pat_weights = pat_weights.len();
    doMatchInner(len_pat_weights, len_runes, pat_types, |a, b| {
        matcher(runes[a], pat_weights[b])
    })
}

// doMatchInner matches the string with patChars and patTypes.
// The algorithm has linear time complexity.
// https://research.swtch.com/glob
// doMatchInner 是 LIKE 匹配核心，使用 Russ Cox glob 算法记录最近一次 '%' 的回退点。
// matcher 把“当前位置是否相等”的细节交给字节版或 rune 版调用者。
fn doMatchInner<F>(len_pat_weights: usize, len_chars: usize, pat_types: &[u8], matcher: F) -> bool
where
    F: Fn(usize, usize) -> bool,
{
    let mut c_idx = 0_usize;
    let mut p_idx = 0_usize;
    let mut next_c_idx = 0_usize;
    let mut next_p_idx = 0_usize;

    while p_idx < len_pat_weights || c_idx < len_chars {
        if p_idx < len_pat_weights {
            match pat_types[p_idx] {
                PatMatch => {
                    if c_idx < len_chars && matcher(c_idx, p_idx) {
                        p_idx += 1;
                        c_idx += 1;
                        continue;
                    }
                }
                PatOne => {
                    if c_idx < len_chars {
                        p_idx += 1;
                        c_idx += 1;
                        continue;
                    }
                }
                PatAny => {
                    // Try to match at sIdx.
                    // If that doesn't work out,
                    // restart at sIdx+1 next.
                    // 记录 '%' 的模式位置与下一个候选字符位置；失败后从这里继续尝试。
                    next_p_idx = p_idx;
                    next_c_idx = c_idx + 1;
                    p_idx += 1;
                    continue;
                }
                _ => {}
            }
        }

        // Mismatch. Maybe restart.
        // 不匹配时，如果之前遇到过 '%'，就回退到该 '%' 并让它多吞一个字符。
        if 0 < next_c_idx && next_c_idx <= len_chars {
            p_idx = next_p_idx;
            c_idx = next_c_idx;
            continue;
        }
        return false;
    }
    // Matched all of pattern to all of name. Success.
    true
}

// IsExactMatch return true if no wildcard character
// IsExactMatch 检查编译后的模式是否全是 PatMatch，即没有 '_' 或 '%' 通配符。
pub fn IsExactMatch(pat_types: &[u8]) -> bool {
    for &pt in pat_types {
        if pt != PatMatch {
            return false;
        }
    }
    true
}

// Copy deep copies a string.
// Copy 对应 Go 的 string(hack.Slice(src))，目的在于强制产生不共享底层内存的新字符串。
pub fn Copy(src: &str) -> String {
    src.to_owned()
}

// StringerFunc defines string func implement fmt.Stringer.
// StringerFunc 是 Go 函数类型实现 fmt.Stringer 的 Rust 表达。
pub struct StringerFunc<F>
where
    F: Fn() -> String,
{
    func: F,
}

impl<F> StringerFunc<F>
where
    F: Fn() -> String,
{
    pub fn new(func: F) -> Self {
        Self { func }
    }

    // String implements fmt.Stringer
    // String 调用底层闭包，保持 Go 方法 `func (l StringerFunc) String() string` 的语义。
    pub fn String(&self) -> String {
        (self.func)()
    }
}

// GoStringer 是 fmt.Stringer 的局部占位 trait。
// 真实迁移时应由统一的跨包接口设计替换；这里仅服务于 MemoizeStr 和 StringerStr 的形状。
pub trait GoStringer {
    fn String(&self) -> String;
}

impl<F> GoStringer for StringerFunc<F>
where
    F: Fn() -> String,
{
    fn String(&self) -> String {
        self.String()
    }
}

// MemoizeStr returns memoized version of stringFunc. When the result of l is not
// "", it will be cached and returned directly next time.
// MemoizeStr is not concurrency safe.
// MemoizedStringer 保存 MemoizeStr 闭包和缓存结果。
// Go 版本闭包捕获 result 字符串且非并发安全；用 RefCell 表达同样的内部可变性与非 Sync 语义。
pub struct MemoizedStringer<F>
where
    F: Fn() -> String,
{
    result: RefCell<String>,
    func: F,
}

impl<F> GoStringer for MemoizedStringer<F>
where
    F: Fn() -> String,
{
    fn String(&self) -> String {
        let mut result = self.result.borrow_mut();
        if !result.is_empty() {
            return result.clone();
        }
        // Go 只缓存非空返回值；如果 func 返回空串，下次仍会重新调用。
        let next = (self.func)();
        *result = next.clone();
        next
    }
}

// MemoizeStr 返回一个实现 GoStringer 的缓存包装器。
// Not concurrency safe.
pub fn MemoizeStr<F>(l: F) -> MemoizedStringer<F>
where
    F: Fn() -> String,
{
    MemoizedStringer {
        result: RefCell::new(String::new()),
        func: l,
    }
}

// StringerStr defines a alias to normal string.
// implement fmt.Stringer
// StringerStr 是普通 string 实现 fmt.Stringer 的新类型。
pub struct StringerStr(pub String);

impl StringerStr {
    // String implements fmt.Stringer
    // String 返回底层字符串副本，对应 Go 的 `return string(i)`。
    pub fn String(&self) -> String {
        self.0.clone()
    }
}

impl GoStringer for StringerStr {
    fn String(&self) -> String {
        self.String()
    }
}

// mysql.SQLMode 的局部占位类型。
// Go 的 mysql.ModeANSIQuotes 常量来自 pkg/parser/mysql；这里保留已验证的位值以表达 Escape 的条件分支。
pub type SQLMode = i64;
pub const ModeANSIQuotes: SQLMode = 0x00000004;

// Escape the identifier for pretty-printing.
// For instance, the identifier
/*
    "foo `bar`" will become "`foo ``bar```".
*/
// The sqlMode controls whether to escape with backquotes (`) or double quotes
// (`"`) depending on whether mysql.ModeANSIQuotes is enabled.
// Escape 根据 SQLMode 选择反引号或双引号，并把标识符内部同类引号翻倍。
pub fn Escape(str_: &str, sql_mode: SQLMode) -> String {
    let quote = if sql_mode & ModeANSIQuotes != 0 {
        "\""
    } else {
        "`"
    };
    format!(
        "{quote}{}{quote}",
        str_.replace(quote, &(quote.to_owned() + quote))
    )
}

// BuildStringFromLabels construct config labels into string by following format:
// "keyA=valueA,keyB=valueB"
// BuildStringFromLabels 把标签 map 按 key 排序后格式化，确保输出稳定。
pub fn BuildStringFromLabels(labels: &HashMap<String, String>) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let mut s: Vec<&String> = labels.keys().collect();
    s.sort();
    let mut r = String::new();
    // visit labels by sorted key in order to make sure that result should be consistency
    // 保持 Go 的“先写 key=value, 最后裁掉尾逗号”流程，而不是直接 join，方便对照原实现。
    for key in s {
        let _ = write!(&mut r, "{}={},", key, labels[key]);
    }
    r.pop();
    r
}

// GetTailSpaceCount returns the number of tailed spaces.
// GetTailSpaceCount 只统计尾部 ASCII 空格字节，和 Go 中 str[length-1] == ' ' 一致。
pub fn GetTailSpaceCount(str_: &str) -> i64 {
    let bytes = str_.as_bytes();
    let mut length = bytes.len();
    while length > 0 && bytes[length - 1] == b' ' {
        length -= 1;
    }
    (bytes.len() - length) as i64
}

// Utf8Len calculates how many bytes the utf8 character takes.
// This b parameter should be the first byte of utf8 character
// Utf8Len 根据 UTF-8 首字节的前导 1 个数估算字符字节长度。
pub fn Utf8Len(b: u8) -> usize {
    let mut flag = 128_u8;
    if (flag & b) == 0 {
        return 1;
    }

    let mut length = 0_usize;
    while (flag & b) != 0 {
        length += 1;
        flag >>= 1;
    }
    length
}

// TrimUtf8String needs the string input should always be valid which means
// that it should always return true in utf8.ValidString(str)
// TrimUtf8String 从字符串头部按 UTF-8 字符数量裁剪，并返回累计裁掉的字节数。
// Go 通过 *string 原地更新；用 &mut String 表达相同的可变引用语义。
pub fn TrimUtf8String(str_: &mut String, mut trimmed_num: i64) -> i64 {
    let mut total_len_trimmed = 0_i64;
    while trimmed_num > 0 {
        // 原 Go 代码要求输入一定是合法 UTF-8；这里假设 length 落在 Rust String 的字符边界上。
        let length = Utf8Len(str_.as_bytes()[0]);
        str_.drain(..length);
        total_len_trimmed += length as i64;
        trimmed_num -= 1;
    }
    total_len_trimmed
}

// ConvertPosInUtf8 converts a binary index to the position which shows the occurrence location in the utf8 string
// Take "你好" as example:
//  binary index for "好" is 3, ConvertPosInUtf8("你好", 3) should return 2
// ConvertPosInUtf8 把字节下标转换为 UTF-8 字符位置，返回值从 1 开始计数。
pub fn ConvertPosInUtf8(str_: &str, pos: i64) -> i64 {
    let pos = pos as usize;
    let prefix = &str_.as_bytes()[..pos];
    // Go allows pos to split a UTF-8 sequence. RuneCountInString counts the
    // incomplete suffix as one RuneError, which from_utf8_lossy preserves as
    // one replacement character instead of counting every byte separately.
    let pre_str_num = String::from_utf8_lossy(prefix).chars().count();
    pre_str_num as i64 + 1
}

// toLowerIfAlphaASCII 通过设置 0x20 位把大写 ASCII 转为小写；调用方负责先判断是字母。
fn toLowerIfAlphaASCII(c: u8) -> u8 {
    c | 0x20
}

// toUpperIfAlphaASCII 通过翻转 0x20 位把小写 ASCII 转为大写；调用方负责先判断是字母。
fn toUpperIfAlphaASCII(c: u8) -> u8 {
    c ^ 0x20
}

// IsUpperASCII judges if this is capital alphabet
// IsUpperASCII 判断字节是否为 ASCII 大写字母。
pub fn IsUpperASCII(c: u8) -> bool {
    if c >= b'A' && c <= b'Z' {
        return true;
    }
    false
}

// IsLowerASCII judges if this is lower alphabet
// IsLowerASCII 判断字节是否为 ASCII 小写字母。
pub fn IsLowerASCII(c: u8) -> bool {
    if c >= b'a' && c <= b'z' {
        return true;
    }
    false
}

// LowerOneString lowers the ascii characters in a string
// LowerOneString 原地降低 ASCII 大写字母；非 ASCII 字节保持原样。
pub fn LowerOneString(str_: &mut [u8]) {
    let str_len = str_.len();
    for i in 0..str_len {
        if IsUpperASCII(str_[i]) {
            str_[i] = toLowerIfAlphaASCII(str_[i]);
        }
    }
}

// IsNumericASCII judges if a byte is numeric
// IsNumericASCII 判断字节是否为 ASCII 数字。
pub fn IsNumericASCII(c: u8) -> bool {
    c >= b'0' && c <= b'9'
}

// LowerOneStringExcludeEscapeChar lowers strings and exclude an escape char
// When escape_char is a capital char, we shouldn't lower the escape char.
// For example, 'aaaa' ilike 'AAAA' escape 'A', we should convert 'AAAA' to 'AaAa'.
// If we do not exclude the escape char, 'AAAA' will be lowered to 'aaaa', and we
// can not get the correct result.
// When escape_char is a lower char, we need to convert it to the capital char
// Because: when lowering "ABC" with escape 'a', after lower, "ABC" -> "abc",
// then 'a' will be an escape char and it is not expected.
// Morever, when escape char is uppered we need to tell it to the caller.
// LowerOneStringExcludeEscapeChar 在 ILIKE 场景中原地降低 ASCII 字母，同时保护 escapeChar。
// 返回值是实际使用的 escape 字节；当传入小写 escape 时，Go 会返回其大写形式通知调用方。
pub fn LowerOneStringExcludeEscapeChar(str_: &mut [u8], escape_char: u8) -> u8 {
    let mut actual_escape_char = escape_char;
    if IsLowerASCII(escape_char) {
        actual_escape_char = toUpperIfAlphaASCII(escape_char);
    }
    let mut escaped = false;
    let str_len = str_.len();
    let mut i = 0_usize;

    while i < str_len {
        if IsUpperASCII(str_[i]) {
            // Do not lower the escape char, however when a char is equal to
            // an escape char and it's after an escape char, we still lower it
            // For example: "AA" (escape 'A'), -> "Aa"
            // 这个分支保持第一个 escapeChar 的大小写，用 escaped 标记说明下一个字符已被转义。
            if !(str_[i] != escape_char || escaped) {
                escaped = true;
                i += 1;
                continue;
            }
            str_[i] = toLowerIfAlphaASCII(str_[i]);
        } else {
            if str_[i] == escape_char && !escaped {
                escaped = true;

                // It should be `str[i] = toUpperIfAlphaASCII(str[i])`,
                // but 'actual_escape_char' is always equal to 'toUpperIfAlphaASCII(str[i])'
                // 小写 escapeChar 会在字符串里被改为大写，避免 lower 后误把普通字符当 escape。
                str_[i] = actual_escape_char;
                i += 1;
                continue;
            }
            // Go 这里额外跳过 Utf8Len(str[i])-1，再由 for 循环自增；Rust while 直接跳过完整 UTF-8 字符。
            let step = Utf8Len(str_[i]);
            escaped = false;
            i += step;
            continue;
        }
        escaped = false;
        i += 1;
    }

    actual_escape_char
}

// EscapeGlobQuestionMark escapes '?' for a glob path pattern.
// EscapeGlobQuestionMark 给 glob 模式中的 '?' 加反斜杠，避免它被当作单字符通配符。
pub fn EscapeGlobQuestionMark(s: &str) -> String {
    let mut buf = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '?' {
            buf.push('\\');
        }
        buf.push(c);
    }
    buf
}
