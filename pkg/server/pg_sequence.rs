// Copyright 2026 AsterSQL.
//! Narrow sequence statements; execution stays in ConcreteSession and Domain.
use crate::pg_catalog_query::{ParseResult, Token};
use astersql_session::runtime::PgSequenceCommand as Command;

fn word(t: Option<&Token>, name: &str) -> bool {
    matches!(t, Some(Token::Word(w)) if w == name)
}
fn ident(t: Option<&Token>) -> Option<String> {
    match t {
        Some(Token::Word(w) | Token::Quoted(w)) => Some(w.clone()),
        _ => None,
    }
}
fn relation(tokens: &[Token], i: &mut usize) -> ParseResult<(Option<String>, String)> {
    let first = ident(tokens.get(*i)).ok_or(("42601", "expected sequence name".into()))?;
    *i += 1;
    if tokens.get(*i) == Some(&Token::Symbol('.')) {
        *i += 1;
        let name = ident(tokens.get(*i)).ok_or(("42601", "expected sequence name".into()))?;
        *i += 1;
        Ok((Some(first), name))
    } else {
        Ok((None, first))
    }
}
pub(crate) fn parse(tokens: &[Token]) -> ParseResult<Option<(Command, Option<String>)>> {
    if (word(tokens.first(), "create") || word(tokens.first(), "drop"))
        && word(tokens.get(1), "sequence")
    {
        let create = word(tokens.first(), "create");
        let mut i = 2;
        let (schema, name) = relation(tokens, &mut i)?;
        if !create {
            if i != tokens.len() {
                return Err(("0A000", "unsupported DROP SEQUENCE form".into()));
            }
            return Ok(Some((Command::Drop { schema, name }, None)));
        }
        if word(tokens.get(i), "as") {
            if !word(tokens.get(i + 1), "bigint") {
                return Err(("0A000", "sequence declaration requires bigint".into()));
            }
            i += 2;
        }
        let mut options = Vec::new();
        while i < tokens.len() {
            let key = ident(tokens.get(i)).ok_or(("42601", "invalid sequence option".into()))?;
            i += 1;
            match key.as_str() {
                "start" | "increment" | "minvalue" | "maxvalue" | "cache" => {
                    if (key == "start" && word(tokens.get(i), "with"))
                        || (key == "increment" && word(tokens.get(i), "by"))
                    {
                        i += 1;
                    }
                    let negative = tokens.get(i) == Some(&Token::Symbol('-'));
                    if negative {
                        i += 1;
                    }
                    let Some(Token::Number(n)) = tokens.get(i) else {
                        return Err(("42601", "sequence option requires an integer".into()));
                    };
                    let value = format!("{}{n}", if negative { "-" } else { "" });
                    let number = value
                        .parse::<i64>()
                        .map_err(|_| ("22003", "sequence option is outside bigint".into()))?;
                    if key == "cache" && number != 1 {
                        return Err(("0A000", "sequence execution requires CACHE 1".into()));
                    }
                    options.push(format!("{key} {value}"));
                    i += 1;
                }
                "no" if word(tokens.get(i), "cycle") => {
                    options.push("NO CYCLE".into());
                    i += 1;
                }
                _ => return Err(("0A000", "unsupported sequence option".into())),
            }
        }
        // Native defaults cache multiple values; this bounded contract is CACHE 1.
        if !options.iter().any(|o| o.starts_with("cache ")) {
            options.push("CACHE 1".into());
        }
        return Ok(Some((
            Command::Create {
                schema,
                name,
                options: options.join(" "),
            },
            None,
        )));
    }
    if (word(tokens.first(), "grant") || word(tokens.first(), "revoke"))
        && tokens
            .windows(2)
            .any(|t| word(t.first(), "on") && word(t.get(1), "sequence"))
    {
        let grant = word(tokens.first(), "grant");
        let mut i = 1;
        let mut privileges = Vec::new();
        loop {
            let privilege =
                ident(tokens.get(i)).ok_or(("42601", "expected sequence privilege".into()))?;
            if !matches!(privilege.as_str(), "usage" | "select" | "update") {
                return Err(("0A000", "unsupported sequence privilege".into()));
            }
            privileges.push(privilege);
            i += 1;
            if tokens.get(i) != Some(&Token::Symbol(',')) {
                break;
            }
            i += 1;
        }
        if !word(tokens.get(i), "on") || !word(tokens.get(i + 1), "sequence") {
            return Err(("42601", "expected ON SEQUENCE".into()));
        }
        i += 2;
        let (schema, name) = relation(tokens, &mut i)?;
        if !word(tokens.get(i), if grant { "to" } else { "from" }) {
            return Err(("42601", "expected sequence grantee".into()));
        }
        i += 1;
        let mut grantee =
            ident(tokens.get(i)).ok_or(("42601", "expected sequence grantee".into()))?;
        if word(tokens.get(i), "public") {
            grantee = "PUBLIC".into();
        }
        if i + 1 != tokens.len() {
            return Err(("0A000", "unsupported sequence grant form".into()));
        }
        return Ok(Some((
            Command::Grant {
                schema,
                name,
                privileges,
                grantee,
                grant,
            },
            None,
        )));
    }
    if !word(tokens.first(), "select") {
        return Ok(None);
    }
    let mut i = 1;
    if word(tokens.get(i), "pg_catalog") && tokens.get(i + 1) == Some(&Token::Symbol('.')) {
        i += 2;
    }
    let Some(function) = ident(tokens.get(i)) else {
        return Ok(None);
    };
    if !matches!(function.as_str(), "nextval" | "currval" | "setval") {
        return Ok(None);
    }
    i += 1;
    if tokens.get(i) != Some(&Token::Symbol('(')) {
        // A column named nextval/currval belongs to the ordinary SELECT path.
        return Ok(None);
    }
    i += 1;
    let target = match tokens.get(i) {
        Some(Token::String(name)) => Some(name.clone()),
        t if word(t, "null") => None,
        _ => return Err(("42883", "sequence function requires regclass".into())),
    };
    i += 1;
    if tokens.get(i) == Some(&Token::Cast) {
        if !word(tokens.get(i + 1), "regclass") {
            return Err(("42883", "sequence function requires regclass".into()));
        }
        i += 2;
    }
    let mut set_value = None;
    let mut is_called = Some(true);
    if function == "setval" {
        if tokens.get(i) != Some(&Token::Symbol(',')) {
            return Err((
                "42883",
                "setval requires regclass, bigint and optional boolean".into(),
            ));
        }
        i += 1;
        let negative = tokens.get(i) == Some(&Token::Symbol('-'));
        if negative {
            i += 1;
        }
        set_value = match tokens.get(i) {
            Some(Token::Number(number)) if !number.contains('.') => Some(
                format!("{}{number}", if negative { "-" } else { "" })
                    .parse::<i64>()
                    .map_err(|_| ("22003", "setval value is outside bigint".into()))?,
            ),
            t if !negative && word(t, "null") => None,
            _ => return Err(("42883", "setval value requires bigint".into())),
        };
        i += 1;
        if tokens.get(i) == Some(&Token::Cast) && word(tokens.get(i + 1), "bigint") {
            i += 2;
        }
        if tokens.get(i) == Some(&Token::Symbol(',')) {
            i += 1;
            is_called = match tokens.get(i) {
                t if word(t, "true") => Some(true),
                t if word(t, "false") => Some(false),
                t if word(t, "null") => None,
                _ => return Err(("42883", "setval is_called requires boolean".into())),
            };
            i += 1;
            if tokens.get(i) == Some(&Token::Cast) && word(tokens.get(i + 1), "boolean") {
                i += 2;
            }
        }
    }
    if tokens.get(i) != Some(&Token::Symbol(')')) {
        return Err((
            "42883",
            "sequence function requires one regclass argument".into(),
        ));
    }
    i += 1;
    let alias = if word(tokens.get(i), "as") {
        i += 1;
        let alias =
            ident(tokens.get(i)).ok_or(("42601", "expected sequence result alias".into()))?;
        i += 1;
        alias
    } else {
        function.clone()
    };
    if i != tokens.len() {
        return Err(("0A000", "unsupported sequence projection".into()));
    }
    let (schema, name) = if let Some(target) = target {
        let parts = crate::pg_oid::name_parts(&target)
            .map_err(|_| ("22P02", "invalid regclass name".into()))?;
        match parts.as_slice() {
            [name] => (None, Some(name.clone())),
            [schema, name] => (Some(schema.clone()), Some(name.clone())),
            _ => return Err(("22P02", "invalid regclass name".into())),
        }
    } else {
        (None, None)
    };
    Ok(Some((
        if function == "setval" {
            Command::Set {
                schema,
                name,
                value: set_value,
                is_called,
            }
        } else {
            Command::Value {
                schema,
                name,
                next: function == "nextval",
            }
        },
        Some(alias),
    )))
}
