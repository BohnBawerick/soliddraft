//! The browser application uses Catallax in its own WebAssembly module.
//! Assistant requests still use the server and stream in the background.

use std::cell::RefCell;
use std::sync::Arc;

use serde::Deserialize;
use soliddraft_assistant::relay::{LineReader, RelayLine};
use soliddraft_assistant::wire::ChatRequest;
use soliddraft_assistant::{Delivery, Provider, ProviderError, ProviderResponse, StreamEvent};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

thread_local! {
    static CONTEXT: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
    static UPLOAD: RefCell<Option<(String, Vec<u8>)>> = const { RefCell::new(None) };
}

/// Lets background work (an upload, an assistant reply) wake the interface.
pub fn set_context(ctx: &egui::Context) {
    CONTEXT.with(|c| *c.borrow_mut() = Some(ctx.clone()));
}

fn repaint() {
    CONTEXT.with(|c| {
        if let Some(ctx) = c.borrow().as_ref() {
            ctx.request_repaint();
        }
    });
}

fn window() -> web_sys::Window {
    web_sys::window().expect("running in a browser window")
}

fn js_error(value: JsValue) -> String {
    value.as_string().or_else(|| js_sys::JSON::stringify(&value).ok().and_then(|s| s.as_string())).unwrap_or_default()
}

/// The server's answer to a failed request, in a line. A page from something that is not
/// the SolidDraft server, such as a proxy's error page, is left out.
fn refusal(status: u16, body: &str) -> String {
    let line = body.lines().next().unwrap_or("").trim();
    if line.is_empty() || line.starts_with('<') {
        format!("the SolidDraft server answered {status}")
    } else {
        format!("the SolidDraft server answered {status}: {}", line.chars().take(200).collect::<String>())
    }
}

/// One synchronous request to the server. Returns the status and the body.
fn request(method: &str, url: &str, body: Option<&str>) -> Result<(u16, String), String> {
    let xhr = web_sys::XmlHttpRequest::new().map_err(js_error)?;
    xhr.open_with_async(method, url, false).map_err(js_error)?;
    if body.is_some() {
        xhr.set_request_header("content-type", "application/json").map_err(js_error)?;
    }
    xhr.send_with_opt_str(body).map_err(|e| format!("cannot reach the SolidDraft server: {}", js_error(e)))?;
    let status = xhr.status().map_err(js_error)?;
    let text = xhr.response_text().map_err(js_error)?.unwrap_or_default();
    Ok((status, text))
}

/// What the server says about itself.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerInfo {
    /// The assistant, if the server has a provider for it.
    pub assistant: AssistantInfo,
}

/// The server's assistant.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AssistantInfo {
    /// Whether the server can reach a model.
    pub on: bool,
    /// The provider's name.
    #[serde(default)]
    pub provider: String,
    /// The model.
    #[serde(default)]
    pub model: String,
    /// Why it is off.
    #[serde(default)]
    pub reason: String,
}

/// Asks the server about itself.
pub fn server_info() -> Result<ServerInfo, String> {
    match request("GET", "api/info", None)? {
        (200, text) => serde_json::from_str(&text).map_err(|e| format!("bad reply from the server: {e}")),
        (status, text) => Err(refusal(status, &text)),
    }
}

/// The assistant through the server, or why the server has none.
pub fn assistant() -> Result<(Arc<dyn Provider>, &'static str), String> {
    let info = server_info().map_err(|e| format!("The assistant is off: {e}"))?;
    if info.assistant.on {
        Ok((Arc::new(RelayProvider::new(&info.assistant)), "through the SolidDraft server"))
    } else {
        Err(info.assistant.reason)
    }
}

/// The assistant's model, reached through the server's relay. The server holds the key.
pub struct RelayProvider {
    name: String,
    model: String,
}

impl RelayProvider {
    /// The provider the server says it runs.
    pub fn new(info: &AssistantInfo) -> Self {
        RelayProvider { name: info.provider.clone(), model: info.model.clone() }
    }
}

impl Provider for RelayProvider {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn model(&self) -> String {
        self.model.clone()
    }

    fn send(
        &self,
        _request: &ChatRequest,
        _on_event: &mut dyn FnMut(StreamEvent) -> bool,
    ) -> Result<ProviderResponse, ProviderError> {
        Err(ProviderError::Protocol("the browser cannot wait for a reply; use start".into()))
    }

