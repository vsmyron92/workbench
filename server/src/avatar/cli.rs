//! `workbench avatar`: how a session without Workbench's MCP tools drives its avatar (OWNER:
//! avatar slice).
//!
//! `workbench avatar guide | status [--look] | say <text…> [--gesture g] [--lang l] [--wait] |
//! ask <text…> [--choice c]… [--no-free-text] [--wait <seconds>] | ask --again <askId> |
//! ask --cancel | act <name> [--mode once|hold|loop] [--times n] | act --stop <name|all> |
//! design <file|->`.
//!
//! It runs inside a hosted session and finds its Workbench the way the status line helper does:
//! `WORKBENCH_URL`, `WORKBENCH_TERMINAL_ID` and `WORKBENCH_AGENT_TOKEN` from the session's
//! environment. The token is sent only to this computer (a loopback URL), in a header, and is
//! never printed. Each form is one call of a hook route (`/api/hooks/avatar/{terminalId}/…`), so
//! the command, the MCP tools and `curl` in a dev container all reach the same state functions
//! and get the same words back.
//!
//! What is drawn never comes from the command line: `design` reads a file, or standard input
//! for `-`. The content is the JSON of a design call, or the artwork alone when it starts with
//! `<`.
//!
//! Output: the answer on standard output (text for guide, say, act and design; JSON for status
//! and ask). Exit status 0: done. 1: Workbench refused the call, or did not answer; the reason
//! is on standard error and says what to do. 2: nothing was sent (not inside a session, no
//! Workbench there, a file that cannot be read, a form this command does not have).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use clap::{Args, Subcommand};
use serde_json::{Map, Value, json};

use super::look::MAX_DESIGN_BYTES;
use super::store::{DEFAULT_WAIT_SECS, MAX_WAIT_HOOK_SECS, MAX_WAIT_TOOL_SECS};

/// `workbench avatar`'s help line.
pub const ABOUT: &str = "Drive this agent session's avatar: for CLIs that have no Workbench tools";

/// Workbench refused the call, or did not answer it.
const REFUSED: i32 = 1;
/// Nothing was sent.
const NOT_SENT: i32 = 2;

/// Workbench is on this computer: it is there at once, or not at all.
const CONNECT: Duration = Duration::from_secs(1);
const WRITE: Duration = Duration::from_secs(10);
/// How long an answer may take when the call waits for nothing.
const PATIENCE: Duration = Duration::from_secs(30);
/// What a call that waits adds to its wait: the server may hold it a little longer.
const MARGIN: u64 = 20;
/// The longest answer read: a status with the stored design is well below it.
const MAX_ANSWER: u64 = 4 * 1024 * 1024;

#[derive(Args, Debug)]
pub struct AvatarArgs {
    #[command(subcommand)]
    pub action: AvatarAction,
}

