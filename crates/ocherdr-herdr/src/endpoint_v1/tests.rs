use super::*;

#[test]
fn upstream_generation_one_fixtures_remain_compatible() {
    let hello: EndpointClientHello = serde_json::from_str(include_str!(
        "../../tests/fixtures/herdr-0.9.4/endpoint-hello-v1.json"
    ))
    .unwrap();
    assert_eq!(hello.generation, ENDPOINT_PROTOCOL_GENERATION);
    assert!(hello.surface_active);
    assert!(!hello.surface_reuse && !hello.surface_delta && !hello.surface_scroll);

    let welcome: EndpointServerWelcome = serde_json::from_str(include_str!(
        "../../tests/fixtures/herdr-0.9.4/endpoint-welcome-v1.json"
    ))
    .unwrap();
    assert_eq!(welcome.generation, ENDPOINT_PROTOCOL_GENERATION);
    assert_eq!(welcome.snapshot_codec, ENDPOINT_SNAPSHOT_KIND);
    assert_eq!(welcome.surface_codec, SURFACE_CODEC_V1);
    assert_eq!(welcome.input_codec, INPUT_CODEC_V1);
    assert_eq!(welcome.blob_codec, BLOB_CODEC_V1);
    assert!(welcome.capabilities.is_empty());

    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/herdr-0.9.4/endpoint-snapshot-v1.json"
    ))
    .unwrap();
    value["future_projection"] = serde_json::json!({ "enabled": true });
    value["commands"][0]["action"] = "FutureAction".into();
    let snapshot: ClientShellSnapshot = serde_json::from_value(value).unwrap();
    assert_eq!(snapshot.boot_id, "boot-v1");
    assert_eq!(snapshot.workspaces[0].agent_status, AgentStatus::Unknown);
    assert_eq!(
        snapshot.commands[0].action,
        ClientShellCommandAction::Unknown
    );
    assert_eq!(snapshot.agents[0].state_change_seq, 9);
}

#[test]
fn hello_only_requests_surface_encodings_the_client_can_decode() {
    let hello = EndpointClientHello::new(ClientSurfaceSize { cols: 80, rows: 24 }, 8, 16);
    let wire = serde_json::to_value(hello).unwrap();
    assert_eq!(wire["surface_reuse"], false);
    assert_eq!(wire["surface_delta"], false);
    assert_eq!(wire["surface_scroll"], false);
    assert_eq!(
        wire["surface_codecs"],
        serde_json::json!([SURFACE_CODEC_V1])
    );
}

#[test]
fn named_control_retains_the_upstream_binary_envelope() {
    // v0.9.4 wire.rs freezes EndpointControl at tag 20 on both directions.
    // u32 LE byte count, varint enum tag, then two length-prefixed UTF-8 strings.
    let golden = [6, 0, 0, 0, 20, 1, b'x', 2, b'y', b'z'];
    let mut bytes = Vec::new();
    write_message(
        &mut bytes,
        &ClientMessage::EndpointControl {
            kind: "x".into(),
            data: "yz".into(),
        },
    )
    .unwrap();
    assert_eq!(bytes, golden);
    let message: ServerMessage = read_message(&mut golden.as_slice(), MAX_FRAME_SIZE).unwrap();
    assert_eq!(
        message,
        ServerMessage::EndpointControl {
            kind: "x".into(),
            data: "yz".into()
        }
    );
}
