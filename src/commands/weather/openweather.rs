use std::{env, sync::OnceLock, time::Duration};

use super::model::OpenWeather;

#[derive(Debug, PartialEq, Eq)]
pub enum WeatherError {
    NotFound,
    Unavailable,
}

/// Retrieves the weather information for a given city.
///
/// # Errors
///
/// Returns `NotFound` if the city does not exist, or `Unavailable` if the
/// service is misconfigured, unreachable, or returns invalid data.
pub async fn get_weather(city: &str) -> Result<OpenWeather, WeatherError> {
    static CLIENT: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
    let key = env::var("openweather_key").map_err(|_| WeatherError::Unavailable)?;
    fetch_weather(
        CLIENT
            .get_or_init(|| {
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(10))
                    .build()
            })
            .as_ref()
            .map_err(|_| WeatherError::Unavailable)?,
        "https://api.openweathermap.org/data/2.5/weather",
        city,
        &key,
    )
    .await
}

async fn fetch_weather(
    client: &reqwest::Client,
    url: &str,
    city: &str,
    key: &str,
) -> Result<OpenWeather, WeatherError> {
    let mut url = url::Url::parse(url).map_err(|_| WeatherError::Unavailable)?;
    url.query_pairs_mut()
        .append_pair("q", city)
        .append_pair("appid", key)
        .append_pair("units", "metric");
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| WeatherError::Unavailable)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(WeatherError::NotFound);
    }
    let weather = response
        .error_for_status()
        .map_err(|_| WeatherError::Unavailable)?
        .json::<OpenWeather>()
        .await
        .map_err(|_| WeatherError::Unavailable)?;
    if weather.name.is_empty() || weather.weather.is_empty() {
        return Err(WeatherError::Unavailable);
    }
    Ok(weather)
}

#[cfg(test)]
mod tests {
    use super::{WeatherError, fetch_weather};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    async fn request(
        status: &str,
        body: &str,
        city: &str,
    ) -> (Result<super::OpenWeather, WeatherError>, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 4096];
            let n = stream.read(&mut buffer).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8(buffer[..n].to_vec()).unwrap()
        });
        let result = fetch_weather(
            &reqwest::Client::new(),
            &format!("http://{address}/weather"),
            city,
            "key&value",
        )
        .await;
        (result, server.join().unwrap())
    }

    const VALID: &str = r#"{"coord":{"lon":1.0,"lat":2.0},"weather":[{"id":1,"main":"Clear","description":"clear","icon":"01d"}],"base":"stations","main":{"temp":1.0,"feels_like":2.0,"temp_min":0.0,"temp_max":3.0,"pressure":1000,"humidity":50},"visibility":10000,"wind":{"speed":1.0,"deg":90},"clouds":{"all":0},"dt":1,"sys":{"type":1,"id":1,"country":"ES","sunrise":1,"sunset":2},"timezone":0,"id":1,"name":"Madrid","cod":200}"#;

    #[tokio::test]
    async fn fetches_and_encodes_query_parameters() {
        let (result, request) = request("200 OK", VALID, "A&B #ñ").await;
        assert_eq!(result.unwrap().name, "Madrid");
        assert!(
            request.starts_with("GET /weather?q=A%26B+%23%C3%B1&appid=key%26value&units=metric ")
        );
    }

    #[tokio::test]
    async fn handles_http_and_invalid_data() {
        for (status, body, expected) in [
            ("404 Not Found", "{}", WeatherError::NotFound),
            ("401 Unauthorized", "{}", WeatherError::Unavailable),
            ("500 Internal Server Error", "{}", WeatherError::Unavailable),
            ("200 OK", "not json", WeatherError::Unavailable),
            ("200 OK", "{}", WeatherError::Unavailable),
        ] {
            assert_eq!(
                request(status, body, "Madrid").await.0.unwrap_err(),
                expected
            );
        }
        let empty = VALID.replace(
            r#"[{"id":1,"main":"Clear","description":"clear","icon":"01d"}]"#,
            "[]",
        );
        assert_eq!(
            request("200 OK", &empty, "Madrid").await.0.unwrap_err(),
            WeatherError::Unavailable
        );
        let no_name = VALID.replace(r#""name":"Madrid""#, r#""name":"""#);
        assert_eq!(
            request("200 OK", &no_name, "Madrid").await.0.unwrap_err(),
            WeatherError::Unavailable
        );
    }

    #[tokio::test]
    async fn handles_connection_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        assert_eq!(
            fetch_weather(
                &reqwest::Client::new(),
                &format!("http://{address}"),
                "Madrid",
                "key"
            )
            .await
            .unwrap_err(),
            WeatherError::Unavailable
        );
    }
}
