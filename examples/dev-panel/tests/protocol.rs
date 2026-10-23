#![cfg(all(feature = "dev", debug_assertions))]

use std::sync::{Arc, mpsc};
use std::time::Duration;
use rune::{Env, Runtime};
use rune_dev_panel::{State, config, spec};
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket, stream::MaybeTlsStream};

struct Client {
    socket: WebSocket<MaybeTlsStream<std::net::TcpStream>>,
    wakes: mpsc::Receiver<()>,
    id: u64,
}

impl Client {
    fn apply(&mut self, rt: &mut Runtime<State>, mut message: Value) -> Value {
        while self.wakes.try_recv().is_ok() {}
        self.id += 1;
        message["id"] = json!(self.id);
        self.socket.send(Message::Text(message.to_string().into())).unwrap();
        self.wakes.recv_timeout(Duration::from_secs(5)).unwrap();
        rt.frame(0.0);
        loop {
            let message = self.socket.read().unwrap();
            let Message::Text(text) = message else { continue };
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["id"] == self.id { return value; }
        }
    }
}

#[test]
fn websocket_drives_the_example_tree_and_retunes_an_inflight_spring() {
    let spec = spec();
    let mut rt = Runtime::new((spec.init)(), spec.view, spec.size, Env { system_dark: true, scale: 1.0 });
    let (wake, wakes) = mpsc::channel();
    let mut config = config();
    config.bind.set_port(0);
    let address = rune::dev::attach(&mut rt, &spec, &config, Arc::new(move || { let _ = wake.send(()); })).unwrap();
    rt.frame(0.0);
    let (mut socket, _) = tungstenite::connect(format!("ws://{address}")).unwrap();
    if let MaybeTlsStream::Plain(stream) = socket.get_mut() { stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap(); }
    let initial: Value = serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(initial["type"], "state");
    assert_eq!(initial["descriptors"].as_array().unwrap().iter().filter(|d| d["key"].as_str().unwrap().starts_with("rooms.")).count(), 4);
    let mut client = Client { socket, wakes, id: 0 };

    assert_eq!(client.apply(&mut rt, json!({"type":"set","key":"rune.state","value":"joining"}))["type"], "ack");
    assert_eq!(rt.state, State::Joining);
    let tree = client.apply(&mut rt, json!({"type":"tree"}));
    assert!(tree["elements"].as_array().unwrap().iter().any(|e| e["text"] == "Joining"));

    client.apply(&mut rt, json!({"type":"action","key":"rune.transition.Join"}));
    assert_eq!(rt.state, State::Lobby, "one settled source frame precedes the action");
    assert_eq!(rt.presented("card").unwrap().0[0], 24.0);
    rt.frame(0.0);
    assert_eq!(rt.state, State::Room);
    rt.frame(0.08);
    let x = rt.presented("card").unwrap().0[0];
    assert!(x > 24.0 && x < 220.0);

    client.apply(&mut rt, json!({"type":"set","key":"rooms.join.spring","value":{"duration":1.2,"bounce":0.0}}));
    assert_eq!(rt.presented("card").unwrap().0[0], x, "retuning does not jump");
    rt.frame(0.16);
    let changed = rt.presented("card").unwrap().0[0];
    let baseline = rune::Spring::new(0.35, 0.0).sample(24.0, 0.0, 220.0, 0.24).0;
    assert!((changed - baseline).abs() > 1.0, "the running spring adopts the new curve: {changed} vs {baseline}");
    let (from, velocity) = rune::Spring::new(0.35, 0.0).sample(24.0, 0.0, 220.0, 0.08);
    let expected = rune::Spring::new(1.2, 0.0).sample(from, velocity, 220.0, 0.16).0;
    assert!((changed - expected).abs() < 0.02, "retuning preserves analytic velocity: {changed} vs {expected}");

    client.apply(&mut rt, json!({"type":"set","key":"rune.paused","value":true}));
    let paused = rt.clock;
    let pose = rt.presented("card").unwrap().0;
    assert!(!rt.frame(1.0));
    assert_eq!(rt.clock, paused);
    assert_eq!(rt.presented("card").unwrap().0, pose);
    client.apply(&mut rt, json!({"type":"action","key":"rune.step"}));
    assert!((rt.clock - paused - 1.0 / 60.0).abs() < 1e-9);
    assert!(!rt.animating);

    let bad = client.apply(&mut rt, json!({"type":"setBatch","changes":[
        {"key":"rooms.card.width","value":400},
        {"key":"rooms.join.spring","value":{"duration":0,"bounce":0}}
    ]}));
    assert_eq!(bad["type"], "error");
    assert_eq!(rt.elements().iter().find(|e| e.key.as_deref() == Some("card")).unwrap().target[2], 260.0, "bad batches are atomic");

    client.apply(&mut rt, json!({"type":"action","key":"rune.transition.Leave"}));
    assert_eq!(rt.state, State::Room);
    rt.frame(0.0);
    assert_eq!(rt.state, State::Lobby);

    client.apply(&mut rt, json!({"type":"setBatch","changes":[
        {"key":"rooms.card.width","value":400},
        {"key":"rune.scale","value":0.1},
        {"key":"rune.velocity","value":true}
    ]}));
    client.apply(&mut rt, json!({"type":"reset","group":"rooms.card"}));
    assert_eq!(rt.elements().iter().find(|e| e.key.as_deref() == Some("card")).unwrap().target[2], 260.0);
    client.apply(&mut rt, json!({"type":"reset","group":"Playback"}));
    let clock = rt.clock;
    rt.frame(0.25);
    assert!((rt.clock - clock - 0.25).abs() < 1e-9, "reset restores unpaused 1× playback");
    drop(client);
    drop(rt); // joins the listener and peer threads
    assert!(std::net::TcpStream::connect(address).is_err());
}
