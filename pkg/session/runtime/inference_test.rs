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

#[cfg(feature = "nextgen")]
#[test]
fn auto_embedding_generated_columns_follow_real_sql_dml_and_load_data() {
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
    session.execute("CREATE TABLE auto_embed (id INT PRIMARY KEY, text TEXT, vec VECTOR(3) GENERATED ALWAYS AS (embed_text('mock/json', text)) STORED)").unwrap();
    session
        .execute("INSERT INTO auto_embed VALUES (1, '[1,2,3]', DEFAULT), (2, NULL, DEFAULT)")
        .unwrap();
    let mut result = session
        .execute("SELECT vec FROM auto_embed WHERE id = 1")
        .unwrap();
    assert_eq!(
        result.remove(0).next_row().unwrap().unwrap(),
        vec!["[1,2,3]"]
    );
    session
        .execute("UPDATE auto_embed SET text = '[4,5,6]' WHERE id = 1")
        .unwrap();
    let mut result = session
        .execute("SELECT vec FROM auto_embed WHERE id = 1")
        .unwrap();
    assert_eq!(
        result.remove(0).next_row().unwrap().unwrap(),
        vec!["[4,5,6]"]
    );
    assert!(
        session
            .execute("UPDATE auto_embed SET vec = '[1,2,3]' WHERE id = 1")
            .is_err()
    );
    session
        .execute("CREATE TABLE auto_embed_source (id INT PRIMARY KEY, text TEXT)")
        .unwrap();
    session
        .execute("INSERT INTO auto_embed_source VALUES (3, '[7,8,9]')")
        .unwrap();
    session
        .execute("INSERT INTO auto_embed(id,text) SELECT id,text FROM auto_embed_source")
        .unwrap();
    let mut result = session
        .execute("SELECT vec FROM auto_embed WHERE id = 3")
        .unwrap();
    assert_eq!(
        result.remove(0).next_row().unwrap().unwrap(),
        vec!["[7,8,9]"]
    );
    session.execute("CREATE TABLE auto_embed_load(id INT PRIMARY KEY, text TEXT, vec VECTOR(3) GENERATED ALWAYS AS (embed_text('mock/json', text, '{\"plus\":1}')) STORED)").unwrap();
    session.execute_with_load_data_reader("LOAD DATA LOCAL INFILE 'auto.csv' INTO TABLE auto_embed_load FIELDS TERMINATED BY ',' ENCLOSED BY '\"' (id,text)", std::io::Cursor::new(b"1,\"[1,2,3]\"\n2,\"[4,5,6]\"\n".to_vec())).unwrap();
    let mut result = session
        .execute("SELECT id,vec FROM auto_embed_load ORDER BY id")
        .unwrap()
        .remove(0);
    assert_eq!(result.next_row().unwrap().unwrap(), vec!["1", "[2,3,4]"]);
    assert_eq!(result.next_row().unwrap().unwrap(), vec!["2", "[5,6,7]"]);
    session.execute("CREATE TABLE auto_embed_multi(id INT PRIMARY KEY, a TEXT, b TEXT, va VECTOR(3) AS (embed_text('mock/json',a)) STORED, vb VECTOR(3) AS (embed_text('mock/json',b,'{\"plus\":0.5}')) STORED)").unwrap();
    session.execute("INSERT INTO auto_embed_multi(id,a,b) VALUES (1,'[1,2,3]','[4,5,6]'), (2,'[7,8,9]','[10,11,12]')").unwrap();
    let mut result = session
        .execute("SELECT id,va,vb FROM auto_embed_multi ORDER BY id")
        .unwrap()
        .remove(0);
    assert_eq!(
        result.next_row().unwrap().unwrap(),
        vec!["1", "[1,2,3]", "[4.5,5.5,6.5]"]
    );
    assert_eq!(
        result.next_row().unwrap().unwrap(),
        vec!["2", "[7,8,9]", "[10.5,11.5,12.5]"]
    );
    assert!(
        session
            .execute("INSERT INTO auto_embed VALUES (4,'not-json',DEFAULT)")
            .is_err()
    );
    let mut result = session
        .execute("SELECT id FROM auto_embed WHERE id=4")
        .unwrap()
        .remove(0);
    assert!(result.next_row().unwrap().is_none());
    session
        .execute("INSERT IGNORE INTO auto_embed VALUES (7,'[1,2]',DEFAULT)")
        .unwrap();
    let warning = session
        .execute("SHOW WARNINGS")
        .unwrap()
        .remove(0)
        .next_row()
        .unwrap()
        .unwrap();
    assert_eq!(warning[1], "1105");
    let row = session
        .execute("SELECT vec FROM auto_embed WHERE id=7")
        .unwrap()
        .remove(0)
        .next_row()
        .unwrap()
        .unwrap();
    assert_eq!(row, vec!["<nil>"]);
    let oversized = format!("[{}0]", "0,".repeat(16_383));
    session
        .execute(&format!(
            "INSERT IGNORE INTO auto_embed VALUES (5,'{oversized}',DEFAULT)"
        ))
        .unwrap();
    let warning = session
        .execute("SHOW WARNINGS")
        .unwrap()
        .remove(0)
        .next_row()
        .unwrap()
        .unwrap();
    assert_eq!(warning[1], "1105");
    assert!(
        session
            .execute(&format!(
                "INSERT INTO auto_embed VALUES (6,'{oversized}',DEFAULT)"
            ))
            .is_err()
    );
    domain.close();
    astersql_config_deploymode::Set(previous).unwrap();
}

