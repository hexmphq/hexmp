//! Conversão Lua ↔ [`ScriptValue`]. Tudo que cruza entre recursos ou vai para a rede passa por aqui.

use hexmp_protocol::{MAX_PAYLOAD_DEPTH, MAX_PAYLOAD_NODES, MAX_PAYLOAD_STRING, ScriptValue};
use mlua::{Lua, Table, Value, Variadic};

pub fn to_script(v: &Value) -> Result<ScriptValue, String> {
    let mut budget = MAX_PAYLOAD_NODES;
    convert(v, 0, &mut budget)
}

pub fn args_to_script(args: &Variadic<Value>) -> Result<Vec<ScriptValue>, String> {
    let mut budget = MAX_PAYLOAD_NODES;
    args.iter().map(|v| convert(v, 0, &mut budget)).collect()
}

fn convert(v: &Value, depth: usize, budget: &mut usize) -> Result<ScriptValue, String> {
    if *budget == 0 {
        return Err("valor grande demais".into());
    }
    *budget -= 1;
    if depth > MAX_PAYLOAD_DEPTH {
        return Err("tabela aninhada demais".into());
    }
    Ok(match v {
        Value::Nil => ScriptValue::Nil,
        Value::Boolean(b) => ScriptValue::Bool(*b),
        Value::Integer(i) => ScriptValue::Number(*i as f64),
        Value::Number(n) => ScriptValue::Number(*n),
        Value::String(s) => {
            let s = s.to_string_lossy();
            if s.len() > MAX_PAYLOAD_STRING {
                return Err("texto grande demais".into());
            }
            ScriptValue::String(s)
        }
        Value::Table(t) => table(t, depth, budget)?,
        other => return Err(format!("{} não pode ser enviado", other.type_name())),
    })
}

/// Tabela sequencial (1..n) vira Array; com chaves texto vira Map. Vazia vira Array.
fn table(t: &Table, depth: usize, budget: &mut usize) -> Result<ScriptValue, String> {
    let len = t.raw_len();
    let mut count = 0usize;
    let mut entries = Vec::new();
    for pair in t.pairs::<Value, Value>() {
        let (k, v) = pair.map_err(|e| e.to_string())?;
        count += 1;
        entries.push((k, v));
    }
    if count == len {
        let mut items = Vec::with_capacity(len);
        for i in 1..=len {
            let v: Value = t.raw_get(i).map_err(|e| e.to_string())?;
            items.push(convert(&v, depth + 1, budget)?);
        }
        return Ok(ScriptValue::Array(items));
    }
    let mut map = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        let key = match k {
            Value::String(s) => s.to_string_lossy(),
            Value::Integer(i) => i.to_string(),
            Value::Number(n) => n.to_string(),
            other => {
                return Err(format!(
                    "chave do tipo {} não é suportada",
                    other.type_name()
                ));
            }
        };
        map.push((key, convert(&v, depth + 1, budget)?));
    }
    // Ordem estável para comparar e testar.
    map.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(ScriptValue::Map(map))
}

pub fn to_lua(lua: &Lua, v: &ScriptValue) -> mlua::Result<Value> {
    Ok(match v {
        ScriptValue::Nil => Value::Nil,
        ScriptValue::Bool(b) => Value::Boolean(*b),
        ScriptValue::Number(n) => Value::Number(*n),
        ScriptValue::String(s) => Value::String(lua.create_string(s)?),
        ScriptValue::Array(items) => {
            let t = lua.create_table_with_capacity(items.len(), 0)?;
            for (i, item) in items.iter().enumerate() {
                t.raw_set(i + 1, to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        ScriptValue::Map(entries) => {
            let t = lua.create_table_with_capacity(0, entries.len())?;
            for (k, item) in entries {
                t.raw_set(k.as_str(), to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
    })
}

pub fn args_to_lua(lua: &Lua, values: &[ScriptValue]) -> mlua::Result<Variadic<Value>> {
    values.iter().map(|v| to_lua(lua, v)).collect()
}

/// Mensagem curta de um erro Lua, sem caminho, linha ou traceback.
/// `error("item inválido")` vira `item inválido`.
pub fn error_message(e: &mlua::Error) -> String {
    match e {
        mlua::Error::CallbackError { cause, .. } => error_message(cause),
        mlua::Error::WithContext { cause, .. } => error_message(cause),
        mlua::Error::RuntimeError(msg) | mlua::Error::SyntaxError { message: msg, .. } => {
            strip_location(msg.lines().next().unwrap_or_default()).to_string()
        }
        mlua::Error::MemoryError(_) => "memória do recurso esgotada".into(),
        other => other.to_string(),
    }
}

fn strip_location(line: &str) -> &str {
    // Formato "caminho:linha: mensagem".
    if let Some((_, after)) = line.split_once(':')
        && let Some((num, msg)) = after.split_once(':')
        && !num.is_empty()
        && num.chars().all(|c| c.is_ascii_digit())
    {
        return msg.trim_start();
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_through_lua() {
        let lua = Lua::new();
        let v = ScriptValue::Map(vec![
            ("a".into(), ScriptValue::Number(1.0)),
            (
                "b".into(),
                ScriptValue::Array(vec![
                    ScriptValue::String("x".into()),
                    ScriptValue::Bool(true),
                ]),
            ),
        ]);
        let back = to_script(&to_lua(&lua, &v).unwrap()).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn rejects_functions_and_cycles() {
        let lua = Lua::new();
        let f: Value = lua.load("return function() end").eval().unwrap();
        assert!(to_script(&f).is_err());
        let cyclic: Value = lua.load("local t = {} t.self = t return t").eval().unwrap();
        assert!(to_script(&cyclic).is_err());
    }

    #[test]
    fn strips_location() {
        assert_eq!(
            strip_location("res/c.luau:1: item inválido"),
            "item inválido"
        );
        assert_eq!(strip_location("boom"), "boom");
        assert_eq!(strip_location("a: b"), "a: b");
    }
}
