use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::OnceLock;

use clap::ValueEnum;
use serde::Serialize;
use tracing_subscriber::EnvFilter;

static JSON: OnceLock<bool> = OnceLock::new();

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

pub fn init(json: bool, level: Option<LogLevel>) {
    let _ = JSON.set(json);
    let filter = match level {
        Some(level) => EnvFilter::new(format!(
            "warn,{}={}",
            env!("CARGO_CRATE_NAME"),
            level.as_str()
        )),
        None => EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(format!("warn,{}=warn", env!("CARGO_CRATE_NAME")))),
    };
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

fn json() -> bool {
    JSON.get().copied().unwrap_or(false)
}

#[derive(Serialize)]
struct Success<'a, T: Serialize> {
    status: &'static str,
    #[serde(flatten)]
    data: &'a T,
}

#[derive(Serialize)]
struct Failure<'a> {
    status: &'static str,
    code: u8,
    error: &'a str,
}

pub fn success<T: Serialize>(data: &T, text: &str) -> ExitCode {
    if json() {
        print_json(&Success { status: "ok", data });
    } else if !text.is_empty() {
        println!("{text}");
    }
    ExitCode::SUCCESS
}

pub fn fail(code: u8, message: &str) -> ExitCode {
    eprintln!("Erro: {message}");
    if json() {
        print_json(&Failure {
            status: "error",
            code,
            error: message,
        });
    }
    ExitCode::from(code)
}

fn print_json<T: Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(line) => println!("{line}"),
        Err(error) => eprintln!("Erro: falha ao gerar JSON: {error}"),
    }
}
