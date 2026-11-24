use std::{env, str::FromStr};
use telegrambot_xiao::{
    commands::{self, Command},
    handle_messages,
};
use teloxide::{prelude::*, update_listeners::webhooks, utils::command::BotCommands};
use tracing::{error, info};
use url::Url;

mod telemetry;

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

    teloxide::repl_with_listener(
        bot,
        move |bot: Bot, msg: Message| {
            let username = username.clone();
            async move {
                if let Some(command) = msg
                    .text()
                    .and_then(|text| Command::parse(text, &username).ok())
                    && let Err(err) = commands::commands(bot.clone(), msg.clone(), command).await
                {
                    error!("Error processing command: {err}");
                }
                if let Err(err) = handle_messages(&bot, &msg).await {
                    error!("Error processing message: {err}");
                }
                Ok(())
            }
        },
        listener,
    )
    .await;
}
