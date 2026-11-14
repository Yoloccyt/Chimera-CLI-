//! MCP 语义层：JSON-RPC 2.0 编解码与传输分帧（ADR-235 S2）。
//!
//! 纪律：每条"畸形必须 Err"的断言都成对存在一个"合法必须 Ok"的正例 ——
//! 只测 Err 的探针无法区分"判据有效"与"解析器整体失能"。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production
#![forbid(unsafe_code)]

use mcp_mesh::protocol::{
    decode_incoming, encode_frame, Incoming, Notification, Request, RequestId, Response, RpcError,
};
use serde_json::{json, Map, Value};

fn empty_extra() -> Map<String, Value> {
    Map::new()
}

#[test]
fn request_roundtrips_with_params_and_id() {
    let request = Request {
        jsonrpc: "2.0".into(),
        id: RequestId::Number(7),
        method: "tools/list".into(),
        params: Some(json!({"cursor": "abc"})),
        extra: empty_extra(),
    };
    let frame = encode_frame(&request).expect("encode");
    match decode_incoming(&frame).expect("decode") {
        Incoming::Request(back) => assert_eq!(back, request),
        other => panic!("expected request, got {other:?}"),
    }
}

#[test]
fn request_id_accepts_numbers_and_strings() {
    for id in [RequestId::Number(1), RequestId::String("req-9".into())] {
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": "initialize"});
        let bytes = serde_json::to_vec(&frame).expect("serialize");
        match decode_incoming(&bytes).expect("decode") {
            Incoming::Request(back) => assert_eq!(back.id, id),
            other => panic!("expected request, got {other:?}"),
        }
    }
}

#[test]
fn unknown_members_survive_a_roundtrip() {
    let frame = br#"{"jsonrpc":"2.0","id":3,"method":"ping","x-trace":"t-1"}"#;
    let Incoming::Request(request) = decode_incoming(frame).expect("decode") else {
        panic!("expected request");
    };
    assert_eq!(request.extra.get("x-trace"), Some(&json!("t-1")));
    let again = encode_frame(&request).expect("encode");
    let value: Value = serde_json::from_slice(&again).expect("re-parse");
    assert_eq!(value.get("x-trace"), Some(&json!("t-1")));
}

#[test]
fn notification_has_no_id_and_is_classified_separately() {
    let frame = br#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#;
    match decode_incoming(frame).expect("decode") {
        Incoming::Notification(Notification { method, .. }) => {
            assert_eq!(method, "notifications/tools/list_changed");
        }
        other => panic!("expected notification, got {other:?}"),
    }
}

#[test]
fn response_with_result_and_with_error_are_distinct() {
    let ok = br#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
    let Incoming::Response(Response { result, error, .. }) =
        decode_incoming(ok).expect("decode ok response")
    else {
        panic!("expected response");
    };
    assert!(result.is_some() && error.is_none());

    let bad = br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}}"#;
    let Incoming::Response(Response { error, .. }) = decode_incoming(bad).expect("decode err")
    else {
        panic!("expected response");
    };
    assert_eq!(
        error,
        Some(RpcError {
            code: -32601,
            message: "nope".into(),
            data: None,
            extra: empty_extra(),
        })
    );
}

#[test]
fn null_result_is_a_valid_success_response() {
    // WHY 单列：`"result": null` 解码后是 Option::None，若用 Option 判"恰有其一"，
    // 一个合法成功响应会被误判成畸形。
    let frame = br#"{"jsonrpc":"2.0","id":2,"result":null}"#;
    match decode_incoming(frame).expect("null result must be accepted") {
        Incoming::Response(Response { result, error, .. }) => {
            assert!(result.is_none(), "null result decodes to None");
            assert!(error.is_none());
        }
        other => panic!("expected response, got {other:?}"),
    }
}