#[derive(Subcommand, Debug)]
pub enum AvatarAction {
    /// Print the guide (how to draw, rig and animate the avatar, and every form of this command) and the avatar's status.
    Guide,
    /// Print the avatar's status as JSON: its parts, animations, the open question, the last lines.
    Status {
        /// Include the stored design in full, the artwork as SVG.
        #[arg(long)]
        look: bool,
    },
    /// Say one line through the avatar: one or two short plain sentences.
    Say {
        /// The line. Several words are joined with spaces.
        #[arg(required = true)]
        text: Vec<String>,
        /// An animation to play with the line.
        #[arg(long)]
        gesture: Option<String>,
        /// A language tag such as de or pt-BR, for a line in another language.
        #[arg(long)]
        lang: Option<String>,
        /// Return only when the line was spoken, skipped or shown, and say which.
        #[arg(long)]
        wait: bool,
    },
    /// Ask the user one short question and wait for the answer; prints JSON.
    Ask {
        /// The question. Several words are joined with spaces.
        text: Vec<String>,
        /// An answer shown as a button (up to 4 of them).
        #[arg(long = "choice", value_name = "ANSWER")]
        choices: Vec<String>,
        /// Only the choices: no typed or dictated answer.
        #[arg(long)]
        no_free_text: bool,
        /// How long to wait for the answer: 0 to 110 (default 45).
        #[arg(long, value_name = "SECONDS")]
        wait: Option<f64>,
        /// Instead of a question: wait again for the answer to the open question with this id.
        #[arg(long, value_name = "ASK_ID")]
        again: Option<String>,
        /// Instead of a question: withdraw the open one.
        #[arg(long)]
        cancel: bool,
        /// An animation to play with the question.
        #[arg(long)]
        gesture: Option<String>,
        /// A language tag for a question in another language.
        #[arg(long)]
        lang: Option<String>,
    },
    /// Play one of the avatar's animations as a gesture, or stop one.
    Act {
        /// The animation to play.
        name: Option<String>,
        /// once (default), hold (keep the last frame) or loop (an animation that loops).
        #[arg(long)]
        mode: Option<String>,
        /// With mode once: how often in a row, 1 to 5.
        #[arg(long)]
        times: Option<u32>,
        /// Instead of a name: the held or looping gesture to stop, or all.
        #[arg(long, value_name = "NAME")]
        stop: Option<String>,
    },
    /// Design or change the avatar from a file: the JSON of a design, or the artwork alone.
    Design {
        /// The file, or - for standard input.
        file: String,
    },
}

/// The Workbench that hosts this session, from the session's environment.
struct Session {
    /// `host:port` of a loopback URL.
    hostport: String,
    terminal: String,
    token: String,
}

impl Session {
    fn from_env() -> Result<Session, String> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Session::of(var("WORKBENCH_URL"), var("WORKBENCH_TERMINAL_ID"), var("WORKBENCH_AGENT_TOKEN"))
    }

    /// The three values of a session's environment, checked. No message names the token's value.
    fn of(url: Option<String>, terminal: Option<String>, token: Option<String>) -> Result<Session, String> {
        let (Some(url), Some(terminal), Some(token)) = (url, terminal, token) else {
            return Err("workbench avatar works inside a Workbench agent session: WORKBENCH_URL, WORKBENCH_TERMINAL_ID and WORKBENCH_AGENT_TOKEN are not all set here.".to_string());
        };
        // Both become part of the request's head: neither may break out of its line.
        if !crate::terminals::valid_id(&terminal) || token.contains(|c: char| c.is_whitespace() || c.is_control()) {
            return Err("workbench avatar works inside a Workbench agent session: this environment does not hold one's id and token.".to_string());
        }
        // The token goes to this computer only.
        let hostport = url.strip_prefix("http://").map(|h| h.trim_end_matches('/')).unwrap_or_default();
        let host = hostport.rsplit_once(':').map(|(h, _)| h).unwrap_or(hostport);
        if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
            return Err("WORKBENCH_URL does not name this computer, and workbench avatar sends the session's token nowhere else. In a dev container use the curl forms of the guide.".to_string());
        }
        Ok(Session { hostport: hostport.to_string(), terminal, token })
    }
}

/// What a form of the command prints of the answer.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Show {
    /// The body as it is: text, or JSON.
    Body,
    /// The `note` of a JSON answer: what happened, in words.
    Note,
}

/// One call of a hook route.
struct Call {
    method: &'static str,
    /// The route below the session's hooks, with its query.
    path: String,
    /// A media type and the bytes.
    body: Option<(&'static str, Vec<u8>)>,
    patience: Duration,
    show: Show,
}

impl Call {
    fn get(path: &str) -> Call {
        Call { method: "GET", path: path.to_string(), body: None, patience: PATIENCE, show: Show::Body }
    }

    fn post(path: &str, body: Value, show: Show) -> Call {
        Call { method: "POST", path: path.to_string(), body: Some(("application/json", body.to_string().into_bytes())), patience: PATIENCE, show }
    }

