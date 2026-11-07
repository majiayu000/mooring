use super::*;
use std::os::unix::net::UnixListener;

/// A polling reply must be complete on its own: even an empty page must not ask for
/// Workspace just to find out whether the beginning of the log is missing.
#[test]
fn polling_chat_needs_only_its_chat_reply() {
    for (after, more, partial) in [
        (None, false, true),
        (Some(42), false, true),
        (Some(42), false, false),
        (None, true, true),
    ] {
        let socket =
            std::path::PathBuf::from(format!("/tmp/mooring-chat-{}", uuid::Uuid::new_v4()));
        let listener = UnixListener::bind(&socket).unwrap();
        let host = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(matches!(
                receive::<Request>(&mut stream).unwrap(),
                Request::Hello { major: MAJOR }
            ));
            send(
                &mut stream,
                CONTROL,
                &Reply::Hello {
                    major: MAJOR,
                    instance: String::new(),
                    build: Default::default(),
                },
            )
            .unwrap();
            match receive::<Request>(&mut stream).unwrap() {
                Request::Chat {
                    session, after: a, ..
                } => {
                    assert_eq!(session, "conversation");
                    assert_eq!(a, after);
                }
                other => panic!("polling requested {other:?} instead of Chat"),
            }
            // No second connection is served: any extra Workspace request fails.
            drop(listener);
            send(
                &mut stream,
                CONTROL,
                &Reply::Chat {
                    messages: vec![],
                    generation: "history-1".into(),
                    more,
                    partial,
                },
            )
            .unwrap();
        });
        let page = chat(
            &socket,
            &json!({"op":"chat", "session":"conversation", "after":after}),
        );
        host.join().unwrap();
        std::fs::remove_file(socket).unwrap();
        let page: rp::Chat =
            serde_json::from_value(page.expect("the Chat reply must be sufficient")).unwrap();
        assert!(page.messages.is_empty());
        assert_eq!(page.generation, "history-1");
        assert_eq!(page.more, more);
        assert_eq!(page.partial, partial && !more);
    }
}
