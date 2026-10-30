//! Signing commits with GPG when the key has a passphrase. Git runs `gpg` to sign, and gpg asks
//! for the passphrase through a "pinentry" program, which needs a terminal or a window the editor
//! may not have: without one, signing fails with "No pinentry". Forge stands in for gpg: git is
//! told `gpg.program` is Forge's own executable (see [`crate::askpass`]), which runs the real gpg
//! with no prompt allowed (it works when gpg-agent has the passphrase cached), and only if gpg
//! says it needs one, asks in the editor and runs gpg again handing it the answer.

use std::io::Write;
use std::process::{Command, Stdio};

/// What a gpg run produced, to hand to git exactly as gpg did.
pub struct Outcome {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

fn run_gpg(real: &str, extra: &[&str], args: &[String], stdin: &[u8], env: &[(&str, &str)]) -> std::io::Result<Outcome> {
    let mut command = Command::new(real);
    command.args(extra).args(args).envs(env.iter().copied()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn()?;
    if let Some(mut input) = child.stdin.take() {
        // gpg may exit before reading everything (a failed key lookup); that is not an error here.
        let _ = input.write_all(stdin);
    }
    // gpg may start gpg-agent, which keeps running with our pipes: wait for gpg, not for the pipes to close.
    let output = crate::git::wait_and_collect(child)?;
    Ok(Outcome { code: output.status.code().unwrap_or(1), stdout: output.stdout, stderr: output.stderr })
}

/// Git signs with `gpg --status-fd=2 -bsau <key>`; verification and key listing are other runs.
fn is_signing(args: &[String]) -> bool {
    args.iter().any(|a| a == "-bsau" || a == "--detach-sign" || a == "--sign" || a == "-bsa")
}

/// The key named after `-u` / `-bsau`, for the prompt.
fn signing_key(args: &[String]) -> String {
    args.iter().position(|a| a == "-bsau" || a == "-u" || a == "--local-user").and_then(|i| args.get(i + 1)).cloned().unwrap_or_default()
}

/// Whether gpg's complaint means "I need the passphrase and cannot ask" (as opposed to a real failure).
fn needs_passphrase(stderr: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    text.contains("no pinentry") || text.contains("passphrase") || text.contains("inappropriate ioctl") || text.contains("need_passphrase")
}

/// Run `real` gpg for git. `ask` shows a question to the user and returns the answer (`None` if cancelled).
pub fn sign_with_prompt(real: &str, args: &[String], stdin: &[u8], ask: &dyn Fn(&str) -> Option<String>, env: &[(&str, &str)]) -> Outcome {
    let fail = |message: String| Outcome { code: 2, stdout: Vec::new(), stderr: format!("{message}\n").into_bytes() };
    if !is_signing(args) {
        return run_gpg(real, &[], args, stdin, env).unwrap_or_else(|e| fail(format!("could not run {real}: {e}")));
    }
    let first = match run_gpg(real, &["--pinentry-mode", "error"], args, stdin, env) {
        Ok(outcome) => outcome,
        Err(e) => return fail(format!("could not run {real}: {e}")),
    };
    if first.code == 0 || !needs_passphrase(&first.stderr) {
        return first;
    }
    let key = signing_key(args);
    let prompt = if key.is_empty() { "Enter the GPG passphrase to sign this commit:".to_owned() } else { format!("Enter the GPG passphrase for key {key} to sign this commit:") };
    let Some(passphrase) = ask(&prompt) else { return fail("gpg: signing cancelled".to_owned()) };
    // gpg reads the passphrase from a file; it exists only for this run, and only its owner can read it.
    let file = std::env::temp_dir().join(format!(
        "forge-gpg-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
    ));
    let written = std::fs::OpenOptions::new().write(true).create_new(true).open(&file).and_then(|mut f| f.write_all(passphrase.as_bytes()));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&file);
        return fail(format!("could not hand the passphrase to gpg: {e}"));
    }
    let file_arg = file.to_string_lossy().into_owned();
    let second = run_gpg(real, &["--pinentry-mode", "loopback", "--passphrase-file", &file_arg], args, stdin, env);
    let _ = std::fs::remove_file(&file);
    second.unwrap_or_else(|e| fail(format!("could not run {real}: {e}")))
}