    fn start(self: Arc<Self>, request: ChatRequest, delivery: Delivery) {
        wasm_bindgen_futures::spawn_local(async move {
            let Delivery { mut on_event, on_done } = delivery;
            let result = relay(&request, &mut *on_event).await;
            on_done(result);
            repaint();
        });
    }
}

async fn relay(
    request: &ChatRequest,
    on_event: &mut dyn FnMut(StreamEvent) -> bool,
) -> Result<ProviderResponse, ProviderError> {
    let network = |e: JsValue| ProviderError::Network(js_error(e));
    let body = serde_json::to_string(request).map_err(|e| ProviderError::Protocol(e.to_string()))?;
    let abort = web_sys::AbortController::new().map_err(network)?;
    let headers = web_sys::Headers::new().map_err(network)?;
    headers.set("content-type", "application/json").map_err(network)?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_headers(&headers);
    init.set_body(&JsValue::from_str(&body));
    init.set_signal(Some(&abort.signal()));
    let response: web_sys::Response = JsFuture::from(window().fetch_with_str_and_init("api/assistant", &init))
        .await
        .map_err(network)?
        .unchecked_into();
    if !response.ok() {
        let text = match response.text() {
            Ok(p) => JsFuture::from(p).await.ok().and_then(|t| t.as_string()).unwrap_or_default(),
            Err(_) => String::new(),
        };
        return Err(ProviderError::Api { status: response.status(), kind: "relay_error".into(), message: text });
    }
    let stream = response.body().ok_or_else(|| ProviderError::Protocol("the relay sent no body".into()))?;
    let reader: web_sys::ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
    let mut lines = LineReader::default();
    loop {
        let chunk = JsFuture::from(reader.read()).await.map_err(network)?;
        let done = js_sys::Reflect::get(&chunk, &"done".into()).ok().and_then(|d| d.as_bool()).unwrap_or(true);
        if done {
            return Err(ProviderError::Network("the relay ended before the reply did".into()));
        }
        let value: js_sys::Uint8Array =
            js_sys::Reflect::get(&chunk, &"value".into()).map_err(network)?.unchecked_into();
        for line in lines.push(&value.to_vec()) {
            match line? {
                RelayLine::Event(e) => {
                    if !on_event(e) {
                        abort.abort();
                        return Err(ProviderError::Cancelled);
                    }
                }
                RelayLine::Done(response) => return Ok(response),
                RelayLine::Error(e) => return Err(e),
            }
        }
    }
}

/// Downloads `bytes` as a file called `name`.
pub fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    let document = window().document().ok_or("no document")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts).map_err(js_error)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js_error)?;
    let link: web_sys::HtmlAnchorElement = document.create_element("a").map_err(js_error)?.unchecked_into();
    link.set_href(&url);
    link.set_download(name);
    link.click();
    // Revoking at once can cancel the download in some browsers.
    let revoke = Closure::once(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(revoke.as_ref().unchecked_ref(), 60_000);
    revoke.forget();
    Ok(())
}

/// Opens the browser's file picker for one file with one of `extensions`. The file
/// arrives through [`take_upload`].
pub fn upload(extensions: &[&str]) {
    let Some(document) = window().document() else { return };
    let Ok(input) = document.create_element("input") else { return };
    let input: web_sys::HtmlInputElement = input.unchecked_into();
    input.set_type("file");
    input.set_accept(&extensions.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(","));
    let picked = input.clone();
    let on_change = Closure::once(move || {
        let Some(file) = picked.files().and_then(|f| f.get(0)) else { return };
        wasm_bindgen_futures::spawn_local(async move {
            match JsFuture::from(file.array_buffer()).await {
                Ok(buffer) => {
                    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                    UPLOAD.with(|u| *u.borrow_mut() = Some((file.name(), bytes)));
                    repaint();
                }
                Err(e) => log::error!("reading {}: {}", file.name(), js_error(e)),
            }
        });
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    on_change.forget();
    input.click();
}

/// The last uploaded file and its name, once.
pub fn take_upload() -> Option<(String, Vec<u8>)> {
    UPLOAD.with(|u| u.borrow_mut().take())
}
