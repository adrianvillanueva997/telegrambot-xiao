use std::{env, future::Future, str::FromStr};
use telegrambot_xiao::{
    commands::{self, Command},
    handle_messages,
};
use teloxide::{prelude::*, update_listeners::webhooks, utils::command::BotCommands};
use tracing::{error, info};
use url::Url;

mod reminders;
mod telemetry;
mod transcription;

async fn dispatch<C, CF, M, MF>(text: Option<&str>, username: &str, command: C, message: M)
where
    C: FnOnce(Command) -> CF,
    CF: Future<Output = ()>,
    M: FnOnce() -> MF,
    MF: Future<Output = ()>,
{
    if let Some(parsed) = text.and_then(|text| Command::parse(text, username).ok()) {
        command(parsed).await;
    }
    message().await;
}

#[tokio::main]
async fn main() {
    telemetry::init();
    let bot = Bot::from_env();
    let username = bot
        .get_me()
        .await
        .expect("Could not get bot identity")
        .user
        .username
        .expect("Bot has no username");
    let addr = ([0, 0, 0, 0], 8080).into();
    let url =
        Url::from_str(&env::var("URL").expect("URL is not set")).expect("Invalid webhook URL");
    let listener = webhooks::axum(bot.clone(), webhooks::Options::new(addr, url.clone()))
        .await
        .expect("Could not set up webhook");
    info!("Running on {url} {addr}");
    if let Ok(chat) = env::var("REMINDERS_CHAT_ID")
        && !chat.is_empty()
    {
        let chat = ChatId(
            chat.parse()
                .expect("REMINDERS_CHAT_ID must be a numeric Telegram chat ID"),
        );
        tokio::spawn(reminders::run(bot.clone(), chat));
    }

    let transcriber = env::var("VOSK_MODEL_PATH")
        .ok()
        .filter(|path| !path.is_empty())
        .map(|_| transcription::Transcriber::start());

    teloxide::repl_with_listener(
        bot,
        move |bot: Bot, msg: Message| {
            let username = username.clone();
            let transcriber = transcriber.clone();
            async move {
                dispatch(
                    msg.text(),
                    &username,
                    |command| async {
                        if let Err(err) =
                            commands::commands(bot.clone(), msg.clone(), command).await
                        {
                            error!("Error processing command: {err}");
                        }
                    },
                    || async {
                        if let Err(err) = handle_messages(&bot, &msg).await {
                            error!("Error processing message: {err}");
                        }
                        if let Some(transcriber) = transcriber
                            && let Err(err) = transcriber.handle(&bot, &msg).await
                        {
                            error!("Error processing voice message: {err}");
                        }
                    },
                )
                .await;
                Ok(())
            }
        },
        listener,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::dispatch;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn routes_commands_and_all_messages() {
        for (text, expected_commands) in [
            (Some("/dota"), 1),
            (Some("hello"), 0),
            (None, 0),
            (Some("/dota@other_bot"), 0),
        ] {
            let commands = AtomicUsize::new(0);
            let messages = AtomicUsize::new(0);
            dispatch(
                text,
                "xiao_bot",
                |_| async {
                    commands.fetch_add(1, Ordering::Relaxed);
                },
                || async {
                    messages.fetch_add(1, Ordering::Relaxed);
                },
            )
            .await;
            assert_eq!(commands.load(Ordering::Relaxed), expected_commands);
            assert_eq!(messages.load(Ordering::Relaxed), 1);
        }
    }
}
