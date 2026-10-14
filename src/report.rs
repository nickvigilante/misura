//! Output: the human waterfall and tables, and JSON.

use std::io::{self, Write};
use std::time::Duration;

use anstyle::{AnsiColor, Style};
use serde::Serialize;

use crate::measure::{Outcome, Phases, Run, round_ms};
use crate::stats::{self, Summary};

/// Bumped when a field changes meaning or goes away. New fields don't bump
/// it.
const JSON_VERSION: u32 = 1;

/// Width of the waterfall, in characters.
const BAR: usize = 40;

/// The runs for one protocol choice.
#[derive(Debug, Serialize)]
pub struct Series {
    /// `auto`, `http1` or `http2`.
    pub protocol: &'static str,
    pub runs: Vec<Run>,
    pub summary: Spread,
}

impl Series {
    pub fn new(protocol: &'static str, runs: Vec<Run>) -> Self {
        let summary = Spread::of(&runs);
        Self {
            protocol,
            runs,
            summary,
        }
    }
}

/// Percentiles for each phase across a series' runs. A connection phase is
/// summarized over the runs that opened a connection, so with `--reuse` it
/// describes the first request.
#[derive(Debug, Serialize)]
pub struct Spread {
    pub dns: Option<Summary>,
    pub tcp: Option<Summary>,
    pub tls: Option<Summary>,
    pub wait: Option<Summary>,
    pub transfer: Option<Summary>,
    pub ttfb: Option<Summary>,
    pub total: Option<Summary>,
}

impl Spread {
    fn of(runs: &[Run]) -> Self {
        let pick = |f: fn(&Phases) -> Option<Duration>| {
            stats::summarize(runs.iter().filter_map(|r| f(&r.phases)).collect())
        };
        Self {
            dns: pick(|p| p.dns),
            tcp: pick(|p| p.tcp),
            tls: pick(|p| p.tls),
            wait: pick(|p| Some(p.wait)),
            transfer: pick(|p| Some(p.transfer)),
            ttfb: pick(|p| Some(p.ttfb)),
            total: pick(|p| Some(p.total)),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub version: u32,
    pub method: String,
    pub url: String,
    pub reuse: bool,
    pub results: Vec<Series>,
    /// Whether misura exits with 1: an error status, or a threshold exceeded.
    pub failed: bool,
    /// Why, one sentence each.
    pub reasons: Vec<String>,
}

impl Report {
    pub fn new(
        method: String,
        url: String,
        reuse: bool,
        results: Vec<Series>,
        reasons: Vec<String>,
    ) -> Self {
        Self {
            version: JSON_VERSION,
            method,
            url,
            reuse,
            results,
            failed: !reasons.is_empty(),
            reasons,
        }
    }
}

pub fn json(report: &Report, out: &mut impl Write) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, report)?;
    writeln!(out)
}

/// The JSON printed when misura can't produce a report.
pub fn json_error(message: &str, out: &mut impl Write) -> io::Result<()> {
    let value = serde_json::json!({ "version": JSON_VERSION, "error": message });
    serde_json::to_writer_pretty(&mut *out, &value)?;
    writeln!(out)
}

const DIM: Style = Style::new().dimmed();
const BOLD: Style = Style::new().bold();
const RED: Style = AnsiColor::Red.on_default();

fn color(c: AnsiColor) -> Style {
    Style::new().fg_color(Some(c.into()))
}

fn status_style(status: u16) -> Style {
    match status {
        200..=299 => color(AnsiColor::Green).bold(),
        300..=399 => color(AnsiColor::Cyan).bold(),
        _ => color(AnsiColor::Red).bold(),
    }
}

fn status_text(status: u16) -> String {
    match hyper::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
    {
        Some(reason) => format!("{status} {reason}"),
        None => status.to_string(),
    }
}

fn bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    let f = n as f64;
    if f < KIB {
        format!("{n} B")
    } else if f < KIB * KIB {
        format!("{:.1} KiB", f / KIB)
    } else if f < KIB * KIB * KIB {
        format!("{:.1} MiB", f / KIB / KIB)
    } else {
        format!("{:.1} GiB", f / KIB / KIB / KIB)
    }
}

fn ms(d: Duration) -> String {
    format!("{:.2}", round_ms(d))
}

