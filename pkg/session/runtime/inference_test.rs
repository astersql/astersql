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
