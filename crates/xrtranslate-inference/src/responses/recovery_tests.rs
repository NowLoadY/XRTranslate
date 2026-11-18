use super::key::KeyResponses;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};
use xrtranslate_prompt::{PromptMessage, PromptMessageRole};
use xrtranslate_protocol::AgentTurn;

async fn read(socket: &mut WebSocketStream<TcpStream>) -> Value {
    let frame = socket.next().await.unwrap().unwrap();
    serde_json::from_str(frame.to_text().unwrap()).unwrap()
}

async fn send(socket: &mut WebSocketStream<TcpStream>, event: Value) {
    socket
        .send(Message::Text(event.to_string().into()))
        .await
        .unwrap();
}

async fn check_replay(dropped_socket: bool, compacted: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut socket = accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        let first = read(&mut socket).await;
        assert_eq!(first["type"], "response.create");
        assert_eq!(first["input"][0]["role"], "developer");
        assert_eq!(first["max_output_tokens"], 25_000);
        assert_eq!(first["context_management"][0]["compact_threshold"], 200_000);
        assert_eq!(first["store"], false);
        assert!(first.get("previous_response_id").is_none());
        let mut output =
            vec![json!({"type":"reasoning","encrypted_content":"fixture-reasoning","summary":[]})];
        if compacted {
            output.push(json!({"type":"compaction","encrypted_content":"fixture-compaction"}));
        }
        let function = json!({"type":"function_call","name":first["tools"][0]["name"],
            "arguments":r#"{"arguments":{},"memory":"read"}"#,"call_id":"call_first"});
        output.push(function.clone());
        send(
            &mut socket,
            json!({"type":"response.completed",
            "response":{"id":"resp_first","status":"completed","output":output}}),
        )
        .await;
        let incremental = read(&mut socket).await;
        let receipt = json!({"type":"function_call_output","call_id":"call_first","output":"actual local receipt"});
        assert_eq!(incremental["input"], json!([receipt]));
        assert_eq!(incremental["previous_response_id"], "resp_first");
        if dropped_socket {
            socket.close(None).await.unwrap();
            socket = accept_async(listener.accept().await.unwrap().0)
                .await
                .unwrap();
        } else {
            send(&mut socket, json!({"type":"error","error":{"code":"previous_response_not_found","message":"expired fixture"}})).await;
        }
        let replay = read(&mut socket).await;
        assert!(replay.get("previous_response_id").is_none());
        assert_eq!(replay["tools"], first["tools"]);
        let items = replay["input"].as_array().unwrap();
        if compacted {
            assert_eq!(items[0]["type"], "compaction");
            assert_eq!(
                items.len(),
                3,
                "only the suffix from the latest compaction is replayed"
            );
        } else {
            assert_eq!(items[..2], first["input"].as_array().unwrap()[..]);
            assert_eq!(items[2]["encrypted_content"], "fixture-reasoning");
        }
        assert_eq!(items[items.len() - 2], function);
        assert_eq!(items.last(), Some(&receipt));
        send(
            &mut socket,
            json!({"type":"response.completed","response":{
            "id":"resp_second","status":"completed","output":[{"type":"message","role":"assistant",
            "phase":"final_answer","content":[{"type":"output_text","text":"verified"}]}]}}),
        )
        .await;
    });
    let client = KeyResponses::new(&endpoint, "fixture").unwrap();
    let mut turn: AgentTurn = serde_json::from_value(json!({"session":"plugin","scope":{},
        "tools":[{"name":"document.read","description":"Read","read_only":true,"inputSchema":{"type":"object"}}]})).unwrap();
    let mut messages = vec![
        PromptMessage {
            role: PromptMessageRole::System,
            content: "stable".into(),
        },
        PromptMessage {
            role: PromptMessageRole::User,
            content: "read".into(),
        },
    ];
    let answer = client
        .agent("fixture", "medium", &messages, &turn, 1_050_000)
        .await
        .unwrap();
    messages.push(PromptMessage {
        role: PromptMessageRole::Assistant,
        content: answer.text,
    });
    turn.feedback_index = Some(messages.len());
    messages.push(PromptMessage {
        role: PromptMessageRole::User,
        content: "actual local receipt".into(),
    });
    let answer = client
        .agent("fixture", "medium", &messages, &turn, 1_050_000)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&answer.text).unwrap()["action"]["done"],
        "verified"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn replays_encrypted_items_when_a_response_id_expires() {
    check_replay(false, false).await;
}

#[tokio::test]
async fn reconnects_with_only_the_retained_history_after_compaction() {
    check_replay(true, true).await;
}
