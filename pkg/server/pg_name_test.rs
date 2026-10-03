// Copyright 2026 AsterSQL.
use crate::pg_name::rewrite;

#[test]
fn pg_introspection_names_token_scopes() {
    for (sql, expected) in [
        (
            "SELECT 'FROM public.fake' FROM public.t /* JOIN other.x */ AS a",
            "SELECT 'FROM public.fake' FROM `db`.`t` /* JOIN other.x */ AS a",
        ),
        (
            "SELECT \"a\".\"id\" FROM \"public\".\"t\" AS \"a\"",
            "SELECT `a`.`id` FROM `db`.`t` AS `a`",
        ),
        (
            "SELECT * FROM t a, public.u b JOIN db.public.v c ON b.id = c.id",
            "SELECT * FROM `db`.`t` a, `db`.`u` b JOIN `db`.`v` c ON b.id = c.id",
        ),
        (
            "WITH t AS (SELECT * FROM public.t) SELECT * FROM t JOIN public.t AS original ON t.id = original.id",
            "WITH t AS (SELECT * FROM `db`.`t`) SELECT * FROM t JOIN `db`.`t` AS original ON t.id = original.id",
        ),
        (
            "SELECT * FROM (WITH t AS (SELECT * FROM public.u) SELECT * FROM t) q JOIN t outer_t ON q.id = outer_t.id",
            "SELECT * FROM (WITH t AS (SELECT * FROM `db`.`u`) SELECT * FROM t) q JOIN `db`.`t` outer_t ON q.id = outer_t.id",
        ),
        (
            "SELECT extract(year FROM '2020-01-01') FROM t",
            "SELECT extract(year FROM '2020-01-01') FROM `db`.`t`",
        ),
        (
            "INSERT INTO public.t SELECT * FROM public.u",
            "INSERT INTO `db`.`t` SELECT * FROM `db`.`u`",
        ),
        (
            "UPDATE public.t SET note = 'JOIN other.t'",
            "UPDATE `db`.`t` SET note = 'JOIN other.t'",
        ),
        (
            "DELETE FROM db.public.t WHERE id = 1",
            "DELETE FROM `db`.`t` WHERE id = 1",
        ),
        (
            "CREATE TABLE IF NOT EXISTS public.t (id INT)",
            "CREATE TABLE IF NOT EXISTS `db`.`t` (id INT)",
        ),
        (
            "ALTER TABLE public.t RENAME TO public.u",
            "ALTER TABLE `db`.`t` RENAME TO `db`.`u`",
        ),
        (
            "DROP TABLE IF EXISTS public.t, public.u",
            "DROP TABLE IF EXISTS `db`.`t`, `db`.`u`",
        ),
        ("DROP VIEW public.v", "DROP VIEW `db`.`v`"),
        ("TRUNCATE TABLE public.t", "TRUNCATE TABLE `db`.`t`"),
        (
            "SELECT * FROM public.\"a\"\"b\"",
            "SELECT * FROM `db`.`a\"b`",
        ),
        (
            "SELECT NULL -- FROM other.t\n",
            "SELECT NULL -- FROM other.t\n",
        ),
    ] {
        assert_eq!(rewrite(sql, "db", true).unwrap(), expected, "{sql}");
    }
    assert_eq!(
        rewrite("SELECT * FROM public.t", "db", false).unwrap(),
        "SELECT * FROM `db`.`t`"
    );
    assert_eq!(
        rewrite("WITH t AS (SELECT 1) SELECT * FROM t", "db", false).unwrap(),
        "WITH t AS (SELECT 1) SELECT * FROM t"
    );
}

#[test]
fn pg_introspection_names_rejection_boundaries() {
    for (sql, db, public, state) in [
        ("SELECT * FROM other.public.t", "db", true, "0A000"),
        ("SELECT * FROM other.t", "db", true, "0A000"),
        ("SELECT * FROM db.private.t", "db", true, "0A000"),
        ("SELECT * FROM \"PUBLIC\".t", "db", true, "0A000"),
        ("SELECT * FROM public.t.extra", "db", true, "0A000"),
        ("SELECT * FROM public.\"\"", "db", true, "42601"),
        ("SELECT * FROM public.\"broken", "db", true, "42601"),
        ("SELECT * FROM public.t", "", true, "3D000"),
        ("SELECT * FROM t", "db", false, "42P01"),
        ("SELECT * FROM `other`.t", "db", true, "0A000"),
        ("SELECT $$FROM other.t$$", "db", true, "0A000"),
        (
            "WITH RECURSIVE t AS (SELECT 1) SELECT * FROM t",
            "db",
            true,
            "0A000",
        ),
        ("SELECT * FROM t /* unfinished", "db", true, "42601"),
        ("SELECT * FROM t /* nested /* x */ */", "db", true, "0A000"),
    ] {
        assert_eq!(rewrite(sql, db, public).unwrap_err().0, state, "{sql}");
    }
}
