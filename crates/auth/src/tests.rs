use super::*;
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn store() -> AccountStore {
    AccountStore::open("sqlite::memory:").unwrap()
}

#[test]
fn identity_roundtrip_and_limits() {
    let id = Identity::new("hexmp:0190abcd", "Ana_Souza");
    assert_eq!(Identity::decode(&id.encode().unwrap()).unwrap(), id);
    assert_eq!(
        Identity::new("a".repeat(121), "x").encode(),
        Err(IdentityError::TooLong)
    );
    assert_eq!(Identity::decode(&[0u8; 256]), Err(IdentityError::Malformed));
    let mut garbage = [0xffu8; 256];
    garbage[0] = 1;
    assert!(Identity::decode(&garbage).is_err());
}

#[test]
fn key_hex_roundtrip_and_no_overwrite() {
    let k = ServerKey::generate();
    assert_eq!(ServerKey::from_hex(&k.to_hex()).unwrap(), k);
    assert!(ServerKey::from_hex("zz").is_err());
    assert_eq!(format!("{k:?}"), "ServerKey(..)");

    let path = std::env::temp_dir().join(format!("hexmp-key-{}.key", random_client_id()));
    k.save_new(&path).unwrap();
    assert_eq!(ServerKey::load(&path).unwrap(), k);
    assert!(ServerKey::generate().save_new(&path).is_err());
    let _ = std::fs::remove_file(path);
}

#[test]
fn register_login_and_validation() {
    let mut st = store();
    let id = st.register("Ana_Souza", "senha-forte-123").unwrap();
    assert!(id.account.starts_with("hexmp:"));
    assert_eq!(id.display_name, "Ana_Souza");

    // Usuário é único sem diferenciar maiúsculas.
    assert_eq!(
        st.register("ana_souza", "outra-senha-123"),
        Err(AuthError::UsernameTaken)
    );
    assert_eq!(
        st.register("a", "senha-forte-123"),
        Err(AuthError::InvalidUsername)
    );
    assert_eq!(
        st.register("com espaço", "senha-forte-123"),
        Err(AuthError::InvalidUsername)
    );
    assert_eq!(
        st.register("joao", "curta"),
        Err(AuthError::InvalidPassword)
    );

    assert_eq!(st.login("ANA_SOUZA", "senha-forte-123").unwrap(), id);
    assert_eq!(
        st.login("Ana_Souza", "errada-errada"),
        Err(AuthError::BadCredentials)
    );
    assert_eq!(
        st.login("ninguem", "senha-forte-123"),
        Err(AuthError::BadCredentials)
    );
}

#[test]
fn bans_block_login_and_expire() {
    let mut st = store();
    let id = st.register("bandido", "senha-forte-123").unwrap();
    st.ban(&id.account, "trapaça", None).unwrap();
    match st.login("bandido", "senha-forte-123") {
        Err(AuthError::Banned(b)) => {
            assert_eq!(b.reason, "trapaça");
            assert_eq!(b.expires_at, None);
        }
        other => panic!("esperava ban, veio {other:?}"),
    }
    assert_eq!(st.unban(&id.account).unwrap(), 1);
    assert!(st.login("bandido", "senha-forte-123").is_ok());

    // Ban temporário já vencido não vale.
    st.ban(&id.account, "antigo", Some(-1000)).unwrap();
    assert_eq!(st.active_ban(&id.account).unwrap(), None);
    st.ban(&id.account, "1 hora", Some(3_600_000)).unwrap();
    let ban = st.active_ban(&id.account).unwrap().unwrap();
    assert!(ban.to_string().contains("1 h"));
}

#[test]
fn token_carries_identity() {
    let key = ServerKey::generate();
    let id = Identity::new("hexmp:x", "Ana");
    let addr: SocketAddr = "127.0.0.1:30120".parse().unwrap();
    let bytes = issue_token(&key, &[addr], 42, &id).unwrap();
    let token = read_token(&bytes).unwrap();
    assert_eq!(token.client_id, 42);
    assert_eq!(token.server_addresses[0], Some(addr));
}

fn http(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, serde_json::Value) {
    http_with(addr, method, path, body, "")
}

/// `headers`: linhas extras, cada uma terminando em `\r\n`.
fn http_with(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: &str,
    headers: &str,
) -> (u16, serde_json::Value) {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: x\r\n{headers}Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out[9..12].parse().unwrap();
    let json = out.split("\r\n\r\n").nth(1).unwrap_or("null");
    (
        status,
        serde_json::from_str(json).unwrap_or(serde_json::Value::Null),
    )
}

