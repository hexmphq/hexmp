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
    let before = rt.ui_pages()[0].generation;
    start(&mut rt, "a", r#"net.on("e", function() ui.send(2) end)"#);
    assert_ne!(rt.ui_pages()[0].generation, before);
    rt.handle(&ServerMessage::Event {
        name: "e".into(),
        args: Payload::from_values(&[]),
    });
    let ui: Vec<_> = rt.drain_ui().into_iter().map(|m| m.data).collect();
    assert_eq!(ui, vec![ScriptValue::Number(2.0)]);
}

#[test]
fn ui_focus_stacks_and_is_released() {
    let mut rt = ClientRuntime::default();
    let code = r#"net.on(RESOURCE_NAME, function(on) ui.focus(on) ui.send(ui.focused()) end)
        events.on("uiFocusReleased", function(name)
            if name == RESOURCE_NAME then ui.send("solto") end
        end)"#;
    start(&mut rt, "loja", code);
    start(&mut rt, "chat", code);
    let set = |rt: &mut ClientRuntime, name: &str, on: bool| {
        rt.handle(&ServerMessage::Event {
            name: name.into(),
            args: Payload::from_values(&[ScriptValue::Bool(on)]),
        });
    };
    let focus = |rt: &ClientRuntime| rt.ui_focus().unwrap_or_default();
    assert_eq!(focus(&rt), "");
    set(&mut rt, "loja", true);
    set(&mut rt, "chat", true);
    assert_eq!(focus(&rt), "chat", "o último pedido vence");
    set(&mut rt, "loja", false);
    assert_eq!(focus(&rt), "chat", "soltar o próprio não tira o do outro");
    set(&mut rt, "loja", true);
    set(&mut rt, "loja", false);
    assert_eq!(focus(&rt), "chat", "volta para quem estava antes");
    let ui: Vec<_> = rt.drain_ui().into_iter().map(|m| m.data).collect();
    let want = [true, true, false, true, false];
    assert_eq!(ui, want.map(ScriptValue::Bool));

    // O jogador tira o foco: os scripts ficam sabendo.
    set(&mut rt, "loja", true);
    rt.drain_ui();
    rt.set_ui_focus(Some("chat"));
    assert_eq!(focus(&rt), "chat");
    rt.set_ui_focus(Some("nao-existe"));
    assert_eq!(focus(&rt), "");
    let who: Vec<_> = rt.drain_ui().into_iter().map(|m| m.resource).collect();
    assert_eq!(who, ["loja", "chat"]);

    // Parar ou reiniciar solta.
    set(&mut rt, "chat", true);
    start(&mut rt, "chat", code);
    assert_eq!(focus(&rt), "");

    // Sem página não há o que focar, e um script que falha ao iniciar não deixa foco preso.
    start(&mut rt, "falha", "ui.focus(true) error('x')");
    assert_eq!(focus(&rt), "");
    rt.handle(&ServerMessage::ResourceStart {
        name: "sem".into(),
        scripts: vec!["c.luau".into()],
        ui_page: None,
        files: vec![ResourceFile {
            path: "c.luau".into(),
            data: b"ui.focus(true)".to_vec(),
        }],
    });
    assert_eq!(focus(&rt), "");
    assert_eq!(rt.running(), ["loja", "chat"]);
}

#[test]
fn keys_reach_only_who_asked() {
    let mut rt = ClientRuntime::default();
    let code = r#"keys.bind("abrir", "F3", "Abrir", function(down)
            ui.send((down and "v" or "^") .. keys.get("abrir"))
        end)"#;
    start(&mut rt, "loja", code);
    let f = |n: &str| keys::code(n).unwrap();
    let out = |rt: &mut ClientRuntime| -> Vec<String> {
        let ui = rt.drain_ui();
        ui.iter()
            .map(|m| match &m.data {
                ScriptValue::String(s) => s.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    };
    rt.key(f("F3"), true);
    rt.key(f("F3"), true); // repetição
    rt.key(f("A"), true);
    rt.key(f("F3"), false);
    assert_eq!(out(&mut rt), ["vF3", "^F3"]);

    // Com o input numa página, nada chega; ao focar, o que estava pressionado solta.
    rt.key(f("F3"), true);
    rt.set_ui_focus(Some("loja"));
    rt.tick(Duration::ZERO);
    assert_eq!(out(&mut rt), ["vF3", "^F3"]);
    rt.key(f("F3"), true);
    rt.key(f("F3"), false);
    assert!(out(&mut rt).is_empty());

    // Tecla segurada desde a página não dispara ao voltar para o jogo.
    rt.key(f("F3"), true);
    rt.set_ui_focus(None);
    rt.key(f("F3"), true);
    assert!(out(&mut rt).is_empty());
    rt.key(f("F3"), false);

    // O jogador troca, e a troca sobrevive ao reinício.
    assert!(!rt.set_key("loja", "abrir", Some("F8")), "reservada");
    assert!(!rt.set_key("loja", "abrir", Some("F7")), "reservada");
    assert!(rt.set_key("loja", "abrir", Some("k")));
    let custom = &rt.custom_keys()[0];
    assert_eq!((&*custom.id, &*custom.key), ("abrir", "K"));
    start(&mut rt, "loja", code);
    assert_eq!(rt.key_bindings()[0].key, "K");
    assert_eq!(rt.key_bindings()[0].default, "F3");
    rt.key(f("K"), true);
    rt.release_keys();
    assert_eq!(out(&mut rt), ["vK", "^K"]);
    assert!(rt.set_key("loja", "abrir", None));
    assert_eq!(rt.key_bindings()[0].key, "F3");
    assert!(rt.custom_keys().is_empty());
    start(&mut rt, ":hexm", code);
    assert_eq!(rt.running(), ["loja"], "nome reservado");

    // Recusas.
    for bad in [
        r#"keys.bind("x", "ESC", "", function() end)"#,
        r#"keys.bind("x", "F7", "", function() end)"#,
        r#"keys.bind("x", "F8", "", function() end)"#,
        r#"keys.bind("", "F1", "", function() end)"#,
        r#"for i = 1, 33 do keys.bind("x" .. i, "F1", "", function() end) end"#,
    ] {
        start(&mut rt, "mau", bad);
    }
    assert_eq!(rt.running(), ["loja"]);
    assert_eq!(
        rt.key_bindings().len(),
        1,
        "recurso que falhou não deixa tecla"
    );
    rt.stop_all();
    assert!(rt.key_bindings().is_empty() && rt.running().is_empty());
}

#[test]
fn player_can_block_ui_focus() {
    let mut rt = ClientRuntime::default();
    // Pede de novo assim que perde.
    start(
        &mut rt,
        "preso",
        r#"ui.focus(true)
           events.on("uiFocusReleased", function() ui.send(ui.focus(true)) end)"#,
    );
    rt.set_ui_focus(None);
    assert_eq!(
        rt.ui_focus().as_deref(),
        Some("preso"),
        "por isso existe a trava"
    );
    rt.drain_ui();
    rt.set_ui_blocked(true);
    assert_eq!(rt.ui_focus(), None);
    assert_eq!(rt.drain_ui()[0].data, ScriptValue::Bool(false));
    rt.set_ui_focus(Some("preso"));
    assert_eq!(rt.ui_focus(), None);
    rt.set_ui_blocked(false);
    rt.set_ui_focus(Some("preso"));
    assert!(rt.ui_focus().is_some());
}
