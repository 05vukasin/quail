//! `ApiSummarizer` against a local mock HTTP server (a `std` `TcpListener` thread serving
//! canned responses): every wire format, retries, timeouts, malformed output, the fact-check
//! fallback, key redaction, the training log and the `summary.llm` record. No request ever
//! leaves the machine.

#![cfg(feature = "api")]

use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use quail_core::{
    AnalyzerId, Blob, Document, Emitter, Extractor, Format, Limits, Page, PageKind, Profile,
    SourceItem, TextSource, Value,
};
use quail_engine::EngineBuilder;
use quail_summary::api::{
    ApiConfig, ApiSummarizer, Connect, Endpoint, Env, Limiter, RetryPolicy, Secret, TrainingLog,
};
use quail_summary::summarizer::checked_or_extractive;
use quail_summary::{
    FileCard, LlmConfig, Method, Summarizer, SummaryCard, SummaryInput, SummaryLlm, keys,
};

const KEY: &str = "sk-test-SECRET-0123456789";

/// One canned reply.
#[derive(Clone)]
struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
    delay: Duration,
}

fn ok(body: impl Into<String>) -> Reply {
    Reply {
        status: 200,
        headers: Vec::new(),
        body: body.into(),
        delay: Duration::ZERO,
    }
}

fn status(code: u16, body: &str) -> Reply {
    Reply {
        status: code,
        ..ok(body)
    }
}

/// A request as the server saw it.
#[derive(Clone, Debug)]
struct Seen {
    line: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

struct Mock {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    thread: Option<JoinHandle<()>>,
}

impl Mock {
    /// Serves `replies` in order, one connection each, then stops listening.
    fn serve(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let thread = std::thread::spawn(move || {
            // One thread per connection: a slow reply must not hold up the retry after it.
            let mut handlers = Vec::new();
            for r in replies {
                let Ok((stream, _)) = listener.accept() else {
                    break;
                };
                let log = Arc::clone(&log);
                handlers.push(std::thread::spawn(move || handle(stream, &r, &log)));
            }
            for h in handlers {
                let _ = h.join();
            }
        });
        Self {
            addr,
            seen,
            thread: Some(thread),
        }
    }

    fn seen(&mut self) -> Vec<Seen> {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.seen.lock().unwrap().clone()
    }
}

fn handle(stream: std::net::TcpStream, r: &Reply, log: &Mutex<Vec<Seen>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut headers = Vec::new();
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap();
            }
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).unwrap();
    log.lock().unwrap().push(Seen {
        line: line.trim_end().to_owned(),
        headers,
        body: String::from_utf8(body).unwrap(),
    });
    std::thread::sleep(r.delay);
    let mut out = stream;
    let mut head = format!(
        "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
        r.status,
        r.body.len()
    );
    for (k, v) in &r.headers {
        let _ = write!(head, "{k}: {v}\r\n");
    }
    head.push_str("\r\n");
    // The client may have timed out and gone: ignore write errors.
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(r.body.as_bytes());
    let _ = out.flush();
}

fn cfg() -> ApiConfig {
    ApiConfig {
        call_timeout: Duration::from_secs(5),
        timeout: Duration::from_secs(20),
        retry: RetryPolicy {
            max_retries: 3,
            base: Duration::from_millis(10),
            max_delay: Duration::from_secs(5),
        },
        limiter: Some(Arc::new(Limiter::new(4))),
        ..ApiConfig::default()
    }
}

fn summarizer(provider: &str, mock: &Mock, cfg: ApiConfig) -> ApiSummarizer {
    let base = match provider {
        "openai_compatible" => format!("http://{}/v1", mock.addr),
        _ => format!("http://{}", mock.addr),
    };
    let req = Connect {
        provider: Some(provider.to_owned()),
        model: Some(
            if provider == "bedrock" {
                "qwen.q3:0"
            } else {
                "m"
            }
            .to_owned(),
        ),
        base_url: Some(base),
        api_key: Some(Secret::new(KEY)),
        region: Some("eu-west-2".to_owned()),
    };
    let ep = Endpoint::resolve(&req, &Env::from_map(Vec::<(String, String)>::new())).unwrap();
    ApiSummarizer::new(ep, cfg)
}

