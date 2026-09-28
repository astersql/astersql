// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 部分 JSON 抽取：流式扫描顶层对象成员，按名提取或丢弃值。
//
// 对应 Go `pkg/util/partialjson`。用栈机跟踪 object/array 嵌套，标量借助
// `serde_json` 解析；对外提供 `topLevelJSONTokenIter` 与 `ExtractTopLevelMembers`。

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

/// JSON token：分隔符、字符串、数字原文、布尔或 null。
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    /// 对象/数组的 `{` `}` `[` `]`。
    Delim(char),
    /// JSON 字符串值（已解码）。
    String(String),
    /// 数字的原始字节文本（保留字面量形态）。
    Number(String),
    /// 布尔字面量。
    Bool(bool),
    /// null 字面量。
    Null,
}

/// 内部错误分类：正常 EOF、意外 EOF、语法错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ErrorKind {
    /// 输入耗尽且处于合法结束位置。
    Eof,
    /// 嵌套未闭合时遇到 EOF。
    UnexpectedEof,
    /// 语法或状态机违规。
    Syntax,
}

/// 部分 JSON 解析错误，携带分类与可读消息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialJsonError {
    kind: ErrorKind,
    message: String,
}

impl PartialJsonError {
    /// 构造正常 EOF 错误。
    fn eof() -> Self {
        Self {
            kind: ErrorKind::Eof,
            message: "EOF".to_owned(),
        }
    }

    /// 构造意外 EOF 错误（对象/数组未闭合）。
    fn unexpected_eof() -> Self {
        Self {
            kind: ErrorKind::UnexpectedEof,
            message: "unexpected EOF".to_owned(),
        }
    }

    /// 构造语法错误。
    fn syntax(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Syntax,
            message: message.into(),
        }
    }

    /// 是否为正常 EOF（顶层对象已结束）。
    pub fn is_eof(&self) -> bool {
        self.kind == ErrorKind::Eof
    }
}

impl fmt::Display for PartialJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for PartialJsonError {}

/// 对象/数组状态机：期望 key、冒号、值或逗号/结束。
#[derive(Clone, Copy, Debug)]
enum State {
    /// 对象：首个 key 或立即结束。
    ObjectFirstKeyOrEnd,
    /// 对象：期望下一个 key。
    ObjectKey,
    /// 对象：key 后期望冒号。
    ObjectColon,
    /// 对象：冒号后期望值。
    ObjectValue,
    /// 对象：值后期望逗号或结束。
    ObjectCommaOrEnd,
    /// 数组：首个值或立即结束。
    ArrayFirstValueOrEnd,
    /// 数组：期望下一个值。
    ArrayValue,
    /// 数组：值后期望逗号或结束。
    ArrayCommaOrEnd,
}

/// 嵌套栈帧：期望的闭合分隔符与当前状态。
#[derive(Clone, Copy, Debug)]
struct Frame {
    /// 期望的闭合字节（`}` 或 `]`）。
    close: u8,
    /// 当前容器内状态。
    state: State,
}

/// 基于字节切片的流式 JSON token 解码器。
struct TokenDecoder<'a> {
    /// 原始 JSON 字节。
    content: &'a [u8],
    /// 当前读取位置。
    pos: usize,
    /// 对象/数组嵌套栈。
    frames: Vec<Frame>,
    /// 是否已消费过顶层值（禁止第二个顶层值）。
    root_consumed: bool,
}

impl<'a> TokenDecoder<'a> {
    /// 从字节切片构造解码器。
    fn new(content: &'a [u8]) -> Self {
        Self {
            content,
            pos: 0,
            frames: Vec::new(),
            root_consumed: false,
        }
    }

    /// 在当前字节位置附加语法错误消息。
    fn syntax(&self, message: impl Into<String>) -> PartialJsonError {
        PartialJsonError::syntax(format!("{} at byte {}", message.into(), self.pos))
    }

    /// 跳过 ASCII 空白。
    fn skip_space(&mut self) {
        while self.pos < self.content.len()
            && matches!(self.content[self.pos], b' ' | b'\t' | b'\r' | b'\n')
        {
            self.pos += 1;
        }
    }

