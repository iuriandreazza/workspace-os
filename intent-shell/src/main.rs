//! `intent-shell`: type a request, see the plan the broker would run, confirm.
//!
//! Needs `llama-server` running Bonsai 8B (docs/s3-intent-shell-prototype.md).

use std::io::{self, Write};
use std::path::PathBuf;

use intent_shell::broker::Authority;
use intent_shell::host::UnixHost;
use intent_shell::log::JsonlLog;
use intent_shell::model::LlamaPlanner;
use intent_shell::ports::{Confirm, User};
use intent_shell::session::Shell;

const USAGE: &str = "usage: intent-shell [--model HOST:PORT] [--log FILE] [--administrator]";

struct Options {
    model: String,
    log: PathBuf,
    administrator: bool,
}

fn parse_options(home: &str) -> Result<Options, String> {
    let mut options = Options {
        model: "127.0.0.1:8089".to_owned(),
        log: PathBuf::from(home).join(".local/state/workspace-os/intent-shell/decisions.jsonl"),
        administrator: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--model" => options.model = args.next().ok_or(USAGE)?,
            "--log" => options.log = PathBuf::from(args.next().ok_or(USAGE)?),
            "--administrator" => options.administrator = true,
            _ => return Err(USAGE.to_owned()),
        }
    }
    Ok(options)
}

struct TerminalConfirm;

fn ask(question: &str) -> bool {
    print!("{question} [y/N] ");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .is_ok_and(|n| n > 0 && matches!(answer.trim(), "y" | "Y" | "yes"))
}

impl Confirm for TerminalConfirm {
    fn plan(&mut self, rendering: &str) -> bool {
        println!("{rendering}");
        ask("Run the steps marked ok?")
    }

    fn root_action(&mut self, rendering: &str, who: &User) -> bool {
        println!(
            "ROOT action, to be confirmed by {}:\n  {rendering}",
            who.name
        );
        ask("Confirm this exact action?")
    }
}

fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set")?;
    let options = parse_options(&home)?;
    let host = UnixHost::from_environment().map_err(|e| e.to_string())?;
    let working_directory = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    let planner = LlamaPlanner {
        address: options.model,
        working_directory,
    };
    let mut log = JsonlLog::open(&options.log).map_err(|e| {
        format!(
            "cannot open the decision log {}: {e}",
            options.log.display()
        )
    })?;
    let user = User {
        name: std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned()),
        administrator: options.administrator,
    };
    let authority = Authority::full(user);
    let mut confirm = TerminalConfirm;

    println!(
        "intent-shell: decisions are logged to {}",
        options.log.display()
    );
    loop {
        print!("intent> ");
        let _ = io::stdout().flush();
        let mut request = String::new();
        if io::stdin()
            .read_line(&mut request)
            .map_err(|e| e.to_string())?
            == 0
        {
            return Ok(());
        }
        let request = request.trim();
        if request.is_empty() {
            continue;
        }
        let mut shell = Shell {
            authority: &authority,
            host: &host,
            planner: &planner,
            confirm: &mut confirm,
            log: &mut log,
        };
        println!("{}", shell.handle(request).render());
    }
}