#[cfg(feature = "nextgen")]
#[test]
fn auto_embedding_ddl_validates_shape_constants_dependencies_and_alter() {
    let previous = astersql_config_deploymode::Get();
    astersql_config_deploymode::Set(astersql_config_deploymode::Starter).unwrap();
    let (_, session) = CreateAnalyzeSession().unwrap();
    for (sql, message) in [
        (
            "CREATE TABLE bad_nested(text TEXT, vec VECTOR(3) AS (embed_text('mock/json',text)+1) STORED)",
            "nested expression",
        ),
        (
            "CREATE TABLE bad_virtual(text TEXT, vec VECTOR(3) AS (embed_text('mock/json',text)) VIRTUAL)",
            "virtual generated column",
        ),
        (
            "CREATE TABLE bad_model(text TEXT, vec VECTOR(3) AS (embed_text(concat('mock','/json'),text)) STORED)",
            "model name using string constant",
        ),
        (
            "CREATE TABLE bad_options(text TEXT, vec VECTOR(3) AS (embed_text('mock/json',text,'{invalid_json}')) STORED)",
            "options in JSON format",
        ),
        (
            "CREATE TABLE bad_dep(text TEXT, vec VECTOR(3) AS (embed_text('mock/json',text)) STORED, copy TEXT AS (vec_as_text(vec)) STORED)",
            "depends on generated column 'vec'",
        ),
        (
            "CREATE TABLE bad_func(vec VECTOR(3), dist DOUBLE AS (vec_embed_l2_distance(vec,'[1,2,3]')) STORED)",
            "disallowed function",
        ),
        (
            "CREATE TABLE bad_index(text TEXT, INDEX idx ((vec_dims(embed_text('mock/json',text)))))",
            "disallowed function",
        ),
    ] {
        let error = session.execute(sql).err().expect("DDL rejected");
        assert!(error.to_string().contains(message), "{sql}: {error}");
    }
    session
        .execute("CREATE TABLE embed_alter(id INT PRIMARY KEY,text TEXT)")
        .unwrap();
    let error = session.execute("ALTER TABLE embed_alter ADD COLUMN vec VECTOR(3) AS (embed_text('mock/json',text)) STORED").err().unwrap();
    assert!(
        error
            .to_string()
            .contains("adding a generated column using EMBED_TEXT() through ALTER TABLE"),
        "{error}"
    );
    session.execute("CREATE TABLE embed_modify(id INT PRIMARY KEY,text TEXT,vec VECTOR(3) AS (embed_text('mock/json',text)) STORED, vec_text TEXT AS (text) VIRTUAL)").unwrap();
    let error = session
        .execute(
            "ALTER TABLE embed_modify MODIFY COLUMN vec_text TEXT AS (vec_as_text(vec)) VIRTUAL",
        )
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("generated column 'vec_text' depends on generated column 'vec'"),
        "{error}"
    );
    astersql_config_deploymode::Set(previous).unwrap();
}

