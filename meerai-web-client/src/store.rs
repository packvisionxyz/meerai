use leptos::prelude::{RwSignal, Update, With, batch};

use crate::state::AppState;
use crate::text::{UiEffect, UiEffects};

#[derive(Clone, Copy)]
pub struct StoreHandle {
    pub state: RwSignal<AppState>,
    effects: RwSignal<Vec<UiEffect>>,
}

impl StoreHandle {
    pub fn new(initial: AppState) -> Self {
        Self {
            state: RwSignal::new(initial),
            effects: RwSignal::new(Vec::new()),
        }
    }

    pub fn update(&self, f: impl FnOnce(&mut AppState)) {
        batch(|| self.state.update(f));
        self.flush();
    }

    pub fn update_with_effects(&self, f: impl FnOnce(&mut AppState, &mut UiEffects)) {
        let mut effects = UiEffects::new(self.effects);
        batch(|| self.state.update(|state| f(state, &mut effects)));
        self.flush();
    }

    pub fn effect(&self, effect: UiEffect) {
        self.effects.update(|queue| queue.push(effect));
        self.flush();
    }

    pub fn with<T>(&self, f: impl FnOnce(&AppState) -> T) -> T {
        self.state.with(f)
    }

    fn flush(&self) {
        let mut effects = Vec::new();
        self.effects.update(|queue| {
            effects = std::mem::take(queue);
        });
        for effect in effects {
            apply_effect(effect);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn after_frame(f: impl FnOnce() + 'static) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    let callback = Closure::once_into_js(f);
    if let Some(window) = web_sys::window()
        && let Some(callback) = callback.dyn_ref::<js_sys::Function>()
    {
        let _ = window.request_animation_frame(callback);
    }
}

#[cfg(target_arch = "wasm32")]
fn apply_effect(effect: UiEffect) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::spawn_local;

    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    match effect {
        UiEffect::Copy(text) => {
            let clipboard = window.navigator().clipboard();
            spawn_local(async move {
                let _ = wasm_bindgen_futures::JsFuture::from(clipboard.write_text(&text)).await;
            });
        }
        UiEffect::Focus(id) => {
            after_frame(move || {
                if let Some(element) = web_sys::window()
                    .and_then(|window| window.document())
                    .and_then(|document| document.get_element_by_id(&id))
                    .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = element.focus();
                }
            });
        }
        UiEffect::ScrollToBottom(id) => {
            after_frame(move || {
                if let Some(element) = web_sys::window()
                    .and_then(|window| window.document())
                    .and_then(|document| document.get_element_by_id(&id))
                {
                    element.set_scroll_top(element.scroll_height());
                }
            });
        }
        UiEffect::ScrollIntoView(id) => {
            after_frame(move || {
                if let Some(element) = web_sys::window()
                    .and_then(|window| window.document())
                    .and_then(|document| document.get_element_by_id(&id))
                {
                    element.scroll_into_view_with_bool(false);
                }
            });
        }
        UiEffect::SetTitle(title) => {
            document.set_title(&title);
        }
        UiEffect::OpenUrl(url) => {
            let _ = window.open_with_url_and_target(&url, "_blank");
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_effect(_effect: UiEffect) {}

#[cfg(target_arch = "wasm32")]
pub fn copy_text(text: String) {
    use wasm_bindgen_futures::spawn_local;

    if let Some(window) = web_sys::window() {
        let clipboard = window.navigator().clipboard();
        spawn_local(async move {
            let _ = wasm_bindgen_futures::JsFuture::from(clipboard.write_text(&text)).await;
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn copy_text(_text: String) {}
