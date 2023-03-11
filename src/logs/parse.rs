use crate::utils::*;
use console::style;
use anyhow::Result;
use regex::Regex;

lazy_static! {
    static ref SYMFONY_LOG_RE: Regex = Regex::new(
        r"(?x)^
        \[(?P<dt>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}.\d+[-\+]\d{2}:\d{2})\]
        (?:\s+(?P<level>\w+\.\w+):)
        (?:\s+\[(?P<class>[^\]]+)\])?
        (?:\s+(?P<msg>.+))
    $").unwrap();

    static ref NGINX_LOG_RE: Regex = Regex::new(
        r#"(?x)^
        "(?P<dt>\d{2}/\w{3}/\d{4}:\d{2}:\d{2}:\d{2}\s\+\d{4})"
        (?:\s+(?P<msg>.+))
    $"#).unwrap();

    static ref PROXY_LOG_RE: Regex = Regex::new( // nginx
        r"(?x)^
        (?P<ip>((25[0-5]|(2[0-4]|1\d|[1-9]|)\d)\.?\b){4})
        (?:\s+-)
        (?:\s+(?P<username>.+)?)
        (?:\s+\[?(?P<dt>\d{2}/\w{3}/\d{4}:\d{2}:\d{2}:\d{2}\s\+\d{4})\]?)
        (?:\s+(?P<msg>.+))
    $").unwrap();

    static ref PHPFPM_LOG_RE: Regex = Regex::new(
        r"(?x)^
        \[(?P<dt>\d{2}-\w{3}-\d{4}\s\d{2}:\d{2}:\d{2})\]
        (?:\s+(?P<lvl>\w+):)
        (?:\s+\[(?P<instance>.+)\])
        (?:\s+(?P<msg>.+))
    $").unwrap();

    static ref PHP_MESSAGE_LOG_RE: Regex = Regex::new(
        r"(?x)^
        NOTICE:\sPHP\smessage:
        (?:\s(?P<dt>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:.\d+)?[-\+]\d{2}:\d{2}))
        (?:\s\[(?P<lvl>\w+)\])
        (?:\s+(?P<msg>.+))
        $"
    ).unwrap();

    static ref FASTCGI_ERROR_LOG_RE: Regex = Regex::new("FastCGI sent in stderr").unwrap();
    static ref REPLACE: Regex = Regex::new(r"\s\s+").unwrap();
}

pub fn parse_log_str(str: &str) -> Result<()> {
    if let Some(cap) = SYMFONY_LOG_RE.captures(&str) {
        match &cap["level"] {
            "security.DEBUG" | "security.INFO" => {}
            _ => {
                // let mut msg = REPLACE.replace_all(&cap["msg"], " ").to_string();

                let dt = chrono::DateTime::parse_from_rfc3339(&cap["dt"])?;
                let ranges = find_json_ranges(&cap["msg"]).unwrap_or_default();
                let msg = make_json_colored(&cap["msg"], ranges);

                println!(
                    "[{timestamp}] {lvl}:{class} {msg}",
                    timestamp = style(format_datetime(&dt)).yellow(),
                    lvl = style(&cap["level"]).bold().bright().underlined(),
                    class = cap
                        .name("class")
                        .map(|class| format!(" [{}]", style(class.as_str()).magenta()))
                        .unwrap_or(Default::default()),
                );
            }
        }
    } else if let Some(cap) = PHP_MESSAGE_LOG_RE.captures(&str) {
        println!(
            "[{}] {} {}",
            style(&cap["dt"]).yellow(),
            style(&cap["lvl"]).bright().bold(),
            &cap["msg"]
        );
    } else if NGINX_LOG_RE.is_match(&str) {
        // ignore
    } else if PROXY_LOG_RE.is_match(&str) {
        // ignore
    } else if FASTCGI_ERROR_LOG_RE.is_match(&str) {
        // ignore
    } else if PHPFPM_LOG_RE.is_match(&str) {
        // ignore
    } else if !str.is_empty() {
        let ranges = find_json_ranges(str).unwrap_or_default();
        println!("{}↵", make_json_colored(str, ranges));
    }

    Ok(())
}
