// Copyright 2026 AsterSQL.

use crate::{PromptGenerator, dml::DmlPromptGenerator};

#[test]
fn dml_response_matches_go_null_and_duplicate_fields() {
    let generator = DmlPromptGenerator;
    for (input, expected) in [
        (r#"{"queries":[null,"x"]}"#, vec!["", "x"]),
        (r#"{"queries":["a","b"],"Queries":[null]}"#, vec!["a"]),
        (r#"{"queries":1,"Queries":["x"]}"#, vec![]),
        (r#"{"queries":[1],"Queries":["x"]}"#, vec![]),
        (r#"{"queries":["a"],"Queries":null}"#, vec![]),
        (r#"{"querieſ":["x"]}"#, vec!["x"]),
        (
            r#"{"queries":["a","b"],"Queries":[null],"QUERIES":[null,null]}"#,
            vec!["a", "b"],
        ),
        (
            r#"{"queries":["a"],"Queries":[],"QUERIES":[null]}"#,
            vec![""],
        ),
        (
            r#"{"queries":["a"],"Queries":null,"QUERIES":[null]}"#,
            vec![""],
        ),
    ] {
        let cases = generator.unmarshal(input);
        assert_eq!(
            cases.iter().map(|c| c.sql.as_str()).collect::<Vec<_>>(),
            expected,
            "{input}"
        );
        assert!(
            cases
                .iter()
                .all(|c| !c.known && !c.pass && c.args.is_none() && c.comment.is_empty())
        );
    }
}
