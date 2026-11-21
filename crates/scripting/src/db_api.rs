//! `db` e `util` para scripts.
//!
//! O banco nunca bloqueia o tick: toda chamada é enfileirada e o callback roda num tick seguinte.
//! As requisições de todos os recursos executam em ordem.
//!
//! ```lua
//! db.query("SELECT cash FROM rp_accounts WHERE character_id = $1", { id }, function(rows, err) end)
//! db.execute("UPDATE ... WHERE id = $1", { id })            -- sem callback: "dispara e esquece"
//! db.transaction({
//!     { "UPDATE contas SET saldo = saldo - $1 WHERE id = $2 AND saldo >= $1", { 50, a }, expect = 1 },
//!     { "UPDATE contas SET saldo = saldo + $1 WHERE id = $2", { 50, b }, expect = 1 },
//! }, function(results, err) end)
//! ```

use crate::value::{to_lua, to_script};
use crate::{Handler, Pending, Shared, rt, upgrade};
use hexmp_db::{DbResult, Request, Response, Statement};
use hexmp_protocol::ScriptValue;
use mlua::{Function, Lua, Table, Value, Variadic};
use std::rc::Rc;
use std::time::SystemTime;

pub(crate) fn install(lua: &Lua, resource: &Rc<str>, shared: &Rc<Shared>) -> mlua::Result<()> {
    let g = lua.globals();
    let db = lua.create_table()?;

    for fname in ["query", "execute"] {
        let (w, r) = (Rc::downgrade(shared), resource.clone());
        db.set(
            fname,
            lua.create_function(move |lua, (sql, a, b): (String, Value, Value)| {
                let s = upgrade(&w)?;
                let (params, cb) = params_and_callback(a, b)?;
                let st = Statement::new(sql, params);
                submit(&s, lua, &r, Request::Run(st), cb)
            })?,
        )?;
    }

    let (w, r) = (Rc::downgrade(shared), resource.clone());
    db.set(
        "transaction",
        lua.create_function(move |lua, (list, cb): (Table, Option<Function>)| {
            let s = upgrade(&w)?;
            let mut stmts = Vec::new();
            for (i, item) in list.sequence_values::<Table>().enumerate() {
                let item = item.map_err(|_| rt(format!("instrução {}: deve ser tabela", i + 1)))?;
                let sql: String = match item.get::<Value>(1)? {
                    Value::String(s) => s.to_string_lossy(),
                    _ => item
                        .get::<Option<String>>("sql")?
                        .ok_or_else(|| rt(format!("instrução {}: falta o SQL", i + 1)))?,
                };
                let params = match item.get::<Value>(2)? {
                    Value::Nil => item.get::<Value>("params")?,
                    v => v,
                };
                let mut st = Statement::new(sql, params_list(params)?);
                if let Some(n) = item.get::<Option<u64>>("expect")? {
                    st = st.expect(n);
                }
                stmts.push(st);
            }
            if stmts.is_empty() {
                return Err(rt("transação vazia"));
            }
            submit(&s, lua, &r, Request::Transaction(stmts), cb)
        })?,
    )?;
    g.set("db", db)?;

    let util = lua.create_table()?;
    util.set(
        "uuid",
        lua.create_function(|_, ()| Ok(uuid::Uuid::now_v7().to_string()))?,
    )?;
    util.set(
        "now",
        lua.create_function(|_, ()| {
            Ok(SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis() as f64)
                .unwrap_or(0.0))
        })?,
    )?;
    g.set("util", util)?;
    Ok(())
}

fn params_and_callback(a: Value, b: Value) -> mlua::Result<(Vec<ScriptValue>, Option<Function>)> {
    match (a, b) {
        (Value::Function(f), Value::Nil) => Ok((Vec::new(), Some(f))),
        (p, Value::Nil) => Ok((params_list(p)?, None)),
        (p, Value::Function(f)) => Ok((params_list(p)?, Some(f))),
        _ => Err(rt("uso: db.query(sql, [parâmetros], [callback])")),
    }
}

fn params_list(v: Value) -> mlua::Result<Vec<ScriptValue>> {
    match v {
        Value::Nil => Ok(Vec::new()),
        Value::Table(t) => {
            let mut out = Vec::new();
            for i in 1..=t.raw_len() {
                out.push(to_script(&t.raw_get::<Value>(i)?).map_err(rt)?);
            }
            Ok(out)
        }
        other => Err(rt(format!(
            "parâmetros devem ser uma lista, veio {}",
            other.type_name()
        ))),
    }
}

fn submit(
    s: &Shared,
    lua: &Lua,
    resource: &Rc<str>,
    req: Request,
    cb: Option<Function>,
) -> mlua::Result<()> {
    let mut db = s.db.borrow_mut();
    let Some(db) = db.as_mut() else {
        return Err(rt(
            "banco de dados não configurado (inicie o servidor com --db)",
        ));
    };
    let ticket = db.submit(req);
    s.pending.borrow_mut().insert(
        ticket,
        Pending {
            resource: resource.clone(),
            callback: cb.map(|func| Handler {
                resource: resource.clone(),
                lua: lua.clone(),
                func,
            }),
        },
    );
    Ok(())
}

fn response_to_lua(lua: &Lua, r: &Response) -> mlua::Result<Value> {
    Ok(match r {
        Response::Rows(rows) => to_lua(lua, &ScriptValue::Array(rows.clone()))?,
        Response::Affected(n) => Value::Number(*n as f64),
        Response::Transaction(list) => {
            let t = lua.create_table()?;
            for (i, item) in list.iter().enumerate() {
                t.raw_set(i + 1, response_to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        Response::Migrated(names) => to_lua(
            lua,
            &ScriptValue::Array(names.iter().cloned().map(ScriptValue::String).collect()),
        )?,
    })
}

/// Argumentos do callback: `(resultado, nil)` ou `(nil, erro)`.
pub(crate) fn result_to_lua(lua: &Lua, r: &DbResult) -> mlua::Result<Variadic<Value>> {
    let mut out = Variadic::new();
    match r {
        Ok(resp) => {
            out.push(response_to_lua(lua, resp)?);
            out.push(Value::Nil);
        }
        Err(e) => {
            out.push(Value::Nil);
            out.push(Value::String(lua.create_string(e)?));
        }
    }
    Ok(out)
}
