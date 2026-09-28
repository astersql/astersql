// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Copyright 2013 The Go-MySQL-Driver Authors. All rights reserved.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this file,
// You can obtain one at http://mozilla.org/MPL/2.0/.

// The MIT License (MIT)
//
// Copyright (c) 2014 wandoulabs
// Copyright (c) 2014 siddontang
//
// Permission is hereby granted, free of charge, to any person obtaining a copy of
// this software and associated documentation files (the "Software"), to deal in
// the Software without restriction, including without limitation the rights to
// use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software is furnished to do so,
// subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

// MySQL 协议与 HTTP 辅助工具。
//
// 提供长度编码整数/字节解析、NUL 终止串、客户端字符集输入解码，
// 以及 CORS 包装与包内测试用默认配置构造。

use crate::config::Config;
use encoding_rs::{Encoding, WINDOWS_1252};
use http::{HeaderValue, Request, Response, header};
use std::io::{self, ErrorKind};

/// Parses a NUL-terminated byte string. A missing terminator maps to Go's nil
/// first slice and leaves the full input unconsumed.
/// 解析以 `\0` 结尾的字节串；无终结符时首返回值对应 Go 的 nil，输入整体未消费。
pub fn ParseNullTermString(input: &[u8]) -> (Option<&[u8]>, &[u8]) {
    match input.iter().position(|byte| *byte == 0) {
        Some(offset) => (Some(&input[..offset]), &input[offset + 1..]),
        None => (None, input),
    }
}

/// Parses a MySQL length-encoded integer and preserves Go's four return values.
/// 解析 MySQL 长度编码整数，保留 Go 的四返回值：(值, 是否 NULL, 消费字节数, 错误)。
///
/// 首字节语义：`0xfb`=NULL，`0xfc`/`0xfd`/`0xfe` 分别后跟 2/3/8 字节小端整数，其余为单字节值。
pub fn ParseLengthEncodedInt(input: &[u8]) -> (u64, bool, usize, Option<io::Error>) {
    let Some(first) = input.first().copied() else {
        return (0, false, 0, Some(ErrorKind::UnexpectedEof.into()));
    };
    match first {
        0xfb => (0, true, 1, None),
        0xfc => parse_fixed_integer(input, 2, 3),
        0xfd => parse_fixed_integer(input, 3, 4),
        0xfe => parse_fixed_integer(input, 8, 9),
        value => (value as u64, false, 1, None),
    }
}

/// 按固定宽度读取小端整数；载荷不足时 consumed=0 并返回 UnexpectedEof。
fn parse_fixed_integer(
    input: &[u8],
    width: usize,
    consumed: usize,
) -> (u64, bool, usize, Option<io::Error>) {
    if input.len() < width + 1 {
        return (0, false, 0, Some(ErrorKind::UnexpectedEof.into()));
    }
    // 从首字节之后按小端拼装 width 字节。
    let value = input[1..=width]
        .iter()
        .enumerate()
        .fold(0_u64, |value, (shift, byte)| {
            value | ((*byte as u64) << (shift * 8))
        });
    (value, false, consumed, None)
}

/// Parses length-encoded bytes, including the declared end offset on a short
/// payload as returned by Go.
/// 解析长度编码字节串；载荷不足时仍返回声明的 end 偏移（对齐 Go 行为）。
pub fn ParseLengthEncodedBytes(input: &[u8]) -> (Option<&[u8]>, bool, usize, Option<io::Error>) {
    let (length, is_null, header_size, error) = ParseLengthEncodedInt(input);
    if error.is_some() {
        return (None, is_null, header_size, error);
    }
    if length == 0 {
        return (None, is_null, header_size, None);
    }
    let end = header_size.saturating_add(length as usize);
    if input.len() < end {
        return (
            None,
            false,
            end,
            Some(io::Error::from(ErrorKind::UnexpectedEof)),
        );
    }
    (Some(&input[header_size..end]), false, end, None)
}

