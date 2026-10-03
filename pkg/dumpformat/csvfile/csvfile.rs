// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

/// Column classification for framing (the later Go move to dumpformat changes no behavior).
#[derive(Clone, Copy, Debug)]
pub enum FieldKind {
    Number,
    String,
    Bytes,
}
#[derive(Clone, Copy, Debug, Default)]
pub enum BinaryFormat {
    #[default]
    UTF8,
    HEX,
    Base64,
}
/// No defaults are applied by Writer; callers provide all framing knobs.
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub fields_terminated_by: Vec<u8>,
    pub fields_enclosed_by: Vec<u8>,
    pub fields_escaped_by: Vec<u8>,
    pub lines_terminated_by: Vec<u8>,
    pub null_value: Vec<u8>,
    pub binary_format: BinaryFormat,
}