    /// 开始解析一个值：推进容器状态，或标记顶层值已消费。
    fn begin_value(&mut self) -> Result<(), PartialJsonError> {
        if let Some(frame) = self.frames.last_mut() {
            // 值出现在 ObjectValue / Array*Value 位置时推进到「逗号或结束」。
            frame.state = match frame.state {
                State::ObjectValue => State::ObjectCommaOrEnd,
                State::ArrayFirstValueOrEnd | State::ArrayValue => State::ArrayCommaOrEnd,
                _ => return Err(self.syntax("expected a JSON name or separator")),
            };
        } else if self.root_consumed {
            return Err(self.syntax("unexpected value after top-level JSON value"));
        } else {
            self.root_consumed = true;
        }
        Ok(())
    }

    /// 用 serde_json 解析下一个标量；对象 key 位置的字符串不调用 begin_value。
    fn parse_scalar(&mut self) -> Result<Token, PartialJsonError> {
        let start = self.pos;
        let mut stream = serde_json::Deserializer::from_slice(&self.content[start..])
            .into_iter::<serde_json::Value>();
        let value = stream
            .next()
            .ok_or_else(PartialJsonError::eof)?
            .map_err(|err| self.syntax(err.to_string()))?;
        let consumed = stream.byte_offset();
        if consumed == 0 {
            return Err(self.syntax("expected value"));
        }
        self.pos += consumed;

        if let serde_json::Value::String(value) = value {
            // 处于对象 key 位置时，字符串是名字而非值。
            if let Some(frame) = self.frames.last_mut() {
                match frame.state {
                    State::ObjectFirstKeyOrEnd | State::ObjectKey => {
                        frame.state = State::ObjectColon;
                        return Ok(Token::String(value));
                    }
                    _ => {}
                }
            }
            self.begin_value()?;
            return Ok(Token::String(value));
        }

        self.begin_value()?;
        match value {
            // Number 保留原文切片，避免精度/格式丢失。
            serde_json::Value::Number(_) => Ok(Token::Number(
                String::from_utf8_lossy(&self.content[start..start + consumed]).into_owned(),
            )),
            serde_json::Value::Bool(value) => Ok(Token::Bool(value)),
            serde_json::Value::Null => Ok(Token::Null),
            _ => Err(self.syntax("internal scalar decoder error")),
        }
    }

    /// 产出下一个 token；冒号/逗号在循环内消费，开闭括号作为 Delim 返回。
    fn next_token(&mut self) -> Result<Token, PartialJsonError> {
        loop {
            self.skip_space();
            if self.pos == self.content.len() {
                return Err(PartialJsonError::eof());
            }

            match self.content[self.pos] {
                b':' => {
                    let Some(frame) = self.frames.last_mut() else {
                        return Err(self.syntax("unexpected ':'"));
                    };
                    if !matches!(frame.state, State::ObjectColon) {
                        return Err(self.syntax("unexpected ':'"));
                    }
                    frame.state = State::ObjectValue;
                    self.pos += 1;
                }
                b',' => {
                    let Some(frame) = self.frames.last_mut() else {
                        return Err(self.syntax("unexpected ','"));
                    };
                    frame.state = match frame.state {
                        State::ObjectCommaOrEnd => State::ObjectKey,
                        State::ArrayCommaOrEnd => State::ArrayValue,
                        _ => return Err(self.syntax("unexpected ','")),
                    };
                    self.pos += 1;
                }
                b'{' | b'[' => {
                    let open = self.content[self.pos];
                    self.begin_value()?;
                    self.pos += 1;
                    // 压入新栈帧，状态为「首元素或立即结束」。
                    self.frames.push(if open == b'{' {
                        Frame {
                            close: b'}',
                            state: State::ObjectFirstKeyOrEnd,
                        }
                    } else {
                        Frame {
                            close: b']',
                            state: State::ArrayFirstValueOrEnd,
                        }
                    });
                    return Ok(Token::Delim(open as char));
                }
                b'}' | b']' => {
                    let close = self.content[self.pos];
                    let Some(frame) = self.frames.last() else {
                        return Err(self.syntax("unexpected closing delimiter"));
                    };
                    if frame.close != close {
                        return Err(self.syntax("mismatched closing delimiter"));
                    }
                    // 仅在「可结束」状态下允许闭合。
                    let can_close = matches!(
                        frame.state,
                        State::ObjectFirstKeyOrEnd
                            | State::ObjectCommaOrEnd
                            | State::ArrayFirstValueOrEnd
                            | State::ArrayCommaOrEnd
                    );
                    if !can_close {
                        return Err(self.syntax("unexpected closing delimiter"));
                    }
                    self.frames.pop();
                    self.pos += 1;
                    return Ok(Token::Delim(close as char));
                }
                _ => return self.parse_scalar(),
            }
        }
    }
}

