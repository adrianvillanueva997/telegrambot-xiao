use teloxide::{Bot, prelude::Requester, types::Message};

use crate::{
    message_checks::{reddit, tiktok, twitter},
    utils::format_message_username,
};

pub async fn process(
    bot: &Bot,
    msg: &Message,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let content = if let Some(twitter) = twitter::update_twitter_links(text).await {
        Some(twitter)
    } else if let Some(tiktok) = tiktok::updated_tiktok(text).await {
        Some(tiktok)
    } else {
        reddit::updated_reddit(text).await
    };

    if let Some(content) = content {
        let formatted = format_message_username(msg, &content);
        bot.send_message(msg.chat.id, formatted).await?;
        bot.delete_message(msg.chat.id, msg.id).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };

    use teloxide::types::{
        Chat, ChatId, ChatKind, ChatPrivate, Message, MessageId, MessageKind, MessageNewChatTitle,
    };

    use super::process;

    async fn run_case(text: &str, responses: &[&str]) -> (bool, Vec<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let stopped = Arc::new(AtomicBool::new(false));
        let server_stopped = Arc::clone(&stopped);
        let responses: Vec<String> = responses.iter().map(ToString::to_string).collect();
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            while !server_stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut request = Vec::new();
                        let mut buffer = [0; 4096];
                        loop {
                            let count = stream.read(&mut buffer).unwrap();
                            assert_ne!(count, 0);
                            request.extend_from_slice(&buffer[..count]);
                            if let Some(header_end) =
                                request.windows(4).position(|w| w == b"\r\n\r\n")
                            {
                                let headers = String::from_utf8_lossy(&request[..header_end]);
                                let length: usize = headers
                                    .lines()
                                    .find_map(|line| {
                                        let (name, value) = line.split_once(':')?;
                                        name.eq_ignore_ascii_case("content-length")
                                            .then(|| value.trim().parse().unwrap())
                                    })
                                    .unwrap();
                                if request.len() >= header_end + 4 + length {
                                    break;
                                }
                            }
                        }
                        let response = responses.get(requests.len()).expect("unexpected request");
                        requests.push(String::from_utf8(request).unwrap());
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("mock server failed: {error}"),
                }
            }
            requests
        });
        let bot = teloxide::Bot::new("test-token").set_api_url(url.parse().unwrap());
        let message = Message {
            id: MessageId(42),
            thread_id: None,
            from: None,
            sender_chat: None,
            date: chrono::DateTime::from_timestamp(0, 0).unwrap(),
            chat: Chat {
                id: ChatId(123),
                kind: ChatKind::Private(ChatPrivate {
                    username: None,
                    first_name: None,
                    last_name: None,
                }),
            },
            is_topic_message: false,
            via_bot: None,
            sender_business_bot: None,
            kind: MessageKind::NewChatTitle(MessageNewChatTitle {
                new_chat_title: String::new(),
            }),
        };
        let result = process(&bot, &message, text).await.is_ok();
        stopped.store(true, Ordering::Relaxed);
        (result, server.join().unwrap())
    }

    const SEND_OK: &str = r#"{"ok":true,"result":{"message_id":43,"date":0,"chat":{"id":123,"type":"private"},"text":"sent"}}"#;
    const ERROR: &str = r#"{"ok":false,"error_code":400,"description":"Bad Request: failed"}"#;

    #[tokio::test]
    async fn sends_replacement_before_deleting_original() {
        let (ok, requests) = run_case(
            "https://twitter.com/user/status/123",
            &[SEND_OK, r#"{"ok":true,"result":true}"#],
        )
        .await;
        assert!(ok);
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0].starts_with("POST /bottest-token/SendMessage "),
            "{}",
            requests[0]
        );
        assert!(requests[0].contains("https://fxtwitter.com/user/status/123"));
        assert!(requests[0].contains("Anonymous User"));
        assert!(requests[1].starts_with("POST /bottest-token/DeleteMessage "));
        assert!(requests[1].contains("42"));
    }

    #[tokio::test]
    async fn send_failure_keeps_original() {
        let (ok, requests) = run_case("https://twitter.com/user/status/123", &[ERROR]).await;
        assert!(!ok);
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].starts_with("POST /bottest-token/SendMessage "),
            "{}",
            requests[0]
        );
    }

    #[tokio::test]
    async fn deletion_failure_is_returned_after_send() {
        let (ok, requests) =
            run_case("https://twitter.com/user/status/123", &[SEND_OK, ERROR]).await;
        assert!(!ok);
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0].starts_with("POST /bottest-token/SendMessage "),
            "{}",
            requests[0]
        );
        assert!(requests[1].starts_with("POST /bottest-token/DeleteMessage "));
    }

    #[tokio::test]
    async fn unchanged_text_makes_no_requests() {
        let (ok, requests) = run_case("no social links", &[]).await;
        assert!(ok);
        assert_eq!(requests, Vec::<String>::new());
    }
}
