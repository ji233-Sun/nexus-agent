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
    if codex {
        println!(r#"{{"type":"thread.started","thread_id":"thread-1"}}"#);
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
