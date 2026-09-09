use std::{
    env, fs,
    io::{self, BufRead as _, Read as _, Write as _},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

#[derive(Clone, Copy, PartialEq)]
enum Harness {
    Claude,
    Codex,
    Omp,
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--child") {
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    if args.as_slice() == ["models", "--json"] {
        run_omp_catalog();
        return;
    }
    let harness = if matches!(
        args.first().map(String::as_str),
        Some("app-server" | "exec")
    ) {
        Harness::Codex
    } else if args.windows(2).any(|pair| pair == ["--mode", "rpc"]) {
        Harness::Omp
    } else {
        Harness::Claude
    };
    let title = match harness {
        Harness::Codex => args
            .windows(2)
            .any(|pair| pair == ["--sandbox", "read-only"]),
        Harness::Omp => args.iter().any(|arg| arg == "--no-tools"),
        Harness::Claude => args
            .windows(2)
            .any(|pair| pair[0] == "--tools" && pair[1].is_empty()),
    };
    fs::write(
        if title {
            "title-args.txt"
        } else {
            match harness {
                Harness::Codex => "codex-args.txt",
                Harness::Omp => "omp-args.txt",
                Harness::Claude => "args.txt",
            }
        },
        args.join("\n"),
    )
    .unwrap();
    if let Ok(value) = env::var("TEST_PROVIDER_API_KEY") {
        fs::write(
            if title {
                "title-provider-env.txt"
            } else {
                "provider-env.txt"
            },
            value,
        )
        .unwrap();
    }
    if title {
        fs::write(
            "title-executable.txt",
            env::current_exe().unwrap().to_string_lossy().as_bytes(),
        )
        .unwrap();
        let mut prompt = String::new();
        io::stdin().read_to_string(&mut prompt).unwrap();
        fs::write("title-prompt.txt", &prompt).unwrap();
        if env::var_os("TEST_TITLE_BLOCK").is_some() {
            let _child = Command::new(env::current_exe().unwrap())
                .arg("--child")
                .stdin(Stdio::null())
                .spawn()
                .unwrap();
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        match harness {
            Harness::Codex => println!(
                r#"{{"type":"item.completed","item":{{"id":"title","type":"agent_message","text":"**Fix authentication flow.**"}}}}"#
            ),
            Harness::Omp => println!(
                r#"{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":"**Fix authentication flow.**"}}],"stopReason":"stop"}}}}"#
            ),
            Harness::Claude => println!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"**Fix authentication flow.**"}}]}}}}"#
            ),
        }
        io::stdout().flush().unwrap();
        return;
    }
    let (send, input) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let resume = args
        .windows(2)
        .find(|pair| pair[0] == "--resume")
        .map(|pair| pair[1].clone());
    let mut resumed = resume.is_some();
    let mut session = resume.unwrap_or_else(|| format!("session-{}", std::process::id()));
    while let Ok(line) = input.recv() {
        let id = request_id(&line);
        if harness == Harness::Codex {
            match string_field(&line, "method").as_str() {
                "initialize" => println!(r#"{{"id":{id},"result":{{"codexHome":"/tmp/.codex"}}}}"#),
                "account/login/start" => println!(r#"{{"id":{id},"result":{{"type":"apiKey"}}}}"#),
                "thread/start" | "thread/resume" => {
                    let resume_id = string_field(&line, "threadId");
                    resumed = !resume_id.is_empty();
                    if resumed {
                        session = resume_id;
                    }
                    fs::write("thread-params.json", &line).unwrap();
                    println!(r#"{{"id":{id},"result":{{"thread":{{"id":{session:?}}}}}}}"#);
                }
                "turn/start" => {
                    fs::write("turn-params.json", &line).unwrap();
                    println!(r#"{{"id":{id},"result":{{"turn":{{"id":"turn-1"}}}}}}"#);
                    run_turn(
                        harness,
                        &string_field(&line, "text"),
                        &session,
                        resumed,
                        &input,
                    );
                }
                "model/list" => run_codex_catalog(&line, &id),
                _ => {}
            }
        } else if harness == Harness::Omp && string_field(&line, "type") == "get_state" {
            println!(
                r#"{{"type":"response","command":"get_state","id":{id},"success":true,"data":{{"sessionId":{session:?}}}}}"#
            );
        } else {
            let prompt = string_field(
                &line,
                if harness == Harness::Omp {
                    "message"
                } else {
                    "content"
                },
            );
            if harness == Harness::Omp {
                println!(r#"{{"type":"response","command":"prompt","id":{id},"success":true}}"#);
            } else {
                println!(r#"{{"type":"system","subtype":"init","session_id":{session:?}}}"#);
            }
            run_turn(harness, &prompt, &session, resumed, &input);
        }
        io::stdout().flush().unwrap();
    }
    fs::write("catalog-stopped.txt", "stopped").unwrap();
}

fn run_turn(
    harness: Harness,
    prompt: &str,
    session: &str,
    resumed: bool,
    input: &Receiver<String>,
) {
    fs::write("stdin.txt", prompt).unwrap();
    let session_file = format!("{session}.txt");
    if resumed {
        let previous = fs::read_to_string(session_file).expect("resume must reuse a saved session");
        message(harness, &previous);
        terminal(harness);
        return;
    }
    fs::write(session_file, prompt).unwrap();
    if prompt.starts_with("codex-async-") {
        println!(r#"{{"method":"item/completed","params":{{"threadId":{session:?},"turnId":"turn-1","item":{{"id":"call-ask","type":"agentMessage","delivery":"async","phase":"final_answer","text":"Choose a skill?\n- 外语\n- 乐器","questions":[{{"title":"Choose a skill?","options":["外语","乐器"]}},{{"title":"Any details?"}}]}}}}}}"#);
        if prompt != "codex-async-live" {
            terminal(harness);
        }
        io::stdout().flush().unwrap();
        let answer = input.recv_timeout(Duration::from_secs(10)).expect("missing async answer");
        fs::write("user-ask-input.json", &answer).unwrap();
        let id = request_id(&answer);
        if prompt == "codex-async-rejected" {
            println!(r#"{{"id":{id},"error":{{"code":-32600,"message":"Cannot accept input"}}}}"#);
        } else {
            let turn = if prompt == "codex-async-live" { "turn-1" } else { "turn-2" };
            println!(r#"{{"id":{id},"result":{{"turn":{{"id":{turn:?}}}}}}}"#);
            println!(r#"{{"method":"item/completed","params":{{"threadId":{session:?},"turnId":{turn:?},"item":{{"id":"reply","type":"agentMessage","text":"answered"}}}}}}"#);
            println!(r#"{{"method":"turn/completed","params":{{"threadId":{session:?},"turn":{{"id":{turn:?},"status":"completed"}}}}}}"#);
        }
        return;
    }
    if prompt == "user-ask-native" && harness != Harness::Omp {
        match harness {
            Harness::Claude => println!(
                "{}",
                r#"{"type":"control_request","request_id":"ask-claude","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which checks?","multiSelect":true,"options":[{"label":"Tests"},{"label":"Clippy"}]},{"question":"Branch name?"}]}}}"#
            ),
            Harness::Codex => println!(
                "{}{:?}{}",
                r#"{"id":77,"method":"item/tool/requestUserInput","params":{"threadId":"#,
                session,
                r#","turnId":"turn-1","questions":[{"id":"checks","question":"Which checks?","isOther":true,"options":[{"label":"Tests"},{"label":"Clippy"}]},{"id":"branch","question":"Branch name?","options":null}]}}"#
            ),
            Harness::Omp => unreachable!(),
        }
        io::stdout().flush().unwrap();
        let answer = input
            .recv_timeout(Duration::from_secs(5))
            .expect("missing native User Ask answer");
        fs::write("user-ask-input.json", &answer).unwrap();
        message(harness, "answered");
        terminal(harness);
        return;
    }
    if prompt.starts_with("omp-text-") {
        let method = if prompt == "omp-text-editor" { "editor" } else { "input" };
        let timeout = if prompt == "omp-text-timeout" { ",\"timeout\":50" } else { "" };
        println!(r#"{{"type":"extension_ui_request","id":"ui_1","method":{method:?},"title":"Branch name"{timeout}}}"#);
        if matches!(prompt, "omp-text-cancel" | "omp-text-timeout") {
            if prompt == "omp-text-cancel" {
                println!(r#"{{"type":"extension_ui_request","id":"cancel-2","method":"cancel","targetId":"ui_1"}}"#);
            }
            // Keep the run alive so only the dialog's cancellation/deadline can resolve it.
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        } else {
            let answer = input.recv_timeout(Duration::from_secs(5)).expect("missing OMP text answer");
            fs::write("user-ask-input.json", &answer).unwrap();
            message(harness, "answered");
        }
        terminal(harness);
        return;
    }
    if prompt.starts_with("user-ask") {
        emit_user_ask();
        if prompt == "user-ask-exit" {
            terminal(harness);
            return;
        }
        if prompt == "user-ask-cancel" {
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        let answer = input
            .recv_timeout(Duration::from_secs(5))
            .expect("missing User Ask answer");
        fs::write("user-ask-input.json", &answer).unwrap();
        let id = request_id(&answer);
        if prompt == "user-ask-native-failure" {
            println!(
                r#"{{"type":"nexus_test.user_ask.finished","id":{id},"status":"failed","message":"request expired"}}"#
            );
        } else {
            println!(
                r#"{{"type":"nexus_test.user_ask.finished","id":{id},"status":"answered"}}"#
            );
            message(harness, "answered");
        }
        terminal(harness);
        return;
    }
    if prompt.starts_with("approval-") {
        match harness {
            Harness::Claude => println!(r#"{{"type":"control_request","request_id":"approval-1","request":{{"subtype":"can_use_tool","tool_name":"Bash","input":{{"command":"echo approved"}}}}}}"#),
            Harness::Codex => println!(r#"{{"id":99,"method":"item/commandExecution/requestApproval","params":{{"threadId":{session:?},"turnId":"turn-1","itemId":"tool-1","command":"echo approved","cwd":"/tmp/project","reason":"test approval"}}}}"#),
            Harness::Omp => {
                let timeout = if prompt == "approval-timeout" { ",\"timeout\":100" } else { "" };
                println!(r#"{{"type":"extension_ui_request","id":"approval-1","method":"select","title":"Allow tool: bash\necho approved","options":["Approve","Deny"]{timeout}}}"#);
            }
        }
        io::stdout().flush().unwrap();
        if prompt == "approval-native-cancel" {
            wait_for_marker("resolve-approval", input);
            match harness {
                Harness::Claude => println!(r#"{{"type":"control_cancel_request","request_id":"approval-1"}}"#),
                Harness::Codex => println!(r#"{{"method":"serverRequest/resolved","params":{{"threadId":{session:?},"requestId":99}}}}"#),
                Harness::Omp => println!(r#"{{"type":"extension_ui_request","id":"cancel-1","method":"cancel","targetId":"approval-1"}}"#),
            }
            io::stdout().flush().unwrap();
            wait_for_marker("finish-turn", input);
        } else if let Ok(response) = input.recv_timeout(Duration::from_secs(10)) {
            fs::write("approval-response.json", &response).unwrap();
            let approved = match harness {
                Harness::Claude => string_field(&response, "behavior") == "allow",
                Harness::Codex => string_field(&response, "decision") == "accept",
                Harness::Omp => string_field(&response, "value") == "Approve",
            };
            message(harness, if approved { "approved" } else { "denied" });
        }
        terminal(harness);
        return;
    }
    if prompt == "wait-for-cancel" {
        let _child = Command::new(env::current_exe().unwrap())
            .arg("--child")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        message(harness, "ready");
        io::stdout().flush().unwrap();
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    if prompt == "steer-no-tools" {
        message(harness, "ready");
        io::stdout().flush().unwrap();
        wait_for_marker("finish-turn", input);
    } else if prompt.starts_with("steer-") {
        if harness == Harness::Claude {
            // One assistant frame can contain several tools in the same batch.
            println!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"tool-1","name":"Read","input":{{}}}},{{"type":"tool_use","id":"tool-2","name":"Read","input":{{}}}}]}}}}"#
            );
        } else {
            tool(harness, "tool-1", false);
            tool(harness, "tool-2", false);
        }
        io::stdout().flush().unwrap();
        wait_for_marker("finish-first", input);
        tool(harness, "tool-1", true);
        message(harness, "first-tool-done");
        io::stdout().flush().unwrap();
        wait_for_marker("finish-second", input);
        tool(harness, "tool-2", true);
        io::stdout().flush().unwrap();
        let steer = input
            .recv_timeout(Duration::from_secs(5))
            .expect("missing Steer at tool boundary");
        fs::write("steer-input.json", &steer).unwrap();
        let id = if harness == Harness::Claude {
            format!("{:?}", string_field(&steer, "uuid"))
        } else {
            request_id(&steer)
        };
        if prompt == "steer-rejected" {
            match harness {
                Harness::Codex => println!(r#"{{"id":{id},"error":{{"message":"turn ended"}}}}"#),
                Harness::Omp => println!(
                    r#"{{"type":"response","command":"steer","id":{id},"success":false,"error":"turn ended"}}"#
                ),
                Harness::Claude => panic!("Claude acknowledges by replay"),
            }
        } else if prompt != "steer-unconfirmed" {
            match harness {
                Harness::Codex => println!(r#"{{"id":{id},"result":{{"turnId":"turn-1"}}}}"#),
                Harness::Omp => {
                    println!(r#"{{"type":"response","command":"steer","id":{id},"success":true}}"#)
                }
                Harness::Claude => println!("{steer}"),
            }
            message(harness, "steered");
        }
    } else if harness == Harness::Claude {
        println!(
            r#"{{"type":"stream_event","event":{{"type":"content_block_delta","delta":{{"type":"text_delta","text":"hello"}}}}}}"#
        );
        message(harness, "hello");
    } else {
        if harness == Harness::Omp {
            println!(
                r#"{{"type":"message_update","assistantMessageEvent":{{"type":"text_delta","contentIndex":0,"delta":"hello"}}}}"#
            );
        }
        tool(harness, "tool-1", false);
        tool(harness, "tool-1", true);
        message(harness, "done");
    }
    terminal(harness);
}

fn emit_user_ask() {
    println!(
        "{}",
        r#"{"type":"nexus_test.user_ask.requested","id":"native-ask-42","questions":[{"id":"target","prompt":"Which target?","answer_mode":{"kind":"choice","multiple":false,"allow_custom":false},"options":[{"id":"library","label":"Library","description":"Core packages"},{"id":"workspace","label":"Workspace"}]},{"id":"checks","prompt":"Which checks?","answer_mode":{"kind":"choice","multiple":true,"allow_custom":false},"options":[{"id":"tests","label":"Tests"},{"id":"clippy","label":"Clippy"},{"id":"build","label":"Build"}]},{"id":"note","prompt":"Any note?","answer_mode":{"kind":"choice","multiple":false,"allow_custom":true},"options":[{"id":"none","label":"None"},{"id":"document","label":"Document"}]}]}"#
    );
    io::stdout().flush().unwrap();
}

fn wait_for_marker(path: &str, input: &Receiver<String>) {
    while !std::path::Path::new(path).exists() {
        match input.recv_timeout(Duration::from_millis(10)) {
            Ok(_) => panic!("Steer arrived before all tools completed"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn message(harness: Harness, text: &str) {
    match harness {
        Harness::Codex => println!(
            r#"{{"method":"item/completed","params":{{"item":{{"id":"message","type":"agentMessage","text":{text:?}}}}}}}"#
        ),
        Harness::Omp => println!(
            r#"{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":{text:?}}}],"stopReason":"stop"}}}}"#
        ),
        Harness::Claude => println!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":{text:?}}}]}}}}"#
        ),
    }
}

fn tool(harness: Harness, id: &str, completed: bool) {
    match (harness, completed) {
        (Harness::Codex, false) => println!(
            r#"{{"method":"item/started","params":{{"item":{{"id":{id:?},"type":"commandExecution","command":"pwd","status":"inProgress"}}}}}}"#
        ),
        (Harness::Codex, true) => println!(
            r#"{{"method":"item/completed","params":{{"item":{{"id":{id:?},"type":"commandExecution","aggregatedOutput":"project","exitCode":0,"status":"completed"}}}}}}"#
        ),
        (Harness::Omp, false) => println!(
            r#"{{"type":"tool_execution_start","toolCallId":{id:?},"toolName":"read","args":{{"path":"README.md"}}}}"#
        ),
        (Harness::Omp, true) => println!(
            r#"{{"type":"tool_execution_end","toolCallId":{id:?},"result":{{"content":[{{"type":"text","text":"project"}}]}},"isError":false}}"#
        ),
        (Harness::Claude, false) => println!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":{id:?},"name":"Read","input":{{}}}}]}}}}"#
        ),
        (Harness::Claude, true) => println!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":{id:?},"content":"project","is_error":false}}]}}}}"#
        ),
    }
}

fn terminal(harness: Harness) {
    match harness {
        Harness::Codex => println!(
            r#"{{"method":"turn/completed","params":{{"turn":{{"id":"turn-1","status":"completed"}}}}}}"#
        ),
        Harness::Omp => println!(r#"{{"type":"agent_end","isTerminal":true}}"#),
        Harness::Claude => println!(r#"{{"type":"result","is_error":false}}"#),
    }
    io::stdout().flush().unwrap();
}

// Fixtures only need string fields; retain escapes when locating a closing quote.
fn string_field(line: &str, key: &str) -> String {
    let Some(value) = line
        .split_once(&format!("\"{key}\":"))
        .map(|(_, value)| value.trim_start())
    else {
        return String::new();
    };
    let Some(value) = value.strip_prefix('"') else {
        return String::new();
    };
    let mut result = String::new();
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            result.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            break;
        } else {
            result.push(character);
        }
    }
    result
}

fn run_omp_catalog() {
    fs::write(
        "omp-catalog-cwd.txt",
        env::current_dir().unwrap().to_string_lossy().as_bytes(),
    )
    .unwrap();
    if let Ok(value) = env::var("TEST_PROVIDER_API_KEY") {
        fs::write("omp-catalog-env.txt", value).unwrap();
    }
    if env::var_os("TEST_OMP_CATALOG_BLOCK").is_some() {
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    if env::var_os("TEST_OMP_CATALOG_ERROR").is_some() {
        eprintln!("provider request failed; credential=test-secret-must-not-leak");
        std::process::exit(23);
    }
    if env::var_os("TEST_OMP_CATALOG_MALFORMED").is_some() {
        println!("not json");
    } else if env::var_os("TEST_OMP_CATALOG_EMPTY").is_some() {
        println!(r#"{{"models":[]}}"#);
    } else {
        println!(
            r#"{{"models":[{{"provider":"alpha","id":"shared-model","selector":"alpha/shared-model","name":"Shared Model","contextWindow":131072,"maxTokens":32768,"reasoning":true,"thinking":["off","low","high","xhigh","auto"],"input":["text"],"cost":{{}}}},{{"provider":"beta","id":"shared-model","selector":"beta/shared-model","name":"Shared Model","contextWindow":65536,"maxTokens":16384,"reasoning":true,"thinking":["minimal","medium"],"input":["text"],"cost":{{}}}}]}}"#
        );
    }
    io::stdout().flush().unwrap();
    fs::write("omp-catalog-stopped.txt", "stopped").unwrap();
}

fn run_codex_catalog(line: &str, id: &str) {
    if env::var_os("TEST_CATALOG_BLOCK").is_some() {
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    if env::var_os("TEST_CATALOG_ERROR").is_some() {
        println!(r#"{{"id":{id},"error":{{"code":-32601,"message":"model/list unavailable"}}}}"#);
    } else if line.contains(r#""cursor":"page-2""#) {
        println!(
            r#"{{"id":{id},"result":{{"data":[{{"id":"gpt-second","displayName":"GPT Second","hidden":false,"isDefault":false,"defaultReasoningEffort":"ultra","supportedReasoningEfforts":[{{"reasoningEffort":"high","description":"High"}},{{"reasoningEffort":"ultra","description":"Ultra"}}]}}],"nextCursor":null}}}}"#
        );
    } else {
        println!(
            r#"{{"id":{id},"result":{{"data":[{{"id":"gpt-first","displayName":"GPT First","hidden":false,"isDefault":true,"defaultReasoningEffort":"low","supportedReasoningEfforts":[{{"reasoningEffort":"low","description":"Low"}}]}}],"nextCursor":"page-2"}}}}"#
        );
    }
}

fn request_id(line: &str) -> String {
    let value = line
        .split_once(r#""id":"#)
        .map(|(_, value)| value.trim_start())
        .unwrap_or("0");
    if value.starts_with('"') {
        format!("{:?}", string_field(line, "id"))
    } else {
        value.chars().take_while(char::is_ascii_digit).collect()
    }
}