    /// The call a form of the command makes. `Err`: the form is incomplete, or its file cannot be read.
    fn of(action: AvatarAction, input: &mut dyn Read) -> Result<Call, String> {
        Ok(match action {
            AvatarAction::Guide => Call::get("/guide"),
            AvatarAction::Status { look } => Call::get(if look { "/status?look=1" } else { "/status" }),
            AvatarAction::Say { text, gesture, lang, wait } => {
                let mut body = Map::new();
                body.insert("text".into(), json!(text.join(" ")));
                put(&mut body, "gesture", gesture);
                put(&mut body, "lang", lang);
                if wait {
                    body.insert("wait".into(), json!(true));
                }
                let mut call = Call::post("/say", Value::Object(body), Show::Note);
                if wait {
                    call.patience = Duration::from_secs(MAX_WAIT_TOOL_SECS + MARGIN);
                }
                call
            }
            AvatarAction::Ask { text, choices, no_free_text, wait, again, cancel, gesture, lang } => {
                let text = Some(text.join(" ")).filter(|t| !t.trim().is_empty());
                if text.is_none() && again.is_none() && !cancel {
                    return Err("ask needs a question, --again <askId> to wait again for the open one, or --cancel to withdraw it.".to_string());
                }
                // Which of them go together is the server's to say, in the words every caller gets.
                let mut body = Map::new();
                put(&mut body, "text", text);
                put(&mut body, "askId", again);
                if cancel {
                    body.insert("cancel".into(), json!(true));
                }
                if !choices.is_empty() {
                    body.insert("choices".into(), json!(choices));
                }
                if no_free_text {
                    body.insert("freeText".into(), json!(false));
                }
                if let Some(seconds) = wait {
                    body.insert("waitSeconds".into(), json!(seconds));
                }
                put(&mut body, "gesture", gesture);
                put(&mut body, "lang", lang);
                let mut call = Call::post("/ask", Value::Object(body), Show::Body);
                let waits = wait.filter(|s| s.is_finite() && *s >= 0.0).map_or(DEFAULT_WAIT_SECS, |s| s.ceil() as u64).min(MAX_WAIT_HOOK_SECS);
                call.patience = Duration::from_secs(waits + MARGIN);
                call
            }
            AvatarAction::Act { name, mode, times, stop } => {
                if name.is_none() && stop.is_none() {
                    return Err("act needs the name of an animation to play, or --stop <name|all>.".to_string());
                }
                let mut body = Map::new();
                put(&mut body, "play", name);
                put(&mut body, "mode", mode);
                if let Some(times) = times {
                    body.insert("times".into(), json!(times));
                }
                put(&mut body, "stop", stop);
                Call::post("/act", Value::Object(body), Show::Note)
            }
            AvatarAction::Design { file } => {
                let mut content = Vec::new();
                // As much as a design may have in the widest encoding read here, and a little
                // more: enough to know that it is too large.
                let most = 2 * MAX_DESIGN_BYTES as u64 + 4;
                let read = if file == "-" {
                    input.take(most).read_to_end(&mut content).map_err(|e| format!("cannot read the design from standard input: {e}"))
                } else {
                    std::fs::File::open(&file).and_then(|f| f.take(most).read_to_end(&mut content)).map_err(|e| format!("cannot read the design file: {e}"))
                };
                read?;
                let content = utf8(content)?;
                if content.len() > MAX_DESIGN_BYTES {
                    return Err(format!("the design is larger than {} KiB: nothing was sent.", MAX_DESIGN_BYTES / 1024));
                }
                if content.iter().all(u8::is_ascii_whitespace) {
                    return Err("the design is empty: write the JSON of a design, or the artwork alone, into the file.".to_string());
                }
                // The artwork alone starts with a tag; a design starts with a brace.
                let artwork = content.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<');
                Call { method: "POST", path: "/design".to_string(), body: Some((if artwork { "image/svg+xml" } else { "application/json" }, content)), patience: PATIENCE, show: Show::Body }
            }
        })
    }
}

/// The content of a design file as UTF-8 without a byte order mark. A file a Windows shell wrote
/// with `>` is UTF-16 with one: it is read as what it is, since the session cannot see why its
/// design "is not JSON".
fn utf8(content: Vec<u8>) -> Result<Vec<u8>, String> {
    let wide = |bytes: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes.chunks_exact(2).map(|pair| unit([pair[0], pair[1]])).collect();
        match String::from_utf16(&units) {
            Ok(text) if bytes.len() % 2 == 0 => Ok(text.into_bytes()),
            _ => Err("the design file is not text: write it as UTF-8.".to_string()),
        }
    };
    match content.as_slice() {
        [0xEF, 0xBB, 0xBF, rest @ ..] => Ok(rest.to_vec()),
        [0xFF, 0xFE, rest @ ..] => wide(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => wide(rest, u16::from_be_bytes),
        _ => Ok(content),
    }
}

fn put(body: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        body.insert(key.to_string(), json!(value));
    }
}