/// Stop gpg (and the gpg-agent it may start and leave running) from inheriting the pipes git gave us. An
/// agent holding git's output pipe open makes git wait for it to exit, so the commit never finishes.
#[cfg(windows)]
fn keep_std_handles_to_ourselves() {
    extern "system" {
        fn GetStdHandle(which: u32) -> isize;
        fn SetHandleInformation(handle: isize, mask: u32, flags: u32) -> i32;
    }
    const STD_HANDLES: [u32; 3] = [-10i32 as u32, -11i32 as u32, -12i32 as u32];
    const HANDLE_FLAG_INHERIT: u32 = 1;
    for which in STD_HANDLES {
        // SAFETY: plain Win32 calls on this process's own standard handles; a failure only leaves them as they were.
        unsafe {
            let handle = GetStdHandle(which);
            if handle != 0 && handle != -1 {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

#[cfg(not(windows))]
fn keep_std_handles_to_ourselves() {}

/// If this process was started by git as its `gpg.program` (Forge sets `FORGE_GPG_REAL`), do the
/// signing and return the exit code. `None` for a normal start.
pub fn run_as_gpg(args: &[String]) -> Option<i32> {
    use std::io::Read;
    let real = std::env::var("FORGE_GPG_REAL").ok()?;
    keep_std_handles_to_ourselves();
    let mut stdin = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut stdin);
    let outcome = sign_with_prompt(&real, &args[1.min(args.len())..], &stdin, &|prompt| crate::askpass::ask_editor(prompt), &[]);
    let _ = std::io::stdout().write_all(&outcome.stdout);
    let _ = std::io::stderr().write_all(&outcome.stderr);
    Some(outcome.code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn only_signing_runs_are_intercepted_and_the_key_is_named() {
        let args = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(is_signing(&args(&["--status-fd=2", "-bsau", "ABCD"])));
        assert!(!is_signing(&args(&["--keyid-format=long", "--status-fd=1", "--verify", "sig", "-"])));
        assert_eq!(signing_key(&args(&["--status-fd=2", "-bsau", "ABCD"])), "ABCD");
        assert_eq!(signing_key(&args(&["--verify"])), "");
    }

    #[test]
    fn a_missing_passphrase_is_told_apart_from_other_failures() {
        assert!(needs_passphrase(b"gpg: signing failed: No pinentry\n"));
        assert!(needs_passphrase(b"gpg: signing failed: Inappropriate ioctl for device\n"));
        assert!(needs_passphrase(b"[GNUPG:] NEED_PASSPHRASE ABCD\n"));
        assert!(!needs_passphrase(b"gpg: skipped \"x\": No secret key\n"));
    }

    #[test]
    fn a_program_that_cannot_run_is_reported_not_hidden() {
        let outcome = sign_with_prompt("forge-no-such-gpg-program", &["-bsau".into(), "k".into()], b"x", &|_| None, &[]);
        assert_eq!(outcome.code, 2);
        assert!(String::from_utf8_lossy(&outcome.stderr).contains("could not run"));
    }

    /// With a real gpg: a key with a passphrase signs only after the editor is asked, and a cancelled
    /// question fails the signing. Skipped when gpg is not installed or no key can be made here.
    #[test]
    fn a_passphrase_protected_key_signs_after_the_user_is_asked() {
        let Ok(version) = Command::new("gpg").arg("--version").output() else {
            eprintln!("skipped: gpg is not installed");
            return;
        };
        if !version.status.success() {
            return;
        }
        let home = std::env::temp_dir().join(format!("forge-gnupg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        // gpg built for MSYS (the one Git for Windows carries) wants /c/Users/..., a native build wants C:/Users/...
        let native = home.to_string_lossy().replace('\\', "/");
        let msys = match native.split_once(":/") {
            Some((drive, rest)) => format!("/{}/{rest}", drive.to_ascii_lowercase()),
            None => native.clone(),
        };
        let mut chosen = None;
        let mut last_error = String::new();
        for candidate in [msys.as_str(), native.as_str()] {
            let generated = Command::new("gpg")
                .env("GNUPGHOME", candidate)
                .args(["--batch", "--pinentry-mode", "loopback", "--passphrase", "forge-secret", "--quick-generate-key", "Forge Test <forge@example.com>", "default", "default", "never"])
                .output();
            match generated {
                Ok(o) if o.status.success() => {
                    chosen = Some(candidate.to_owned());
                    break;
                }
                Ok(o) => last_error = String::from_utf8_lossy(&o.stderr).into_owned(),
                Err(e) => last_error = e.to_string(),
            }
        }
        let Some(home_text) = chosen else {
            eprintln!("skipped: could not create a test key: {last_error}");
            let _ = std::fs::remove_dir_all(&home);
            return;
        };
        let env = [("GNUPGHOME", home_text.as_str())];
        // Make sure the agent holds no cached passphrase from key creation.
        let _ = Command::new("gpgconf").envs(env).args(["--kill", "gpg-agent"]).output();
        let args: Vec<String> = ["--status-fd=2", "-bsau", "forge@example.com"].iter().map(|s| (*s).to_owned()).collect();
        let asked = Cell::new(0);
        let outcome = sign_with_prompt(
            "gpg",
            &args,
            b"commit payload",
            &|prompt| {
                asked.set(asked.get() + 1);
                assert!(prompt.contains("forge@example.com"), "{prompt}");
                Some("forge-secret".to_owned())
            },
            &env,
        );
        let signature = String::from_utf8_lossy(&outcome.stdout).into_owned();
        assert_eq!(outcome.code, 0, "stderr: {}", String::from_utf8_lossy(&outcome.stderr));
        assert!(signature.contains("BEGIN PGP SIGNATURE"), "{signature}");
        assert_eq!(asked.get(), 1, "the editor was asked once");

        // A cancelled question fails the signing (after the cache is emptied again).
        let _ = Command::new("gpgconf").envs(env).args(["--kill", "gpg-agent"]).output();
        let cancelled = sign_with_prompt("gpg", &args, b"commit payload", &|_| None, &env);
        assert_ne!(cancelled.code, 0);
        assert!(String::from_utf8_lossy(&cancelled.stderr).contains("cancelled"));

        let _ = Command::new("gpgconf").envs(env).args(["--kill", "all"]).output();
        let _ = std::fs::remove_dir_all(&home);
    }
}
