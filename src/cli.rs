//! Command-line options.

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

/// Where an HTTP request spends its time: DNS, TCP, TLS, waiting and
/// transfer.
#[derive(Debug, Parser)]
#[command(version, about, max_term_width = 100)]
pub struct Args {
    /// The http:// or https:// URL to request.
    pub url: String,

    /// Make N requests and report percentiles for each phase.
    #[arg(short = 'n', long = "count", value_name = "N", default_value_t = 1,
          value_parser = clap::value_parser!(u32).range(1..))]
    pub count: u32,

    /// Keep one connection open across the -n requests, instead of a new
    /// one each time.
    #[arg(long)]
    pub reuse: bool,

    /// Use HTTP/1.1 only.
    #[arg(long, group = "protocol")]
    pub http1: bool,

    /// Use HTTP/2 only (https:// URLs).
    #[arg(long, group = "protocol")]
    pub http2: bool,

    /// Run over HTTP/1.1 and HTTP/2 and show them side by side.
    #[arg(long, group = "protocol")]
    pub compare: bool,

    /// Follow redirects, timing each hop.
    #[arg(short = 'L', long)]
    pub location: bool,

    /// With -L, also follow redirects to a different host.
    #[arg(long)]
    pub cross_host: bool,

    /// With -L, the most redirects to follow.
    #[arg(long, value_name = "N", default_value_t = 10)]
    pub max_redirects: u32,

    /// The request method.
    #[arg(
        short = 'X',
        long = "request",
        value_name = "METHOD",
        default_value = "GET"
    )]
    pub method: String,

    /// Add a request header, such as 'Accept: text/html'. Repeatable.
    #[arg(short = 'H', long = "header", value_name = "NAME: VALUE")]
    pub headers: Vec<String>,

    /// Give up on a request (with its redirects) after this long: 500ms,
    /// 10s, 1m, or a number of seconds.
    #[arg(long, value_name = "DURATION", default_value = "30s", value_parser = parse_duration)]
    pub timeout: Duration,

    /// Use ADDR for HOST:PORT instead of looking it up, like curl's
    /// --resolve. Repeatable; ADDR can be a comma-separated list.
    #[arg(long, value_name = "HOST:PORT:ADDR")]
    pub resolve: Vec<String>,

    /// Also trust the certificates in this PEM file.
    #[arg(long, value_name = "PATH")]
    pub cacert: Option<PathBuf>,

    /// Print JSON instead of a waterfall. Durations are in milliseconds.
    #[arg(long)]
    pub json: bool,

    /// Exit with 1 if time to first byte goes over DURATION.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    pub max_ttfb: Option<Duration>,

    /// Exit with 1 if the total time goes over DURATION.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    pub max_total: Option<Duration>,
}

/// `500ms`, `1.5s`, `2m`, or a bare number of seconds.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (number, scale) = if let Some(n) = s.strip_suffix("ms") {
        (n, 0.001)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1.0)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60.0)
    } else {
        (s, 1.0)
    };
    let value: f64 = number
        .trim()
        .parse()
        .map_err(|_| format!("{s:?} isn't a duration; try 500ms, 10s or 1m"))?;
    Duration::try_from_secs_f64(value * scale)
        .map_err(|_| format!("{s:?} isn't a duration; try 500ms, 10s or 1m"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(parse_duration("3"), Ok(Duration::from_secs(3)));
        assert!(parse_duration("fast").is_err());
        assert!(parse_duration("-1s").is_err());
    }

    #[test]
    fn options_are_consistent() {
        use clap::CommandFactory;
        Args::command().debug_assert();
    }
}