#[cfg(feature = "nextgen")]
#[test]
fn auto_embedding_load_counts_preserve_global_one_based_bad_null_diagnostics() {
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
    session.execute("CREATE TABLE embedding_load_counts(id INT PRIMARY KEY,text TEXT,vec VECTOR NOT NULL AS (embed_text('mock/json',text)) STORED)").unwrap();
    session
        .execute_with_load_data_reader(
            "LOAD DATA LOCAL INFILE 'auto.tsv' INTO TABLE embedding_load_counts (id,text)",
            std::io::Cursor::new(b"1\t[1]\n2\t\\N\n".to_vec()),
        )
        .unwrap();
    let mut warnings = session.execute("SHOW WARNINGS").unwrap().remove(0);
    let mut found = false;
    while let Some(warning) = warnings.next_row().unwrap() {
        if warning
            .iter()
            .any(|value| value.contains("NULL supplied to NOT NULL column 'vec' at row 2"))
        {
            assert_eq!(warning[1], "1263");
            found = true;
        }
    }
    assert!(
        found,
        "LOAD DATA must carry its physical row number to generated bad-null warning"
    );
    let (_, table) = domain.stats_table("test", "embedding_load_counts").unwrap();
    let mut rows = vec![
        Some(std::collections::HashMap::from([(
            "text".to_owned(),
            Some("[3]".to_owned()),
        )])),
        None,
        Some(std::collections::HashMap::from([("text".to_owned(), None)])),
    ];
    let error = session
        .fill_embedding_generated_rows(&table, &mut rows, false, Some(42))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("NULL supplied to NOT NULL column 'vec' at row 42"),
        "{error}"
    );
    assert_eq!(rows[0].as_ref().unwrap()["vec"].as_deref(), Some("[3]"));
    assert!(
        rows[1].is_none(),
        "nil conversion row must remain nil and occupy its physical position"
    );
    assert!(
        !rows[2].as_ref().unwrap().contains_key("vec"),
        "failing task must not install a value"
    );
    let (_, without_runtime) = CreateAnalyzeSession().unwrap();
    let mut nil_rows = vec![None, None];
    without_runtime
        .fill_embedding_generated_rows(&table, &mut nil_rows, false, Some(2))
        .unwrap();
    assert_eq!(nil_rows, vec![None, None]);
    without_runtime
        .fill_embedding_generated_rows(&table, &mut [], false, Some(0))
        .unwrap();
    astersql_config_deploymode::Set(astersql_config_deploymode::Premium).unwrap();
    assert!(
        without_runtime
            .fill_embedding_generated_rows(&table, &mut nil_rows, false, Some(2))
            .unwrap_err()
            .to_string()
            .contains("starter deployment mode")
    );
    astersql_config_deploymode::Set(astersql_config_deploymode::Starter).unwrap();
    domain.close();
    astersql_config_deploymode::Set(previous).unwrap();
}

#[cfg(feature = "nextgen")]
#[test]
fn auto_embedding_rejects_unsafe_arguments_before_allowing_stored_form() {
    let previous = astersql_config_deploymode::Get();
    astersql_config_deploymode::Set(astersql_config_deploymode::Starter).unwrap();
    let (_, session) = CreateAnalyzeSession().unwrap();
    for sql in [
        "CREATE TABLE embed_rand(text TEXT,vec VECTOR(3) AS (embed_text('mock/json',rand())) STORED)",
        "CREATE TABLE embed_var(text TEXT,vec VECTOR(3) AS (embed_text('mock/json',@text)) STORED)",
    ] {
        let error = session.execute(sql).err().expect(
            "allowing EMBED_TEXT must preserve the illegal-function checker for its arguments",
        );
        assert!(error.to_string().contains("disallowed function"), "{error}");
    }
    astersql_config_deploymode::Set(previous).unwrap();
}
