//! Rodam em SQLite sempre, e também em PostgreSQL se `HEXMP_TEST_POSTGRES_URL` estiver definida.

use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

fn backends() -> Vec<String> {
    let mut urls = vec!["sqlite::memory:".to_string()];
    if let Ok(pg) = std::env::var("HEXMP_TEST_POSTGRES_URL") {
        urls.push(pg);
    }
    urls
}

/// Nome único por teste para não colidir no mesmo Postgres.
fn unique(prefix: &str) -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        "{prefix}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

fn s(v: &str) -> ScriptValue {
    ScriptValue::String(v.into())
}

fn n(v: f64) -> ScriptValue {
    ScriptValue::Number(v)
}

fn run(db: &mut Db, sql: &str, params: Vec<ScriptValue>) -> DbResult {
    db.call(Request::Run(Statement::new(sql, params)))
}

fn setup(url: &str) -> (Db, String) {
    let mut db = Db::connect(url).unwrap();
    let t = unique("contas");
    run(
        &mut db,
        &format!(
            "CREATE TABLE {t} (id TEXT PRIMARY KEY, saldo BIGINT NOT NULL, taxa DOUBLE PRECISION)"
        ),
        vec![],
    )
    .unwrap();
    (db, t)
}

#[test]
fn insert_select_update() {
    for url in backends() {
        let (mut db, t) = setup(&url);
        let r = run(
            &mut db,
            &format!("INSERT INTO {t} (id, saldo, taxa) VALUES ($1, $2, $3)"),
            vec![s("ana"), n(100.0), n(0.5)],
        );
        assert_eq!(r, Ok(Response::Affected(1)), "{url}");

        let r = run(
            &mut db,
            &format!(
                "INSERT INTO {t} (id, saldo) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET saldo = excluded.saldo"
            ),
            vec![s("ana"), n(250.0)],
        );
        assert_eq!(r, Ok(Response::Affected(1)), "{url}");

        let r = run(
            &mut db,
            &format!("SELECT id, saldo, taxa, saldo * 2 AS dobro FROM {t} WHERE id = $1"),
            vec![s("ana")],
        )
        .unwrap();
        assert_eq!(
            r,
            Response::Rows(vec![ScriptValue::Map(vec![
                ("id".into(), s("ana")),
                ("saldo".into(), n(250.0)),
                ("taxa".into(), n(0.5)),
                ("dobro".into(), n(500.0)),
            ])]),
            "{url}"
        );
    }
}

#[test]
fn nulls_and_missing_rows() {
    for url in backends() {
        let (mut db, t) = setup(&url);
        run(
            &mut db,
            &format!("INSERT INTO {t} (id, saldo) VALUES ($1, $2)"),
            vec![s("bia"), n(1.0)],
        )
        .unwrap();
        let r = run(&mut db, &format!("SELECT taxa FROM {t}"), vec![]).unwrap();
        assert_eq!(
            r,
            Response::Rows(vec![ScriptValue::Map(vec![(
                "taxa".into(),
                ScriptValue::Nil
            )])]),
            "{url}"
        );
        let r = run(
            &mut db,
            &format!("SELECT * FROM {t} WHERE id = $1"),
            vec![s("ninguem")],
        );
        assert_eq!(r, Ok(Response::Rows(vec![])), "{url}");
    }
}

#[test]
fn transaction_rolls_back_on_expectation() {
    for url in backends() {
        let (mut db, t) = setup(&url);
        run(
            &mut db,
            &format!("INSERT INTO {t} (id, saldo) VALUES ('a', 100), ('b', 0)"),
            vec![],
        )
        .unwrap();
        let transfer = |valor: f64| {
            Request::Transaction(vec![
                Statement::new(
                    format!("UPDATE {t} SET saldo = saldo - $1 WHERE id = 'a' AND saldo >= $1"),
                    vec![n(valor)],
                )
                .expect(1),
                Statement::new(
                    format!("UPDATE {t} SET saldo = saldo + $1 WHERE id = 'b'"),
                    vec![n(valor)],
                )
                .expect(1),
            ])
        };
        assert!(db.call(transfer(60.0)).is_ok(), "{url}");
        let err = db.call(transfer(60.0)).unwrap_err();
        assert!(err.contains("esperava 1"), "{url}: {err}");

        let r = run(
            &mut db,
            &format!("SELECT id, saldo FROM {t} ORDER BY id"),
            vec![],
        )
        .unwrap();
        assert_eq!(
            r,
            Response::Rows(vec![
                ScriptValue::Map(vec![("id".into(), s("a")), ("saldo".into(), n(40.0))]),
                ScriptValue::Map(vec![("id".into(), s("b")), ("saldo".into(), n(60.0))]),
            ]),
            "{url}: segunda transferência deveria ter sido desfeita"
        );
    }
}

