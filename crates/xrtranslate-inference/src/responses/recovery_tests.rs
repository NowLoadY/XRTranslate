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
        let output = if dropped_socket {
            json!([{"type":"input_text","text":"actual local receipt"},
                {"type":"input_image","image_url":"data:image/jpeg;base64,/9j/"}])
        } else {
            json!("actual local receipt")
        };
        let receipt = json!({"type":"function_call_output","call_id":"call_first","output":output});
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
        .agent("fixture", Some("medium"), &messages, &turn, 1_050_000)
        .await
        .unwrap();
    messages.push(PromptMessage {
        role: PromptMessageRole::Assistant,
        content: answer.text,
    });
    turn.feedback_jpeg = dropped_socket.then(|| "/9j/".into());
    turn.feedback_index = Some(messages.len());
    messages.push(PromptMessage {
        role: PromptMessageRole::User,
        content: "actual local receipt".into(),
    });
    let answer = client
        .agent("fixture", Some("medium"), &messages, &turn, 1_050_000)
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

fn read_http_request(socket: &std::net::TcpStream) -> (Value, bool) {
    use std::io::{BufRead, BufReader, Read};
    let mut reader = BufReader::new(socket);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.starts_with("POST /responses "));
    let mut len = 0;
    let mut session_cache = false;
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line.trim().is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        if name.eq_ignore_ascii_case("content-length") {
            len = value.trim().parse().unwrap();
        }
        if name.eq_ignore_ascii_case("x-dashscope-session-cache") {
            session_cache = value.trim() == "enable";
        }
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body).unwrap();
    (serde_json::from_slice(&body).unwrap(), session_cache)
}

fn send_http(mut socket: std::net::TcpStream, status: &str, mime: &str, body: &str) {
    use std::io::Write;
    write!(socket,"HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
}

#[tokio::test]
async fn dashscope_retries_only_an_expired_parent_with_the_accepted_receipt() {
    use xrtranslate_assets::remote::AgentApi;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let call = json!({"type":"function_call","name":"document_read",
            "arguments":r#"{"arguments":{},"memory":"reading"}"#,"call_id":"call_1"});
        let receipt =
            json!({"type":"function_call_output","call_id":"call_1","output":"actual receipt"});
        for step in 0..3 {
            let (socket, _) = listener.accept().unwrap();
            let (req, cache) = read_http_request(&socket);
            assert!(cache);
            assert_eq!(req["stream"], true);
            assert_eq!(req["store"], true);
            assert_eq!(req["reasoning"]["effort"], "medium");
            assert!(req.get("context_management").is_none());
            assert!(req.get("prompt_cache_options").is_none());
            let (status, mime, body) = if step == 1 {
                assert_eq!(req["previous_response_id"], "resp_1");
                assert_eq!(req["input"], json!([receipt]));
                ("400 Bad Request","application/json",json!({"error":{"code":"InvalidParameter","message":"Not found previous_response_id: resp_1."}}).to_string())
            } else {
                assert!(req.get("previous_response_id").is_none());
                assert_eq!(req["input"][0]["role"], "developer");
                let output = if step == 0 {
                    json!([{"type":"reasoning","content":[{"text":"private fixture reasoning"}]},call])
                } else {
                    assert_eq!(req["input"].as_array().unwrap().len(), 4);
                    assert_eq!(req["input"][2], call);
                    assert_eq!(req["input"][3], receipt);
                    json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"verified"}]}])
                };
                let event = json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":output}});
                ("200 OK", "text/event-stream", format!("data: {event}\n\n"))
            };
            send_http(socket, status, mime, &body);
        }
    });
    let client = KeyResponses::for_api(AgentApi::DashscopeResponses, &endpoint, "fixture").unwrap();
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
        .agent("qwen3.8-flash", Some("medium"), &messages, &turn, 1_000_000)
        .await
        .unwrap();
    messages.push(PromptMessage {
        role: PromptMessageRole::Assistant,
        content: answer.text,
    });
    turn.feedback_index = Some(messages.len());
    messages.push(PromptMessage {
        role: PromptMessageRole::User,
        content: "actual receipt".into(),
    });
    let answer = client
        .agent("qwen3.8-flash", Some("medium"), &messages, &turn, 1_000_000)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&answer.text).unwrap()["action"]["done"],
        "verified"
    );
    server.join().unwrap();
}

