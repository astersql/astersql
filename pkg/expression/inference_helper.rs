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

use crate::{Error, ast, errors};

/// Constant metadata preserves raw options, rather than canonicalizing JSON.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbedTextInfo {
    pub ModelNameWithProvider: String,
    pub OptsInJSON: String,
}
impl EmbedTextInfo {
    pub fn Equal(left: Option<&Self>, right: Option<&Self>) -> bool {
        left == right
    }
}
pub fn IsEmbedTextFuncCall(expr: &ast::ExprNode) -> bool {
    matches!(&expr.Kind, ast::ExprKind::Function { FnName, .. } if FnName.L == "embed_text")
}
pub fn ContainsEmbedTextFunc(expr: Option<&ast::ExprNode>) -> bool {
    struct Finder(bool);
    impl ast::ExprNodeVisitor for Finder {
        fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
            if IsEmbedTextFuncCall(input) {
                self.0 = true;
                return (input.clone(), true);
            }
            (input.clone(), false)
        }
        fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
            (input.clone(), true)
        }
    }
    let mut finder = Finder(false);
    if let Some(expr) = expr {
        expr.Accept(&mut finder);
    }
    finder.0
}
pub fn ExtractEmbedTextInfo(expr: &ast::ExprNode) -> Result<EmbedTextInfo, Error> {
    let ast::ExprKind::Function { FnName, Args, .. } = &expr.Kind else {
        return Err(errors::New(
            "only generated columns using EMBED_TEXT() are allowed",
        ));
    };
    if FnName.L != "embed_text" {
        return Err(errors::New(
            "only generated columns using EMBED_TEXT() are allowed",
        ));
    }
    if !(2..=3).contains(&Args.len()) {
        return Err(errors::New("invalid EMBED_TEXT() usage"));
    }
    let constant = |arg: &ast::ExprNode, message: &str| -> Result<String, Error> {
        match &arg.Kind {
            ast::ExprKind::Value(value) => match &value.Datum {
                ast::ValueDatum::String(value) => Ok(value.clone()),
                _ => Err(errors::New(message)),
            },
            _ => Err(errors::New(message)),
        }
    };
    let model = constant(
        &Args[0],
        "EMBED_TEXT() only accepts model name using string constant",
    )?;
    let options = if Args.len() == 3 {
        constant(
            &Args[2],
            "EMBED_TEXT() only accepts JSON options using string constant",
        )?
    } else {
        String::new()
    };
    if !options.is_empty()
        && !serde_json::from_str::<serde_json::Value>(&options).is_ok_and(|value| value.is_object())
    {
        return Err(errors::New("EMBED_TEXT expects options in JSON format"));
    }
    Ok(EmbedTextInfo {
        ModelNameWithProvider: model,
        OptsInJSON: options,
    })
}
