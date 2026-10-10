#![allow(clippy::wildcard_imports, clippy::default_trait_access)]

use lsp_server::{ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use super::config::LspServerConfig;

const INIT_TIMEOUT_SECS: u64 = 60;
const REQUEST_TIMEOUT_SECS: u64 = 30;
const SHUTDOWN_TIMEOUT_SECS: u64 = 5;

/// The percent-encoded `file://` URI of `path`, made absolute first.
pub fn file_path_to_uri(path: &str) -> Result<Uri, String> {
    let p = Path::new(path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::fs::canonicalize(p).map_err(|e| format!("Cannot resolve path '{path}': {e}"))?
    };
    let url = url::Url::from_file_path(&abs)
        .map_err(|()| format!("Cannot make a file URI from '{}'", abs.display()))?;
    url.as_str()
        .parse::<Uri>()
        .map_err(|e| format!("Invalid URI: {e}"))
}

/// The local path of a `file://` URI, percent-decoded.
pub fn uri_to_file_path(uri: &Uri) -> Option<String> {
    url::Url::parse(uri.as_str())
        .ok()?
        .to_file_path()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

pub struct LspClient {
    child: Child,
    channel: Channel,
    next_id: AtomicI32,
    initialized: bool,
    open_documents: OpenDocuments,
}

/// The documents this client has opened, with the version last sent. The
/// protocol allows one `didOpen` per document: a document opened again is
/// sent as a full-text `didChange`, so the server sees the file as it is now.
#[derive(Default)]
struct OpenDocuments {
    versions: std::collections::HashMap<String, i32>,
}

impl OpenDocuments {
    /// The notification that gives the server `text` as the content of `uri`.
    fn sync(&mut self, uri: &Uri, language_id: &str, text: &str) -> Result<Notification, String> {
        use lsp_types::notification::Notification as _;
        let (method, params) = if let Some(version) = self.versions.get_mut(uri.as_str()) {
            *version += 1;
            let params = serde_json::to_value(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version: *version,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: text.to_string(),
                }],
            });
            (notification::DidChangeTextDocument::METHOD, params)
        } else {
            self.versions.insert(uri.as_str().to_string(), 1);
            let params = serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: language_id.to_string(),
                    version: 1,
                    text: text.to_string(),
                },
            });
            (notification::DidOpenTextDocument::METHOD, params)
        };
        Ok(Notification {
            method: method.to_string(),
            params: params.map_err(|e| e.to_string())?,
        })
    }
}

/// The message stream to and from a language server: `lsp-server` frames
/// each message, a reader thread delivers the incoming ones so that every wait
/// has a timeout.
struct Channel {
    writer: Box<dyn Write + Send>,
    incoming: Receiver<Result<Message, String>>,
}

impl Channel {
    fn new(reader: impl Read + Send + 'static, writer: impl Write + Send + 'static) -> Self {
        let (tx, incoming) = mpsc::channel();
        std::thread::Builder::new()
            .name("lsp-reader".into())
            .spawn(move || {
                let mut reader = BufReader::new(reader);
                loop {
                    let msg = match Message::read(&mut reader) {
                        Ok(Some(msg)) => Ok(msg),
                        Ok(None) => Err("LSP server closed connection (EOF)".to_string()),
                        Err(e) => Err(format!("Read LSP message: {e}")),
                    };
                    let last = msg.is_err();
                    if tx.send(msg).is_err() || last {
                        break;
                    }
                }
            })
            .ok();
        Self {
            writer: Box::new(writer),
            incoming,
        }
    }

    fn send(&mut self, msg: &Message) -> Result<(), String> {
        msg.write(&mut self.writer)
            .map_err(|e| format!("Write to LSP server: {e}"))
    }