/// 顶层 JSON 对象的 token 迭代器：交替读字段名与字段值。
pub struct topLevelJSONTokenIter<'a> {
    decoder: TokenDecoder<'a>,
    /// 当前嵌套深度：1=顶层对象内，>1=嵌套容器内。
    level: usize,
}

/// 构造顶层对象 token 迭代器。
pub fn newTopLevelJSONTokenIter(content: &[u8]) -> topLevelJSONTokenIter<'_> {
    topLevelJSONTokenIter {
        decoder: TokenDecoder::new(content),
        level: 0,
    }
}

impl topLevelJSONTokenIter<'_> {
    /// 读取下一个顶层字段名（单个 String token）。
    pub fn readName(&mut self) -> Result<String, PartialJsonError> {
        let tokens = self.next(false)?;
        match tokens.as_slice() {
            [Token::String(name)] => Ok(name.clone()),
            _ => Err(PartialJsonError::syntax(format!(
                "unexpected JSON name, {tokens:?}"
            ))),
        }
    }

    /// 读取或丢弃下一个字段值；`discard=true` 时不收集嵌套 token。
    pub fn readOrDiscardValue(&mut self, discard: bool) -> Result<Vec<Token>, PartialJsonError> {
        self.next(discard)
    }

    /// 推进迭代：首次调用消费顶层 `{`；之后在 level==1 读名/标量值，level>1 扫完整嵌套值。
    pub fn next(&mut self, discard: bool) -> Result<Vec<Token>, PartialJsonError> {
        if self.level == 0 {
            let token = self.decoder.next_token()?;
            if token != Token::Delim('{') {
                return Err(PartialJsonError::syntax(format!(
                    "expected '{{' for topLevelJSONTokenIter, got {token:?}"
                )));
            }
            self.level = 1;
        }

        let mut long_value = Vec::new();
        if self.level == 1 {
            // 顶层对象内：EOF 视为 unexpected；`}` 映射为正常 EOF。
            let token = self.decoder.next_token().map_err(|err| {
                if err.is_eof() {
                    PartialJsonError::unexpected_eof()
                } else {
                    err
                }
            })?;
            match token {
                Token::Delim('}') => {
                    self.level = 0;
                    return Err(PartialJsonError::eof());
                }
                Token::Delim('{') | Token::Delim('[') => {
                    self.level += 1;
                    if !discard {
                        long_value.push(token);
                    }
                }
                token => return Ok(vec![token]),
            }
        }

        // 扫完嵌套容器直到回到顶层对象（level==1）。
        while self.level > 1 {
            let token = self.decoder.next_token().map_err(|err| {
                if err.is_eof() {
                    PartialJsonError::unexpected_eof()
                } else {
                    err
                }
            })?;
            if !discard {
                long_value.push(token.clone());
            }
            match token {
                Token::Delim('{') | Token::Delim('[') => self.level += 1,
                Token::Delim('}') | Token::Delim(']') => self.level -= 1,
                _ => {}
            }
        }
        Ok(long_value)
    }
}

/// 从顶层 JSON 对象中按 `names` 提取成员，未请求的字段值被丢弃。
pub fn ExtractTopLevelMembers(
    content: &[u8],
    names: &[String],
) -> Result<HashMap<String, Vec<Token>>, PartialJsonError> {
    let mut remaining: HashSet<String> = names.iter().cloned().collect();
    let mut result = HashMap::with_capacity(remaining.len());
    let mut iter = newTopLevelJSONTokenIter(content);

    while !remaining.is_empty() {
        let name = iter.readName()?;
        if remaining.remove(&name) {
            result.insert(name, iter.readOrDiscardValue(false)?);
        } else {
            // 非目标字段：丢弃其值以继续扫描。
            iter.readOrDiscardValue(true)?;
        }
    }
    Ok(result)
}
