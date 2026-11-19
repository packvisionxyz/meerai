use leptos::prelude::{RwSignal, Update};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextBuffer {
    pub text: String,
    pub cursor: usize,
}

impl TextBuffer {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.len();
        Self { text, cursor }
    }

    pub fn replace_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    pub fn insert(&mut self, text: &str) {
        self.text.push_str(text);
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEffect {
    Copy(String),
    Focus(String),
    ScrollToBottom(String),
    ScrollToBottomIfNear(String),
    ScrollIntoView(String),
    SetTitle(String),
    OpenUrl(String),
}

#[derive(Clone, Copy)]
pub struct UiEffects {
    queue: RwSignal<Vec<UiEffect>>,
}

impl UiEffects {
    pub fn new(queue: RwSignal<Vec<UiEffect>>) -> Self {
        Self { queue }
    }

    fn push(&mut self, effect: UiEffect) {
        self.queue.update(|queue| queue.push(effect));
    }

    pub fn copy(&mut self, text: String) {
        self.push(UiEffect::Copy(text));
    }

    pub fn focus(&mut self, id: &str) {
        self.push(UiEffect::Focus(id.to_owned()));
    }

    pub fn scroll_to_bottom(&mut self, id: &str) {
        self.push(UiEffect::ScrollToBottom(id.to_owned()));
    }

    pub fn scroll_to_bottom_if_near(&mut self, id: &str) {
        self.push(UiEffect::ScrollToBottomIfNear(id.to_owned()));
    }

    pub fn scroll_into_view(&mut self, id: &str) {
        self.push(UiEffect::ScrollIntoView(id.to_owned()));
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.push(UiEffect::SetTitle(title.into()));
    }

    pub fn open_url(&mut self, url: &str) {
        self.push(UiEffect::OpenUrl(url.to_owned()));
    }
}