#[test]
fn requests_run_in_order() {
    for url in backends() {
        let (mut db, t) = setup(&url);
        let tickets: Vec<Ticket> = (0..50)
            .map(|i| {
                db.submit(Request::Run(Statement::new(
                    format!("INSERT INTO {t} (id, saldo) VALUES ($1, $2)"),
                    vec![s(&format!("j{i}")), n(i as f64)],
                )))
            })
            .collect();
        let last = db.submit(Request::Run(Statement::new(
            format!("SELECT COUNT(*) AS total FROM {t}"),
            vec![],
        )));
        let mut results = Vec::new();
        while results.len() < 51 {
            results.extend(db.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        let order: Vec<Ticket> = results.iter().map(|(t, _)| *t).collect();
        let mut expected = tickets.clone();
        expected.push(last);
        assert_eq!(order, expected, "{url}");
        let (_, count) = results.last().unwrap();
        assert_eq!(
            count,
            &Ok(Response::Rows(vec![ScriptValue::Map(vec![(
                "total".into(),
                n(50.0)
            )])])),
            "{url}"
        );
    }
}

#[test]
fn rejects_table_params_and_reports_sql_errors() {
    for url in backends() {
        let (mut db, t) = setup(&url);
        let r = run(
            &mut db,
            &format!("SELECT * FROM {t} WHERE id = $1"),
            vec![ScriptValue::Array(vec![])],
        );
        assert!(r.unwrap_err().contains("tabelas não podem"), "{url}");
        assert!(run(&mut db, "SELECT * FROM tabela_que_nao_existe", vec![]).is_err());
    }
}

#[test]
fn migrations_apply_once_and_detect_edits() {
    for url in backends() {
        let mut db = Db::connect(&url).unwrap();
        let res = unique("res");
        let t = unique("itens");
        let files = vec![
            (
                "001_init.sql".to_string(),
                format!("CREATE TABLE {t} (id TEXT PRIMARY KEY);"),
            ),
            (
                "002_coluna.sql".to_string(),
                format!("ALTER TABLE {t} ADD COLUMN peso BIGINT NOT NULL DEFAULT 0;"),
            ),
        ];
        let m = |files: &Vec<(String, String)>| Request::Migrate {
            resource: res.clone(),
            files: files.clone(),
        };
        assert_eq!(
            db.call(m(&files)),
            Ok(Response::Migrated(vec![
                "001_init.sql".into(),
                "002_coluna.sql".into()
            ])),
            "{url}"
        );
        assert_eq!(db.call(m(&files)), Ok(Response::Migrated(vec![])), "{url}");

        let mut edited = files.clone();
        edited[0].1.push_str(" -- mudou");
        assert!(
            db.call(m(&edited)).unwrap_err().contains("foi alterada"),
            "{url}"
        );

        // Migração com erro não fica registrada.
        let broken = vec![("003_ruim.sql".to_string(), "ISTO NAO E SQL".to_string())];
        let mut all = files.clone();
        all.extend(broken);
        assert!(db.call(m(&all)).is_err(), "{url}");
        assert!(
            db.call(m(&all)).is_err(),
            "{url}: deveria tentar de novo e falhar"
        );
    }
}

#[test]
fn shutdown_flushes_pending_writes() {
    let path = std::env::temp_dir().join(format!("{}.db", unique("hexmp-flush")));
    let url = format!("sqlite:{}", path.display().to_string().replace('\\', "/"));
    {
        let mut db = Db::connect(&url).unwrap();
        db.call(Request::Run(Statement::new(
            "CREATE TABLE IF NOT EXISTS f (id BIGINT)",
            vec![],
        )))
        .unwrap();
        for i in 0..100 {
            db.submit(Request::Run(Statement::new(
                "INSERT INTO f (id) VALUES ($1)",
                vec![n(i as f64)],
            )));
        }
        // Drop sem esperar as respostas.
    }
    let mut db = Db::connect(&url).unwrap();
    let r = db
        .call(Request::Run(Statement::new(
            "SELECT COUNT(*) AS c FROM f",
            vec![],
        )))
        .unwrap();
    assert_eq!(
        r,
        Response::Rows(vec![ScriptValue::Map(vec![("c".into(), n(100.0))])])
    );
    drop(db);
    let _ = std::fs::remove_file(path);
}

#[test]
fn bad_url_is_an_error() {
    assert!(Db::connect("mysql://x").is_err());
    assert!(Db::connect("sqlite:/caminho/que/nao/existe/x.db").is_err());
}

/// Vários programas abrindo um banco vazio ao mesmo tempo.
fn migrate_together(url: &str) {
    let start = std::sync::Barrier::new(8);
    let applied: usize = std::thread::scope(|scope| {
        let all: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    let mut db = Db::connect(url).unwrap();
                    let files = ["a", "b"].map(|t| {
                        let sql = format!("CREATE TABLE juntos_{t} (id BIGINT PRIMARY KEY);");
                        (format!("{t}.sql"), sql)
                    });
                    start.wait();
                    let r = db.call(Request::Migrate {
                        resource: "juntos".into(),
                        files: files.to_vec(),
                    });
                    match r.unwrap() {
                        Response::Migrated(names) => names.len(),
                        other => panic!("{other:?}"),
                    }
                })
            })
            .collect();
        all.into_iter().map(|t| t.join().unwrap()).sum()
    });
    assert_eq!(applied, 2);
}

#[test]
fn concurrent_migrations_sqlite() {
    for _ in 0..10 {
        let file = std::env::temp_dir().join(format!("{}.db", unique("hexmp_juntos")));
        migrate_together(&format!("sqlite:{}", file.display()));
        let _ = std::fs::remove_file(&file);
    }
}

#[test]
fn concurrent_migrations_postgres() {
    let Ok(url) = std::env::var("HEXMP_TEST_POSTGRES_URL") else {
        return;
    };
    let mut db = Db::connect(&url).unwrap();
    for _ in 0..10 {
        // Um esquema novo faz as vezes do banco vazio.
        let schema = unique("juntos");
        let mut run = |sql: String| db.call(Request::Run(Statement::new(sql, vec![]))).unwrap();
        run(format!("CREATE SCHEMA {schema}"));
        migrate_together(&format!("{url}?options=-csearch_path%3D{schema}"));
        run(format!("DROP SCHEMA {schema} CASCADE"));
    }
}
