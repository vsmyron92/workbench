//! Pictures are drawn in a process of their own (OWNER: avatar slice).
//!
//! A look is written by a session, and drawing it means handing it to a renderer that is not
//! ours. `preview` estimates what a picture costs before it draws and refuses with a sentence,
//! but an estimate of another program's memory is not a bound: twice a look got past it and asked
//! the renderer for more memory than there is, and an allocation that fails ends the whole
//! process. So the server never draws. It starts this same program again as `workbench
//! avatar-draw`, gives it the look and what to draw on standard input, and reads the PNG from its
//! standard output. That process limits itself before it reads a byte: its address space, its
//! processor time, no core file, the lowest priority, and it ends with its parent. The server
//! waits a few seconds and then kills it; a caller that goes away takes its drawing with it.
//! Whatever the renderer does with a look (an allocation that fails, a loop that does not end, a
//! panic), one picture is lost and the server says so in a sentence.
//!
//! On Windows there is no limit a process can put on itself this way: the wait and the kill
//! hold there, and an allocation that fails still ends only the drawing.
//!
//! The answer of the child: status 0 and the PNG on standard output, with what it cost and what
//! it showed as one line of JSON on standard error ([`report`]); status [`EXIT_REFUSED`] and a
//! sentence for the session on standard error; anything else (a signal, another status) means
//! the drawing died of the look.

use std::io::{Read, Write};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use super::preview::{self, Chosen, Inputs, Moment, Picture, Played, Pose, Spent};

/// The subcommand that draws: `workbench avatar-draw`.
pub const COMMAND: &str = "avatar-draw";
/// The child's status for a picture it refuses with a sentence.
pub const EXIT_REFUSED: i32 = 3;

/// The address space a drawing may have. The program itself takes a tenth of it; the layers of
/// a picture that `preview` lets through take a few megabytes.
#[cfg(any(unix, test))]
const CHILD_MEMORY_BYTES: u64 = 1024 * 1024 * 1024;
/// The processor time a drawing may take. A sheet of an ordinary look takes a few hundredths of
/// a second.
#[cfg(unix)]
const CHILD_CPU_SECS: u64 = 6;
/// How long the server waits for a drawing before it kills it.
pub const DRAW_WAIT: Duration = Duration::from_secs(8);
/// The most a request to the child holds: a stored look is at most 128 KiB.
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;
/// The most the server reads of a child's answer.
const MAX_ANSWER_BYTES: usize = 8 * 1024 * 1024;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// What to draw of a look.
#[derive(Debug, Clone, PartialEq)]
pub enum Want {
    /// The figure in one pose, on the light and on the dark panel.
    Sheet(Pose),
    /// Moments of one of its animations (its own, or a built-in one), with these inputs held, and
    /// close up on the part `zoom` names, where it names one.
    Strip { animation: String, frames: usize, inputs: Inputs, zoom: Option<String> },
}

impl Want {
    fn to_json(&self) -> (&'static str, Value) {
        match self {
            Want::Sheet(pose) => ("sheet", pose.to_json()),
            Want::Strip { animation, frames, inputs, zoom } => {
                let mut strip = json!({ "animation": animation, "frames": frames, "inputs": Value::Object(inputs.to_json()) });
                if let Some(zoom) = zoom {
                    strip["zoom"] = json!(zoom);
                }
                ("strip", strip)
            }
        }
    }

    fn from_json(request: &Value) -> Result<Want, String> {
        if let Some(pose) = request.get("sheet") {
            return Pose::from_json(pose).map(Want::Sheet);
        }
        let strip = request.get("strip").ok_or("nothing to draw was named")?;
        let animation = strip.get("animation").and_then(Value::as_str).ok_or("a strip names an animation")?.to_string();
        let frames = strip.get("frames").and_then(Value::as_u64).ok_or("a strip names its frames")? as usize;
        let inputs = Inputs::from_json(strip.get("inputs").unwrap_or(&Value::Null))?;
        let zoom = match strip.get("zoom") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) => Some(id.clone()),
            Some(_) => return Err("zoom in a pose is the id of the part to look at closely".to_string()),
        };
        Ok(Want::Strip { animation, frames, inputs, zoom })
    }
}

