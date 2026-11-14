//! Native command line for the already-running application; no graphics or model startup.
use serde_json::{Value, json};
use std::time::Duration;

const HELP: &str = "ui-director [--port 18920] COMMAND\n\
  tools [QUERY]                 List operations; a query includes argument schemas\n\
  describe NAME [NAME ...]       Read selected argument schemas\n\
  call NAME 'JSON'               Run an operation once\n\
  help                          Show this help\n\n\
Examples:\n\
  ui-director tools\n\
  ui-director describe ui.pages ui.read ui.set\n\
  ui-director call ui.read '{}'\n\
The application must be running. Results are JSON; failures exit with code 1.\n";

pub fn run(mut args: Vec<String>) -> i32 {
    #[cfg(windows)]
    unsafe {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        AttachConsole(u32::MAX);
    }
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h") {
        print!("{HELP}");
        return 0;
    }
    let result = (|| {
        let mut port = crate::DEFAULT_PORT;
        if args.first().is_some_and(|arg| arg == "--port") {
            port = args
                .get(1)
                .ok_or("Missing port")?
                .parse::<u16>()
                .map_err(|_| "Invalid port")?;
            args.drain(..2);
        }
        let request = match args.first().map(String::as_str) {
            Some("tools") if args.len() <= 2 => json!({
                "name":"tools.list","arguments":args.get(1).map_or(json!({}), |query| json!({"query":query}))
            }),
            Some("describe") if args.len() > 1 => json!({
                "name":"tools.describe","arguments":{"names":&args[1..]}
            }),
            Some("call") if args.len() == 3 => {
                let arguments: Value = serde_json::from_str(&args[2]).map_err(|e| e.to_string())?;
                if !arguments.is_object() {
                    return Err("Arguments must be a JSON object".into());
                }
                json!({"name":args[1],"arguments":arguments})
            }
            _ => return Err("Invalid command. Use help".into()),
        };
        crate::client::send(port, &request, Duration::from_secs(6))
    })();
    let response = match result {
        Ok(value) => value,
        Err(error) => json!({"success":false,"message":error}),
    };
    println!("{}", serde_json::to_string_pretty(&response).unwrap());
    if response["success"] == true { 0 } else { 1 }
}
