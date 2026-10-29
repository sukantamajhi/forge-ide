//! Asking for credentials. Git, and ssh under it, run a helper program when they need a
//! username, password, passphrase or yes/no answer. Forge is that helper: git is started with
//! `GIT_ASKPASS` and `SSH_ASKPASS` pointing at Forge's own executable, which asks the running
//! editor over a local socket (so the question shows up in the editor window), prints the
//! answer for git, and exits. The same approach VS Code's git extension takes.
//!
//! The socket listens on 127.0.0.1 only and every request carries a random token that only
//! the processes started by this editor know.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::RwLock;
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Set in the helper's environment; its presence is what makes `forge.exe <prompt>` the helper.
const TOKEN_VAR: &str = "FORGE_ASKPASS_TOKEN";
const PORT_VAR: &str = "FORGE_ASKPASS_PORT";

/// A question from git. Send the answer, or `None` to cancel (git then fails the command).
pub struct Request {
    pub prompt: String,
    pub reply: Sender<Option<String>>,
}

struct Config {
    exe: PathBuf,
    port: u16,
    token: String,
}

static CONFIG: RwLock<Option<Config>> = RwLock::new(None);

/// How long the editor waits for the user to answer before giving up on the command.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(600);

fn random_token() -> String {
    let mut hasher = Sha256::new();
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()).to_le_bytes());
    let marker = 0u8;
    hasher.update((std::ptr::addr_of!(marker) as usize).to_le_bytes());
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `prompt` asks for something secret, which must not be echoed as it is typed.
pub fn is_secret(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    lower.contains("password") || lower.contains("passphrase") || lower.contains("token") || lower.contains("pin")
}

/// Start listening and make git commands use `exe` as their askpass helper. Returns the
/// questions as they come in, or `None` if no socket could be opened.
pub fn start(exe: &Path) -> Option<Receiver<Request>> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    let token = random_token();
    let (tx, rx) = mpsc::channel();
    let expected = token.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (tx, expected) = (tx.clone(), expected.clone());
            std::thread::spawn(move || handle(stream, &expected, tx));
        }
    });
    if let Ok(mut config) = CONFIG.write() {
        *config = Some(Config { exe: exe.to_path_buf(), port, token });
    }
    Some(rx)
}

fn handle(stream: TcpStream, token: &str, requests: Sender<Request>) {
    let Ok(clone) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    let (mut sent_token, mut prompt) = (String::new(), String::new());
    if reader.read_line(&mut sent_token).is_err() || reader.read_line(&mut prompt).is_err() {
        return;
    }
    let mut out = clone;
    if sent_token.trim_end() != token {
        let _ = out.write_all(b"CANCEL\n");
        return;
    }
    let (reply, answer) = mpsc::channel();
    if requests.send(Request { prompt: prompt.trim_end().to_owned(), reply }).is_err() {
        let _ = out.write_all(b"CANCEL\n");
        return;
    }
    let line = match answer.recv_timeout(ANSWER_TIMEOUT) {
        Ok(Some(secret)) => format!("OK\n{}\n", secret.replace(['\r', '\n'], "")),
        _ => "CANCEL\n".to_owned(),
    };
    let _ = out.write_all(line.as_bytes());
}

/// Environment for a git process so that it asks through the editor. Empty until [`start`] ran.
pub fn env_vars() -> Vec<(&'static str, String)> {
    let Ok(guard) = CONFIG.read() else { return Vec::new() };
    let Some(config) = guard.as_ref() else { return Vec::new() };
    let exe = config.exe.to_string_lossy().into_owned();
    vec![
        ("GIT_ASKPASS", exe.clone()),
        ("SSH_ASKPASS", exe),
        // Use the helper even though ssh has a terminal or no display (OpenSSH 8.4 and later).
        ("SSH_ASKPASS_REQUIRE", "force".to_owned()),
        ("DISPLAY", "forge:0".to_owned()),
        (PORT_VAR, config.port.to_string()),
        (TOKEN_VAR, config.token.clone()),
    ]
}

/// If this process was started by git as the askpass helper (`forge.exe "Password for ...:"`),
/// ask the editor, print the answer and return the exit code. `None` for a normal start.
pub fn run_as_helper(args: &[String]) -> Option<i32> {
    let token = std::env::var(TOKEN_VAR).ok()?;
    let port: u16 = std::env::var(PORT_VAR).ok()?.parse().ok()?;
    let prompt = args.get(1).cloned().unwrap_or_default().replace(['\r', '\n'], " ");
    let code = (|| {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
        stream.write_all(format!("{token}\n{prompt}\n").as_bytes()).ok()?;
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status).ok()?;
        if status.trim_end() != "OK" {
            return None;
        }
        let mut secret = String::new();
        reader.read_line(&mut secret).ok()?;
        println!("{}", secret.trim_end_matches(['\r', '\n']));
        Some(0)
    })();
    Some(code.unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_recognised_by_their_prompt() {
        assert!(is_secret("Password for 'https://user@github.com': "));
        assert!(is_secret("Enter passphrase for key '/home/u/.ssh/id_ed25519': "));
        assert!(!is_secret("Username for 'https://github.com': "));
        assert!(!is_secret("Are you sure you want to continue connecting (yes/no)? "));
    }

    /// The editor side and the helper side talk to each other over the socket.
    #[test]
    fn a_question_reaches_the_editor_and_the_answer_comes_back() {
        let requests = start(Path::new("forge.exe")).expect("listening");
        let vars = env_vars();
        let get = |k: &str| vars.iter().find(|(name, _)| *name == k).map(|(_, v)| v.clone()).unwrap();
        let (port, token) = (get(PORT_VAR), get(TOKEN_VAR));
        let ask = |token: String, port: String| {
            std::thread::spawn(move || {
                let mut stream = TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).unwrap();
                stream.write_all(format!("{token}\nUsername for 'https://example.com': \n").as_bytes()).unwrap();
                let mut line = String::new();
                BufReader::new(stream).read_line(&mut line).unwrap();
                line
            })
        };
        let good = ask(token.clone(), port.clone());
        let request = requests.recv_timeout(Duration::from_secs(5)).expect("the editor gets the question");
        assert_eq!(request.prompt, "Username for 'https://example.com':");
        request.reply.send(Some("alice".to_owned())).unwrap();
        assert_eq!(good.join().unwrap(), "OK\n");
        // A request without the right token is refused before the editor hears of it.
        let bad = ask("wrong".to_owned(), port);
        assert_eq!(bad.join().unwrap(), "CANCEL\n");
        assert!(requests.recv_timeout(Duration::from_millis(300)).is_err());
    }
}
