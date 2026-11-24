use std::{error::Error, io, net::IpAddr, path::Path, time::Duration};

use tokio::io::AsyncWriteExt;

pub const MAX_VIDEO_SIZE: u64 = 20_000_000;
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

type MediaResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Counts downloaded bytes without exceeding the media limit.
///
/// # Errors
/// Returns an error if the total exceeds the limit.
pub fn checked_size(size: u64, chunk_len: usize) -> io::Result<u64> {
    size.checked_add(chunk_len as u64)
        .filter(|&total| total <= MAX_VIDEO_SIZE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "media exceeds size limit"))
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        // Reject IPv4-mapped, local and special-purpose IPv6 ranges.
        IpAddr::V6(ip) => {
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && !(s[0] == 0x3fff && (s[1] & 0xf000) == 0)
        }
    }
}

fn redirect_target(base: &reqwest::Url, location: &str) -> Result<reqwest::Url, url::ParseError> {
    base.join(location)
}

/// Downloads a public HTTP(S) video with a strict size and time limit.
///
/// # Errors
/// Returns an error for unsafe URLs, network failures, or oversized content.
pub async fn download_video(url: &str, output_filename: impl AsRef<Path>) -> MediaResult<()> {
    tokio::time::timeout(DOWNLOAD_TIMEOUT, async {
        let mut url = reqwest::Url::parse(url)?;
        for redirect in 0..=5 {
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(
                    io::Error::new(io::ErrorKind::InvalidInput, "invalid media URL").into(),
                );
            }
            let host = url
                .host_str()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing host"))?;
            let port = url
                .port_or_known_default()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing port"))?;
            let addresses: Vec<_> = tokio::net::lookup_host((host.trim_matches(['[', ']']), port))
                .await?
                .collect();
            if addresses.is_empty() || addresses.iter().any(|addr| !public_ip(addr.ip())) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "media host is not public",
                )
                .into());
            }
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .resolve_to_addrs(host, &addresses)
                .build()?;
            let mut response = client.get(url.clone()).send().await?;
            if response.status().is_redirection() {
                if redirect == 5 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "too many media redirects",
                    )
                    .into());
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "redirect without location")
                    })?
                    .to_str()?;
                url = redirect_target(&url, location)?;
                continue;
            }
            response = response.error_for_status()?;
            if !response.status().is_success()
                || response
                    .content_length()
                    .is_some_and(|size| size > MAX_VIDEO_SIZE)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid or oversized media response",
                )
                .into());
            }
            let mut file = tokio::fs::File::create(output_filename).await?;
            let mut size = 0u64;
            while let Some(chunk) = response.chunk().await? {
                size = checked_size(size, chunk.len())?;
                file.write_all(&chunk).await?;
            }
            return Ok(());
        }
        unreachable!("redirect loop returns at the limit")
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_address_filter() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}");
        }
    }
    #[test]
    fn resolves_relative_and_absolute_redirects() {
        let base = reqwest::Url::parse("https://example.com/a/video.mp4").unwrap();
        assert_eq!(
            redirect_target(&base, "../other.mp4").unwrap().as_str(),
            "https://example.com/other.mp4"
        );
        assert_eq!(
            redirect_target(&base, "http://127.0.0.1/file")
                .unwrap()
                .host_str(),
            Some("127.0.0.1")
        );
        assert!(!public_ip("127.0.0.1".parse().unwrap()));
    }
    #[test]
    fn size_limit_counts_all_chunks() {
        assert_eq!(checked_size(MAX_VIDEO_SIZE - 1, 1).unwrap(), MAX_VIDEO_SIZE);
        assert!(checked_size(MAX_VIDEO_SIZE, 1).is_err());
        assert!(checked_size(u64::MAX, 1).is_err());
    }
    #[tokio::test]
    async fn rejects_local_and_invalid_urls() {
        for url in [
            "http://127.0.0.1/video.mp4",
            "file:///etc/passwd",
            "http://user@example.com/video.mp4",
        ] {
            assert!(download_video(url, "unused").await.is_err());
        }
    }
}