    /// Waits for the response to request `id`, answering the requests the
    /// server sends meanwhile. A server request that reuses the number `id`
    /// is a request, not the response.
    fn read_response(&mut self, id: &RequestId, timeout: Duration) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "LSP response timeout ({}s) for request id={id}",
                    timeout.as_secs()
                ));
            }

            match self.incoming.recv_timeout(remaining) {
                Ok(Ok(Message::Response(resp))) if resp.id == *id => {
                    return resp
                        .response_result
                        .map_err(|err| format!("LSP error: {}", err.message));
                }
                Ok(Ok(Message::Request(req))) => self.answer(req)?,
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(format!("LSP reader error: {e}")),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(format!("LSP response timeout ({}s)", timeout.as_secs()));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("LSP server connection lost".into());
                }
            }
        }
    }

    /// Answers a request from the server. The two that servers send without
    /// being offered and then wait on get an empty success: no settings for
    /// each configuration item, and a registration taken note of. Anything
    /// else is a method this client does not have.
    fn answer(&mut self, req: Request) -> Result<(), String> {
        use lsp_types::request::Request as _;
        let Request { id, method, params } = req;
        let resp = if method == request::WorkspaceConfiguration::METHOD {
            let items =
                serde_json::from_value::<ConfigurationParams>(params).map_or(0, |p| p.items.len());
            Response::new_ok(id, vec![Value::Null; items])
        } else if method == request::RegisterCapability::METHOD {
            Response::new_ok(id, ())
        } else {
            Response::new_err(
                id,
                ErrorCode::MethodNotFound as i32,
                format!("method not supported by this client: {method}"),
            )
        };
        self.send(&resp.into())
    }
}

impl LspClient {
    pub fn start(config: &LspServerConfig, root_uri: &Uri) -> Result<Self, String> {
        let mut child = Command::new(&config.command)
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start LSP server '{}': {e}", config.command))?;

        let stdin = child.stdin.take().ok_or("No stdin")?;
        let stdout = child.stdout.take().ok_or("No stdout")?;

        // On a failed initialize, dropping `client` stops the process.
        let mut client = Self {
            child,
            channel: Channel::new(stdout, stdin),
            next_id: AtomicI32::new(1),
            initialized: false,
            open_documents: OpenDocuments::default(),
        };

        client.initialize(root_uri)?;
        Ok(client)
    }

