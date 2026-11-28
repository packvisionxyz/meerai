use leptos::prelude::*;

#[component]
pub fn IconSidebarLeft() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="1.75" y="2.25" width="12.5" height="11.5" rx="2" />
            <path d="M5.75 2.5v11" />
        </svg>
    }
}

#[component]
pub fn IconSidebarRight() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="1.75" y="2.25" width="12.5" height="11.5" rx="2" />
            <path d="M10.25 2.5v11" />
        </svg>
    }
}

#[component]
pub fn IconPlus() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M8 3.25v9.5M3.25 8h9.5" />
        </svg>
    }
}

#[component]
pub fn IconRefresh() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M13.25 8a5.25 5.25 0 1 1-1.35-3.5" />
            <path d="M13.25 2.75v2.5h-2.5" />
        </svg>
    }
}

#[component]
pub fn IconSearch() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="7" cy="7" r="4.5" />
            <path d="m10.5 10.5 3.25 3.25" />
        </svg>
    }
}

#[component]
pub fn IconDiff() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M4.5 3v10M11.5 5v8M4.5 5.5c0 2 7 1.5 7 3.5" />
            <circle cx="4.5" cy="3" r="1.25" />
            <circle cx="4.5" cy="13" r="1.25" />
            <circle cx="11.5" cy="4.25" r="1.25" />
        </svg>
    }
}

#[component]
pub fn IconCommand() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M5.75 5.75h4.5v4.5h-4.5z" />
            <path d="M5.75 5.75V4a1.75 1.75 0 1 0-1.75 1.75h1.75Zm4.5 0V4A1.75 1.75 0 1 1 12 5.75h-1.75Zm-4.5 4.5V12A1.75 1.75 0 1 1 4 10.25h1.75Zm4.5 0V12A1.75 1.75 0 1 0 12 10.25h-1.75Z" />
        </svg>
    }
}

#[component]
pub fn IconPlug() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M6 2.5v3M10 2.5v3M4.5 5.5h7v2a3.5 3.5 0 0 1-7 0v-2ZM8 11v2.75" />
        </svg>
    }
}

#[component]
pub fn IconSettings() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="8" cy="8" r="2.25" />
            <path d="M8 1.75v1.5M8 12.75v1.5M2.6 4.88l1.3.75M12.1 10.37l1.3.75M2.6 11.12l1.3-.75M12.1 5.63l1.3-.75" />
        </svg>
    }
}

#[component]
pub fn IconHelp() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="8" cy="8" r="6" />
            <path d="M6.25 6.25a1.75 1.75 0 0 1 3.38.62c0 1.13-1.63 1.5-1.63 2.5" />
            <circle cx="8" cy="11.75" r="0.5" fill="currentColor" stroke="none" />
        </svg>
    }
}

#[component]
pub fn IconChevronRight() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="m6 3.75 4.25 4.25L6 12.25" />
        </svg>
    }
}

#[component]
pub fn IconChevronDown() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="m3.75 6 4.25 4.25L12.25 6" />
        </svg>
    }
}

#[component]
pub fn IconSparkles() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M8 2.25 9.2 6.8 13.75 8 9.2 9.2 8 13.75 6.8 9.2 2.25 8 6.8 6.8Z" />
        </svg>
    }
}

#[component]
pub fn IconGitBranch() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="4.5" cy="3.5" r="1.5" />
            <circle cx="4.5" cy="12.5" r="1.5" />
            <circle cx="11.5" cy="5.5" r="1.5" />
            <path d="M4.5 5v6M11.5 7c0 2.5-7 1.5-7 4" />
        </svg>
    }
}

#[component]
pub fn IconCopy() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="5.25" y="5.25" width="8.5" height="8.5" rx="1.5" />
            <path d="M10.75 5.25V3.5A1.25 1.25 0 0 0 9.5 2.25h-6A1.25 1.25 0 0 0 2.25 3.5v6a1.25 1.25 0 0 0 1.25 1.25h1.75" />
        </svg>
    }
}

#[component]
pub fn IconCheck() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="m3.5 8.5 3 3 6-6.5" />
        </svg>
    }
}

#[component]
pub fn IconSend() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M8 12.5V3.5" />
            <path d="M4 7.5 8 3.5l4 4" />
        </svg>
    }
}

#[component]
pub fn IconClose() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="m4 4 8 8M12 4l-8 8" />
        </svg>
    }
}

#[component]
pub fn IconMap() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M2.25 3.75 6 2.25l4 1.5 3.75-1.5v10L10 13.75l-4-1.5-3.75 1.5Z" />
            <path d="M6 2.25v10M10 3.75v10" />
        </svg>
    }
}

#[component]
pub fn IconWrench() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M10.75 2.5a3.25 3.25 0 0 0-3.08 4.3L3.1 11.38a1.25 1.25 0 1 0 1.77 1.77l4.58-4.58a3.25 3.25 0 0 0 4.3-3.08l-2 2-1.5-1.5 2-2Z" />
        </svg>
    }
}

#[component]
pub fn IconFileDiff() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M9.5 2.25H4.25a1.5 1.5 0 0 0-1.5 1.5v8.5a1.5 1.5 0 0 0 1.5 1.5h7.5a1.5 1.5 0 0 0 1.5-1.5V6Z" />
            <path d="M9.5 2.25V6h3.75M6 8.25h4M8 6.5v3.5M6 11.5h4" />
        </svg>
    }
}

#[component]
pub fn IconServer() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="2.25" y="2.5" width="11.5" height="4.5" rx="1.25" />
            <rect x="2.25" y="9" width="11.5" height="4.5" rx="1.25" />
            <circle cx="4.75" cy="4.75" r="0.6" fill="currentColor" stroke="none" />
            <circle cx="4.75" cy="11.25" r="0.6" fill="currentColor" stroke="none" />
        </svg>
    }
}

#[component]
pub fn IconClock() -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="8" cy="8" r="5.75" />
            <path d="M8 4.75V8l2.25 1.5" />
        </svg>
    }
}
