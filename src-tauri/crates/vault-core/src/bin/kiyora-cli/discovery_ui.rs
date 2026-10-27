use super::{active, wait_authorized};
use std::{
    io::{self, IsTerminal, Write},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use vault_core::{ApiError, Backend, ErrorCode};
use vault_telegram::{DiscoveryPhase, DiscoveryProgress, DiscoveryReport};

#[derive(Default)]
struct Options {
    deep: bool,
    json: bool,
}
fn options(args: &[String]) -> Result<Options, ApiError> {
    let mut result = Options::default();
    for arg in args {
        match arg.as_str() {
            "--deep" if !result.deep => result.deep = true,
            "--json" if !result.json => result.json = true,
            _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
        }
    }
    Ok(result)
}

pub(super) async fn run(
    backend: &Backend,
    args: &[String],
    cancel: &CancellationToken,
) -> Result<(), ApiError> {
    if args == ["--help"] || args == ["-h"] {
        println!(
            "kiyora-cli vault discover [--deep] [--json]\n\nChecks the saved Vault, then searches known Telegram channels by name.\n--deep also scans main/archive chat lists and bounded legacy history.\nNo Vault is selected or created. Press Ctrl+C to cancel."
        );
        return Ok(());
    }
    // Keep stdout a single JSON object, including pre-discovery failures.
    let json_mode = args.iter().any(|arg| arg == "--json");
    let result = async {
        let options = options(args)?;
        if !options.json {
            eprintln!("Kiyora — {}Vault Discovery\n\nPress Ctrl+C to cancel.\nConnecting to the active Telegram profile...",
                if options.deep { "Deep " } else { "" });
        }
        let status = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ApiError::new(ErrorCode::Cancelled)),
            result = active(backend) => result?,
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ApiError::new(ErrorCode::Cancelled)),
            result = wait_authorized(backend, status.context.session_id) => result?,
        }
        let (tx, mut rx) = tokio::sync::watch::channel(DiscoveryProgress::default());
        let operation = backend.discover_vaults_with_options(status.context.session_id, options.deep, Some(&tx), cancel);
        tokio::pin!(operation);
        let started = tokio::time::Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let interactive = io::stderr().is_terminal();
        let mut last = String::new();
        let report = loop {
            tokio::select! {
                result = &mut operation => break result?,
                _ = tick.tick() => {},
                changed = rx.changed() => { if changed.is_err() { continue; } },
            }
            if !options.json {
                let progress = rx.borrow().clone();
                let text = progress_text(&progress);
                if interactive {
                    eprint!("\r\x1b[2K{text} | {}s", started.elapsed().as_secs());
                    let _ = io::stderr().flush();
                } else if text != last {
                    eprintln!("{text}");
                }
                last = text;
            }
        };
        if interactive && !options.json { eprintln!(); }
        Ok((options, report))
    }.await;
    match result {
        Ok((options, report)) => {
            let output = report_output(&report, options.json)?;
            print!("{output}");
            if let Some(error) = report.failure {
                return Err(error.into());
            }
            if !report.complete {
                return Err(vault_telegram::TelegramError::IncompleteHistory.into());
            }
            Ok(())
        }
        Err(error) => {
            if json_mode {
                super::print_json(&serde_json::json!({"error": error}))?;
            }
            Err(error)
        }
    }
}

fn progress_text(p: &DiscoveryProgress) -> String {
    let phase = match p.phase {
        DiscoveryPhase::Saved => "[1/3] Checking saved Vault and account",
        DiscoveryPhase::Search => "[2/3] Searching Telegram",
        DiscoveryPhase::Validate => "[3/3] Validating channel ownership and VaultInit/schema",
        DiscoveryPhase::Deep => "Scanning Telegram main/archive channels",
        DiscoveryPhase::Finished => "Discovery finished",
    };
    format!(
        "{phase}{} | Chats: {} | Candidates: {} | Channels: {} | Validated: {} | History: {} | Vaults: {} | TDLib calls: {}",
        p.query
            .as_ref()
            .map(|q| format!(" ({q})"))
            .unwrap_or_default(),
        p.stats.chats_enumerated,
        p.stats.candidates_found,
        p.stats.channels_inspected,
        p.stats.candidates_validated,
        p.stats.history_messages,
        p.stats.vaults_found,
        p.stats.tdlib_requests
    )
}