#[test]
fn malformed_frames_are_errors_not_panics() {
    let cases = [
        // 不是 JSON
        (b"not json".as_slice(), "valid JSON"),
        // 不是对象
        (br#"[1,2,3]"#.as_slice(), "object"),
        // 缺 jsonrpc
        (br#"{"id":1,"method":"ping"}"#.as_slice(), "no \"jsonrpc\""),
        // jsonrpc 值错
        (
            br#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#.as_slice(),
            "2.0",
        ),
        // id 类型错（布尔）
        (
            br#"{"jsonrpc":"2.0","id":true,"method":"ping"}"#.as_slice(),
            "malformed request",
        ),
        // 响应同时带 result 与 error
        (
            br#"{"jsonrpc":"2.0","id":1,"result":1,"error":{"code":1,"message":"x"}}"#.as_slice(),
            "exactly one",
        ),
        // 响应两者都缺
        (br#"{"jsonrpc":"2.0","id":1}"#.as_slice(), "exactly one"),
        // 既无 method 也无 id
        (br#"{"jsonrpc":"2.0","params":{}}"#.as_slice(), "neither"),
    ];
    for (frame, needle) in cases {
        let err = decode_incoming(frame).err().unwrap_or_else(|| {
            panic!("frame must be rejected: {}", String::from_utf8_lossy(frame))
        });
        // 用 Display（Debug 会把引号转义成 \"，needle 里有引号就永远匹配不上）
        let text = format!("{err}");
        assert!(
            text.contains(needle),
            "error for {:?} should mention {needle:?}, got {text}",
            String::from_utf8_lossy(frame)
        );
    }
}

#[test]
fn null_params_normalize_to_absent() {
    // 规则：`Some(Null)` 编码成**省略** params，解码回 None（null ≡ 缺省）。
    let request = Request {
        jsonrpc: "2.0".into(),
        id: RequestId::Number(9),
        method: "ping".into(),
        params: Some(Value::Null),
        extra: empty_extra(),
    };
    let frame = encode_frame(&request).expect("encode");
    let value: Value = serde_json::from_slice(&frame).expect("parse");
    assert!(
        value.get("params").is_none(),
        "null params must be omitted on the wire, got {value}"
    );
    let Incoming::Request(back) = decode_incoming(&frame).expect("decode") else {
        panic!("expected request");
    };
    assert!(back.params.is_none());
}

// ------------------------------------------------------------
// DuplexTransport 分帧
// ------------------------------------------------------------

use mcp_mesh::protocol::{DuplexTransport, McpTransport};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn duplex_transport_frames_by_newline() {
    let (mut transport, server) = DuplexTransport::pair(4096);
    let (read_half, mut write_half) = tokio::io::split(server);
    let mut reader = BufReader::new(read_half);

    transport.send(br#"{"a":1}"#).await.expect("send 1");
    transport.send(br#"{"b":2}"#).await.expect("send 2");

    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read 1");
    assert_eq!(line, "{\"a\":1}\n");
    line.clear();
    reader.read_line(&mut line).await.expect("read 2");
    assert_eq!(line, "{\"b\":2}\n");

    // 反方向：服务器写一帧，客户端按行收
    write_half
        .write_all(b"{\"ok\":true}\r\n")
        .await
        .expect("server write");
    write_half.flush().await.expect("flush");
    let frame = transport.recv().await.expect("recv").expect("one frame");
    assert_eq!(
        String::from_utf8(frame).expect("utf8"),
        "{\"ok\":true}",
        "CRLF 也要被剥掉（部分实现对最后一行带 \\r）"
    );
}

#[tokio::test]
async fn duplex_transport_eof_is_none_and_truncation_is_error() {
    let (mut transport, server) = DuplexTransport::pair(4096);
    drop(server);
    assert!(
        transport.recv().await.expect("clean EOF").is_none(),
        "无半帧的 EOF 必须报 Ok(None)，让上层当作正常关闭"
    );

    let (mut transport, server) = DuplexTransport::pair(4096);
    {
        let mut writer = server;
        writer.write_all(b"{\"half\":").await.expect("write half");
        writer.flush().await.expect("flush");
        // drop 关闭对端，且没有换行 ⇒ 半帧截断
    }
    let err = transport
        .recv()
        .await
        .expect_err("truncated frame must fail");
    assert!(format!("{err:?}").contains("mid-frame"), "got {err:?}");
}

#[tokio::test]
async fn duplex_transport_rejects_oversize_frames_in_both_directions() {
    let (client, server) = tokio::io::duplex(4096);
    let mut transport = DuplexTransport::with_limit(client, 8);
    let err = transport
        .send(b"123456789")
        .await
        .expect_err("outbound over limit");
    assert!(format!("{err:?}").contains("exceeds limit"), "got {err:?}");

    let mut writer = server;
    writer
        .write_all(b"123456789\n")
        .await
        .expect("server write");
    writer.flush().await.expect("flush");
    let err = transport.recv().await.expect_err("inbound over limit");
    assert!(format!("{err:?}").contains("exceeds limit"), "got {err:?}");
}

// ------------------------------------------------------------
// 属性测试：任意 JSON 作为 params 往返不丢内容
// ------------------------------------------------------------

use proptest::prelude::*;

fn arb_json(depth: u32) -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|value| Value::Number(value.into())),
        "[a-zA-Z0-9 _.-]{0,8}".prop_map(Value::String),
    ];
    leaf.prop_recursive(depth, 16, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::hash_map("[a-z]{1,4}", inner, 0..4)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn prop_params_roundtrip(params in arb_json(3)) {
        let request = Request {
            jsonrpc: "2.0".into(),
            id: RequestId::Number(1),
            method: "tools/call".into(),
            params: Some(params.clone()),
            extra: Map::new(),
        };
        let frame = encode_frame(&request).expect("encode");
        let decoded = decode_incoming(&frame).expect("decode");
        match decoded {
            Incoming::Request(back) => {
                // null ≡ 缺省（见 jsonrpc::params_absent），故期望值也要归一。
                let expected = if params.is_null() { None } else { Some(params.clone()) };
                prop_assert_eq!(back.params, expected);
            }
            other => prop_assert!(false, "expected request, got {:?}", other),
        }
    }
}
