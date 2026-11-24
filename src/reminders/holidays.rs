use chrono::{Datelike, NaiveDate};
use serde::Deserialize;

#[derive(Deserialize)]
struct Text {
    text: String,
}

#[derive(Deserialize)]
struct Region {
    code: String,
    name: Vec<Text>,
    #[serde(default)]
    children: Vec<Region>,
}

#[derive(Deserialize)]
struct Subdivision {
    code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Holiday {
    start_date: NaiveDate,
    name: Vec<Text>,
    nationwide: bool,
    #[serde(default)]
    subdivisions: Vec<Subdivision>,
}

fn flatten<'a>(regions: &'a [Region], output: &mut Vec<&'a Region>) {
    for region in regions {
        output.push(region);
        flatten(&region.children, output);
    }
}

fn escape(text: &str) -> String {
    let mut output = String::new();
    for c in text.chars() {
        if "_*[]()~`>#+-=|{}.!\\".contains(c) {
            output.push('\\');
        }
        output.push(c);
    }
    output
}

fn format_holidays(date: NaiveDate, regions: &[Region], holidays: &[Holiday]) -> Option<String> {
    let mut flat = Vec::new();
    flatten(regions, &mut flat);
    let mut messages = Vec::new();
    for holiday in holidays.iter().filter(|h| h.start_date == date) {
        let Some(name) = holiday.name.first() else {
            continue;
        };
        let mut chars = name.text.chars();
        let name = escape(&format!(
            "{}{}",
            chars
                .next()
                .map_or_else(String::new, |c| c.to_uppercase().to_string()),
            chars.as_str()
        ));
        let suffix = if holiday.nationwide {
            String::new()
        } else {
            let names: Vec<_> = flat
                .iter()
                .filter(|r| holiday.subdivisions.iter().any(|s| s.code == r.code))
                .filter_map(|r| r.name.first())
                .map(|n| format!("_{}_", escape(&n.text)))
                .collect();
            if names.is_empty() {
                continue;
            }
            let list = if names.len() == 1 {
                names[0].clone()
            } else {
                format!(
                    "{} y {}",
                    names[..names.len() - 1].join(", "),
                    names[names.len() - 1]
                )
            };
            format!(" en {list}")
        };
        messages.push(format!(
            "📅 *Mañana es {name}*{suffix}\n\n🔔 Acuérdate de quitar la alarma 🙏"
        ));
    }
    (!messages.is_empty()).then(|| messages.join("\n\n"))
}

pub async fn get(
    client: &reqwest::Client,
    date: NaiveDate,
) -> Result<Option<String>, reqwest::Error> {
    let regions = client
        .get("https://openholidaysapi.org/Subdivisions?countryIsoCode=ES&languageIsoCode=ES")
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<Region>>()
        .await?;
    let year = date.year();
    let holidays = client
        .get(format!("https://openholidaysapi.org/PublicHolidays?countryIsoCode=ES&languageIsoCode=ES&validFrom={year}-01-01&validTo={year}-12-31"))
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<Holiday>>()
        .await?;
    Ok(format_holidays(date, &regions, &holidays))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_new_year_and_escapes_formatting() {
        let holidays: Vec<Holiday> = serde_json::from_str(
            r#"[{"startDate":"2026-01-01","name":[{"text":"año nuevo!"}],"nationwide":true}]"#,
        )
        .unwrap();
        let tomorrow = NaiveDate::from_ymd_opt(2025, 12, 31)
            .unwrap()
            .succ_opt()
            .unwrap();
        assert_eq!(
            format_holidays(tomorrow, &[], &holidays).unwrap(),
            "📅 *Mañana es Año nuevo\\!*\n\n🔔 Acuérdate de quitar la alarma 🙏"
        );
    }

    #[test]
    fn formats_nationwide_and_nested_regions_and_ignores_unknown() {
        let regions: Vec<Region> = serde_json::from_str(r#"[{"code":"ES-AN","name":[{"text":"Andalucía"}],"children":[{"code":"ES-MA","name":[{"text":"Málaga"}]}]},{"code":"ES-AR","name":[{"text":"Aragón"}]}]"#).unwrap();
        let holidays: Vec<Holiday> = serde_json::from_str(r#"[{"startDate":"2026-06-09","name":[{"text":"fiesta uno"}],"nationwide":false,"subdivisions":[{"code":"ES-MA"}]},{"startDate":"2026-06-09","name":[{"text":"fiesta dos"}],"nationwide":false,"subdivisions":[{"code":"ES-AR"},{"code":"ES-AN"}]},{"startDate":"2026-06-09","name":[{"text":"ignorada"}],"nationwide":false,"subdivisions":[{"code":"unknown"}]},{"startDate":"2026-06-09","name":[{"text":"año nuevo"}],"nationwide":true}]"#).unwrap();
        let result = format_holidays(
            NaiveDate::from_ymd_opt(2026, 6, 9).unwrap(),
            &regions,
            &holidays,
        )
        .unwrap();
        assert!(result.contains("*Mañana es Fiesta uno* en _Málaga_"));
        assert!(result.contains("*Mañana es Fiesta dos* en _Andalucía_ y _Aragón_"));
        assert!(result.contains("*Mañana es Año nuevo*"));
        assert!(!result.contains("ignorada"));
    }
}
