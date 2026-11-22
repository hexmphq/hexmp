use super::*;
use hexmp_protocol::{Payload, ResourceFile, ScriptValue};

fn start(rt: &mut ClientRuntime, name: &str, code: &str) {
    rt.handle(&ServerMessage::ResourceStart {
        name: name.into(),
        scripts: vec!["client.luau".into()],
        ui_page: Some("ui/index.html".into()),
        files: vec![
            ResourceFile {
                path: "client.luau".into(),
                data: code.as_bytes().to_vec(),
            },
            ResourceFile {
                path: "ui/index.html".into(),
                data: b"<p>oi</p>".to_vec(),
            },
        ],
    });
}

fn s(v: &str) -> ScriptValue {
    ScriptValue::String(v.into())
}

#[test]
fn net_events_reach_ui() {
    let mut rt = ClientRuntime::default();
    start(
        &mut rt,
        "hud",
        r#"net.on("hud:money", function(cash) ui.send({ type = "money", cash = cash }) end)"#,
    );
    assert_eq!(rt.running(), vec!["hud"]);
    assert_eq!(rt.ui_pages()[0].page, "ui/index.html");
    rt.handle(&ServerMessage::Event {
        name: "hud:money".into(),
        args: Payload::from_values(&[ScriptValue::Number(500.0)]),
    });
    let ui = rt.drain_ui();
    assert_eq!(ui.len(), 1);
    assert_eq!(ui[0].resource, "hud");
    let ScriptValue::Map(fields) = &ui[0].data else {
        panic!("{:?}", ui[0].data);
    };
    assert!(fields.contains(&("cash".into(), ScriptValue::Number(500.0))));
}

#[test]
fn rpc_call_and_callback() {
    let mut rt = ClientRuntime::default();
    start(
        &mut rt,
        "a",
        r#"rpc.call("shop.buy", { "agua" }, function(err, saldo)
               ui.send({ err = err, saldo = saldo })
           end)"#,
    );
    let out = rt.drain_outgoing();
    let [ClientMessage::Rpc { id, name, args }] = out.as_slice() else {
        panic!("{out:?}");
    };
    assert_eq!(name, "shop.buy");
    assert_eq!(args.to_values().unwrap(), vec![s("agua")]);

    rt.handle(&ServerMessage::RpcResult {
        id: *id,
        result: Ok(Payload::from_values(&[ScriptValue::Number(495.0)])),
    });
    let ui = rt.drain_ui();
    let ScriptValue::Map(f) = &ui[0].data else {
        panic!()
    };
    assert_eq!(f, &vec![("saldo".to_string(), ScriptValue::Number(495.0))]);
}

#[test]
fn rpc_timeout_and_error() {
    let mut rt = ClientRuntime::new(RuntimeConfig {
        rpc_timeout: Duration::from_millis(100),
        ..RuntimeConfig::default()
    });
    start(
        &mut rt,
        "a",
        r#"rpc.call("lento", nil, function(err) ui.send(err) end)
           rpc.call("falha", nil, function(err) ui.send(err) end)"#,
    );
    let out = rt.drain_outgoing();
    let ClientMessage::Rpc { id, .. } = &out[1] else {
        panic!()
    };
    rt.handle(&ServerMessage::RpcResult {
        id: *id,
        result: Err("saldo insuficiente".into()),
    });
    rt.tick(Duration::from_millis(200));
    let ui: Vec<_> = rt.drain_ui().into_iter().map(|m| m.data).collect();
    assert_eq!(ui, vec![s("saldo insuficiente"), s("tempo esgotado")]);
}

#[test]
fn ui_callbacks_timers_and_stop() {
    let mut rt = ClientRuntime::default();
    start(
        &mut rt,
        "menu",
        r#"ui.on("fechar", function(data) return "ok:" .. data.motivo end)
           timer.every(100, function() ui.send("tick") end)"#,
    );
    let mut data = ScriptValue::Map(vec![("motivo".into(), s("esc"))]);
    assert_eq!(
        rt.ui_callback("menu", "fechar", data.clone()),
        Ok(vec![s("ok:esc")])
    );
    rt.tick(Duration::from_millis(250));
    rt.tick(Duration::from_millis(100));
    assert_eq!(rt.drain_ui().len(), 2);

    rt.handle(&ServerMessage::ResourceStop {
        name: "menu".into(),
    });
    assert!(rt.running().is_empty());
    rt.tick(Duration::from_millis(500));
    assert!(rt.drain_ui().is_empty());
    data = ScriptValue::Nil;
    assert!(rt.ui_callback("menu", "fechar", data).is_err());
}

#[test]
fn sandbox_and_limits() {
    let mut rt = ClientRuntime::default();
    start(&mut rt, "mau", "while true do end");
    assert!(rt.running().is_empty(), "loop infinito deveria abortar");
    start(&mut rt, "mau2", "os.exit()");
    assert!(rt.running().is_empty());
    start(&mut rt, "mau3", "string.rep = nil");
    assert!(
        rt.running().is_empty(),
        "biblioteca padrão deveria ser somente leitura"
    );

    // Erro num handler não derruba o recurso.
    start(
        &mut rt,
        "ok",
        r#"net.on("x", function() error("falhou") end)"#,
    );
    rt.handle(&ServerMessage::Event {
        name: "x".into(),
        args: Payload::from_values(&[]),
    });
    assert_eq!(rt.running(), vec!["ok"]);
    assert_eq!(rt.script_errors(), 1);
}

#[test]
fn restart_replaces_handlers() {
    let mut rt = ClientRuntime::default();
    start(&mut rt, "a", r#"net.on("e", function() ui.send(1) end)"#);
    start(&mut rt, "a", r#"net.on("e", function() ui.send(2) end)"#);
    rt.handle(&ServerMessage::Event {
        name: "e".into(),
        args: Payload::from_values(&[]),
    });
    let ui: Vec<_> = rt.drain_ui().into_iter().map(|m| m.data).collect();
    assert_eq!(ui, vec![ScriptValue::Number(2.0)]);
}