/// An HTTP answer: its status and body.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

/// Why a call has no answer.
enum Failure {
    /// Workbench was not reached: nothing was sent.
    NotSent(String),
    /// The call went out and no answer came back whole.
    NoAnswer(String),
}

/// Makes the call over a connection of its own and reads the whole answer. Loopback only: the
/// session's token is in the request.
fn send(session: &Session, call: &Call) -> Result<Answer, Failure> {
    let nobody = || Failure::NotSent(format!("cannot reach Workbench at http://{}: is it running? Say so once and work without the avatar.", session.hostport));
    let addr = session.hostport.to_socket_addrs().ok().and_then(|mut a| a.next()).ok_or_else(nobody)?;
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT).map_err(|_| nobody())?;
    let _ = stream.set_write_timeout(Some(WRITE));
    let _ = stream.set_read_timeout(Some(call.patience));
    let mut head = format!(
        "{} /api/hooks/avatar/{}{} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nAccept: */*\r\nConnection: close\r\n",
        call.method, session.terminal, call.path, session.hostport, session.token
    );
    match &call.body {
        Some((media, bytes)) => head.push_str(&format!("Content-Type: {media}\r\nContent-Length: {}\r\n\r\n", bytes.len())),
        None => head.push_str("\r\n"),
    }
    let sent = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(call.body.as_ref().map_or(&[][..], |(_, bytes)| bytes))).and_then(|_| stream.flush());
    if sent.is_err() {
        return Err(Failure::NoAnswer("the connection to Workbench broke while the call was sent.".to_string()));
    }
    let mut raw = Vec::new();
    if (&mut stream).take(MAX_ANSWER).read_to_end(&mut raw).is_err() && parse(&raw).is_none() {
        return Err(Failure::NoAnswer(format!("Workbench did not answer within {} seconds. The call may still have been made.", call.patience.as_secs())));
    }
    parse(&raw).ok_or_else(|| Failure::NoAnswer("Workbench's answer could not be read.".to_string()))
}

/// The status and body of an HTTP/1.1 answer that was read to its end.
fn parse(raw: &[u8]) -> Option<Answer> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let (mut chunked, mut length) = (false, None);
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        match name.trim().to_ascii_lowercase().as_str() {
            "transfer-encoding" => chunked = value.to_ascii_lowercase().contains("chunked"),
            "content-length" => length = value.trim().parse::<usize>().ok(),
            _ => {}
        }
    }
    let rest = &raw[end + 4..];
    let body = match (chunked, length) {
        (true, _) => unchunked(rest)?,
        // Less than was announced: the answer was cut off.
        (false, Some(n)) => rest.get(..n)?.to_vec(),
        (false, None) => rest.to_vec(),
    };
    Some(Answer { status, body })
}

