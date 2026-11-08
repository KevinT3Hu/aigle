//! Adapt image-bearing Rig tool results to Chat Completions' user-only images.
use rig_agent::agent::{AgentHook, DispatchAction, DispatchEvent, HookContext};
use rig_core::{
    effect::EffectKind,
    message::{Message, ToolResultContent, UserContent},
};

pub struct UserImagePreviews;

/// Keep Rig's canonical transcript intact. Only the outgoing request is changed,
/// on every turn, so earlier previews remain visible during later refinements.
impl AgentHook for UserImagePreviews {
    async fn on_dispatch(&self, _: &HookContext, event: DispatchEvent<'_>) -> DispatchAction {
        let EffectKind::Completion { request, stream } = event.kind else {
            return DispatchAction::Proceed;
        };
        let mut request = request.clone();
        request.chat_history = user_image_previews(request.chat_history);
        DispatchAction::Patch(EffectKind::Completion {
            request,
            stream: *stream,
        })
    }
}

fn user_image_previews(messages: Vec<Message>) -> Vec<Message> {
    let mut output = Vec::with_capacity(messages.len());
    let mut previews = Vec::new();
    for message in messages {
        match message {
            Message::User { mut content }
                if content
                    .iter()
                    .all(|part| matches!(part, UserContent::ToolResult(_))) =>
            {
                for part in &mut content {
                    let UserContent::ToolResult(result) = part else {
                        unreachable!()
                    };
                    let mut text_result = Vec::new();
                    let mut images = Vec::new();
                    for block in std::mem::take(&mut result.content) {
                        match block {
                            ToolResultContent::Image(image) => images.push(image),
                            block => text_result.push(block),
                        }
                    }
                    if !images.is_empty() {
                        text_result.push(ToolResultContent::text(
                            "Rendered preview attached in the following user message.",
                        ));
                        previews.push(UserContent::text(format!(
                            "Rendered preview from tool {} (call {}). Inspect it against the user's request; refine the profile or finish.",
                            result.name.as_str(), result.call.wire(),
                        )));
                        previews.extend(images.into_iter().map(UserContent::Image));
                    }
                    result.content = text_result;
                }
                output.push(Message::User { content });
            }
            message => {
                if !previews.is_empty() {
                    output.push(Message::User {
                        content: std::mem::take(&mut previews),
                    });
                }
                output.push(message);
            }
        }
    }
    // Flush only after all adjacent tool replies, never between replies in a batch.
    if !previews.is_empty() {
        output.push(Message::User { content: previews });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_core::{
        completion::CompletionRequest,
        message::{
            AssistantContent, CallId, ImageMediaType, ToolCall, ToolFunction, ToolName, ToolResult,
        },
        providers::openai::OpenAIConfig,
        wire::{Body, Mode, Wire},
    };
    use serde_json::{Value, json};

    fn name(s: &str) -> ToolName {
        ToolName::new(s).unwrap()
    }
    fn call(id: &str) -> AssistantContent {
        AssistantContent::ToolCall(ToolCall::from_wire(
            id,
            ToolFunction::new(name("see_preview"), json!({})),
        ))
    }
    fn result(id: &str, image: bool) -> Message {
        let mut content = vec![ToolResultContent::text("Preview rendered")];
        if image {
            content.push(ToolResultContent::image_base64(
                "/9j/preview",
                Some(ImageMediaType::JPEG),
                None,
            ));
        }
        Message::User {
            content: vec![UserContent::ToolResult(ToolResult {
                call: CallId::from_wire(id),
                name: name("see_preview"),
                content,
            })],
        }
    }
    fn encode(messages: Vec<Message>) -> Value {
        let mut request = CompletionRequest::new("initial");
        request.chat_history = messages;
        let mut config = OpenAIConfig::new("test-key");
        config.base_url = "https://api.deepseek.com".into();
        let encoded = config
            .chat("deepseek-flash")
            .encode(request, Mode::Unary)
            .unwrap();
        let Body::Bytes(body) = encoded.request.into_body() else {
            panic!("expected JSON")
        };
        serde_json::from_slice(&body).unwrap()
    }
    #[test]
    fn wire_sends_preview_as_user_image_after_text_tool_acknowledgement() {
        let initial = Message::User {
            content: vec![
                UserContent::text("Brighten it"),
                UserContent::image_base64("/9j/original", Some(ImageMediaType::JPEG), None),
            ],
        };
        let messages = vec![
            initial,
            Message::Assistant {
                id: None,
                content: vec![call("preview-1")],
            },
            result("preview-1", true),
        ];
        let converted = user_image_previews(messages);
        // Reapplying the adapter must not duplicate previews on subsequent requests.
        assert_eq!(user_image_previews(converted.clone()), converted);
        let body = encode(converted);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|m| m["role"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["user", "assistant", "tool", "user"]
        );
        assert_eq!(
            messages[0]["content"][1]["image_url"]["url"],
            "data:image/jpeg;base64,/9j/original"
        );
        assert_eq!(messages[2]["tool_call_id"], "preview-1");
        assert!(messages[2]["content"].is_string());
        assert_eq!(
            messages[3]["content"][1]["image_url"]["url"],
            "data:image/jpeg;base64,/9j/preview"
        );
    }
    #[test]
    fn batched_replies_precede_previews_and_history_retains_prior_images() {
        let messages = vec![
            Message::user("Edit"),
            Message::Assistant {
                id: None,
                content: vec![call("a"), call("b")],
            },
            result("a", true),
            result("b", false),
            Message::assistant("Refine again"),
            Message::Assistant {
                id: None,
                content: vec![call("c")],
            },
            result("c", true),
        ];
        let body = encode(user_image_previews(messages));
        let messages = body["messages"].as_array().unwrap();
        let roles: Vec<_> = messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(
            roles,
            vec![
                "user",
                "assistant",
                "tool",
                "tool",
                "user",
                "assistant",
                "assistant",
                "tool",
                "user"
            ]
        );
        assert_eq!(messages[2]["tool_call_id"], "a");
        assert_eq!(messages[3]["tool_call_id"], "b");
        assert!(
            messages[4]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("call a")
        );
        assert!(
            messages[8]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("call c")
        );
        assert_eq!(
            messages
                .iter()
                .filter(|m| m["content"]
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url")))
                .count(),
            2
        );
    }
    #[test]
    fn rig_loop_applies_the_hook_before_encoding_the_preview_request() {
        use std::{
            io::{BufRead, BufReader, Read, Write},
            net::TcpListener,
            time::{Duration, Instant},
        };
        struct PreviewTool;
        impl rig_core::tool::PortableTool for PreviewTool {
            const NAME: &'static str = "see_preview";
            type Args = Value;
            type Output = rig_core::tool::ToolOutput;
            type Error = std::convert::Infallible;
            fn description(&self) -> String {
                "Render a preview".into()
            }
            fn parameters(&self) -> Value {
                json!({"type":"object","properties":{}})
            }
            async fn call(&self, _: Value) -> Result<Self::Output, Self::Error> {
                Ok(rig_core::tool::ToolOutput::one(
                    ToolResultContent::image_base64(
                        "/9j/preview",
                        Some(ImageMediaType::JPEG),
                        None,
                    ),
                ))
            }
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for step in 0..2 {
                let deadline = Instant::now() + Duration::from_secs(10);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(10))
                        }
                        Err(e) => panic!("waiting for model request: {e}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse::<usize>().unwrap();
                        }
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                requests.push(serde_json::from_slice::<Value>(&body).unwrap());
                let message = if step == 0 {
                    json!({"role":"assistant","content":null,"tool_calls":[{"id":"preview-1","type":"function","function":{"name":"see_preview","arguments":"{}"}}]})
                } else {
                    json!({"role":"assistant","content":"The preview matches."})
                };
                let body = json!({"id":"test-completion","object":"chat.completion","created":0,"model":"vision-test","choices":[{"index":0,"message":message,"finish_reason":if step == 0 {"tool_calls"} else {"stop"}}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}).to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
            requests
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let answer = runtime.block_on(async {
            let mut config = OpenAIConfig::new("test-key");
            config.base_url = format!("http://{addr}");
            let client = rig_reqwest::reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let provider = config.connect(rig_reqwest::ReqwestClient::from(client));
            rig_agent::AgentBuilder::new(provider.chat("vision-test"))
                .tool(PreviewTool)
                .add_hook(UserImagePreviews)
                .build()
                .prompt("Inspect a preview before finishing")
                .max_turns(2)
                .run()
                .await
        });
        assert_eq!(answer.unwrap().output(), "The preview matches.");
        let requests = server.join().unwrap();
        let messages = requests[1]["messages"].as_array().unwrap();
        let tool = messages.iter().find(|m| m["role"] == "tool").unwrap();
        assert_eq!(tool["tool_call_id"], "preview-1");
        assert!(tool["content"].is_string());
        let last = messages.last().unwrap();
        assert_eq!(last["role"], "user");
        assert_eq!(
            last["content"][1]["image_url"]["url"],
            "data:image/jpeg;base64,/9j/preview"
        );
    }

    #[test]
    fn text_only_messages_are_unchanged() {
        let messages = vec![
            Message::user("Edit"),
            Message::Assistant {
                id: None,
                content: vec![call("a")],
            },
            result("a", false),
        ];
        assert_eq!(user_image_previews(messages.clone()), messages);
    }
}
