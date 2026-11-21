use hexmp_protocol::{ClientMessage, PROTOCOL_VERSION};

fn main() {
    let hello = ClientMessage::Hello {
        version: PROTOCOL_VERSION,
        name: "bot-1".into(),
    };
    println!("headless-bot: {hello:?}");
}