/// A chunked body put together.
fn unchunked(mut rest: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = rest.windows(2).position(|w| w == b"\r\n")?;
        let size = std::str::from_utf8(&rest[..line_end]).ok()?.split(';').next()?.trim();
        let size = usize::from_str_radix(size, 16).ok()?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(rest.get(..size)?);
        rest = rest.get(size + 2..)?;
    }
}

/// What a refusal says: the message of Workbench's error answer, or what came instead.
fn refusal(answer: &Answer) -> String {
    let text = String::from_utf8_lossy(&answer.body);
    match serde_json::from_str::<Value>(&text).ok().as_ref().and_then(|v| v.pointer("/error/message")).and_then(Value::as_str) {
        Some(message) => message.to_string(),
        None => {
            let first: String = text.lines().next().unwrap_or_default().chars().take(200).collect();
            format!("Workbench answered {}{}", answer.status, if first.is_empty() { String::new() } else { format!(": {first}") })
        }
    }
}

/// One run of the command: the call, then what to print. Returns the exit status.
fn run(action: AvatarAction, session: Result<Session, String>, input: &mut dyn Read, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let session = match session {
        Ok(session) => session,
        Err(why) => {
            let _ = writeln!(err, "{why}");
            return NOT_SENT;
        }
    };
    let call = match Call::of(action, input) {
        Ok(call) => call,
        Err(why) => {
            let _ = writeln!(err, "{why}");
            return NOT_SENT;
        }
    };
    let answer = match send(&session, &call) {
        Ok(answer) => answer,
        Err(Failure::NotSent(why)) => {
            let _ = writeln!(err, "{why}");
            return NOT_SENT;
        }
        Err(Failure::NoAnswer(why)) => {
            let _ = writeln!(err, "{why}");
            return REFUSED;
        }
    };
    if !(200..300).contains(&answer.status) {
        let _ = writeln!(err, "{}", refusal(&answer));
        return REFUSED;
    }
    let text = String::from_utf8_lossy(&answer.body);
    let shown = match call.show {
        Show::Body => text.trim_end().to_string(),
        Show::Note => serde_json::from_str::<Value>(&text).ok().and_then(|v| v.get("note").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| text.trim_end().to_string()),
    };
    let _ = writeln!(out, "{shown}");
    0
}