/// Returns the encoded size of a MySQL length-encoded integer.
/// 返回将 `value` 编码为长度编码整数所需字节数（1/3/4/9）。
pub fn LengthEncodedIntSize(value: u64) -> usize {
    match value {
        0..=250 => 1,
        251..=0xffff => 3,
        0x1_0000..=0xff_ffff => 4,
        _ => 9,
    }
}

/// Decodes client input using the configured MySQL character set.
/// 按配置的 MySQL 字符集解码客户端输入；utf8/utf8mb4/binary 等直通原字节。
pub struct InputDecoder {
    /// `None` 表示无需转码；否则为 encoding_rs 标签对应编码。
    encoding: Option<&'static Encoding>,
}

/// 按字符集名构造解码器；`latin1` 映射为 Windows-1252（与 MySQL 常见实践一致）。
pub fn NewInputDecoder(charset: &str) -> InputDecoder {
    let encoding = match charset {
        "" | "ascii" | "binary" | "utf8" | "utf8mb4" => None,
        "latin1" => Some(WINDOWS_1252),
        "gbk" | "gb18030" => Encoding::for_label(charset.as_bytes()),
        _ => None,
    };
    InputDecoder { encoding }
}

impl InputDecoder {
    /// Returns the original bytes when decoding reports malformed input, as Go
    /// does when `Transform` fails.
    /// 解码失败（含非法序列）时退回原始字节，对齐 Go `Transform` 失败语义。
    pub fn DecodeInput(&self, source: &[u8]) -> Vec<u8> {
        let Some(encoding) = self.encoding else {
            return source.to_vec();
        };
        let (decoded, had_errors) = encoding.decode_without_bom_handling(source);
        if had_errors {
            source.to_vec()
        } else {
            decoded.into_owned().into_bytes()
        }
    }
}

/// Minimal HTTP handler abstraction used by the package-local native harness.
/// 包内原生测试用的最小 HTTP Handler 抽象（对齐 Go `http.Handler`）。
pub trait Handler {
    fn ServeHTTP(&self, response: &mut Response<Vec<u8>>, request: Request<Vec<u8>>);
}

impl<F> Handler for F
where
    F: Fn(&mut Response<Vec<u8>>, Request<Vec<u8>>),
{
    fn ServeHTTP(&self, response: &mut Response<Vec<u8>>, request: Request<Vec<u8>>) {
        self(response, request);
    }
}

/// Adds CORS headers before delegating, matching Go's response-writer order.
/// 在委托内层 handler 前写入 CORS 头，顺序对齐 Go ResponseWriter。
pub struct CorsHandler<H> {
    handler: H,
    config: Config,
}

/// 用内层 handler 与配置构造 CORS 包装器。
pub fn NewCorsHandler<H: Handler>(handler: H, config: Config) -> CorsHandler<H> {
    CorsHandler { handler, config }
}

impl<H: Handler> CorsHandler<H> {
    /// 先按 `config.cors` 写 Allow-Origin/Methods，再调用内层 `ServeHTTP`。
    pub fn ServeHTTP(&self, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
        let mut response = Response::new(Vec::new());
        if !self.config.cors.is_empty() {
            if let Ok(origin) = HeaderValue::from_str(&self.config.cors) {
                response
                    .headers_mut()
                    .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                response.headers_mut().insert(
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    HeaderValue::from_static("GET"),
                );
            }
        }
        self.handler.ServeHTTP(&mut response, request);
        response
    }
}

/// Creates the server test configuration used throughout the Go package.
/// 构造包内测试用默认服务器配置（本机回环、关闭 auto_tls、清空 socket）。
pub fn NewTestConfig() -> Config {
    let mut config = Config::default();
    config.host = "127.0.0.1".into();
    config.status.status_host = "127.0.0.1".into();
    config.security.auto_tls = false;
    config.socket.clear();
    config
}
