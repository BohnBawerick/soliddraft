//! The web build of SolidDraft: the app compiled to WebAssembly and drawn into the page's
//! canvas. `trunk build` in this directory makes it; docs/web.md has the rest.
//!
//! Add `?demo` to the page's address to start with the sample part, `?preset=fast`,
//! `studio` or `realistic` to start in that look, and `?theme=dark` for the dark
//! interface.

#![cfg(target_arch = "wasm32")]

use soliddraft_app::app::{Automation, SolidDraftApp};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// Starts the app when the page loads the module.
#[wasm_bindgen(start)]
pub fn start() {
    let _ = eframe::WebLogger::init(log::LevelFilter::Info);
    std::panic::set_hook(Box::new(|info| {
        soliddraft_app::journal_browser::mark_crashed();
        let message = format!("SolidDraft stopped: {info}");
        web_sys::console::error_1(&message.clone().into());
        show_status(&message);
    }));
    let window = web_sys::window().expect("a browser window");
    let document = window.document().expect("a document");
    let canvas: web_sys::HtmlCanvasElement =
        document.get_element_by_id("soliddraft").expect("the page's canvas").unchecked_into();
    let query = window.location().search().unwrap_or_default();
    let demo = query.split(['?', '&']).any(|p| p == "demo");
    let preset = query
        .split(['?', '&'])
        .find_map(|p| p.strip_prefix("preset="))
        .and_then(soliddraft_app::viewport::look::Preset::parse);
    let theme =
        query.split(['?', '&']).find_map(|p| p.strip_prefix("theme=")).and_then(soliddraft_app::theme::Mode::parse);
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = soliddraft_app::journal_browser::start().await {
            show_status(&format!("SolidDraft could not start its command journal: {error}"));
            return;
        }
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| {
                    let doc = if demo { soliddraft_app::demo_document() } else { soliddraft_app::new_document() };
                    let automation = Automation { connect_assistant: true, preset, theme, ..Default::default() };
                    Ok(Box::new(SolidDraftApp::new(cc, doc, automation)))
                }),
            )
            .await;
        match started {
            Ok(()) => hide_status(),
            Err(e) => show_status(&format!("SolidDraft could not start: {e:?}")),
        }
    });
}

fn status() -> Option<web_sys::Element> {
    web_sys::window()?.document()?.get_element_by_id("status")
}

fn show_status(text: &str) {
    if let Some(el) = status() {
        el.set_text_content(Some(text));
        let _ = el.set_attribute("class", "shown");
    }
}

fn hide_status() {
    if let Some(el) = status() {
        let _ = el.set_attribute("class", "hidden");
    }
}