/// The phases in order, with their label and colour.
const PHASES: [(&str, AnsiColor); 5] = [
    ("DNS lookup", AnsiColor::Cyan),
    ("TCP connect", AnsiColor::Green),
    ("TLS handshake", AnsiColor::Yellow),
    ("Server wait", AnsiColor::Magenta),
    ("Transfer", AnsiColor::Blue),
];

fn header(out: &mut impl Write, method: &str, hop: &Outcome) -> io::Result<()> {
    write!(
        out,
        "{BOLD}{method} {}{BOLD:#} {DIM}→{DIM:#} {}",
        hop.url, hop.peer
    )?;
    if hop.reused_connection {
        write!(out, " {DIM}(reused connection){DIM:#}")?;
    } else if hop.connect_attempts > 1 {
        write!(
            out,
            " {DIM}(after trying {} addresses){DIM:#}",
            hop.connect_attempts
        )?;
    }
    writeln!(out)?;
    let mut parts = vec![hop.protocol.to_owned()];
    if let Some(v) = &hop.tls_version {
        parts.push(v.clone());
    }
    if let Some(c) = &hop.tls_cipher {
        parts.push(c.clone());
    }
    let s = status_style(hop.status);
    write!(
        out,
        "{} · {s}{}{s:#} · {}",
        parts.join(" · "),
        status_text(hop.status),
        bytes(hop.body_bytes)
    )?;
    if let Some(location) = &hop.location {
        write!(out, " · {DIM}Location:{DIM:#} {location}")?;
    }
    writeln!(out)
}

fn waterfall(out: &mut impl Write, hop: &Outcome) -> io::Result<()> {
    let p = &hop.phases;
    let rows = [
        (p.dns, true),
        (p.tcp, true),
        (p.tls, hop.tls_version.is_some()),
        (Some(p.wait), true),
        (Some(p.transfer), true),
    ];
    let scale = BAR as f64 / p.total.as_secs_f64().max(f64::MIN_POSITIVE);
    let col = |d: Duration| ((d.as_secs_f64() * scale).round() as usize).min(BAR);
    let mut elapsed = Duration::ZERO;
    for ((label, c), (value, applies)) in PHASES.iter().zip(rows) {
        if !applies {
            continue;
        }
        write!(out, "  {label:<15}")?;
        let Some(d) = value else {
            // DNS is the only phase that can be missing on a new
            // connection: an IP address or --resolve needs no lookup.
            let why = if hop.reused_connection {
                "reused"
            } else {
                "no lookup"
            };
            writeln!(out, "{DIM}{why:>12}{DIM:#}")?;
            continue;
        };
        let (from, to) = (col(elapsed), col(elapsed + d));
        elapsed += d;
        let bar = if to > from {
            "█".repeat(to - from)
        } else if d > Duration::ZERO {
            "▏".to_owned()
        } else {
            String::new()
        };
        let style = color(*c);
        writeln!(
            out,
            "{:>9} ms  {}{style}{bar}{style:#}",
            ms(d),
            " ".repeat(from)
        )?;
    }
    writeln!(out, "  {BOLD}{:<15}{:>9} ms{BOLD:#}", "Total", ms(p.total))?;
    writeln!(out, "  {:<15}{:>9} ms", "TTFB", ms(p.ttfb))
}

fn single(out: &mut impl Write, method: &str, run: &Run) -> io::Result<()> {
    let many = run.hops.len() > 1;
    for (i, hop) in run.hops.iter().enumerate() {
        if i > 0 {
            writeln!(out)?;
        }
        if many {
            writeln!(out, "{DIM}Hop {} of {}{DIM:#}", i + 1, run.hops.len())?;
        }
        header(out, method, hop)?;
        writeln!(out)?;
        waterfall(out, hop)?;
    }
    if many {
        writeln!(
            out,
            "\n{BOLD}{} hops: {} ms total, TTFB {} ms{BOLD:#}",
            run.hops.len(),
            ms(run.phases.total),
            ms(run.phases.ttfb)
        )?;
    }
    Ok(())
}

