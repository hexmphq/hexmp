//! Leitura do `resource.luau`. O manifesto roda num ambiente vazio:
//! só atribuições de dados, sem nenhuma função disponível.

use mlua::{Lua, Table, Value, VmState};
use std::path::{Component, Path};
use std::time::{Duration, Instant};

pub const MANIFEST_FILE: &str = "resource.luau";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Manifest {
    pub version: Option<String>,
    pub description: Option<String>,
    pub dependencies: Vec<String>,
    pub shared_scripts: Vec<String>,
    pub server_scripts: Vec<String>,
    pub client_scripts: Vec<String>,
    /// Arquivos `.sql` aplicados uma vez cada, em ordem, antes dos scripts.
    pub migrations: Vec<String>,
}

pub fn parse(source: &str, chunk_name: &str) -> Result<Manifest, String> {
    let lua = Lua::new();
    lua.set_memory_limit(1 << 20).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_millis(200);
    lua.set_interrupt(move |_| {
        if Instant::now() > deadline {
            Err(mlua::Error::runtime("manifesto demorou demais"))
        } else {
            Ok(VmState::Continue)
        }
    });
    let env = lua.create_table().map_err(|e| e.to_string())?;
    lua.load(source)
        .set_name(format!("@{chunk_name}"))
        .set_environment(env.clone())
        .exec()
        .map_err(|e| crate::value::error_message(&e))?;

    let m = Manifest {
        version: opt_string(&env, "version")?,
        description: opt_string(&env, "description")?,
        dependencies: string_list(&env, "dependencies")?,
        shared_scripts: script_list(&env, "shared_scripts")?,
        server_scripts: script_list(&env, "server_scripts")?,
        client_scripts: script_list(&env, "client_scripts")?,
        migrations: file_list(&env, "migrations", &[".sql"])?,
    };
    Ok(m)
}

fn opt_string(env: &Table, key: &str) -> Result<Option<String>, String> {
    match env.raw_get::<Value>(key).map_err(|e| e.to_string())? {
        Value::Nil => Ok(None),
        Value::String(s) => Ok(Some(s.to_string_lossy())),
        other => Err(format!(
            "`{key}` deve ser texto, veio {}",
            other.type_name()
        )),
    }
}

fn string_list(env: &Table, key: &str) -> Result<Vec<String>, String> {
    match env.raw_get::<Value>(key).map_err(|e| e.to_string())? {
        Value::Nil => Ok(Vec::new()),
        Value::Table(t) => (1..=t.raw_len())
            .map(|i| match t.raw_get::<Value>(i) {
                Ok(Value::String(s)) => Ok(s.to_string_lossy()),
                _ => Err(format!("`{key}[{i}]` deve ser texto")),
            })
            .collect(),
        other => Err(format!(
            "`{key}` deve ser lista, veio {}",
            other.type_name()
        )),
    }
}

/// Lista de arquivos de script: caminhos relativos, sem `..`, terminando em `.luau` ou `.lua`.
fn script_list(env: &Table, key: &str) -> Result<Vec<String>, String> {
    file_list(env, key, &[".luau", ".lua"])
}

fn file_list(env: &Table, key: &str, extensions: &[&str]) -> Result<Vec<String>, String> {
    let list = string_list(env, key)?;
    for p in &list {
        if !is_safe_relative(p) {
            return Err(format!("caminho inválido em `{key}`: {p}"));
        }
        if !extensions.iter().any(|e| p.ends_with(e)) {
            return Err(format!(
                "`{p}` precisa terminar em {}",
                extensions.join(" ou ")
            ));
        }
    }
    Ok(list)
}

pub fn is_safe_relative(p: &str) -> bool {
    let path = Path::new(p);
    !p.is_empty()
        && !p.contains('\\')
        && path.components().all(|c| matches!(c, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fields() {
        let m = parse(
            r#"
            version = "1.0.0"
            dependencies = { "bank" }
            shared_scripts = { "shared/config.luau" }
            server_scripts = { "server/main.luau" }
            ui = { { name = "hud", backend = "rmlui" } }
            "#,
            "shop/resource.luau",
        )
        .unwrap();
        assert_eq!(m.version.as_deref(), Some("1.0.0"));
        assert_eq!(m.dependencies, vec!["bank"]);
        assert_eq!(m.server_scripts, vec!["server/main.luau"]);
    }

    #[test]
    fn has_no_functions_available() {
        assert!(parse("print('oi')", "x").is_err());
        assert!(parse("server_scripts = { string.rep('a', 3) }", "x").is_err());
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(parse(r#"server_scripts = { "../../segredo.luau" }"#, "x").is_err());
        assert!(parse(r#"server_scripts = { "/etc/passwd.luau" }"#, "x").is_err());
        assert!(parse(r#"server_scripts = { "a\\b.luau" }"#, "x").is_err());
        assert!(parse(r#"server_scripts = { "main.txt" }"#, "x").is_err());
    }

    #[test]
    fn infinite_loop_times_out() {
        assert!(parse("while true do end", "x").is_err());
    }
}
