// Copyright 2026 AsterSQL.

use super::CreateAnalyzeSession;

#[test]
fn go_merge_43_embed_text_sql_enforces_starter_mode() {
    let (_, session) = CreateAnalyzeSession().expect("SQL session");
    let error = session
        .execute("SELECT EMBED_TEXT('openai/model', 'hello')")
        .err()
        .expect("EMBED_TEXT is starter-only");
    assert!(
        error.to_string().contains("starter deployment mode"),
        "{error}"
    );
    let error = session
        .execute("SELECT CONCAT('vector=', EMBED_TEXT('openai/model', 'hello'))")
        .err()
        .expect("nested EMBED_TEXT must be evaluated");
    assert!(
        error.to_string().contains("starter deployment mode"),
        "{error}"
    );
    let mut skipped = session
        .execute("SELECT IF(0, EMBED_TEXT('openai/model', 'hello'), 'unused')")
        .expect("unselected EMBED_TEXT branch must stay lazy");
    assert_eq!(
        skipped.remove(0).next_row().unwrap().unwrap(),
        vec!["unused".to_owned()]
    );
}

#[cfg(feature = "nextgen")]
#[test]
fn go_merge_43_embed_text_sql_calls_domain_provider_and_returns_vector() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    let previous = astersql_config_deploymode::Get();
    astersql_config_deploymode::Set(astersql_config_deploymode::Starter).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 2048];
        let mut request = Vec::new();
        loop {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|part| part == b"\r\n\r\n") {
                break;
            }
        }
        let response =
            serde_json::json!({"data":[{"index":0,"embedding":"AACAPwAAAEA="}]}).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        )
        .unwrap();
    });
    let (domain, session) = CreateAnalyzeSession().expect("SQL session");
    domain
        .start(astersql_domain::domain::StartMode::Normal)
        .expect("start inference providers");
    domain.set_global_system_variable("tidb_exp_embed_openai_api_key", "test-key");
    domain.set_global_system_variable(
        "tidb_exp_embed_openai_api_base",
        &format!("http://{address}/v1"),
    );
    let mut result = session
        .execute("SELECT EMBED_TEXT('openai/model', 'hello')")
        .expect("execute EMBED_TEXT through SQL");
    let row = result.remove(0).next_row().unwrap().unwrap();
    assert_eq!(row, vec!["[1,2]".to_owned()]);
    let mut nested_result = session
        .execute("SELECT CONCAT('vector=', EMBED_TEXT('openai/model', 'hello'))")
        .expect("execute nested EMBED_TEXT through SQL");
    assert_eq!(
        nested_result.remove(0).next_row().unwrap().unwrap(),
        vec!["vector=[1,2]".to_owned()]
    );
    session
        .execute("CREATE TABLE embed_source (id INT PRIMARY KEY, value VARCHAR(20))")
        .expect("create inference source table");
    session
        .execute("INSERT INTO embed_source VALUES (1, 'hello')")
        .expect("insert inference source row");
    let mut table_result = session
        .execute("SELECT EMBED_TEXT('openai/model', value) FROM embed_source WHERE id = 1")
        .expect("embed a table column through SQL");
    assert_eq!(
        table_result.remove(0).next_row().unwrap().unwrap(),
        vec!["[1,2]".to_owned()]
    );
    let mut nested_table_result = session
        .execute("SELECT CONCAT('vector=', EMBED_TEXT('openai/model', value)) FROM embed_source WHERE id = 1")
        .expect("embed a table column in nested SQL expression");
    assert_eq!(
        nested_table_result.remove(0).next_row().unwrap().unwrap(),
        vec!["vector=[1,2]".to_owned()]
    );
    server.join().unwrap();
    domain.close();
    astersql_config_deploymode::Set(previous).unwrap();
}