#[test]
fn http_service_register_login_and_rate_limit() {
    let config = ServiceConfig {
        game_servers: vec!["127.0.0.1:30120".parse().unwrap()],
        workers: 2,
        max_failed_logins: 3,
        max_registrations: 2,
        password: true,
        platform: Some((
            ticket_key().verifying_key(),
            "https://login.exemplo/".into(),
        )),
        listing: None,
    };
    let service = Arc::new(
        AuthService::bind(
            "127.0.0.1:0".parse().unwrap(),
            config,
            ServerKey::generate(),
            store(),
        )
        .unwrap(),
    );
    let addr = service.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let handle = {
        let (service, stop) = (service.clone(), stop.clone());
        std::thread::spawn(move || service.run(&stop))
    };

    assert_eq!(http(addr, "GET", "/v1/health", "").0, 200);
    let creds = r#"{"username":"Ana","password":"senha-forte-123"}"#;
    let (status, body) = http(addr, "POST", "/v1/register", creds);
    assert_eq!(status, 201, "{body}");
    assert_eq!(http(addr, "POST", "/v1/register", creds).0, 409);
    assert_eq!(http(addr, "POST", "/v1/register", "{").0, 400);

    let (status, body) = http(addr, "POST", "/v1/login", creds);
    assert_eq!(status, 200, "{body}");
    let token = base64::engine::general_purpose::STANDARD
        .decode(body["token"].as_str().unwrap())
        .unwrap();
    assert!(read_token(&token).is_ok());
    assert_eq!(body["displayName"], "Ana");

    // Conta HexM: o bilhete vira token uma vez só, e o ban deste servidor vale.
    let ticket = |sub: &str, jti: &str| {
        let t = hexmp_ticket::Ticket {
            sub: sub.into(),
            name: "Bia".into(),
            aud: "https://login.exemplo".into(),
            iat: crate::now().as_secs(),
            jti: jti.into(),
        };
        format!(
            r#"{{"ticket":"{}"}}"#,
            hexmp_ticket::sign(&ticket_key(), &t)
        )
    };
    assert_eq!(http(addr, "GET", "/v1/methods", "").1["hexm"], true);
    let (status, body) = http(addr, "POST", "/v1/ticket", &ticket("hexm:1", "a"));
    assert_eq!((status, &body["account"]), (200, &"hexm:1".into()));
    assert_eq!(
        http(addr, "POST", "/v1/ticket", &ticket("hexm:1", "a")).0,
        401
    );

    let wrong = r#"{"username":"Ana","password":"errada-errada"}"#;
    for _ in 0..2 {
        assert_eq!(http(addr, "POST", "/v1/login", wrong).0, 401);
    }
    // Bloqueado depois de 3 falhas, mesmo com a senha certa.
    assert_eq!(http(addr, "POST", "/v1/login", creds).0, 429);
    // Atrás de um proxy local, cada jogador conta pelo próprio IP.
    let via = |ip: &str| format!("X-Forwarded-For: {ip}\r\n");
    let other = via("203.0.113.9");
    assert_eq!(http_with(addr, "POST", "/v1/login", creds, &other).0, 200);
    // O proxy acrescenta o IP que viu no fim; o que o jogador escreveu antes não vale.
    let forged = via("203.0.113.9, 127.0.0.1");
    assert_eq!(http_with(addr, "POST", "/v1/login", creds, &forged).0, 429);
    assert_eq!(http(addr, "GET", "/nada", "").0, 404);

    stop.store(true, Ordering::Relaxed);
    handle.join().unwrap();
}

use base64::Engine;

fn ticket_key() -> hexmp_ticket::SigningKey {
    hexmp_ticket::SigningKey::from_bytes(&[7; 32])
}

#[test]
fn platform_accounts_respect_bans() {
    let mut st = store();
    assert_eq!(st.admit("hexm:1", "Bia").unwrap().account, "hexm:1");
    st.ban("hexm:1", "x", None).unwrap();
    assert!(matches!(
        st.admit("hexm:1", "Bia"),
        Err(AuthError::Banned(_))
    ));
}

#[test]
fn public_info() {
    let dir = std::env::temp_dir().join(format!("hexmp-info-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (info, status) = (dir.join("server.json"), dir.join("status.json"));
    let load = |text: &str, list: &str| {
        std::fs::write(&info, text).unwrap();
        crate::Listing::load(&info, &status, list, "https://login.exemplo")
    };
    let list = "https://lista.exemplo/";
    let long = format!(r#"{{"name":"{}"}}"#, "x".repeat(49));
    for bad in [
        "{",
        "{}",
        r#"{"name":" "}"#,
        long.as_str(),
        r#"{"name":"a","typo":1}"#,
        r#"{"name":"a","image":"server.json"}"#,
        r#"{"name":"a","tags":["1","2","3","4","5"]}"#,
    ] {
        assert!(load(bad, list).is_err(), "{bad}");
    }
    assert!(load(r#"{"name":"a"}"#, "http://lista.exemplo").is_err());
    let l = load(r#"{"name":"Cidade","language":"pt"}"#, list).unwrap();
    let write = |age: i64| {
        let at = crate::now_ms() - age;
        std::fs::write(
            &status,
            format!(r#"{{"players":3,"maxPlayers":8,"updatedAt":{at}}}"#),
        )
        .unwrap();
        l.reply()
    };
    assert_eq!(l.reply()["online"], false);
    let r = write(0);
    assert_eq!((&r["online"], &r["players"]), (&true.into(), &3.into()));
    assert_eq!(r["list"], "https://lista.exemplo");
    // Servidor de jogo parado não conta como online.
    assert_eq!(write(60_000)["online"], false);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Roda no PostgreSQL só quando `HEXMP_TEST_POSTGRES_URL` existe (CI).
#[test]
fn postgres_accounts_and_bans() {
    let Ok(url) = std::env::var("HEXMP_TEST_POSTGRES_URL") else {
        return;
    };
    let mut st = AccountStore::open(&url).unwrap();
    let user = format!("pg_{}", random_client_id() % 1_000_000_000);
    let id = st.register(&user, "senha-forte-123").unwrap();
    assert_eq!(st.login(&user, "senha-forte-123").unwrap(), id);
    st.ban(&id.account, "permanente", None).unwrap();
    assert_eq!(
        st.active_ban(&id.account).unwrap().unwrap().expires_at,
        None
    );
    st.unban(&id.account).unwrap();
    st.ban(&id.account, "temporário", Some(60_000)).unwrap();
    assert!(
        st.active_ban(&id.account)
            .unwrap()
            .unwrap()
            .expires_at
            .is_some()
    );
    assert!(matches!(
        st.login(&user, "senha-forte-123"),
        Err(AuthError::Banned(_))
    ));
}
