mod holidays;

use chrono::{Datelike, NaiveDate, Timelike, Utc, Weekday};
use std::{collections::HashMap, time::Duration};
use teloxide::{prelude::*, types::ChatId};
use tracing::{error, info};

fn events() -> HashMap<String, HashMap<String, String>> {
    serde_json::from_str(include_str!("events.json")).expect("invalid bundled reminder events")
}

fn special_event(
    events: &HashMap<String, HashMap<String, String>>,
    date: NaiveDate,
) -> Option<&str> {
    events
        .get(&date.month().to_string())?
        .get(&date.day().to_string())
        .map(String::as_str)
}

fn due(local: chrono::DateTime<chrono_tz::Tz>) -> (bool, bool) {
    (
        local.hour() == 0 && local.minute() == 0,
        local.hour() == 21 && local.minute() == 0,
    )
}

async fn send(bot: &Bot, chat: ChatId, message: &str) {
    if let Err(err) = bot.send_message(chat, message).await {
        error!(%err, "reminder send failed");
    }
}

pub async fn run(bot: Bot, chat: ChatId) {
    let events = events();
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            error!(%err, "could not initialize reminder HTTP client");
            return;
        }
    };
    let mut midnight_sent = None;
    let mut holiday_checked = None;
    info!(chat_id = chat.0, "reminder scheduler started");
    loop {
        let now = Utc::now();
        let delay = 60 - u64::from(now.second());
        tokio::time::sleep(Duration::from_secs(delay)).await;
        let local = Utc::now().with_timezone(&chrono_tz::Europe::Madrid);
        let date = local.date_naive();
        let (midnight, holiday_time) = due(local);
        if midnight && midnight_sent != Some(date) {
            midnight_sent = Some(date);
            if local.weekday() == Weekday::Thu {
                send(&bot, chat, "Feliz jueves! 🐸").await;
            }
            if let Some(event) = special_event(&events, date) {
                send(&bot, chat, event).await;
            }
        }
        if holiday_time && holiday_checked != Some(date) {
            holiday_checked = Some(date);
            if let Some(tomorrow) = date.succ_opt() {
                match holidays::get(&client, tomorrow).await {
                    Ok(Some(message)) => send(&bot, chat, &message).await,
                    Ok(None) => (),
                    Err(err) => error!(%err, "holiday lookup failed"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn special_events_match_dates_and_preserve_content() {
        let events = events();
        assert!(
            special_event(&events, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap())
                .unwrap()
                .contains("Año nuevo")
        );
        assert!(
            special_event(&events, NaiveDate::from_ymd_opt(2026, 12, 28).unwrap())
                .unwrap()
                .contains('⣿')
        );
        assert_eq!(
            special_event(&events, NaiveDate::from_ymd_opt(2026, 1, 2).unwrap()),
            None
        );
    }

    #[test]
    fn madrid_local_date_handles_dst_and_new_year() {
        let utc = Utc.with_ymd_and_hms(2025, 12, 31, 23, 0, 0).unwrap();
        assert_eq!(
            utc.with_timezone(&chrono_tz::Europe::Madrid).date_naive(),
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()
        );
        let summer = Utc.with_ymd_and_hms(2026, 6, 1, 19, 0, 0).unwrap();
        assert_eq!(summer.with_timezone(&chrono_tz::Europe::Madrid).hour(), 21);
        assert_eq!(
            due(summer.with_timezone(&chrono_tz::Europe::Madrid)),
            (false, true)
        );
        assert_eq!(
            due(utc.with_timezone(&chrono_tz::Europe::Madrid)),
            (true, false)
        );
    }
}