#[tokio::test]
async fn deepseek_replays_reasoning_and_pairs_every_call_then_rebases_at_the_window_limit() {
    use xrtranslate_assets::remote::AgentApi;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let reasoning = json!({"type":"reasoning","content":[{"type":"reasoning_text","text":"private fixture reasoning"}]});
        let call = |id, name| json!({"type":"function_call","call_id":id,"name":name,"arguments":r#"{"arguments":{},"memory":"checking"}"#});
        let batch = json!([
            reasoning,
            call("fc1", "document_read"),
            call("fc2", "document_write")
        ]);
        let initial = json!([{"role":"system","content":"stable rules"},{"role":"user","content":"read the document"}]);
        let mut tools = Value::Null;
        for step in 0..3 {
            let (socket, _) = listener.accept().unwrap();
            let (req, session_cache) = read_http_request(&socket);
            assert!(!session_cache);
            assert_eq!(req["model"], "deepseek-v4-pro");
            assert_eq!(req["stream"], true);
            assert_eq!(req["reasoning"], json!({"effort":"high"}));
            assert_eq!(req["max_output_tokens"], 65_536);
            for unsupported in [
                "previous_response_id",
                "store",
                "parallel_tool_calls",
                "include",
                "context_management",
                "prompt_cache_options",
                "prompt_cache_key",
            ] {
                assert!(req.get(unsupported).is_none(), "{unsupported}");
            }
            let input = req["input"].as_array().unwrap();
            if step == 0 {
                assert_eq!(req["input"], initial);
                tools = req["tools"].clone();
                assert_eq!(tools[0]["name"], "document_read");
            } else {
                assert_eq!(req["tools"], tools);
                assert_eq!(input[..2], initial.as_array().unwrap()[..]);
            }
            if step == 1 {
                assert_eq!(input[2..5], batch.as_array().unwrap()[..]);
                assert_eq!(
                    input[5],
                    json!({"type":"function_call_output","call_id":"fc1","output":"actual receipt 73"})
                );
                assert_eq!(input[6]["call_id"], "fc2");
                assert_eq!(input[6]["type"], "function_call_output");
                assert!(
                    input[6]["output"]
                        .as_str()
                        .unwrap()
                        .starts_with("Not executed.")
                );
                assert_eq!(
                    input[7],
                    json!({"role":"user","content":"also check the title"})
                );
            }
            if step == 2 {
                assert!(
                    input
                        .iter()
                        .all(|item| matches!(item["role"].as_str(), Some("system" | "user")))
                );
                assert!(
                    input
                        .iter()
                        .any(|item| item["content"] == "actual receipt 73")
                );
                assert!(
                    !req["input"]
                        .to_string()
                        .contains("private fixture reasoning")
                );
            }
            let output = if step == 0 {
                batch.clone()
            } else {
                json!([{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"73 confirmed"}]}])
            };
            let total = if step == 1 { 982_782 } else { 613 };
            let event = json!({"type":"response.completed","response":{"id":format!("r{step}"),"status":"completed","output":output,
                "usage":{"total_tokens":total,"input_tokens_details":{"cached_tokens":256}}}});
            send_http(
                socket,
                "200 OK",
                "text/event-stream",
                &format!("event: response.completed\ndata: {event}\n\n"),
            );
        }
    });
    let client = KeyResponses::for_api(AgentApi::DeepseekResponses, &endpoint, "fixture").unwrap();
    let mut turn: AgentTurn = serde_json::from_value(json!({"session":"test","scope":{},"tools":[
        {"name":"document.write","description":"Write the document","read_only":false,"inputSchema":{"type":"object"}},
        {"name":"document.read","description":"Read the document","read_only":true,"inputSchema":{"type":"object"}}]})).unwrap();
    let msg = |role, text: &str| PromptMessage {
        role,
        content: text.into(),
    };
    let mut transcript = vec![
        msg(PromptMessageRole::System, "stable rules"),
        msg(PromptMessageRole::User, "read the document"),
    ];
    let first = client
        .agent("deepseek-v4-pro", None, &transcript, &turn, 1_048_576)
        .await
        .unwrap();
    let accepted: Value = serde_json::from_str(&first.text).unwrap();
    assert_eq!(accepted["action"]["tool"]["name"], "document.read");
    assert!(!first.text.contains("private fixture reasoning"));
    transcript.push(msg(PromptMessageRole::Assistant, &first.text));
    transcript.push(msg(PromptMessageRole::User, "also check the title"));
    turn.feedback_index = Some(transcript.len());
    transcript.push(msg(PromptMessageRole::User, "actual receipt 73"));
    let second = client
        .agent("deepseek-v4-pro", None, &transcript, &turn, 1_048_576)
        .await
        .unwrap();
    assert_eq!(second.usage.unwrap().cached_input_tokens, Some(256));
    transcript.push(msg(PromptMessageRole::Assistant, &second.text));
    transcript.push(msg(PromptMessageRole::User, "repeat the reading"));
    turn.feedback_index = None;
    let third = client
        .agent("deepseek-v4-pro", None, &transcript, &turn, 1_048_576)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&third.text).unwrap()["action"]["done"],
        "73 confirmed"
    );
    server.join().unwrap();
}
