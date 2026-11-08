//! OpenAI-compatible model discovery shared by the desktop frontends.
pub fn fetch_models(endpoint: String, api_key: String) -> anyhow::Result<Vec<String>> {
    let base = crate::agent::endpoint_base(&endpoint)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let http = rig_reqwest::reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        let mut config = rig_core::providers::openai::OpenAIConfig::new(api_key);
        config.base_url = base;
        let list = config
            .connect(rig_reqwest::ReqwestClient::from(http))
            .list_models()
            .await?;
        let mut models: Vec<_> = list
            .data
            .into_iter()
            .map(|m| m.id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect();
        models.sort();
        models.dedup();
        Ok(models)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovers_models_using_rig_with_provider_endpoint_and_key() {
        use std::{
            io::{BufRead, BufReader, Write},
            net::TcpListener,
            time::Duration,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) => panic!("model discovery request: {e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                headers.push(line);
            }
            assert!(headers[0].starts_with("GET /v1/models "));
            assert!(headers.iter().any(|s| {
                s.to_ascii_lowercase()
                    .starts_with("authorization: bearer test-key")
            }));
            let body = r#"{"object":"list","data":[{"id":"vision-b"},{"id":"vision-a"},{"id":"vision-b"}],"has_more":false}"#;
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let models = fetch_models(
            format!("http://{address}/v1/chat/completions"),
            "test-key".into(),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(models, vec!["vision-a", "vision-b"]);
    }
}