/// What the child says beside a picture, as one line of JSON: what drawing it costs (`filters`,
/// `layers`), the same at rest (`rest`), and what it showed of an animation (`builtin`, `loop`,
/// and for a pose its moment: `at`, `chosen`, `still`).
fn report(picture: &Picture) -> Value {
    let mut out = json!({
        "filters": picture.spent.filters,
        "layers": picture.spent.layers,
        "rest": { "filters": picture.rest.filters, "layers": picture.rest.layers },
    });
    if let Some(played) = &picture.played {
        out["builtin"] = json!(played.builtin);
        out["loop"] = json!(played.looping);
        if let Some(moment) = &played.moment {
            out["at"] = json!(moment.at);
            out["chosen"] = json!(match moment.chosen {
                Chosen::Given => "given",
                Chosen::Furthest => "furthest",
                Chosen::Unmoved => "unmoved",
            });
            out["still"] = json!(moment.still);
        }
    }
    out
}

/// [`report`] read back with the picture. What it does not say reads as it did before the child
/// said it: no cost is 0, the cost at rest is the cost, and without `builtin` nothing is known of
/// an animation (the caption then asks the look).
fn reported(png: Vec<u8>, line: &str) -> Picture {
    let said = serde_json::from_str::<Value>(line).unwrap_or(Value::Null);
    let spent_of = |v: &Value| Spent { filters: v["filters"].as_f64().unwrap_or(0.0), layers: v["layers"].as_f64().unwrap_or(0.0) };
    let spent = spent_of(&said);
    let rest = if said["rest"].is_object() { spent_of(&said["rest"]) } else { spent };
    let moment = said["at"].as_f64().filter(|at| (0.0..=1.0).contains(at)).map(|at| Moment {
        at,
        chosen: match said["chosen"].as_str() {
            Some("furthest") => Chosen::Furthest,
            Some("unmoved") => Chosen::Unmoved,
            _ => Chosen::Given,
        },
        still: said["still"].as_bool().unwrap_or(false),
    });
    let played = said["builtin"].as_bool().map(|builtin| Played { builtin, looping: said["loop"].as_bool().unwrap_or(false), moment });
    Picture { png, spent, rest, played }
}

/// Why there is no picture.
#[derive(Debug, Clone, PartialEq)]
pub enum NotDrawn {
    /// The renderer's own answer: a sentence the session can act on (it may quote the session).
    Refused(String),
    /// The drawing ran out of the memory or the time a picture may take, or died of the look.
    TooCostly,
    /// The drawing could not be started or read: nothing is known about the look.
    Unavailable(String),
    /// No drawing was started: the turns are taken. The sentence says to ask again.
    Busy(String),
}

/// What a session is told when a drawing died of its look.
pub const TOO_COSTLY: &str = "this look could not be drawn within the memory and the time a picture may take: draw it more simply (filters on parts that are stretched or skewed, shapes or texts scaled far beyond the figure, masks or clips on many parts)";

impl NotDrawn {
    /// The sentence for the session.
    pub fn sentence(&self) -> String {
        match self {
            NotDrawn::Refused(why) => why.clone(),
            NotDrawn::TooCostly => TOO_COSTLY.to_string(),
            NotDrawn::Unavailable(why) => format!("Workbench could not draw the picture just now ({why}): ask again in a moment"),
            NotDrawn::Busy(why) => why.clone(),
        }
    }
}

