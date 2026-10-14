//! misura: where an HTTP request spends its time.

mod cli;
mod connect;
mod measure;
mod report;
mod resolve;
mod stats;
mod target;
mod tls;

use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use hyper::Method;
use hyper::header::HeaderMap;

use crate::measure::{Conn, Follow, Plan, Run, round_ms};
use crate::report::{Report, Series};
use crate::resolve::Override;
use crate::target::{Scheme, Target};
use crate::tls::Offer;

fn main() -> ExitCode {
    let args = cli::Args::parse();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("starting the runtime: {e}"))
        .and_then(|rt| rt.block_on(run(&args)));
    let mut out = anstream::stdout().lock();
    match result {
        Ok(report) => {
            let printed = if args.json {
                report::json(&report, &mut out)
            } else {
                report::human(&report, &mut out)
            };
            // A closed pipe (`misura … | head`) isn't worth an error.
            if let Err(e) = printed.and_then(|()| out.flush())
                && e.kind() != std::io::ErrorKind::BrokenPipe
            {
                eprintln!("misura: error: writing output: {e}");
                return ExitCode::from(2);
            }
            if report.failed {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            if args.json {
                let _ = report::json_error(&e, &mut out);
            }
            let _ = out.flush();
            eprintln!("misura: error: {e}");
            ExitCode::from(2)
        }
    }
}

async fn run(args: &cli::Args) -> Result<Report, String> {
    let target = Target::parse(&args.url)?;
    let method = Method::from_bytes(args.method.as_bytes())
        .map_err(|_| format!("invalid method {:?}", args.method))?;
    let mut headers = HeaderMap::new();
    for spec in &args.headers {
        let (name, value) = measure::parse_header(spec)?;
        headers.append(name, value);
    }
    let overrides = args
        .resolve
        .iter()
        .map(|s| Override::parse(s))
        .collect::<Result<Vec<_>, _>>()?;
    let offers: &[(&'static str, Offer)] = if args.compare {
        &[("http1", Offer::Http1), ("http2", Offer::Http2)]
    } else if args.http1 {
        &[("http1", Offer::Http1)]
    } else if args.http2 {
        &[("http2", Offer::Http2)]
    } else {
        &[("auto", Offer::Both)]
    };
    if (args.compare || args.http2) && target.scheme == Scheme::Http {
        let flag = if args.compare { "--compare" } else { "--http2" };
        return Err(format!(
            "{flag} needs an https:// URL; misura doesn't do HTTP/2 without TLS"
        ));
    }
    let follow = Follow {
        enabled: args.location,
        cross_host: args.cross_host,
        max: args.max_redirects,
    };
    let plans: Vec<Plan> = offers
        .iter()
        .map(|&(_, offer)| {
            Plan::new(
                method.clone(),
                headers.clone(),
                overrides.clone(),
                args.cacert.clone(),
                offer,
            )
        })
        .collect();

    let mut conns: Vec<Option<Conn>> = plans.iter().map(|_| None).collect();
    let mut runs: Vec<Vec<Run>> = plans.iter().map(|_| Vec::new()).collect();
    // With --compare, the protocols take turns, so a change in the network
    // during the runs affects both alike.
    for _ in 0..args.count {
        for (i, plan) in plans.iter().enumerate() {
            let conn = if args.reuse { conns[i].take() } else { None };
            let (run, conn) =
                tokio::time::timeout(args.timeout, measure::chain(plan, &target, follow, conn))
                    .await
                    .map_err(|_| {
                        format!("{target} took longer than --timeout {}", show(args.timeout))
                    })??;
            if args.reuse {
                conns[i] = conn;
            }
            runs[i].push(run);
        }
    }

    let results: Vec<Series> = offers
        .iter()
        .zip(runs)
        .map(|(&(name, _), runs)| Series::new(name, runs))
        .collect();
    let reasons = failures(args, &results);
    Ok(Report::new(
        method.to_string(),
        target.to_string(),
        args.reuse,
        results,
        reasons,
    ))
}

/// Milliseconds to two decimal places, without trailing zeros.
fn show(d: Duration) -> String {
    let s = format!("{:.2}", round_ms(d));
    format!("{} ms", s.trim_end_matches('0').trim_end_matches('.'))
}

/// Why misura should exit with 1, if it should.
fn failures(args: &cli::Args, results: &[Series]) -> Vec<String> {
    let runs: Vec<&Run> = results.iter().flat_map(|s| &s.runs).collect();
    let of = |n: usize| {
        if runs.len() == 1 {
            String::new()
        } else {
            format!(" in {n} of {} requests", runs.len())
        }
    };
    let mut reasons = Vec::new();
    let errors: Vec<u16> = runs
        .iter()
        .map(|r| r.last().status)
        .filter(|s| *s >= 400)
        .collect();
    if let Some(&status) = errors.first() {
        reasons.push(format!("The server answered {status}{}", of(errors.len())));
    }
    let ttfb = runs.iter().map(|r| r.phases.ttfb);
    let total = runs.iter().map(|r| r.phases.total);
    if let Some(reason) = over("TTFB", "--max-ttfb", args.max_ttfb, ttfb, &of) {
        reasons.push(reason);
    }
    if let Some(reason) = over("Total time", "--max-total", args.max_total, total, &of) {
        reasons.push(reason);
    }
    reasons
}

fn over(
    what: &str,
    flag: &str,
    limit: Option<Duration>,
    values: impl Iterator<Item = Duration>,
    of: &dyn Fn(usize) -> String,
) -> Option<String> {
    let limit = limit?;
    let over: Vec<Duration> = values.filter(|d| *d > limit).collect();
    let worst = over.iter().max()?;
    Some(format!(
        "{what} went over {flag} {}{}, reaching {}",
        show(limit),
        of(over.len()),
        show(*worst)
    ))
}