fn report_output(report: &DiscoveryReport, json: bool) -> Result<String, ApiError> {
    if json {
        return serde_json::to_string_pretty(report)
            .map(|s| format!("{s}\n"))
            .map_err(|_| ApiError::new(ErrorCode::Internal));
    }
    use std::fmt::Write;
    let mut text = String::new();
    if report.candidates.is_empty() {
        text.push_str(if report.complete {
            "No Kiyora Vault found in this search.\n"
        } else {
            "Search incomplete; no Vault validated.\n"
        });
    } else {
        let _ = writeln!(text, "Vaults found: {}", report.candidates.len());
        for c in &report.candidates {
            // Telegram titles are untrusted terminal data (including escape codes).
            let title: String = c
                .channel
                .title
                .chars()
                .filter(|c| !c.is_control())
                .collect();
            let _ = writeln!(
                text,
                "\nName: {title}\nVault ID: {}\nChat ID: {}\nSchema: {}\nAttach: kiyora-cli vault attach {}",
                c.init.vault_id,
                c.channel.chat_id,
                vault_protocol::PROTOCOL,
                c.channel.chat_id
            );
        }
        text.push_str("\nNo Vault was attached. Choose a Chat ID explicitly.\n");
    }
    let _ = writeln!(
        text,
        "\nChats enumerated: {}\nChannels inspected: {}\nCandidates validated: {}\nHistory messages checked: {}\nTDLib calls: {}",
        report.stats.chats_enumerated,
        report.stats.channels_inspected,
        report.stats.candidates_validated,
        report.stats.history_messages,
        report.stats.tdlib_requests
    );
    for diagnostic in &report.diagnostics {
        let _ = writeln!(
            text,
            "Diagnostic{}: {}",
            diagnostic
                .chat_id
                .map(|id| format!(" for chat {id}"))
                .unwrap_or_default(),
            diagnostic.error
        );
    }
    if let Some(error) = &report.failure {
        let _ = writeln!(text, "Search stopped: {error}");
    }
    if !report.complete {
        text.push_str(
            "The search could not be completed within its limits. Results may be partial.\n",
        );
    }
    text.push_str("\nKnown-chat search may miss private or renamed channels. You can:\n  kiyora-cli vault discover --deep\n  kiyora-cli vault attach <chat-id>\n  kiyora-cli vault create  (only to create a new Vault)\n");
    Ok(text)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn flags_are_explicit_and_unknown_options_are_rejected() {
        assert!(!options(&[]).unwrap().deep);
        assert!(options(&["--deep".into(), "--json".into()]).unwrap().deep);
        assert!(options(&["--depe".into()]).is_err());
        assert!(options(&["--deep".into(), "--deep".into()]).is_err());
    }
    #[test]
    fn json_is_one_document_and_preserves_partial_results_and_diagnostics() {
        let report = DiscoveryReport {
            candidates: vec![],
            stats: Default::default(),
            complete: false,
            diagnostics: vec![],
            failure: Some(vault_telegram::TelegramError::RateLimited {
                retry_after_seconds: 42,
            }),
        };
        let value: serde_json::Value =
            serde_json::from_str(&report_output(&report, true).unwrap()).unwrap();
        assert_eq!(value["complete"], false);
        assert_eq!(value["failure"]["RateLimited"]["retry_after_seconds"], 42);
        assert!(!report_output(&report, true).unwrap().contains("Kiyora —"));
        assert!(
            report_output(&report, false)
                .unwrap()
                .contains("Search incomplete")
        );
    }
}
