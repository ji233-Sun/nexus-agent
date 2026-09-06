use std::{
    env, fs,
    io::{self, Read as _, Write as _},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

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
    if args.first().map(String::as_str) == Some("app-server") {
        run_app_server();
        return;
    }
    let codex = args.first().map(String::as_str) == Some("exec");
    let omp = args
        .windows(2)
        .any(|pair| pair == ["--mode", "json"]);
    fs::write(
        if codex {
            "codex-args.txt"
        } else if omp {
            "omp-args.txt"
        } else {
            "args.txt"
        },
        args.join("\n"),
    )
    .unwrap();
    if let Ok(value) = env::var("TEST_PROVIDER_API_KEY") {
        fs::write("provider-env.txt", value).unwrap();
    }
    let mut prompt = String::new();
    io::stdin().read_to_string(&mut prompt).unwrap();
    fs::write("stdin.txt", &prompt).unwrap();
    if prompt == "wait-for-cancel" {
        let _child = Command::new(env::current_exe().unwrap())
            .arg("--child")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        println!(
            r#"{{"type":"item.completed","item":{{"id":"ready","type":"agent_message","text":"ready"}}}}"#
        );
        io::stdout().flush().unwrap();
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    let resume_id = args.windows(2)
        .find(|pair| pair[0] == "resume" || pair[0] == "--resume")
        .map(|pair| &pair[1]);
    let session_id = resume_id.cloned().unwrap_or_else(|| format!("session-{}", std::process::id()));
    let session_file = format!("{session_id}.txt");
    let previous_prompt = if resume_id.is_some() {
        Some(fs::read_to_string(&session_file).expect("resume must reuse a saved session"))
    } else {
        fs::write(&session_file, &prompt).unwrap();
        None
    };
    if codex {
        println!(r#"{{"type":"thread.started","thread_id":{session_id:?}}}"#);
    } else if omp {
        println!(r#"{{"type":"session","id":{session_id:?},"version":3}}"#);
    } else {
        println!(r#"{{"type":"system","subtype":"init","session_id":{session_id:?}}}"#);
    }
    if let Some(text) = previous_prompt {
        if codex {
            println!(r#"{{"type":"item.completed","item":{{"id":"resumed","type":"agent_message","text":{text:?}}}}}"#);
        } else if omp {
            println!(r#"{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":{text:?}}}],"stopReason":"stop"}}}}"#);
        } else {
            println!(r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":{text:?}}}]}}}}"#);
        }
        return;
    }
    if codex {
        println!(
            r#"{{"type":"item.started","item":{{"id":"item-1","type":"command_execution","command":"pwd","aggregated_output":"","exit_code":null,"status":"in_progress"}}}}"#
        );
        println!(
            r#"{{"type":"item.completed","item":{{"id":"item-1","type":"command_execution","command":"pwd","aggregated_output":"project","exit_code":0,"status":"completed"}}}}"#
        );
        println!(
            r#"{{"type":"item.completed","item":{{"id":"item-2","type":"agent_message","text":"done"}}}}"#
        );
    } else if omp {
        println!(
            r#"{{"type":"message_update","assistantMessageEvent":{{"type":"text_delta","contentIndex":0,"delta":"hello"}}}}"#
        );
        println!(
            r#"{{"type":"tool_execution_start","toolCallId":"tool-1","toolName":"read","args":{{"path":"README.md"}}}}"#
        );
        println!(
            r#"{{"type":"tool_execution_end","toolCallId":"tool-1","toolName":"read","result":{{"content":[{{"type":"text","text":"project"}}]}},"isError":false}}"#
        );
        println!(
            r#"{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":"done"}}],"stopReason":"stop"}}}}"#
        );
    } else {
        println!(
            r#"{{"type":"stream_event","event":{{"type":"content_block_delta","delta":{{"type":"text_delta","text":"hello"}}}}}}"#
        );
        println!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"hello"}}]}}}}"#
        );
    }
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

fn run_app_server() {
    let mut line = String::new();
    while io::stdin().read_line(&mut line).unwrap() != 0 {
        let id = request_id(&line);
        if line.contains(r#""method":"initialize""#) {
            println!(r#"{{"id":{id},"result":{{"codexHome":"/tmp/.codex"}}}}"#);
        } else if line.contains(r#""method":"model/list""#) {
            if env::var_os("TEST_CATALOG_BLOCK").is_some() {
                loop {
                    thread::sleep(Duration::from_secs(1));
                }
            }
            if env::var_os("TEST_CATALOG_ERROR").is_some() {
                println!(
                    r#"{{"id":{id},"error":{{"code":-32601,"message":"model/list unavailable"}}}}"#
                );
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
        io::stdout().flush().unwrap();
        line.clear();
    }
    fs::write("catalog-stopped.txt", "stopped").unwrap();
}

fn request_id(line: &str) -> &str {
    line.split(r#""id":"#)
        .nth(1)
        .and_then(|value| value.split(|character: char| !character.is_ascii_digit()).next())
        .filter(|value| !value.is_empty())
        .unwrap_or("0")
}
