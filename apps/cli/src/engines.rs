use std::fmt::Write;
use std::process::ExitCode;

use database_backup_core::{DatabaseEngine, Support};
use serde::Serialize;

use crate::output::success;

#[derive(Serialize)]
struct EnginesResult {
    engines: Vec<EngineEntry>,
}

#[derive(Serialize)]
struct EngineEntry {
    engine: &'static str,
    name: &'static str,
    capabilities: Vec<CapabilityEntry>,
}

#[derive(Serialize)]
struct CapabilityEntry {
    operation: &'static str,
    support: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue: Option<&'static str>,
    note: &'static str,
}

pub fn run() -> ExitCode {
    let engines = DatabaseEngine::ALL
        .iter()
        .map(|&engine| EngineEntry {
            engine: engine.as_str(),
            name: engine.display_name(),
            capabilities: engine
                .capabilities()
                .iter()
                .map(|capability| CapabilityEntry {
                    operation: capability.operation.as_str(),
                    support: capability.support.as_str(),
                    issue: match capability.support {
                        Support::Planned { issue } => Some(issue),
                        Support::Supported | Support::Unsupported => None,
                    },
                    note: capability.note,
                })
                .collect(),
        })
        .collect();
    success(&EnginesResult { engines }, &text())
}

fn text() -> String {
    let mut text = String::new();
    for (index, engine) in DatabaseEngine::ALL.iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let _ = writeln!(text, "{} ({})", engine.display_name(), engine.as_str());
        for capability in engine.capabilities() {
            let status = match capability.support {
                Support::Supported => "sim".to_owned(),
                Support::Planned { issue } => format!("planejado ({issue})"),
                Support::Unsupported => "não".to_owned(),
            };
            let _ = writeln!(
                text,
                "  {:<42} {:<18} {}",
                capability.operation.description(),
                status,
                capability.note
            );
        }
    }
    text.trim_end().to_owned()
}
