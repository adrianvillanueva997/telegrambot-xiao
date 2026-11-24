#[must_use]
pub fn is_mp4_url(url: &str) -> bool {
    std::path::Path::new(url)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"))
}

#[must_use]
pub fn is_webm_url(url: &str) -> bool {
    std::path::Path::new(url)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("webm"))
}