/// How the child is started: this very program, with the one word that makes it draw.
fn command() -> std::io::Result<tokio::process::Command> {
    #[cfg(not(test))]
    {
        // The file this process runs, also after an update put another one in its place.
        #[cfg(target_os = "linux")]
        let program = std::path::PathBuf::from("/proc/self/exe");
        #[cfg(not(target_os = "linux"))]
        let program = std::env::current_exe()?;
        let mut command = tokio::process::Command::new(program);
        command.arg(COMMAND);
        Ok(command)
    }
    // Under test this program is the test harness: it is started on the one test that is the
    // child (`tests::the_child_a_test_starts`), which does what the subcommand does.
    #[cfg(test)]
    {
        let mut command = tokio::process::Command::new(std::env::current_exe()?);
        command.args(["avatar::draw::tests::the_child_a_test_starts", "--exact", "--ignored", "--nocapture", "--test-threads", "1"]).env(tests::CHILD_MARK, "1");
        Ok(command)
    }
}

/// Draws `want` of the stored look in a process of its own and waits for it at most
/// [`DRAW_WAIT`]. Dropping the future kills the drawing.
pub async fn draw(look: &Value, want: &Want) -> Result<Picture, NotDrawn> {
    let (key, value) = want.to_json();
    let request = serde_json::to_vec(&json!({ "look": look, key: value })).map_err(|e| NotDrawn::Unavailable(e.to_string()))?;
    let mut command = command().map_err(|e| NotDrawn::Unavailable(e.to_string()))?;
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = command.spawn().map_err(|e| NotDrawn::Unavailable(format!("the drawing did not start: {e}")))?;
    let mut stdin = child.stdin.take();
    let answered = async {
        if let Some(mut stdin) = stdin.take() {
            // A child that died before it read is told by its status, not by this error.
            let _ = stdin.write_all(&request).await;
            let _ = stdin.shutdown().await;
        }
        child.wait_with_output().await
    };
    // Past the wait the future is dropped, and the child with it: it is killed.
    let output = match tokio::time::timeout(DRAW_WAIT, answered).await {
        Err(_) => return Err(NotDrawn::TooCostly),
        Ok(Err(e)) => return Err(NotDrawn::Unavailable(format!("the drawing could not be read: {e}"))),
        Ok(Ok(output)) => output,
    };
    read_answer(output.status.code(), &output.stdout, &output.stderr)
}

