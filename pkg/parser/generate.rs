// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Parser generation compatibility entry.
//
// The explicit `astersql-parsergen generate` command reads the maintained
// `.astergram` grammars and writes the Rust token and table modules below
// `generated/`. `parser_impl` includes those checked-in sources directly, so a
// normal parser build does not run the generator.

/// Checked-in Rust parser table outputs produced by `astersql-parsergen generate`.
pub const GENERATED_RUST_TABLES: &[&str] = &[
    "generated/main_tables.rs",
    "generated/hint_tables.rs",
    "generated/lexer_tokens.rs",
];