fn statuses(runs: &[Run]) -> String {
    let mut counts: Vec<(u16, usize)> = Vec::new();
    for run in runs {
        let status = run.last().status;
        match counts.iter_mut().find(|(s, _)| *s == status) {
            Some((_, n)) => *n += 1,
            None => counts.push((status, 1)),
        }
    }
    counts
        .iter()
        .map(|(s, n)| {
            let style = status_style(*s);
            format!("{style}{}{style:#} ×{n}", status_text(*s))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn spread_rows(spread: &Spread) -> [(&'static str, Option<Summary>); 7] {
    [
        (PHASES[0].0, spread.dns),
        (PHASES[1].0, spread.tcp),
        (PHASES[2].0, spread.tls),
        (PHASES[3].0, spread.wait),
        (PHASES[4].0, spread.transfer),
        ("TTFB", spread.ttfb),
        ("Total", spread.total),
    ]
}

fn percentiles(out: &mut impl Write, report: &Report, series: &Series) -> io::Result<()> {
    let first = series.runs[0].last();
    let n = series.runs.len();
    let how = if report.reuse {
        "one connection"
    } else {
        "a new connection for each"
    };
    writeln!(
        out,
        "{BOLD}{} {}{BOLD:#} {DIM}→{DIM:#} {}",
        report.method, report.url, first.peer
    )?;
    let mut parts = vec![first.protocol.to_owned()];
    parts.extend(first.tls_version.clone());
    writeln!(
        out,
        "{} · {} · {n} requests, {how}",
        parts.join(" · "),
        statuses(&series.runs)
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "  {DIM}{:<15}{:>9}{:>9}{:>9}{:>9}{:>9}{DIM:#}",
        "ms", "min", "p50", "p90", "p99", "max"
    )?;
    for (label, s) in spread_rows(&series.summary) {
        let Some(s) = s else { continue };
        let style = if label == "Total" { BOLD } else { Style::new() };
        writeln!(
            out,
            "  {style}{label:<15}{:>9}{:>9}{:>9}{:>9}{:>9}{style:#}",
            ms(s.min),
            ms(s.p50),
            ms(s.p90),
            ms(s.p99),
            ms(s.max)
        )?;
    }
    if report.reuse && series.summary.tcp.is_some() {
        writeln!(
            out,
            "\n  {DIM}DNS, TCP and TLS are from requests that opened a connection.{DIM:#}"
        )?;
    }
    Ok(())
}

fn label(protocol: &str) -> &str {
    match protocol {
        "http1" => "HTTP/1.1",
        "http2" => "h2",
        other => other,
    }
}

fn compare(out: &mut impl Write, report: &Report) -> io::Result<()> {
    let n = report.results[0].runs.len();
    writeln!(out, "{BOLD}{} {}{BOLD:#}", report.method, report.url)?;
    for series in &report.results {
        let last = series.runs[0].last();
        write!(
            out,
            "  {:<9}{DIM}→{DIM:#} {}",
            label(series.protocol),
            last.peer
        )?;
        if let Some(v) = &last.tls_version {
            write!(out, " · {v}")?;
        }
        writeln!(out, " · {}", statuses(&series.runs))?;
    }
    writeln!(out)?;
    let what = if n > 1 {
        format!("p50 of {n}, ms")
    } else {
        "ms".to_owned()
    };
    write!(out, "  {DIM}{what:<15}")?;
    for series in &report.results {
        write!(out, "{:>11}", label(series.protocol))?;
    }
    writeln!(out, "{DIM:#}")?;
    let rows: Vec<_> = report
        .results
        .iter()
        .map(|s| spread_rows(&s.summary))
        .collect();
    for i in 0..rows[0].len() {
        let name = rows[0][i].0;
        if rows.iter().all(|r| r[i].1.is_none()) {
            continue;
        }
        let style = if name == "Total" { BOLD } else { Style::new() };
        write!(out, "  {style}{name:<15}")?;
        for r in &rows {
            match r[i].1 {
                Some(s) => write!(out, "{:>11}", ms(s.p50))?,
                None => write!(out, "{:>11}", "—")?,
            }
        }
        writeln!(out, "{style:#}")?;
    }
    Ok(())
}

pub fn human(report: &Report, out: &mut impl Write) -> io::Result<()> {
    if report.results.len() > 1 {
        compare(out, report)?;
    } else {
        let series = &report.results[0];
        if series.runs.len() == 1 {
            single(out, &report.method, &series.runs[0])?;
        } else {
            percentiles(out, report, series)?;
        }
    }
    let stopped = report
        .results
        .iter()
        .flat_map(|s| &s.runs)
        .find_map(|r| r.stopped.as_deref());
    if let Some(note) = stopped {
        writeln!(out, "\n{DIM}Note: {note}.{DIM:#}")?;
    }
    if !report.reasons.is_empty() {
        writeln!(out)?;
    }
    for reason in &report.reasons {
        writeln!(out, "{RED}✗ {reason}.{RED:#}")?;
    }
    Ok(())
}