/// What a child's status and output mean.
fn read_answer(status: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Result<Picture, NotDrawn> {
    let said = String::from_utf8_lossy(stderr);
    let last = said.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("").trim();
    match status {
        Some(0) => {
            // Under test the harness prints a line of its own before the child's bytes.
            let from = if cfg!(test) { stdout.windows(PNG_SIGNATURE.len()).position(|w| w == PNG_SIGNATURE) } else { stdout.starts_with(PNG_SIGNATURE).then_some(0) };
            let Some(from) = from.filter(|_| stdout.len() <= MAX_ANSWER_BYTES) else {
                return Err(NotDrawn::Unavailable("the drawing answered with no picture".to_string()));
            };
            Ok(reported(stdout[from..].to_vec(), last))
        }
        Some(EXIT_REFUSED) if !last.is_empty() => Err(NotDrawn::Refused(last.chars().take(600).collect())),
        // A signal (no status), the status of an abort or of a panic: the drawing died of the look.
        _ => Err(NotDrawn::TooCostly),
    }
}

// ---------------------------------------------------------------- the child

/// `workbench avatar-draw`: reads `{look, sheet | strip}` from standard input, draws, writes the
/// PNG to standard output, and ends the process with the status that says what happened.
pub fn child() -> ! {
    limit_this_process();
    // A panic of the renderer is an answer like any other here: no trace on standard error.
    std::panic::set_hook(Box::new(|_| {}));
    let mut request = Vec::new();
    let read = std::io::stdin().lock().take(MAX_REQUEST_BYTES).read_to_end(&mut request);
    let drawn = match read {
        Ok(_) => std::panic::catch_unwind(|| answer(&request)).unwrap_or_else(|_| Err(String::new())),
        Err(_) => Err(String::new()),
    };
    match drawn {
        Ok(picture) => {
            let mut out = std::io::stdout().lock();
            let written = out.write_all(&picture.png).and_then(|_| out.flush());
            eprintln!("{}", report(&picture));
            std::process::exit(if written.is_ok() { 0 } else { 1 })
        }
        // No sentence: the drawing failed in a way that says nothing to a session.
        Err(why) if why.is_empty() => std::process::exit(1),
        Err(why) => {
            eprintln!("{}", why.replace(['\n', '\r'], " "));
            std::process::exit(EXIT_REFUSED)
        }
    }
}

fn answer(request: &[u8]) -> Result<Picture, String> {
    let request: Value = serde_json::from_slice(request).map_err(|_| String::new())?;
    #[cfg(test)]
    tests::misbehave(&request);
    let look = request.get("look").ok_or_else(String::new)?;
    match Want::from_json(&request)? {
        Want::Sheet(pose) => preview::draw_sheet(look, &pose),
        Want::Strip { animation, frames, inputs, zoom } => preview::draw_strip(look, &animation, frames, &inputs, zoom.as_deref()),
    }
}

/// Before anything of a look is read: this process may use so much memory and so much processor
/// time and no more, writes no core file when it dies, yields to everything else, and does not
/// outlive the server that started it.
#[cfg(unix)]
fn limit_this_process() {
    let limit = |resource, most: u64| {
        let rlimit = libc::rlimit { rlim_cur: most as libc::rlim_t, rlim_max: most as libc::rlim_t };
        // SAFETY: setrlimit reads the struct it is given and changes this process's own limits.
        unsafe { libc::setrlimit(resource, &rlimit) };
    };
    limit(libc::RLIMIT_AS, CHILD_MEMORY_BYTES);
    limit(libc::RLIMIT_CPU, CHILD_CPU_SECS);
    limit(libc::RLIMIT_CORE, 0);
    // SAFETY: both calls take plain numbers and act on this process alone.
    unsafe {
        #[cfg(target_os = "linux")]
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong);
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
    }
}

