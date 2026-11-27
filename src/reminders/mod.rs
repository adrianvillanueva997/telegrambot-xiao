mod holidays;

use chrono::{Datelike, NaiveDate, Timelike, Utc, Weekday};
use std::{collections::HashMap, future::Future, time::Duration};
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

async fn send(bot: &Bot, chat: ChatId, kind: &'static str, message: String) {
    match bot.send_message(chat, message).await {
        Ok(_) => info!(kind, "reminder sent"),
        Err(err) => error!(kind, %err, "reminder send failed"),
    }
}

#[derive(Default)]
struct Schedule {
    midnight_sent: Option<NaiveDate>,
    holiday_checked: Option<NaiveDate>,
}

impl Schedule {
    async fn tick<S, SF, H, HF>(
        &mut self,
        local: chrono::DateTime<chrono_tz::Tz>,
        events: &HashMap<String, HashMap<String, String>>,
        send: S,
        holidays: H,
    ) where
        S: Fn(&'static str, String) -> SF,
        SF: Future<Output = ()>,
        H: Fn(NaiveDate) -> HF,
        HF: Future<Output = ()>,
    {
        let date = local.date_naive();
        let (midnight, holiday_time) = due(local);
        if midnight && self.midnight_sent != Some(date) {
            self.midnight_sent = Some(date);
            if local.weekday() == Weekday::Thu {
                send("thursday", "Feliz jueves! 🐸".to_string()).await;
            }
            if let Some(event) = special_event(events, date) {
                send("special_date", event.to_string()).await;
            }
        }
        if holiday_time && self.holiday_checked != Some(date) {
            self.holiday_checked = Some(date);
            if let Some(tomorrow) = date.succ_opt() {
                holidays(tomorrow).await;
            }
        }
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
    let mut schedule = Schedule::default();
    info!(chat_id = chat.0, "reminder scheduler started");
    loop {
        let now = Utc::now();
        let delay = 60 - u64::from(now.second());
        tokio::time::sleep(Duration::from_secs(delay)).await;
        let local = Utc::now().with_timezone(&chrono_tz::Europe::Madrid);
        schedule.tick(local, &events, |kind, message| send(&bot, chat, kind, message), |tomorrow| {
            let client = &client;
            let bot = &bot;
            async move {
            for attempt in 1..=3 {
                match holidays::get(client, tomorrow).await {
                    Ok(Some(message)) => {
                        send(bot, chat, "holiday", message).await;
                        return;
                    }
                    Ok(None) => {
                        info!(date = %tomorrow, "holiday lookup found no reminders");
                        return;
                    }
                    Err(err) if attempt < 3 && (err.is_timeout() || err.is_connect() || err.status().is_some_and(|status| status.is_server_error() || status.as_u16() == 429)) => {
                        tracing::warn!(date = %tomorrow, attempt, %err, "holiday lookup failed; retrying");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                    Err(err) => {
                        error!(date = %tomorrow, attempt, %err, "holiday lookup failed");
                        return;
                    }
                }
            }
            }
        }).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[tokio::test]
    async fn scheduler_runs_due_jobs_once_per_day() {
        use std::cell::RefCell;
        let mut schedule = Schedule::default();
        let sent = RefCell::new(Vec::new());
        let checked = RefCell::new(Vec::new());
        let events = events();
        let madrid = chrono_tz::Europe::Madrid;
        let midnight = madrid.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let evening = madrid.with_ymd_and_hms(2026, 1, 1, 21, 0, 0).unwrap();
        for time in [midnight, midnight, evening, evening] {
            schedule
                .tick(
                    time,
                    &events,
                    |kind, _| {
                        sent.borrow_mut().push(kind);
                        async {}
                    },
                    |date| {
                        checked.borrow_mut().push(date);
                        async {}
                    },
                )
                .await;
        }
        assert_eq!(*sent.borrow(), ["thursday", "special_date"]);
        assert_eq!(
            *checked.borrow(),
            [NaiveDate::from_ymd_opt(2026, 1, 2).unwrap()]
        );
    }

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