    fn check_alive(&mut self) -> Result<(), String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Err(format!("LSP server exited: {status}")),
            Ok(None) => Ok(()),
            Err(e) => Err(format!("Cannot check LSP server status: {e}")),
        }
    }

    /// Waits up to `SHUTDOWN_TIMEOUT_SECS` for the server to exit, then kills
    /// it, and reaps it either way: no server outlives its client, none is
    /// left a zombie.
    fn reap(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(SHUTDOWN_TIMEOUT_SECS);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    #[allow(deprecated)]
    fn initialize(&mut self, root_uri: &Uri) -> Result<(), String> {
        let params = InitializeParams {
            root_uri: Some(root_uri.clone()),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    rename: Some(RenameClientCapabilities {
                        dynamic_registration: Some(false),
                        prepare_support: Some(true),
                        ..Default::default()
                    }),
                    references: Some(DynamicRegistrationClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    definition: Some(GotoCapability {
                        dynamic_registration: Some(false),
                        link_support: Some(false),
                    }),
                    implementation: Some(GotoCapability {
                        dynamic_registration: Some(false),
                        link_support: Some(false),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let _result = self.request_with_timeout::<request::Initialize>(
            params,
            Duration::from_secs(INIT_TIMEOUT_SECS),
        )?;
        self.send_notification::<notification::Initialized>(InitializedParams {})?;
        self.initialized = true;
        Ok(())
    }

    /// Gives the server `text` as the content of `uri`: `didOpen` the first
    /// time, a full-text `didChange` after that.
    pub fn did_open(&mut self, uri: &Uri, language_id: &str, text: &str) -> Result<(), String> {
        self.check_alive()?;
        let msg = self.open_documents.sync(uri, language_id, text)?;
        self.channel.send(&msg.into())
    }

    pub fn references(&mut self, uri: &Uri, position: Position) -> Result<Vec<Location>, String> {
        self.check_alive()?;
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            context: ReferenceContext {
                include_declaration: true,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let result = self.request_with_timeout::<request::References>(
            params,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        )?;
        Ok(result.unwrap_or_default())
    }

    pub fn definition(
        &mut self,
        uri: &Uri,
        position: Position,
    ) -> Result<GotoDefinitionResponse, String> {
        self.check_alive()?;
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let result = self.request_with_timeout::<request::GotoDefinition>(
            params,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        )?;
        Ok(result.unwrap_or(GotoDefinitionResponse::Array(vec![])))
    }

    pub fn rename(
        &mut self,
        uri: &Uri,
        position: Position,
        new_name: &str,
    ) -> Result<Option<WorkspaceEdit>, String> {
        self.check_alive()?;
        let params = RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            new_name: new_name.to_string(),
            work_done_progress_params: Default::default(),
        };
        self.request_with_timeout::<request::Rename>(
            params,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        )
    }

    pub fn implementations(
        &mut self,
        uri: &Uri,
        position: Position,
    ) -> Result<Vec<Location>, String> {
        self.check_alive()?;
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let value = self.request_raw_with_timeout(
            "textDocument/implementation",
            serde_json::to_value(params).unwrap_or_default(),
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        )?;
        Ok(serde_json::from_value(value).unwrap_or_default())
    }

    fn request_with_timeout<R: request::Request>(
        &mut self,
        params: R::Params,
        timeout: Duration,
    ) -> Result<R::Result, String>
    where
        R::Params: Serialize,
        R::Result: for<'de> Deserialize<'de>,
    {
        let value = self.request_raw_with_timeout(
            R::METHOD,
            serde_json::to_value(params).map_err(|e| e.to_string())?,
            timeout,
        )?;
        serde_json::from_value(value).map_err(|e| format!("Deserialize error: {e}"))
    }

    fn request_raw_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = RequestId::from(self.next_id.fetch_add(1, Ordering::Relaxed));
        self.channel.send(
            &Request {
                id: id.clone(),
                method: method.to_string(),
                params,
            }
            .into(),
        )?;
        self.channel.read_response(&id, timeout)
    }

    fn send_notification<N: notification::Notification>(
        &mut self,
        params: N::Params,
    ) -> Result<(), String>
    where
        N::Params: Serialize,
    {
        self.channel.send(
            &Notification {
                method: N::METHOD.to_string(),
                params: serde_json::to_value(params).map_err(|e| e.to_string())?,
            }
            .into(),
        )
    }

    fn shutdown(&mut self) {
        let _ = self.request_raw_with_timeout(
            "shutdown",
            Value::Null,
            Duration::from_secs(SHUTDOWN_TIMEOUT_SECS),
        );
        let _ = self.send_notification::<notification::Exit>(());
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if self.initialized && self.check_alive().is_ok() {
            self.shutdown();
        }
        self.reap();
    }
}

impl crate::lsp::backend::LspBackend for LspClient {
    fn is_alive(&mut self) -> bool {
        self.check_alive().is_ok()
    }
    fn open_file(
        &mut self,
        uri: &lsp_types::Uri,
        language_id: &str,
        text: &str,
    ) -> Result<(), String> {
        LspClient::did_open(self, uri, language_id, text)
    }
    fn references(
        &mut self,
        uri: &lsp_types::Uri,
        position: lsp_types::Position,
    ) -> Result<Vec<lsp_types::Location>, String> {
        LspClient::references(self, uri, position)
    }
    fn definition(
        &mut self,
        uri: &lsp_types::Uri,
        position: lsp_types::Position,
    ) -> Result<lsp_types::GotoDefinitionResponse, String> {
        LspClient::definition(self, uri, position)
    }
    fn implementations(
        &mut self,
        uri: &lsp_types::Uri,
        position: lsp_types::Position,
    ) -> Result<Vec<lsp_types::Location>, String> {
        LspClient::implementations(self, uri, position)
    }
    fn rename(
        &mut self,
        uri: &lsp_types::Uri,
        position: lsp_types::Position,
        new_name: &str,
    ) -> Result<Option<lsp_types::WorkspaceEdit>, String> {
        LspClient::rename(self, uri, position, new_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    /// A writer whose bytes the test reads back.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Sink {
        fn bytes(&self) -> Vec<u8> {
            self.0.lock().unwrap().clone()
        }

        fn messages(&self) -> Vec<Message> {
            let mut bytes = std::io::Cursor::new(self.bytes());
            std::iter::from_fn(|| Message::read(&mut bytes).unwrap()).collect()
        }
    }

    fn server_stream(messages: &[Message]) -> std::io::Cursor<Vec<u8>> {
        let mut bytes = Vec::new();
        for msg in messages {
            msg.write(&mut bytes).unwrap();
        }
        std::io::Cursor::new(bytes)
    }

    fn next(channel: &Channel) -> Result<Message, String> {
        channel
            .incoming
            .recv_timeout(Duration::from_secs(5))
            .expect("the reader thread delivers every message")
    }

    #[test]
    fn framing_round_trips() {
        let sink = Sink::default();
        let mut out = Channel::new(std::io::empty(), sink.clone());
        out.send(
            &Request {
                id: 7.into(),
                method: "textDocument/references".into(),
                params: json!({ "text": "a b é" }),
            }
            .into(),
        )
        .unwrap();
        out.send(
            &Notification {
                method: "exit".into(),
                params: Value::Null,
            }
            .into(),
        )
        .unwrap();
        assert!(sink.bytes().starts_with(b"Content-Length: "));

        let back = Channel::new(std::io::Cursor::new(sink.bytes()), Sink::default());
        match next(&back).unwrap() {
            Message::Request(req) => {
                assert_eq!(req.id, 7.into());
                assert_eq!(req.method, "textDocument/references");
                assert_eq!(req.params, json!({ "text": "a b é" }));
            }
            other => panic!("expected the request, got {other:?}"),
        }
        match next(&back).unwrap() {
            Message::Notification(n) => assert_eq!(n.method, "exit"),
            other => panic!("expected the notification, got {other:?}"),
        }
        assert!(next(&back).is_err(), "end of stream is reported");
    }

    #[test]
    fn server_requests_are_answered_and_never_taken_for_the_response() {
        let stream = server_stream(&[
            // Same number as the client's request: a request, not its response.
            Request {
                id: 1.into(),
                method: "workspace/configuration".into(),
                params: json!({ "items": [{ "section": "a" }, { "section": "b" }] }),
            }
            .into(),
            Request {
                id: 2.into(),
                method: "client/registerCapability".into(),
                params: json!({ "registrations": [] }),
            }
            .into(),
            Request {
                id: 3.into(),
                method: "window/workDoneProgress/create".into(),
                params: json!({ "token": "t" }),
            }
            .into(),
            Notification {
                method: "window/logMessage".into(),
                params: json!({ "type": 3, "message": "indexing" }),
            }
            .into(),
            Response::new_ok(1.into(), json!({ "answer": 42 })).into(),
        ]);
        let sink = Sink::default();
        let mut channel = Channel::new(stream, sink.clone());

        let result = channel
            .read_response(&1.into(), Duration::from_secs(5))
            .unwrap();
        assert_eq!(result, json!({ "answer": 42 }));

        let replies: Vec<Response> = sink
            .messages()
            .into_iter()
            .map(|m| match m {
                Message::Response(r) => r,
                other => panic!("the client only answers, got {other:?}"),
            })
            .collect();
        assert_eq!(replies.len(), 3);
        assert_eq!(replies[0].id, 1.into());
        assert_eq!(
            replies[0].response_result.as_ref().unwrap(),
            &json!([null, null])
        );
        assert_eq!(replies[1].id, 2.into());
        assert_eq!(replies[1].response_result.as_ref().unwrap(), &Value::Null);
        assert_eq!(replies[2].id, 3.into());
        assert_eq!(
            replies[2].response_result.as_ref().unwrap_err().code,
            ErrorCode::MethodNotFound as i32
        );
    }

    #[test]
    fn an_error_response_is_an_error() {
        let stream = server_stream(&[Response::new_err(
            4.into(),
            ErrorCode::InvalidParams as i32,
            "bad position".into(),
        )
        .into()]);
        let mut channel = Channel::new(stream, Sink::default());
        let err = channel
            .read_response(&4.into(), Duration::from_secs(5))
            .unwrap_err();
        assert_eq!(err, "LSP error: bad position");
    }

    /// A document is opened once; opening it again sends its new text as a
    /// versioned change.
    #[test]
    fn reopening_a_document_sends_a_change() {
        let mut docs = OpenDocuments::default();
        let uri: Uri = "file:///p/a.rs".parse().unwrap();

        let first = docs.sync(&uri, "rust", "fn a() {}").unwrap();
        assert_eq!(first.method, "textDocument/didOpen");
        assert_eq!(first.params["textDocument"]["version"], 1);

        let second = docs.sync(&uri, "rust", "fn b() {}").unwrap();
        assert_eq!(second.method, "textDocument/didChange");
        assert_eq!(second.params["textDocument"]["version"], 2);
        assert_eq!(second.params["contentChanges"][0]["text"], "fn b() {}");
        assert!(second.params["contentChanges"][0].get("range").is_none());

        let other: Uri = "file:///p/b.rs".parse().unwrap();
        assert_eq!(
            docs.sync(&other, "rust", "").unwrap().method,
            "textDocument/didOpen"
        );
    }

    #[test]
    fn file_uri_is_percent_encoded_and_round_trips() {
        let path = std::env::temp_dir().join("dir with space").join("a#b.rs");
        let uri = file_path_to_uri(&path.to_string_lossy()).unwrap();
        assert!(uri.as_str().starts_with("file:///"), "{}", uri.as_str());
        assert!(
            uri.as_str().ends_with("/dir%20with%20space/a%23b.rs"),
            "{}",
            uri.as_str()
        );
        assert_eq!(
            uri_to_file_path(&uri).as_deref(),
            Some(&*path.to_string_lossy())
        );
    }
}