#[test]
fn embed_text_checks_deployment_before_null_and_invalid_options() {
    let (_, session) = CreateAnalyzeSession().expect("SQL session");
    for sql in [
        "SELECT EMBED_TEXT(NULL, 'text')",
        "SELECT EMBED_TEXT('mock/json', NULL)",
        "SELECT EMBED_TEXT('mock/json', '[1]', '{invalid}')",
    ] {
        let error = session
            .execute(sql)
            .err()
            .expect("deployment rejected before arguments");
        assert!(
            error.to_string().contains("starter deployment mode"),
            "{sql}: {error}"
        );
    }
}

#[cfg(feature = "nextgen")]
#[test]
fn embedding_sql_starter_options_nulls_errors_and_key_variables() {
    let previous = astersql_config_deploymode::Get();
    astersql_config_deploymode::Set(astersql_config_deploymode::Starter).unwrap();
    let (domain, session) = CreateAnalyzeSession().unwrap();
    domain
        .start(astersql_domain::domain::StartMode::Normal)
        .unwrap();
    domain
        .get_embed_fn()
        .unwrap()
        .register(
            "mock",
            std::sync::Arc::new(astersql_inference::MockEmbedder),
        )
        .unwrap();
    for (sql, expected) in [
        ("SELECT EMBED_TEXT('mock/json', '[1,2,3]')", "[1,2,3]"),
        (
            "SELECT EMBED_TEXT('mock/json', '[1,2,3]', '{\"plus\":1,\"plus@search\":10}')",
            "[2,3,4]",
        ),
        ("SELECT EMBED_TEXT('mock/json', '[1]', NULL)", "[1]"),
    ] {
        let mut result = session.execute(sql).unwrap();
        assert_eq!(
            result.remove(0).next_row().unwrap().unwrap(),
            vec![expected.to_owned()]
        );
    }
    for sql in [
        "SELECT EMBED_TEXT('mock/json', '[1]', 'null')",
        "SELECT EMBED_TEXT('mock/json', '[1]', '[]')",
        "SELECT EMBED_TEXT('mock/json', '[1]', '{bad}')",
    ] {
        assert!(
            session
                .execute(sql)
                .err()
                .unwrap()
                .to_string()
                .contains("options in JSON")
        );
    }
    for name in [
        "tidb_exp_embed_jina_ai_api_key",
        "tidb_exp_embed_openai_api_key",
        "tidb_exp_embed_cohere_api_key",
        "tidb_exp_embed_huggingface_api_key",
        "tidb_exp_embed_nvidia_nim_api_key",
        "tidb_exp_embed_gemini_api_key",
    ] {
        session
            .execute(&format!("SET GLOBAL {name} = '1234567890'"))
            .unwrap();
        assert_eq!(
            domain.global_system_variable(name).as_deref(),
            Some("1234567890")
        );
        let mut persisted = session
            .execute(&format!(
                "SELECT variable_value FROM mysql.global_variables WHERE variable_name = '{name}'"
            ))
            .unwrap();
        assert_eq!(
            persisted.remove(0).next_row().unwrap().unwrap(),
            vec!["1234567890".to_owned()]
        );
        let mut result = session.execute(&format!("SELECT @@global.{name}")).unwrap();
        assert_eq!(
            result.remove(0).next_row().unwrap().unwrap(),
            vec!["******7890".to_owned()]
        );
        assert!(session.execute(&format!("SET {name} = 'key'")).is_err());
    }
    assert!(
        session
            .execute("SET GLOBAL tidb_exp_embed_openai_api_base = 'https://evil.example/v1'")
            .is_err()
    );
    session
        .execute(
            "SET GLOBAL tidb_exp_embed_openai_api_base = 'https://api.openai.com/v1/embeddings'",
        )
        .unwrap();
    assert_eq!(
        domain
            .global_system_variable("tidb_exp_embed_openai_api_base")
            .as_deref(),
        Some("https://api.openai.com/v1")
    );
    domain.close();
    astersql_config_deploymode::Set(previous).unwrap();
}
