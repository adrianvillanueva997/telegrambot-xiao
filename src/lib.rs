#![warn(clippy::pedantic)]

use futures_util::StreamExt;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use crate::error::BotError;
use message_checks::friday::fetch_friday_video;
use message_checks::{bad_words, webm};
use online_downloads::url_checker::{is_mp4_url, is_webm_url};
use online_downloads::video_downloader::{DOWNLOAD_TIMEOUT, checked_size, download_video};
use prank::day_check::is_prank_day;
use prank::randomizer::should_trigger;
use prank::reverse_words::upside_down_string;
use prank::sarcastic_agree::random_sarcastic_reply;
use std::error::Error;
use teloxide::Bot;
use teloxide::net::Download;
use teloxide::payloads::{SendMessageSetters, SendPhotoSetters, SendVideoSetters};
use teloxide::requests::Requester;
use teloxide::types::{Document, Message, ReplyParameters};
use tokio::time::sleep;
use tracing::instrument;

pub mod commands;
mod error;
pub mod message_checks;
pub mod online_downloads;
pub mod prank;
mod social_media_handler;
mod utils;

pub const PRANK_THRESHOLD: u32 = 10;
static MEDIA_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

type ProcessingResult = Result<(), Box<dyn Error + Send + Sync>>;

#[instrument]
async fn process_video_url(bot: &Bot, msg: &Message, url: &str, convert: bool) -> ProcessingResult {
    let _permit = MEDIA_SLOTS.acquire().await?;
    let dir = tempfile::tempdir()?;
    let input = dir
        .path()
        .join(if convert { "input.webm" } else { "input.mp4" });
    let output = dir.path().join("output.mp4");
    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::UploadVideo)
        .await?;
    if let Err(err) = download_video(url, &input).await {
        tracing::warn!(%err, "video download failed");
        bot.send_message(msg.chat.id, "El video no existe o no está disponible 😭")
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
        return Ok(());
    }
    let video = if convert {
        webm::convert_webm_to_mp4(&input, &output).await?;
        &output
    } else {
        &input
    };
    bot.send_video(msg.chat.id, teloxide::types::InputFile::file(video))
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;
    Ok(())
}

async fn process_urls(bot: &Bot, msg: &Message, text: &str) -> ProcessingResult {
    if message_checks::url::is_url(text) {
        social_media_handler::process(bot, msg, text).await?;
        if is_webm_url(text) {
            process_video_url(bot, msg, text, true).await?;
        } else if is_mp4_url(text) {
            process_video_url(bot, msg, text, false).await?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn process_text_messages(
    bot: &Bot,
    msg: &Message,
    text: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut actions: Vec<_> = Vec::new();
    process_urls(bot, msg, text).await?;
    if is_prank_day() && should_trigger(PRANK_THRESHOLD) {
        if should_trigger(15) {
            let reversed_message = upside_down_string(text);
            actions.push(
                bot.send_message(msg.chat.id, reversed_message)
                    .reply_parameters(ReplyParameters::new(msg.id)),
            );
        } else if should_trigger(70) {
            let reply = random_sarcastic_reply();
            actions.push(
                bot.send_message(msg.chat.id, reply)
                    .reply_parameters(ReplyParameters::new(msg.id)),
            );
        } else if let Some((caption, image)) = prank::mario::fetch_random_image() {
            bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::UploadPhoto)
                .await?;
            bot.send_photo(msg.chat.id, image)
                .reply_parameters(ReplyParameters::new(msg.id))
                .caption(caption)
                .await?;
        }
    }
    let message = text.to_lowercase();
    if bad_words::find_bad_words(&message).await {
        actions.push(
            bot.send_message(msg.chat.id, "Deficiente")
                .reply_parameters(ReplyParameters::new(msg.id)),
        );
    }
    let copypastas = message_checks::copypasta::find_copypasta(&message).await;
    for copypasta in copypastas {
        if copypasta.trigger == "viernes" {
            bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::UploadVideo)
                .await?;
            let video = fetch_friday_video().ok_or("Friday video asset is missing")?;
            bot.send_video(msg.chat.id, video)
                .reply_parameters(ReplyParameters::new(msg.id))
                .await?;
        } else {
            actions.push(
                bot.send_message(msg.chat.id, copypasta.response)
                    .reply_parameters(ReplyParameters::new(msg.id)),
            );
        }
    }
    if let Some(happy_thursday) = message_checks::thursday::check_thursday(&message) {
        actions.push(
            bot.send_message(msg.chat.id, happy_thursday)
                .reply_parameters(ReplyParameters::new(msg.id)),
        );
    }
    if !actions.is_empty() {
        bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
            .await?;
        for action in actions {
            action.await?;
            sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(())
}

#[allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]
#[instrument]
pub async fn process_files(
    bot: &Bot,
    msg: &Message,
    file_to_read: &Document,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if file_to_read
        .file_name
        .as_deref()
        .is_some_and(|name| name.contains("webm"))
        && file_to_read.file.size <= 20_000_000
    {
        let _permit = MEDIA_SLOTS.acquire().await?;
        let dir = tempfile::tempdir()?;
        let input = dir.path().join("input.webm");
        let output = dir.path().join("output.mp4");
        bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::UploadVideo)
            .await?;
        let file =
            tokio::time::timeout(DOWNLOAD_TIMEOUT, bot.get_file(file_to_read.file.id.clone()))
                .await??;
        tokio::time::timeout(DOWNLOAD_TIMEOUT, async {
            let mut dst = tokio::fs::File::create(&input).await?;
            let stream = bot.download_file_stream(&file.path);
            futures_util::pin_mut!(stream);
            let mut size = 0u64;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                size = checked_size(size, chunk.len())?;
                dst.write_all(&chunk).await?;
            }
            Ok::<(), Box<dyn Error + Send + Sync>>(())
        })
        .await??;
        webm::convert_webm_to_mp4(&input, &output).await?;
        bot.send_video(msg.chat.id, teloxide::types::InputFile::file(&output))
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
    }
    Ok(())
}

#[allow(clippy::missing_errors_doc)]
#[instrument]
pub async fn handle_messages(bot: &Bot, msg: &Message) -> Result<(), BotError> {
    if let Some(text) = msg.text() {
        process_text_messages(bot, msg, text)
            .await
            .map_err(|e| BotError::Processing(e.to_string()))?;
    } else if let Some(doc) = msg.document() {
        process_files(bot, msg, doc)
            .await
            .map_err(|e| BotError::Processing(e.to_string()))?;
    }
    Ok(())
}