const TEXT: &str = "MAINTENANCE AGREEMENT between Northwind Traders and Contoso Ltd. Northwind \
Traders maintains the office printers of Contoso Ltd for 24 months from 1 March 2024 for a fee \
of 4,200.00 EUR per year.";
const CARD: &str = "doc: contract lang=en\nparties: Northwind Traders, Contoso Ltd";

/// A grounded model output (every name and number is in the card or the text).
const GOOD: &str = r#"{"summary": "A maintenance agreement between Northwind Traders and Contoso Ltd for 24 months at 4,200.00 EUR per year.", "subject": "printer maintenance", "title_en": "Maintenance Agreement"}"#;
/// An invented amount.
const BAD: &str = r#"{"summary": "A maintenance agreement between Northwind Traders and Contoso Ltd worth 9,999,999 EUR.", "subject": "printer maintenance", "title_en": "Maintenance Agreement"}"#;

fn openai_reply(content: &str) -> Reply {
    ok(serde_json::json!({
        "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 321, "completion_tokens": 45}
    })
    .to_string())
}

fn with_input<T>(f: impl FnOnce(&SummaryInput<'_>) -> T) -> T {
    let card = FileCard {
        kind: Some("contract".into()),
        lang: Some("en".into()),
        ..FileCard::default()
    };
    let input = SummaryInput {
        card: &card,
        card_text: CARD,
        subject: None,
        text_head: TEXT,
        lang: Some("en"),
        deadline: None,
        document: None,
    };
    f(&input)
}

#[test]
fn openai_compatible_success() {
    let mut mock = Mock::serve(vec![openai_reply(GOOD)]);
    let s = summarizer("openai_compatible", &mock, cfg());
    let out = with_input(|i| s.summarize(i)).unwrap();
    assert!(out.checked, "{:?}", out.warnings);
    assert_eq!(out.method, Method::Llm);
    assert_eq!(out.title_en.as_deref(), Some("Maintenance Agreement"));
    assert_eq!(
        (out.usage.prompt_tokens, out.usage.output_tokens),
        (321, 45)
    );
    let seen = mock.seen();
    assert_eq!(seen[0].line, "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(
        seen[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    let b = seen[0].json();
    assert_eq!(b["model"], "m");
    assert_eq!(b["temperature"], 0.0);
    assert_eq!(b["max_tokens"], 1024);
    assert_eq!(b["response_format"]["type"], "json_object");
    assert_eq!(b["chat_template_kwargs"]["enable_thinking"], false);
    let user = b["messages"][1]["content"].as_str().unwrap();
    assert!(user.starts_with("FACTS:\ndoc: contract") && user.contains("Contoso Ltd"));
    assert!(
        b["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("exactly one JSON object")
    );
}

#[test]
fn anthropic_success() {
    let mut mock = Mock::serve(vec![ok(serde_json::json!({
        "type": "message",
        "content": [{"type": "text", "text": GOOD}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 500, "output_tokens": 60}
    })
    .to_string())]);
    let s = summarizer("anthropic", &mock, cfg());
    let out = with_input(|i| s.summarize(i)).unwrap();
    assert!(out.checked, "{:?}", out.warnings);
    assert_eq!(out.usage.prompt_tokens, 500);
    let seen = mock.seen();
    assert_eq!(seen[0].line, "POST /v1/messages HTTP/1.1");
    assert_eq!(seen[0].header("x-api-key"), Some(KEY));
    assert_eq!(seen[0].header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(seen[0].header("authorization"), None);
    let b = seen[0].json();
    assert!(
        b.get("temperature").is_none(),
        "no sampling parameters for Claude"
    );
    assert!(b.get("thinking").is_none(), "no thinking field for Claude");
    assert_eq!(b["output_config"]["effort"], "low");
    assert_eq!(b["messages"][0]["role"], "user");
}

#[test]
fn bedrock_success() {
    let mut mock = Mock::serve(vec![ok(serde_json::json!({
        "output": {"message": {"role": "assistant", "content": [{"text": GOOD}]}},
        "stopReason": "end_turn",
        "usage": {"inputTokens": 400, "outputTokens": 50, "totalTokens": 450}
    })
    .to_string())]);
    let s = summarizer("bedrock", &mock, cfg());
    let out = with_input(|i| s.summarize(i)).unwrap();
    assert!(out.checked);
    assert_eq!(out.usage.output_tokens, 50);
    let seen = mock.seen();
    assert_eq!(seen[0].line, "POST /model/qwen.q3%3A0/converse HTTP/1.1");
    assert_eq!(
        seen[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    let b = seen[0].json();
    assert_eq!(b["inferenceConfig"]["temperature"], 0.0);
    assert_eq!(
        b["system"][0]["text"].as_str().map(|s| s.contains("JSON")),
        Some(true)
    );
}

#[test]
fn rate_limit_waits_for_retry_after() {
    let mut limited = status(429, r#"{"error":{"message":"slow down"}}"#);
    limited.headers.push(("retry-after", "1".to_owned()));
    let mut mock = Mock::serve(vec![limited, openai_reply(GOOD)]);
    let s = summarizer("openai_compatible", &mock, cfg());
    let t = Instant::now();
    let out = with_input(|i| s.summarize(i)).unwrap();
    assert!(out.checked);
    assert!(
        t.elapsed() >= Duration::from_millis(950),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(mock.seen().len(), 2);
}

#[test]
fn server_errors_are_retried_then_fall_back() {
    // One 500, then success.
    let mut mock = Mock::serve(vec![status(500, "{}"), openai_reply(GOOD)]);
    let s = summarizer("openai_compatible", &mock, cfg());
    assert!(with_input(|i| s.summarize(i)).unwrap().checked);
    assert_eq!(mock.seen().len(), 2);

    // 500 on every attempt: 1 + max_retries requests, then the extractive summary.
    let mut mock = Mock::serve(vec![status(500, r#"{"error":{"message":"boom"}}"#); 4]);
    let s = summarizer("openai_compatible", &mock, cfg());
    let out = with_input(|i| checked_or_extractive(&s, i));
    assert_eq!(out.method, Method::ExtractiveFallback);
    assert!(
        out.warnings[0].contains("HTTP 500: boom"),
        "{:?}",
        out.warnings
    );
    assert_eq!(mock.seen().len(), 4);

    // A client error is not retried.
    let mut mock = Mock::serve(vec![status(401, r#"{"error":{"message":"bad key"}}"#)]);
    let s = summarizer("anthropic", &mock, cfg());
    let err = with_input(|i| s.summarize(i)).unwrap_err().to_string();
    assert!(err.contains("HTTP 401: bad key"), "{err}");
    assert_eq!(mock.seen().len(), 1);
}

#[test]
fn a_timed_out_attempt_is_retried_and_the_deadline_holds() {
    let mut slow = openai_reply(GOOD);
    slow.delay = Duration::from_millis(1500);
    let mut mock = Mock::serve(vec![slow.clone(), openai_reply(GOOD)]);
    let s = summarizer(
        "openai_compatible",
        &mock,
        ApiConfig {
            call_timeout: Duration::from_millis(300),
            ..cfg()
        },
    );
    let out = with_input(|i| s.summarize(i)).unwrap();
    assert!(out.checked);
    assert_eq!(mock.seen().len(), 2);

    // The file's deadline is never exceeded, retries or not.
    let mock = Mock::serve(vec![slow.clone(), slow.clone(), slow]);
    let s = summarizer("openai_compatible", &mock, cfg());
    let card = FileCard::default();
    let t = Instant::now();
    let input = SummaryInput {
        card: &card,
        card_text: CARD,
        subject: None,
        text_head: TEXT,
        lang: Some("en"),
        deadline: Some(Instant::now() + Duration::from_millis(400)),
        document: None,
    };
    let err = s.summarize(&input).unwrap_err();
    assert!(matches!(err, quail_core::Error::Timeout), "{err}");
    assert!(
        t.elapsed() < Duration::from_millis(1200),
        "{:?}",
        t.elapsed()
    );
}

#[test]
fn malformed_responses_fall_back() {
    // Not JSON at all.
    let mut mock = Mock::serve(vec![ok("<html>gateway</html>")]);
    let s = summarizer("openai_compatible", &mock, cfg());
    let out = with_input(|i| checked_or_extractive(&s, i));
    assert_eq!(out.method, Method::ExtractiveFallback);
    assert!(
        out.warnings[0].contains("malformed response"),
        "{:?}",
        out.warnings
    );
    mock.seen();

    // Valid envelope, but the model wrote no JSON object.
    let mock = Mock::serve(vec![openai_reply(
        "Sure! Here is a summary of the document.",
    )]);
    let s = summarizer("openai_compatible", &mock, cfg());
    let out = with_input(|i| checked_or_extractive(&s, i));
    assert_eq!(out.method, Method::ExtractiveFallback);
    assert!(
        out.warnings[0].contains("model output"),
        "{:?}",
        out.warnings
    );

    // A refusal.
    let mock = Mock::serve(vec![ok(
        r#"{"content":[],"stop_reason":"refusal","usage":{"input_tokens":1,"output_tokens":0}}"#,
    )]);
    let s = summarizer("anthropic", &mock, cfg());
    let out = with_input(|i| checked_or_extractive(&s, i));
    assert_eq!(out.method, Method::ExtractiveFallback);
    assert!(out.warnings[0].contains("refused"), "{:?}", out.warnings);
}

#[test]
fn an_output_that_fails_the_fact_check_falls_back() {
    let mock = Mock::serve(vec![openai_reply(BAD)]);
    let s = summarizer("openai_compatible", &mock, cfg());
    let out = with_input(|i| checked_or_extractive(&s, i));
    assert_eq!(out.method, Method::ExtractiveFallback);
    assert!(
        out.warnings[0].contains("fact check failed") && out.warnings[0].contains("9999999"),
        "{:?}",
        out.warnings
    );
    // The tokens were spent all the same.
    assert_eq!(out.usage.output_tokens, 45);
}

#[test]
fn a_rejected_optional_field_is_dropped_once() {
    let mut mock = Mock::serve(vec![
        status(
            400,
            r#"{"error":{"message":"unknown field chat_template_kwargs"}}"#,
        ),
        openai_reply(GOOD),
        openai_reply(GOOD),
    ]);
    let s = summarizer("openai_compatible", &mock, cfg());
    let out = with_input(|i| s.summarize(i)).unwrap();
    assert!(out.checked);
    assert!(
        out.warnings[0].contains("rejected an optional setting"),
        "{:?}",
        out.warnings
    );
    let second = with_input(|i| s.summarize(i)).unwrap();
    assert!(second.warnings.is_empty(), "{:?}", second.warnings);
    let seen = mock.seen();
    assert_eq!(seen.len(), 3);
    assert!(seen[0].json().get("chat_template_kwargs").is_some());
    for later in &seen[1..] {
        let b = later.json();
        assert!(b.get("chat_template_kwargs").is_none() && b.get("temperature").is_none());
    }
}

#[test]
fn keys_never_leak() {
    // An error body that echoes the key.
    let mock = Mock::serve(vec![status(
        403,
        &format!(r#"{{"error":{{"message":"key {KEY} is not allowed"}}}}"#),
    )]);
    let s = summarizer("openai_compatible", &mock, cfg());
    let err = with_input(|i| s.summarize(i)).unwrap_err().to_string();
    assert!(
        !err.contains(KEY) && err.contains("key *** is not allowed"),
        "{err}"
    );
    let dbg = format!("{s:?} {:?}", s.endpoint());
    assert!(!dbg.contains(KEY), "{dbg}");
    assert!(!s.id().fingerprint.contains(KEY));
    assert!(!s.endpoint().describe().contains(KEY));
}

#[test]
fn the_fingerprint_follows_model_host_and_decoding_not_the_key() {
    let mock = Mock::serve(Vec::new());
    let a = summarizer("openai_compatible", &mock, cfg());
    let fp = a.id();
    assert_eq!(fp.name, "openai_compatible:m");
    assert!(
        fp.fingerprint.contains(&format!("host={}", mock.addr)),
        "{}",
        fp.fingerprint
    );
    assert!(fp.fingerprint.contains("model=m") && fp.fingerprint.contains("temp=0"));
    let mut ep = a.endpoint().clone();
    ep.key = Some(Secret::new("another-key"));
    assert_eq!(
        ApiSummarizer::new(ep.clone(), cfg()).id(),
        fp,
        "the key is not in the id"
    );
    ep.model = "m2".into();
    assert_ne!(ApiSummarizer::new(ep.clone(), cfg()).id(), fp);
    let hot = ApiConfig {
        temperature: Some(0.7),
        ..cfg()
    };
    assert_ne!(ApiSummarizer::new(a.endpoint().clone(), hot).id(), fp);
}

#[test]
fn the_training_log_records_each_call_without_the_key() {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("quail-api-training.jsonl");
    let _ = std::fs::remove_file(&path);
    let log = Arc::new(TrainingLog::open(&path).unwrap());
    let mock = Mock::serve(vec![openai_reply(GOOD), openai_reply(BAD)]);
    let s = summarizer(
        "openai_compatible",
        &mock,
        ApiConfig {
            log: Some(log),
            ..cfg()
        },
    );
    assert!(with_input(|i| s.summarize(i)).unwrap().checked);
    assert!(!with_input(|i| s.summarize(i)).unwrap().checked);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains(KEY));
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["step"], "single");
    assert_eq!(rows[0]["provider"], "openai_compatible");
    assert_eq!(rows[0]["model"], "m");
    assert_eq!(rows[0]["summarizer"], "openai_compatible:m@1");
    assert_eq!(rows[0]["card_text"], CARD);
    assert_eq!(rows[0]["digest"], TEXT);
    assert_eq!(rows[0]["output"], GOOD);
    assert_eq!(rows[0]["checked"], true);
    assert_eq!(rows[0]["prompt_tokens"], 321);
    assert_eq!(rows[1]["checked"], false);
    assert!(
        rows[1]["unsupported"][0]
            .as_str()
            .unwrap()
            .contains("9999999")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }
    let _ = std::fs::remove_file(&path);
}

struct TextExtractor;

impl Extractor for TextExtractor {
    fn id(&self) -> AnalyzerId {
        AnalyzerId::new("test.text", 1)
    }
    fn formats(&self) -> &'static [Format] {
        &[Format::Text]
    }
    fn extract(&self, blob: &Blob, _: &Limits, _: &mut Emitter) -> quail_core::Result<Document> {
        Ok(Document {
            pages: vec![Page {
                text: String::from_utf8_lossy(blob.as_bytes()).into_owned(),
                text_source: TextSource::Native,
                ..Page::new(0, PageKind::Logical)
            }],
        })
    }
}

#[test]
fn summary_llm_records_the_api_summary_and_its_tokens() {
    let mut mock = Mock::serve(vec![openai_reply(GOOD), openai_reply(BAD)]);
    let s: Arc<dyn Summarizer> = Arc::new(summarizer("openai_compatible", &mock, cfg()));
    let step = || {
        SummaryLlm::new(
            Arc::clone(&s),
            LlmConfig {
                enabled: true,
                ..LlmConfig::default()
            },
            None,
        )
    };
    // Deep by default: a Standard run leaves it out.
    let standard = EngineBuilder::new()
        .profile(Profile::Standard)
        .extractor(Box::new(TextExtractor))
        .analyzer(Box::new(step()))
        .build()
        .unwrap();
    assert!(!standard.plan().any(|id| id == SummaryLlm::ID));
    // Registered on explicit request, it runs in the profile it is given.
    let engine = EngineBuilder::new()
        .profile(Profile::Standard)
        .quiet_panics(true)
        .extractor(Box::new(TextExtractor))
        .analyzer(Box::new(SummaryCard::new()))
        .analyzer(Box::new(step().with_profile(Profile::Standard)))
        .build()
        .unwrap();
    let record = |name: &str| {
        let rec = engine.process(SourceItem {
            locator: name.into(),
            blob: Blob::new(TEXT.as_bytes().to_vec()),
            modified: None,
        });
        rec.facts_for(keys::LLM.as_str())
            .next()
            .unwrap()
            .value
            .clone()
    };
    let field = |v: &Value, k: &str| v.field(k).cloned();
    let good = record("a.txt");
    assert_eq!(field(&good, "method"), Some(Value::text("llm")));
    assert_eq!(
        field(&good, "model"),
        Some(Value::text("openai_compatible:m"))
    );
    assert_eq!(field(&good, "prompt_tokens"), Some(Value::Int(321)));
    assert_eq!(field(&good, "output_tokens"), Some(Value::Int(45)));
    let bad = record("b.txt");
    assert_eq!(
        field(&bad, "method"),
        Some(Value::text("extractive_fallback"))
    );
    assert_eq!(field(&bad, "output_tokens"), Some(Value::Int(45)));
    let user = mock.seen()[0].json()["messages"][1]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(user.contains("Northwind Traders maintains"), "{user}");
}

#[test]
fn map_reduce_sums_tokens_and_logs_every_call() {
    use quail_summary::{LongInput, MapPart};
    let part = |title: &str, body: &str| MapPart {
        title: title.into(),
        page: 0,
        end_page: 9,
        body: format!("## {title} (pp. 1-10)\n{body}"),
        tokens: 20,
        sections: Vec::new(),
        truncated: false,
    };
    let parts = vec![
        part(
            "Scope",
            "Northwind Traders maintains the office printers of Contoso Ltd.",
        ),
        part("Fees", "The fee is 4,200.00 EUR per year for 24 months."),
    ];
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("quail-api-mapreduce.jsonl");
    let _ = std::fs::remove_file(&path);
    let mut mock = Mock::serve(vec![
        openai_reply(r#"{"summary": "Northwind Traders maintains the printers of Contoso Ltd."}"#),
        openai_reply(r#"{"summary": "The fee is 4,200.00 EUR per year."}"#),
        openai_reply(GOOD),
    ]);
    let s = summarizer(
        "openai_compatible",
        &mock,
        ApiConfig {
            log: Some(Arc::new(TrainingLog::open(&path).unwrap())),
            ..cfg()
        },
    );
    let (out, calls) = with_input(|i| {
        s.summarize_long(
            i,
            &LongInput {
                outline: "- Scope (pp. 1-10)\n- Fees (pp. 11-20)",
                parts: &parts,
            },
        )
    })
    .unwrap();
    assert_eq!(calls, 3);
    assert!(out.checked, "{:?}", out.warnings);
    assert_eq!(
        (out.usage.prompt_tokens, out.usage.output_tokens),
        (963, 135)
    );
    let seen = mock.seen();
    assert_eq!(
        seen[0].json()["max_tokens"],
        512,
        "map steps have their own cap"
    );
    assert_eq!(seen[2].json()["max_tokens"], 1024);
    let steps: Vec<String> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["step"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(steps, ["map", "map", "reduce"]);
    let _ = std::fs::remove_file(&path);
}
