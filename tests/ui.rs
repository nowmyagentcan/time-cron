const PAGE: &str = include_str!("../src/ui.html");

#[test]
fn page_reaches_no_other_host() {
    let allowed = [
        "https://nowmyagentcan.com",
        "https://github.com/nowmyagentcan/time-cron",
        "https://x.com/chayp05",
        "http://www.w3.org/2000/svg",
    ];
    for (i, _) in PAGE.match_indices("://") {
        let start = PAGE[..i]
            .rfind(|c: char| !c.is_ascii_alphanumeric())
            .map_or(0, |p| p + 1);
        let url = &PAGE[start..];
        assert!(
            allowed.iter().any(|a| url.starts_with(a)),
            "unexpected URL: {}",
            &url[..url.len().min(60)]
        );
    }
    for loader in ["<script src", "<link rel=\"stylesheet\"", "@import", "url("] {
        assert!(!PAGE.contains(loader), "page loads a file: {loader}");
    }
}

#[test]
fn page_has_the_title_and_one_button() {
    assert!(PAGE.contains("<title>Explain a cron schedule</title>"));
    assert!(PAGE.contains("<button class=\"go\">Explain</button>"));
}
