//! `--llm`: a clear error without the `llm-api` feature; with it, `quail inspect` against a
//! local mock server (a `TcpListener` thread). No request leaves the machine.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const TEXT: &str = "MAINTENANCE AGREEMENT between Northwind Traders and Contoso Ltd. Northwind \
Traders maintains the office printers of Contoso Ltd for 24 months from 1 March 2024 for a fee \
of 4,200.00 EUR per year.\n";

/// Variables a developer's shell may set; the tests must not see them.
const ENV: &[&str] = &[
    "AI_PROVIDER",
    "AI_CHAT_MODEL",
    "AI_MAX_CONCURRENT_LLM",
    "API_URL",
    "API_TOKEN",
    "API_MODEL",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
];

fn quail(args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_quail"));
    for v in ENV {
        c.env_remove(v);
    }
    c.args(args).output().expect("run quail")
}

fn text_file(dir: &Path) -> String {
    let p = dir.join("a.txt");
    fs::write(&p, TEXT).unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn isolate_and_llm_do_not_mix() {
    let t = tempfile::tempdir().unwrap();
    let f = text_file(t.path());
    let o = quail(&["scan", &f, "--isolate", "--llm", "anthropic", "-q"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("--llm does not work with --isolate"), "{err}");
}

#[cfg(not(feature = "llm-api"))]
#[test]
fn llm_without_the_feature_is_a_clear_error() {
    let t = tempfile::tempdir().unwrap();
    let f = text_file(t.path());
    let o = quail(&["inspect", &f, "--llm", "anthropic"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("`llm-api` feature"), "{err}");
    // Without --llm nothing changes.
    let o = quail(&["inspect", &f, "--json"]);
    assert!(o.status.success());
    assert!(!String::from_utf8_lossy(&o.stdout).contains("summary.llm"));
}

#[cfg(feature = "llm-api")]
mod with_api {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use super::*;

    const KEY: &str = "sk-cli-SECRET-42";

    /// Serves `n` OpenAI-style replies with `content`; returns the address and the bodies seen.
    fn serve(n: usize, content: &str) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let body = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 300, "completion_tokens": 40}
        })
        .to_string();
        std::thread::spawn(move || {
            for _ in 0..n {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut len = 0usize;
                let mut auth = String::new();
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 {
                        break;
                    }
                    let h = h.trim_end().to_owned();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        if k.eq_ignore_ascii_case("content-length") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                        if k.eq_ignore_ascii_case("authorization") {
                            v.trim().clone_into(&mut auth);
                        }
                    }
                }
                let mut b = vec![0u8; len];
                let _ = r.read_exact(&mut b);
                log.lock()
                    .unwrap()
                    .push(format!("{auth}\n{}", String::from_utf8_lossy(&b)));
                let mut out = stream;
                let _ = write!(
                    out,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (addr, seen)
    }

    #[test]
    fn inspect_with_llm_against_a_mock_server() {
        let good = r#"{"summary": "A maintenance agreement between Northwind Traders and Contoso Ltd for 24 months.", "subject": "printer maintenance", "title_en": "Maintenance Agreement"}"#;
        let (addr, seen) = serve(1, good);
        let t = tempfile::tempdir().unwrap();
        let f = text_file(t.path());
        let env = t.path().join("test.env");
        fs::write(
            &env,
            format!(
                "AI_PROVIDER=vllm\nAPI_URL=http://{addr}\nAPI_TOKEN={KEY}\nAPI_MODEL=tiny-model\n"
            ),
        )
        .unwrap();
        let log = t.path().join("train.jsonl");
        let o = quail(&[
            "inspect",
            &f,
            "--json",
            "--llm",
            "--env-file",
            env.to_str().unwrap(),
            "--llm-log",
            log.to_str().unwrap(),
        ]);
        let out = String::from_utf8_lossy(&o.stdout);
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(o.status.success(), "{err}");
        assert!(
            err.contains(&format!(
                "quail: llm: provider=vllm model=tiny-model host={addr}"
            )),
            "{err}"
        );
        assert!(!err.contains(KEY) && !out.contains(KEY));
        let rec: serde_json::Value = serde_json::from_str(&out).unwrap();
        let fact = rec["facts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["key"] == "summary.llm")
            .unwrap_or_else(|| panic!("no summary.llm in {out}"));
        let v = fact["value"].to_string();
        assert!(
            v.contains("\"llm\"") && v.contains("vllm:tiny-model"),
            "{v}"
        );
        assert!(v.contains("300"), "prompt tokens recorded: {v}");
        let seen = seen.lock().unwrap();
        assert!(seen[0].starts_with(&format!("Bearer {KEY}\n")));
        assert!(seen[0].contains("Northwind Traders maintains"));
        let rows = fs::read_to_string(&log).unwrap();
        assert_eq!(rows.lines().count(), 1);
        assert!(!rows.contains(KEY));
    }

    #[test]
    fn a_bad_configuration_is_a_clear_error() {
        let t = tempfile::tempdir().unwrap();
        let f = text_file(t.path());
        let env = t.path().join("empty.env");
        fs::write(&env, "AI_PROVIDER=\n").unwrap();
        let o = quail(&[
            "inspect",
            &f,
            "--llm",
            "anthropic",
            "--env-file",
            env.to_str().unwrap(),
        ]);
        assert!(!o.status.success());
        assert!(String::from_utf8_lossy(&o.stderr).contains("ANTHROPIC_API_KEY"));
        let o = quail(&[
            "inspect",
            &f,
            "--llm",
            "groq",
            "--env-file",
            "/nonexistent/quail.env",
        ]);
        assert!(String::from_utf8_lossy(&o.stderr).contains("env file"));
    }
}
