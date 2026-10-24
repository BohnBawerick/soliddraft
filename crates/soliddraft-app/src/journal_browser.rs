//! Browser journal storage and the minimum crash-report download window.

use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};

use soliddraft_document::journal::{self, Session};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(module = "/src/journal_browser.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = prepare)]
    fn js_prepare(export: &js_sys::Function) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_name = retry)]
    fn js_retry(message: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = ready)]
    fn js_ready();
    #[wasm_bindgen(catch, js_name = start)]
    fn js_start(id: &str) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(catch, js_name = append)]
    fn js_append(id: &str, line: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(catch, js_name = read)]
    fn js_read(id: &str) -> Result<String, JsValue>;
    #[wasm_bindgen(js_name = close)]
    fn js_close(id: &str);
    #[wasm_bindgen(js_name = crashed)]
    fn js_crashed();
    #[wasm_bindgen(catch, js_name = discard)]
    fn js_discard(id: &str) -> Result<js_sys::Promise, JsValue>;
}

fn error(value: JsValue) -> String {
    js_sys::Reflect::get(&value, &"message".into())
        .ok()
        .and_then(|s| s.as_string())
        .or_else(|| value.as_string())
        .unwrap_or_else(|| "browser journal storage failed".into())
}

struct BrowserWriter(String);
impl Write for BrowserWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let line = std::str::from_utf8(bytes).map_err(std::io::Error::other)?;
        js_append(&self.0, line).map_err(|e| std::io::Error::other(error(e)))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct BrowserJournals {
    _session: Session,
    offers: Vec<String>,
    show: bool,
    error: Option<String>,
}
thread_local! {
    static STATE: RefCell<Option<BrowserJournals>> = const { RefCell::new(None) };
}

/// Installs read-only recovery before attempting a writable session. No document
/// or editor is created until a header has reached browser storage.
pub async fn start() -> Result<(), String> {
    let export = Closure::<dyn FnMut(String) -> Result<(), JsValue>>::new(|text: String| {
        export_report(&text).map_err(|e| JsValue::from_str(&e))
    });
    let prepared = JsFuture::from(js_prepare(export.as_ref().unchecked_ref()).map_err(error)?).await.map_err(error)?;
    let mut offers = parse_offers(prepared)?;
    loop {
        let id = soliddraft_document::DocumentId::new().to_string();
        let session = async {
            JsFuture::from(js_start(&id).map_err(error)?).await.map_err(error)?;
            Session::start_external(PathBuf::from(&id), BrowserWriter(id.clone()), |path: &Path| {
                if let Some(id) = path.to_str() {
                    js_close(id);
                }
            })
            .map_err(|e| e.to_string())
        }
        .await;
        match session {
            Ok(session) => {
                STATE.with(|state| {
                    *state.borrow_mut() =
                        Some(BrowserJournals { _session: session, show: !offers.is_empty(), offers, error: None })
                });
                js_ready();
                return Ok(());
            }
            Err(error) => {
                // A failed header is not an active session. Release its lock,
                // retaining any records, so another attempt can start safely.
                js_close(&id);
                offers = parse_offers(JsFuture::from(js_retry(&error)).await.map_err(self::error)?)?;
            }
        }
    }
}

fn parse_offers(value: JsValue) -> Result<Vec<String>, String> {
    serde_json::from_str(&value.as_string().ok_or("invalid browser journal offers")?).map_err(|e| e.to_string())
}

fn export_report(text: &str) -> Result<(), String> {
    let bytes = journal::report_bytes(text.as_bytes())?;
    crate::web::download("crash-report.jsonl", &bytes)
}

/// Prevents normal page teardown from removing a journal after a Rust panic.
pub fn mark_crashed() {
    js_crashed();
}

/// Whether the File menu has crash reports to offer.
pub fn has_offers() -> bool {
    STATE.with(|state| state.borrow().as_ref().is_some_and(|s| !s.offers.is_empty()))
}

/// Opens the crash-report window again after Later.
pub fn show() {
    STATE.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.show = true;
        }
    });
}

/// Offers redacted downloads, never the raw localStorage journal.
pub fn window(ctx: &egui::Context, app: &mut crate::state::AppState) {
    if let Some(failure) = journal::take_failure() {
        app.note(failure, crate::state::NoteKind::Error);
    }
    let mut export = None;
    let mut discard = None;
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(state) = state.as_mut() else { return };
        if !state.show {
            return;
        }
        egui::Window::new("Crash report journals").open(&mut state.show).show(ctx, |ui| {
            ui.label("SolidDraft did not close properly. Personal paths are replaced in report downloads.");
            for (index, id) in state.offers.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("Session {}", index + 1));
                    if ui.button("Save crash report journal").clicked() {
                        export = Some(id.clone());
                    }
                    if ui.button("Discard journal").clicked() {
                        discard = Some(id.clone());
                    }
                });
            }
            if let Some(error) = &state.error {
                ui.label(error);
            }
        });
    });
    if let Some(id) = export {
        let result = js_read(&id).map_err(error).and_then(|text| export_report(&text));
        match result {
            Ok(()) => {
                app.note("Crash report journal downloaded. Review it before sharing.", crate::state::NoteKind::Ok)
            }
            Err(error) => app.note(error, crate::state::NoteKind::Error),
        }
    }
    if let Some(id) = discard {
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match js_discard(&id) {
                Ok(promise) => JsFuture::from(promise).await.map(|_| ()).map_err(error),
                Err(value) => Err(error(value)),
            };
            STATE.with(|state| {
                if let Some(state) = state.borrow_mut().as_mut() {
                    match result {
                        Ok(()) => {
                            state.offers.retain(|offer| offer != &id);
                            state.error = None;
                        }
                        Err(error) => state.error = Some(error),
                    }
                }
            });
            ctx.request_repaint();
        });
    }
}