#[cfg(not(unix))]
fn limit_this_process() {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set for the one run of this test program that is a drawing child.
    pub const CHILD_MARK: &str = "WORKBENCH_AVATAR_DRAW_CHILD";

    /// Not a test: what `command()` starts under test. Without the mark it does nothing.
    #[test]
    #[ignore]
    fn the_child_a_test_starts() {
        if std::env::var_os(CHILD_MARK).is_some() {
            child();
        }
    }

    /// Under test a request may ask the child to do what a renderer gone wrong does, so that the
    /// limits are seen to hold whatever the estimate lets through.
    pub fn misbehave(request: &Value) {
        match request.get("misbehave").and_then(Value::as_str) {
            // More memory than the process may have: the allocation fails and the process aborts.
            Some("allocate") => {
                let much: Vec<u8> = vec![1; 4 * CHILD_MEMORY_BYTES as usize];
                std::hint::black_box(&much);
            }
            // Work without an end: the processor time runs out, or the server's wait.
            Some("spin") => loop {
                std::hint::black_box(0u64);
            },
            Some("panic") => panic!("the renderer gave up"),
            _ => {}
        }
    }

    /// `draw` with a request of the test's own making.
    #[cfg(unix)]
    async fn ask(request: Value) -> Result<Picture, NotDrawn> {
        let mut command = command().unwrap();
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(&serde_json::to_vec(&request).unwrap()).await.unwrap();
        drop(stdin);
        match tokio::time::timeout(DRAW_WAIT, child.wait_with_output()).await {
            Err(_) => Err(NotDrawn::TooCostly),
            Ok(output) => {
                let output = output.unwrap();
                read_answer(output.status.code(), &output.stdout, &output.stderr)
            }
        }
    }

    /// Whatever goes wrong inside the drawing, it is the drawing that ends: too much memory at
    /// once, work that does not end, a panic.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_limits_hold_whatever_the_renderer_does() {
        let started = std::time::Instant::now();
        assert_eq!(ask(json!({ "look": dot(), "sheet": {}, "misbehave": "allocate" })).await, Err(NotDrawn::TooCostly));
        assert!(started.elapsed() < Duration::from_secs(4), "an allocation beyond the limit fails at once: {:?}", started.elapsed());
        assert_eq!(ask(json!({ "look": dot(), "sheet": {}, "misbehave": "panic" })).await, Err(NotDrawn::TooCostly));
        let started = std::time::Instant::now();
        assert_eq!(ask(json!({ "look": dot(), "sheet": {}, "misbehave": "spin" })).await, Err(NotDrawn::TooCostly));
        let took = started.elapsed();
        assert!(took >= Duration::from_secs(CHILD_CPU_SECS - 1) && took <= DRAW_WAIT + Duration::from_secs(2), "ended by its processor time or the wait: {took:?}");
        // A request without a look, or that is no JSON, is no picture and no sentence either.
        assert_eq!(ask(json!({ "sheet": {} })).await, Err(NotDrawn::TooCostly));
        assert_eq!(ask(json!({ "look": dot(), "sheet": {} })).await.map(|p| p.png.starts_with(PNG_SIGNATURE)), Ok(true));
    }

    /// A caller that goes away takes its drawing with it: the child is killed, not left to run.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_drawing_nobody_waits_for_is_killed() {
        let mut command = command().unwrap();
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let pid = child.id().unwrap() as libc::pid_t;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(&serde_json::to_vec(&json!({ "look": dot(), "sheet": {}, "misbehave": "spin" })).unwrap()).await.unwrap();
        drop(stdin);
        tokio::time::sleep(Duration::from_millis(300)).await;
        // SAFETY: signal 0 only asks whether the process is there.
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "the child runs");
        drop(child);
        let gone = || unsafe { libc::kill(pid, 0) } != 0 || std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| stat.contains(") Z "));
        for _ in 0..100 {
            if gone() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(gone(), "the child is gone, or dead and waiting to be collected");
    }

    /// Every design of a directory through the validator and then through the drawing process, at
    /// rest and inside each of its animations: a collection of looks that broke the renderer or
    /// kept it busy for minutes. `AVATAR_DRAW_CORPUS=/some/dir[:/another] cargo test
    /// avatar::draw::tests::a_collection -- --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn a_collection_of_looks_that_broke_the_renderer_breaks_nothing_now() {
        let dirs = std::env::var("AVATAR_DRAW_CORPUS").expect("AVATAR_DRAW_CORPUS");
        let (mut drawn, mut refused, mut costly, mut not_designs, mut slowest) = (0, 0, 0, 0, Duration::ZERO);
        for dir in dirs.split(':') {
            let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
            files.sort();
            for file in files {
                let Ok(design) = serde_json::from_slice::<Value>(&std::fs::read(&file).unwrap()) else { continue };
                // A design call, or a look as it is stored.
                let look = match crate::avatar::look::apply(None, &design) {
                    Ok(applied) => match applied.design.look {
                        Some(look) => serde_json::to_value(look).unwrap(),
                        None => continue,
                    },
                    Err(_) if design.get("nodes").is_some() => design.clone(),
                    Err(_) => {
                        not_designs += 1;
                        continue;
                    }
                };
                let mut wants = vec![Want::Sheet(Pose::default())];
                for name in look.get("animations").and_then(Value::as_object).into_iter().flat_map(|a| a.keys()) {
                    wants.push(Want::Sheet(Pose { animation: Some((name.clone(), Some(0.5))), ..Pose::default() }));
                    wants.push(Want::Strip { animation: name.clone(), frames: 6, inputs: Inputs::default(), zoom: None });
                }
                wants.push(Want::Sheet(Pose { inputs: Inputs { look_x: 1.0, look_y: -1.0, mouth: Some(1.0), pace: 1.0, turn: -1.0, held: 1.0 }, ..Pose::default() }));
                for want in wants {
                    let started = std::time::Instant::now();
                    let outcome = draw(&look, &want).await;
                    let took = started.elapsed();
                    slowest = slowest.max(took);
                    assert!(took <= DRAW_WAIT + Duration::from_secs(2), "{}: {took:?}", file.display());
                    match outcome {
                        Ok(_) => drawn += 1,
                        Err(NotDrawn::Refused(_)) => refused += 1,
                        Err(NotDrawn::TooCostly) => {
                            costly += 1;
                            eprintln!("died or ran out of time after {took:?}: {} {want:?}", file.display());
                        }
                        Err(other) => panic!("{}: {other:?}", file.display()),
                    }
                }
            }
        }
        eprintln!("{drawn} drawn, {refused} refused with a sentence, {costly} ended by the limits, {not_designs} files that are no design; the slowest took {slowest:?}");
        assert!(draw(&dot(), &Want::Sheet(Pose::default())).await.is_ok());
    }

    fn look(svg_nodes: Value) -> Value {
        json!({ "viewBox": [0, 0, 200, 200], "nodes": svg_nodes })
    }

    fn dot() -> Value {
        look(json!([{ "tag": "circle", "id": "dot", "attrs": { "cx": 100, "cy": 100, "r": 40, "fill": "#3366cc" } }]))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_look_is_drawn_by_a_process_of_its_own() {
        let picture = draw(&dot(), &Want::Sheet(Pose::default())).await.unwrap();
        assert!(picture.png.starts_with(PNG_SIGNATURE), "{} bytes", picture.png.len());
        // The same picture as the renderer draws in this process.
        assert_eq!(picture.png, preview::sheet(&dot(), &Pose::default()).unwrap());
        // And what it says beside it: the cost in the pose and at rest, the animation and the
        // moment the child chose where the pose names none.
        let mut hazy = look(json!([
            { "tag": "g", "id": "hub", "attrs": { "opacity": 0.5 }, "box": [60, 60, 80, 80], "children": [{ "tag": "circle", "attrs": { "cx": 100, "cy": 100, "r": 40, "fill": "#3366cc" } }] }
        ]));
        hazy["animations"] = json!({ "grow": { "duration": 1, "tracks": [{ "target": "hub", "prop": "scale", "values": [1, 2, 1] }] } });
        // Numbers travel as JSON text: equal to the last few digits.
        let near = |a: Spent, b: Spent| (a.filters - b.filters).abs() < 1e-9 && (a.layers - b.layers).abs() < 1e-9;
        for want in [
            Want::Sheet(Pose { animation: Some(("grow".into(), None)), ..Pose::default() }),
            Want::Strip { animation: "grow".into(), frames: 3, inputs: Inputs::default(), zoom: Some("hub".into()) },
        ] {
            let in_here = match &want {
                Want::Sheet(pose) => preview::draw_sheet(&hazy, pose).unwrap(),
                Want::Strip { animation, frames, inputs, zoom } => preview::draw_strip(&hazy, animation, *frames, inputs, zoom.as_deref()).unwrap(),
            };
            let there = draw(&hazy, &want).await.unwrap();
            assert!(in_here.spent.layers > in_here.rest.layers && in_here.played.is_some(), "{want:?}: {in_here:?}");
            assert!(there.png == in_here.png && near(there.spent, in_here.spent) && near(there.rest, in_here.rest) && there.played == in_here.played, "{want:?}: {there:?}");
        }
        let strip = draw(&look(json!([{ "tag": "circle", "id": "dot", "attrs": { "cx": 100, "cy": 100, "r": 40, "fill": "#3366cc" } }])), &Want::Strip { animation: "none".into(), frames: 3, inputs: Inputs::default(), zoom: None }).await;
        // What the renderer refuses comes back as its sentence.
        assert_eq!(strip.unwrap_err(), NotDrawn::Refused("no animation \"none\" in this look: it has no animations of its own, and its roles give it none of Workbench's built-in ones".to_string()));
    }

    /// The two looks that got past the estimate and made the renderer ask for more memory than
    /// there is (53 GB at rest; 159 TB in a pose of its own animation), and one whose strokes
    /// take minutes: each ends its own drawing and nothing else.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_look_that_kills_the_renderer_loses_only_its_picture() {
        let stored = |design: Value| {
            let applied = crate::avatar::look::apply(None, &design).unwrap_or_else(|problems| panic!("refused: {}", crate::avatar::look::message(&problems)));
            serde_json::to_value(applied.design.look.unwrap()).unwrap()
        };
        let shifted = stored(json!({ "svg": "<svg viewBox='0 0 16 16'><defs><filter id='f'><feFlood flood-color='#ff0000'/></filter><clipPath id='c'><circle r='64'/></clipPath></defs><g id='a' transform='translate(-15 -55)' clip-path='url(#c)'><rect width='45' height='60' fill='#336699'/><g id='b' transform='scale(1000 0.001) skewX(79.9)'><rect id='c2' width='5' height='27' fill='#336699' filter='url(#f)'/></g></g></svg>" }));
        let saturated = stored(json!({
            "svg": "<svg viewBox='0 0 200 200'><defs><filter id='f1'><feFlood flood-color='#ff0000'/></filter></defs><g id='g0' transform='scale(1000 1)'><g id='g1' transform='matrix(1 999 0 -1 0 0)' filter='url(#f1)'><g id='g2' transform='skewX(-79.9)'><path id='leaf' d='M0 0 L10 0 L10 10 Z' fill='#336699'/></g></g></g></svg>",
            "animations": { "move": { "duration": 2, "loop": true, "tracks": [{ "prop": "scaleY", "target": "g0", "values": [1, 10, 0.1, 1] }, { "prop": "rotate", "target": "g0", "values": [0, 90, 45, 0] }] } }
        }));
        let started = std::time::Instant::now();
        for (name, look, want) in [
            ("a layer inside a shifted layer", &shifted, Want::Sheet(Pose::default())),
            ("a layer past the integers", &saturated, Want::Sheet(Pose { animation: Some(("move".into(), Some(0.5))), ..Pose::default() })),
            ("the same as a strip", &saturated, Want::Strip { animation: "move".into(), frames: 6, inputs: Inputs::default(), zoom: None }),
        ] {
            match draw(look, &want).await {
                // Refused with a sentence by the estimate, or dead of the look: never a picture, never more.
                Err(NotDrawn::Refused(why)) => assert!(why.contains("stretched or skewed") || why.contains("reaches"), "{name}: {why}"),
                Err(NotDrawn::TooCostly) => {}
                other => panic!("{name}: {other:?}"),
            }
        }
        assert!(started.elapsed() < 3 * DRAW_WAIT, "{:?}", started.elapsed());
        // The server is still here, and still draws.
        assert!(draw(&dot(), &Want::Sheet(Pose::default())).await.is_ok());
    }

    #[test]
    fn a_childs_answer_is_read_by_its_status() {
        let png = [PNG_SIGNATURE, b"rest"].concat();
        let drawn = read_answer(Some(0), &png, b"{\"filters\":12.5,\"layers\":3}\n").unwrap();
        assert_eq!((drawn.png.as_slice(), drawn.spent), (png.as_slice(), Spent { filters: 12.5, layers: 3.0 }));
        // What an answer of fewer words leaves out reads as before: the cost at rest is the cost,
        // and nothing is known of an animation.
        assert_eq!((drawn.rest, drawn.played), (drawn.spent, None));
        assert_eq!(read_answer(Some(0), b"not a picture", b""), Err(NotDrawn::Unavailable("the drawing answered with no picture".into())));
        assert_eq!(read_answer(Some(EXIT_REFUSED), b"", b"noise\nno animation \"x\" in this look\n"), Err(NotDrawn::Refused("no animation \"x\" in this look".into())));
        // An abort, a panic, a kill, a refusal without words: the look cost too much.
        for status in [None, Some(1), Some(101), Some(134), Some(EXIT_REFUSED)] {
            assert_eq!(read_answer(status, b"", b""), Err(NotDrawn::TooCostly), "{status:?}");
        }
        assert!(NotDrawn::TooCostly.sentence().starts_with("this look could not be drawn within the memory and the time"));
    }

    #[test]
    fn what_to_draw_reads_back_as_it_was_written() {
        let pose = Pose { animation: Some(("walk".into(), Some(0.25))), inputs: Inputs { look_x: -1.0, turn: 1.0, mouth: Some(0.5), ..Inputs::default() }, zoom: Some("mouth".into()) };
        // A pose that names no moment is sent without one: the child chooses it.
        let open = Pose { animation: Some(("nod".into(), None)), ..Pose::default() };
        assert!(Want::Sheet(open.clone()).to_json().1.get("at").is_none());
        for want in [
            Want::Sheet(Pose::default()),
            Want::Sheet(pose),
            Want::Sheet(open),
            Want::Strip { animation: "walk".into(), frames: 4, inputs: Inputs { turn: -1.0, pace: 1.0, ..Inputs::default() }, zoom: None },
            Want::Strip { animation: "nod".into(), frames: 6, inputs: Inputs::default(), zoom: Some("head".into()) },
        ] {
            let (key, value) = want.to_json();
            assert_eq!(Want::from_json(&json!({ "look": {}, key: value })).unwrap(), want);
        }
        // A strip of an older request has no zoom.
        assert_eq!(Want::from_json(&json!({ "strip": { "animation": "walk", "frames": 2 } })).unwrap(), Want::Strip { animation: "walk".into(), frames: 2, inputs: Inputs::default(), zoom: None });
    }

    /// What the child says beside a picture is read back as it said it; what it leaves out
    /// reads as before.
    #[test]
    fn what_the_child_says_beside_a_picture_reads_back() {
        let png = [PNG_SIGNATURE, b"posed"].concat();
        let moment = Moment { at: 0.25, chosen: Chosen::Furthest, still: false };
        for played in [None, Some(Played { builtin: true, looping: false, moment: Some(moment) }), Some(Played { builtin: false, looping: true, moment: None })] {
            let picture = Picture { png: png.clone(), spent: Spent { filters: 9.5, layers: 2.25 }, rest: Spent { filters: 3.0, layers: 1.0 }, played };
            assert_eq!(reported(png.clone(), &report(&picture).to_string()), picture);
        }
        for (chosen, word) in [(Chosen::Given, "given"), (Chosen::Furthest, "furthest"), (Chosen::Unmoved, "unmoved")] {
            let played = Played { builtin: false, looping: false, moment: Some(Moment { at: 0.5, chosen, still: true }) };
            let line = report(&Picture { png: png.clone(), spent: Spent::default(), rest: Spent::default(), played: Some(played) });
            assert_eq!((line["chosen"].as_str(), reported(png.clone(), &line.to_string()).played), (Some(word), Some(played)));
        }
        // A moment without its words is one the pose gave; nothing at all is no cost.
        let said = reported(png.clone(), "{\"filters\":1,\"layers\":2,\"builtin\":false,\"at\":0.5}");
        assert_eq!(said.played, Some(Played { builtin: false, looping: false, moment: Some(Moment { at: 0.5, chosen: Chosen::Given, still: false }) }));
        assert_eq!(reported(png.clone(), "not json"), Picture { png, spent: Spent::default(), rest: Spent::default(), played: None });
    }
}
