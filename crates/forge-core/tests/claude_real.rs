//! Runs the real `claude` program through `forge_core::claude`: finds it, starts a turn, reads the events, and stops a
//! second turn. It uses the user's own Claude Code sign-in and costs a few tokens, so it only runs on request:
//!
//!     cargo test -p forge-core --test claude_real -- --ignored --nocapture

use std::time::{Duration, Instant};

use forge_core::claude::{self, Message, Options, PermissionMode};

fn options() -> Option<Options> {
    let binary = claude::find_claude("")?;
    println!("claude found at {}", binary.display());
    Some(Options { binary, cwd: std::env::var_os("FORGE_TEST_CWD").map(Into::into).unwrap_or_else(std::env::temp_dir), mode: PermissionMode::Ask, model: "haiku".to_owned(), effort: String::new(), show_thinking: std::env::var_os("FORGE_TEST_THINKING").is_some(), resume: None, images: Vec::new() })
}

/// How many `claude.exe` processes are running (Windows only; 0 elsewhere).
fn claude_processes() -> usize {
    if !cfg!(windows) {
        return 0;
    }
    let out = std::process::Command::new("tasklist").args(["/FO", "CSV", "/NH", "/FI", "IMAGENAME eq claude.exe"]).output().ok();
    out.map(|o| String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.to_lowercase().contains("claude.exe")).count()).unwrap_or(0)
}

fn collect(mut run: claude::Run, limit: Duration) -> Vec<Message> {
    let start = Instant::now();
    let mut all = Vec::new();
    while start.elapsed() < limit {
        let (messages, ended) = run.poll();
        all.extend(messages);
        if ended {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    all
}

#[test]
#[ignore = "runs the real claude program and uses the user's Claude Code sign-in"]
fn a_question_is_answered_through_the_real_program() {
    let Some(opts) = options() else {
        println!("claude is not installed here: nothing to check");
        return;
    };
    let run = claude::start(&opts, "Reply with the single word: pong").expect("the program starts");
    let messages = collect(run, Duration::from_secs(120));
    let mut convo = claude::Conversation::default();
    convo.ask("Reply with the single word: pong");
    let mut stderr = Vec::new();
    for m in messages {
        match m {
            Message::Event(e) => convo.apply(e),
            Message::Stderr(s) => stderr.push(s),
            Message::Exited(code) => println!("exited with {code:?}"),
        }
    }
    println!("entries: {:#?}\nstderr: {stderr:?}", convo.entries);
    assert!(!convo.busy, "the turn finished");
    assert!(convo.session_id.is_some(), "a session id came back");
    assert!(convo.entries.iter().any(|e| matches!(e, claude::Entry::Assistant { text, .. } if text.to_lowercase().contains("pong"))), "an answer came back");
}

#[test]
#[ignore = "runs the real claude program and uses the user's Claude Code sign-in"]
fn a_running_turn_is_stopped_and_leaves_nothing_behind() {
    let Some(opts) = options() else { return };
    let before = claude_processes();
    let run = claude::start(&opts, "Count slowly from 1 to 200, one number per line.").expect("the program starts");
    std::thread::sleep(Duration::from_secs(4));
    println!("claude.exe processes: {before} before, {} while running", claude_processes());
    run.stop();
    let started = Instant::now();
    let mut run = run;
    let mut ended = false;
    while started.elapsed() < Duration::from_secs(20) {
        let (_, over) = run.poll();
        if over {
            ended = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ended, "the program ended after it was stopped");
}

#[test]
#[ignore = "runs the real claude program and uses the user's Claude Code sign-in"]
fn stopping_leaves_no_claude_process_running() {
    let Some(opts) = options() else { return };
    let before = claude_processes();
    let run = claude::start(&opts, "Count slowly from 1 to 200, one number per line.").expect("the program starts");
    std::thread::sleep(Duration::from_secs(4));
    run.stop();
    drop(run);
    let started = Instant::now();
    let mut now = claude_processes();
    while now > before && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(250));
        now = claude_processes();
    }
    assert!(now <= before, "{now} claude.exe processes after the stop, {before} before");
}

#[test]
#[ignore = "runs the real claude program and uses the user's Claude Code sign-in"]
fn a_command_waits_for_the_user_and_runs_once_allowed() {
    let Some(opts) = options() else { return };
    let mut run = claude::start(&opts, &std::env::var("FORGE_TEST_PROMPT").unwrap_or_else(|_| "Use the Bash tool to run: echo hello".to_owned())).expect("the program starts");
    let started = Instant::now();
    let mut asked = false;
    let mut output = String::new();
    let mut done = false;
    while started.elapsed() < Duration::from_secs(120) && !done {
        let (messages, ended) = run.poll();
        for m in messages {
            match m {
                Message::Event(claude::Event::Permission(request)) => {
                    println!("asked: {} {}", request.tool_name, request.input);
                    asked = true;
                    run.respond(&request.id, &claude::Decision::Allow { input: request.input.clone(), permissions: Vec::new() }).unwrap();
                }
                Message::Event(claude::Event::ToolResult { text, .. }) => output = text,
                Message::Event(claude::Event::Done { .. }) => done = true,
                Message::Event(e) => println!("event: {e:?}"),
                Message::Stderr(s) => println!("stderr: {s}"),
                Message::Exited(c) => println!("exited {c:?}"),
            }
        }
        if ended {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("output: {output:?}");
    assert!(asked, "the program asked before running the command");
    assert!(output.contains("hello"), "the command ran once it was allowed");
}