/// `workbench avatar …`.
pub fn cli(args: AvatarArgs) -> anyhow::Result<()> {
    let code = {
        let (mut input, mut out, mut err) = (std::io::stdin().lock(), std::io::stdout().lock(), std::io::stderr().lock());
        let code = run(args.action, Session::from_env(), &mut input, &mut out, &mut err);
        let _ = out.flush();
        let _ = err.flush();
        code
    };
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A session's environment as the terminals slice sets it, for the command's tests.
    pub fn session(url: &str, terminal: &str, token: &str) -> Result<(), String> {
        Session::of(Some(url.to_string()), Some(terminal.to_string()), Some(token.to_string())).map(|_| ())
    }

    /// Runs the command as a process would: its exit status, standard output and standard error.
    pub fn run_with(action: AvatarAction, env: [Option<&str>; 3], stdin: &str) -> (i32, String, String) {
        let [url, terminal, token] = env.map(|v| v.map(str::to_string));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(action, Session::of(url, terminal, token), &mut stdin.as_bytes(), &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    /// The command line of `workbench avatar <words…>` read as clap reads it.
    pub fn parsed(words: &[&str]) -> Result<AvatarAction, String> {
        use clap::Parser;
        #[derive(Parser)]
        struct Line {
            #[command(subcommand)]
            action: AvatarAction,
        }
        Line::try_parse_from(std::iter::once("avatar").chain(words.iter().copied())).map(|line| line.action).map_err(|e| e.to_string())
    }

    const TID: &str = "0a297845bc8b";
    const TOKEN: &str = "wba_not-a-real-token";

    #[test]
    fn workbench_has_the_avatar_command() {
        use clap::{CommandFactory, Parser};
        // The whole command line of the program still holds together with it.
        crate::Cli::command().debug_assert();
        let cli = crate::Cli::try_parse_from(["workbench", "avatar", "say", "Tests", "are", "green.", "--wait"]).unwrap();
        let Some(crate::Command::Avatar(args)) = cli.command else { panic!("avatar is not a command of workbench") };
        assert!(matches!(&args.action, AvatarAction::Say { text, wait: true, gesture: None, lang: None } if text.join(" ") == "Tests are green."));
        // Its help names every form and what the command is for.
        let mut workbench = crate::Cli::command();
        let help = workbench.find_subcommand_mut("avatar").expect("the subcommand").render_long_help().to_string();
        assert!(help.starts_with(ABOUT), "{help}");
        for form in ["guide", "status", "say", "ask", "act", "design"] {
            assert!(help.lines().any(|line| line.trim_start().starts_with(form)), "the help lists {form}:\n{help}");
        }
    }

    #[test]
    fn outside_a_session_it_says_so_and_sends_nothing() {
        for env in [[None, None, None], [Some("http://127.0.0.1:9"), Some(TID), None], [Some("http://127.0.0.1:9"), None, Some(TOKEN)]] {
            let (code, out, err) = run_with(AvatarAction::Guide, env, "");
            assert_eq!((code, out.as_str()), (NOT_SENT, ""), "{err}");
            assert!(err.contains("inside a Workbench agent session"), "{err}");
        }
        // An id or a token that could not be one.
        assert!(session("http://127.0.0.1:7777", "../../etc", TOKEN).is_err());
        assert!(session("http://127.0.0.1:7777", TID, "wba_x\r\nX-Injected: 1").is_err());
        assert!(session("http://127.0.0.1:7777", TID, TOKEN).is_ok());
        assert!(session("http://localhost:7777/", TID, TOKEN).is_ok() && session("http://[::1]:7777", TID, TOKEN).is_ok());
    }

    #[test]
    fn the_token_goes_to_this_computer_only() {
        for url in ["http://172.17.0.1:7777", "https://127.0.0.1:7777", "http://127.0.0.1.example.com:7777", "http://example.com", "127.0.0.1:7777", "http://user:pw@127.0.0.1:7777"] {
            let (code, out, err) = run_with(AvatarAction::Guide, [Some(url), Some(TID), Some(TOKEN)], "");
            assert_eq!((code, out.as_str()), (NOT_SENT, ""), "{url}: {err}");
            assert!(err.contains("does not name this computer") && !err.contains(TOKEN), "{url}: {err}");
        }
    }

    #[test]
    fn no_workbench_there_is_said_without_the_token() {
        // A port nobody listens on.
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let url = format!("http://{free}");
        let (code, out, err) = run_with(AvatarAction::Say { text: vec!["Hello.".into()], gesture: None, lang: None, wait: false }, [Some(&url), Some(TID), Some(TOKEN)], "");
        assert_eq!((code, out.as_str()), (NOT_SENT, ""), "{err}");
        assert!(err.contains("cannot reach Workbench") && err.contains("work without the avatar") && !err.contains(TOKEN), "{err}");
    }

    #[test]
    fn a_form_that_is_incomplete_sends_nothing() {
        let env = [Some("http://127.0.0.1:9"), Some(TID), Some(TOKEN)];
        let (code, _, err) = run_with(parsed(&["act"]).unwrap(), env, "");
        assert_eq!(code, NOT_SENT);
        assert!(err.contains("--stop"), "{err}");
        let (code, _, err) = run_with(parsed(&["ask"]).unwrap(), env, "");
        assert_eq!(code, NOT_SENT);
        assert!(err.contains("--again") && err.contains("--cancel"), "{err}");
        let (code, _, err) = run_with(parsed(&["design", "/nonexistent/look.json"]).unwrap(), env, "");
        assert_eq!(code, NOT_SENT);
        assert!(err.contains("cannot read the design file"), "{err}");
        let (code, _, err) = run_with(parsed(&["design", "-"]).unwrap(), env, "  \n");
        assert_eq!(code, NOT_SENT);
        assert!(err.contains("empty"), "{err}");
        let (code, _, err) = run_with(parsed(&["design", "-"]).unwrap(), env, &"x".repeat(MAX_DESIGN_BYTES + 1));
        assert_eq!(code, NOT_SENT);
        assert!(err.contains("larger than 256 KiB"), "{err}");
        // What clap itself refuses.
        assert!(parsed(&["say"]).is_err() && parsed(&["design"]).is_err() && parsed(&["dance"]).is_err());
    }

    #[test]
    fn each_form_makes_the_call_of_its_route() {
        let call = |words: &[&str]| Call::of(parsed(words).unwrap(), &mut "{\"name\": \"Tin\"}".as_bytes()).unwrap_or_else(|why| panic!("{words:?}: {why}"));
        let body = |call: &Call| serde_json::from_slice::<Value>(&call.body.as_ref().unwrap().1).unwrap();

        let c = call(&["guide"]);
        assert_eq!((c.method, c.path.as_str(), c.body.is_none(), c.show), ("GET", "/guide", true, Show::Body));
        assert_eq!(call(&["status"]).path, "/status");
        assert_eq!(call(&["status", "--look"]).path, "/status?look=1");

        let c = call(&["say", "Tests", "are", "green."]);
        assert_eq!((c.method, c.path.as_str(), c.show, c.patience), ("POST", "/say", Show::Note, PATIENCE));
        assert_eq!(body(&c), json!({ "text": "Tests are green." }));
        let c = call(&["say", "Fertig.", "--lang", "de", "--gesture", "hop", "--wait"]);
        assert_eq!(body(&c), json!({ "text": "Fertig.", "lang": "de", "gesture": "hop", "wait": true }));
        assert!(c.patience > Duration::from_secs(MAX_WAIT_TOOL_SECS));

        let c = call(&["ask", "Deploy now?", "--choice", "Yes", "--choice", "Not yet", "--no-free-text", "--wait", "60"]);
        assert_eq!((c.path.as_str(), c.show), ("/ask", Show::Body));
        assert_eq!(body(&c), json!({ "text": "Deploy now?", "choices": ["Yes", "Not yet"], "freeText": false, "waitSeconds": 60.0 }));
        assert_eq!(c.patience, Duration::from_secs(60 + MARGIN));
        assert_eq!(call(&["ask", "Shall I?"]).patience, Duration::from_secs(DEFAULT_WAIT_SECS + MARGIN));
        assert_eq!(body(&call(&["ask", "--again", "Qab12-3", "--wait", "5"])), json!({ "askId": "Qab12-3", "waitSeconds": 5.0 }));
        assert_eq!(body(&call(&["ask", "--cancel"])), json!({ "cancel": true }));
        // Never held longer than the server holds a hook request.
        assert_eq!(call(&["ask", "Shall I?", "--wait", "100000"]).patience, Duration::from_secs(MAX_WAIT_HOOK_SECS + MARGIN));

        assert_eq!(body(&call(&["act", "hop"])), json!({ "play": "hop" }));
        assert_eq!(body(&call(&["act", "hop", "--mode", "hold"])), json!({ "play": "hop", "mode": "hold" }));
        assert_eq!(body(&call(&["act", "hop", "--times", "3"])), json!({ "play": "hop", "times": 3 }));
        assert_eq!(body(&call(&["act", "--stop", "all"])), json!({ "stop": "all" }));

        // A design is read from standard input or a file, as it is: JSON, or the artwork alone.
        let c = call(&["design", "-"]);
        assert_eq!((c.path.as_str(), c.body.as_ref().map(|(media, bytes)| (*media, bytes.as_slice()))), ("/design", Some(("application/json", "{\"name\": \"Tin\"}".as_bytes()))));
        let c = Call::of(parsed(&["design", "-"]).unwrap(), &mut "\n <svg viewBox='0 0 200 200'/>".as_bytes()).unwrap();
        assert_eq!(c.body.as_ref().map(|(media, _)| *media), Some("image/svg+xml"));
    }

    #[test]
    fn a_design_file_is_read_in_the_encoding_a_shell_wrote_it_in() {
        let design = "{\"name\": \"Zo\u{eb}\"}";
        let sent = |bytes: &[u8]| Call::of(parsed(&["design", "-"]).unwrap(), &mut &bytes[..]).map(|call| call.body.unwrap());
        // UTF-8 with a byte order mark, and UTF-16 in either order (what `>` writes in Windows PowerShell).
        let marked = [&[0xEF, 0xBB, 0xBF][..], design.as_bytes()].concat();
        let little: Vec<u8> = [0xFF, 0xFE].into_iter().chain(design.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        let big: Vec<u8> = [0xFE, 0xFF].into_iter().chain(design.encode_utf16().flat_map(u16::to_be_bytes)).collect();
        for bytes in [design.as_bytes(), marked.as_slice(), little.as_slice(), big.as_slice()] {
            assert_eq!(sent(bytes).unwrap(), ("application/json", design.as_bytes().to_vec()));
        }
        let artwork: Vec<u8> = [0xFF, 0xFE].into_iter().chain("<svg viewBox='0 0 200 200'/>".encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(sent(&artwork).unwrap().0, "image/svg+xml");
        // Half a character, or a lone surrogate: not text.
        assert!(sent(&[0xFF, 0xFE, 0x7B]).is_err_and(|why| why.contains("write it as UTF-8")));
        assert!(sent(&[0xFF, 0xFE, 0x00, 0xD8]).is_err_and(|why| why.contains("write it as UTF-8")));
        // The limit is that of the design itself, whatever its encoding: twice the bytes in UTF-16.
        let large = format!("{{\"name\": \"{}\"}}", "x".repeat(MAX_DESIGN_BYTES - 40));
        let wide: Vec<u8> = [0xFF, 0xFE].into_iter().chain(large.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert!(wide.len() > MAX_DESIGN_BYTES && sent(&wide).is_ok_and(|(_, bytes)| bytes.len() == large.len()));
        let too_large = format!("{large}{}", " ".repeat(64));
        assert!(sent(too_large.as_bytes()).is_err_and(|why| why.contains("larger than 256 KiB")));
    }

    #[test]
    fn an_answer_is_read_whole_in_either_framing() {
        let plain = parse(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello").unwrap();
        assert_eq!((plain.status, plain.body.as_slice()), (200, &b"hello"[..]));
        let chunked = parse(b"HTTP/1.1 409 Conflict\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;x=1\r\n world\r\n0\r\n\r\n").unwrap();
        assert_eq!((chunked.status, chunked.body.as_slice()), (409, &b"hello world"[..]));
        let closed = parse(b"HTTP/1.0 200 OK\r\n\r\nuntil the end").unwrap();
        assert_eq!(closed.body, b"until the end");
        // Cut off, or not HTTP at all.
        assert!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\nhello").is_none());
        assert!(parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel").is_none());
        assert!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n").is_none() && parse(b"").is_none() && parse(b"garbage\r\n\r\n").is_none());

        let refused = |body: &str| refusal(&Answer { status: 409, body: body.as_bytes().to_vec() });
        assert_eq!(refused("{\"error\":{\"code\":\"conflict\",\"message\":\"The user turned this session's avatar off.\"}}"), "The user turned this session's avatar off.");
        assert_eq!(refused("length limit exceeded\nmore"), "Workbench answered 409: length limit exceeded");
        assert_eq!(refused(""), "Workbench answered 409");
    }
}
