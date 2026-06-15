//! Embedded Lucide icons (stroke 1.75, matching the legacy frontend's
//! `lucide.createIcons` defaults) served to GPUI through an [`AssetSource`].

use std::borrow::Cow;

use gpui::{svg, AssetSource, Rgba, SharedString, Styled, Svg};

macro_rules! lucide {
    ($body:expr) => {
        concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">"##,
            $body,
            "</svg>"
        )
    };
}

static ICONS: &[(&str, &str)] = &[
    (
        "search",
        lucide!(r##"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>"##),
    ),
    (
        "bell",
        lucide!(
            r##"<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/>"##
        ),
    ),
    (
        "share-2",
        lucide!(
            r##"<circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><line x1="8.59" x2="15.42" y1="13.51" y2="17.49"/><line x1="15.41" x2="8.59" y1="6.51" y2="10.49"/>"##
        ),
    ),
    (
        "sparkles",
        lucide!(
            r##"<path d="M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z"/><path d="M20 3v4"/><path d="M22 5h-4"/>"##
        ),
    ),
    (
        "brain",
        lucide!(
            r##"<path d="M12 5a3 3 0 1 0-5.997.125 4 4 0 0 0-2.526 5.77 4 4 0 0 0 .556 6.588A4 4 0 1 0 12 18Z"/><path d="M12 5a3 3 0 1 1 5.997.125 4 4 0 0 1 2.526 5.77 4 4 0 0 1-.556 6.588A4 4 0 1 1 12 18Z"/><path d="M15 13a4.5 4.5 0 0 1-3-4 4.5 4.5 0 0 1-3 4"/>"##
        ),
    ),
    (
        "arrow-up",
        lucide!(r##"<path d="m5 12 7-7 7 7"/><path d="M12 19V5"/>"##),
    ),
    (
        "square",
        lucide!(r##"<rect width="14" height="14" x="5" y="5" rx="2" fill="#000" stroke="none"/>"##),
    ),
    ("chevron-down", lucide!(r##"<path d="m6 9 6 6 6-6"/>"##)),
    ("chevron-right", lucide!(r##"<path d="m9 18 6-6-6-6"/>"##)),
    ("chevron-up", lucide!(r##"<path d="m18 15-6-6-6 6"/>"##)),
    ("chevron-left", lucide!(r##"<path d="m15 18-6-6 6-6"/>"##)),
    (
        "pin",
        lucide!(
            r##"<path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>"##
        ),
    ),
    (
        "plus",
        lucide!(r##"<path d="M5 12h14"/><path d="M12 5v14"/>"##),
    ),
    (
        "globe",
        lucide!(
            r##"<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>"##
        ),
    ),
    (
        "mic",
        lucide!(
            r##"<path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"/><path d="M19 10v2a7 7 0 0 1-14 0v-2"/><line x1="12" x2="12" y1="19" y2="22"/>"##
        ),
    ),
    (
        "x",
        lucide!(r##"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"##),
    ),
    (
        "infinity",
        lucide!(
            r##"<path d="M12 12c-2-2.67-4-4-6-4a4 4 0 1 0 0 8c2 0 4-1.33 6-4Zm0 0c2 2.67 4 4 6 4a4 4 0 0 0 0-8c-2 0-4 1.33-6 4Z"/>"##
        ),
    ),
    (
        "paperclip",
        lucide!(
            r##"<path d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"/>"##
        ),
    ),
    (
        "folder",
        lucide!(
            r##"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"##
        ),
    ),
    (
        "file",
        lucide!(
            r##"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/>"##
        ),
    ),
    (
        "list-tree",
        lucide!(
            r##"<path d="M21 12h-8"/><path d="M21 6H8"/><path d="M21 18h-8"/><path d="M3 6v4c0 1.1.9 2 2 2h3"/><path d="M3 10v6c0 1.1.9 2 2 2h3"/>"##
        ),
    ),
    (
        "list-todo",
        lucide!(
            r##"<rect x="3" y="5" width="6" height="6" rx="1"/><path d="m3 17 2 2 4-4"/><path d="M13 6h8"/><path d="M13 12h8"/><path d="M13 18h8"/>"##
        ),
    ),
    (
        "git-compare",
        lucide!(
            r##"<circle cx="18" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><path d="M13 6h3a2 2 0 0 1 2 2v7"/><path d="M11 18H8a2 2 0 0 1-2-2V9"/>"##
        ),
    ),
    (
        "git-branch",
        lucide!(
            r##"<line x1="6" x2="6" y1="3" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/>"##
        ),
    ),
    (
        "git-fork",
        lucide!(
            r##"<circle cx="12" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><circle cx="18" cy="6" r="3"/><path d="M18 9v2c0 .6-.4 1-1 1H7c-.6 0-1-.4-1-1V9"/><path d="M12 12v3"/>"##
        ),
    ),
    (
        "terminal",
        lucide!(r##"<polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/>"##),
    ),
    (
        "book-open",
        lucide!(
            r##"<path d="M12 7v14"/><path d="M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z"/>"##
        ),
    ),
    (
        "more-horizontal",
        lucide!(
            r##"<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>"##
        ),
    ),
    (
        "settings",
        lucide!(
            r##"<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>"##
        ),
    ),
    (
        "shopping-bag",
        lucide!(
            r##"<path d="M6 2 3 6v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V6l-3-4Z"/><path d="M3 6h18"/><path d="M16 10a4 4 0 0 1-8 0"/>"##
        ),
    ),
    (
        "user",
        lucide!(
            r##"<path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>"##
        ),
    ),
    (
        "folder-tree",
        lucide!(
            r##"<path d="M20 10a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1h-2.5a1 1 0 0 1-.8-.4l-.9-1.2A1 1 0 0 0 15 3h-2a1 1 0 0 0-1 1v5a1 1 0 0 0 1 1Z"/><path d="M20 21a1 1 0 0 0 1-1v-3a1 1 0 0 0-1-1h-2.5a1 1 0 0 1-.8-.4l-.9-1.2a1 1 0 0 0-.8-.4h-2a1 1 0 0 0-1 1v5a1 1 0 0 0 1 1Z"/><path d="M3 5a2 2 0 0 0 2 2h3"/><path d="M3 3v13a2 2 0 0 0 2 2h3"/>"##
        ),
    ),
    (
        "play",
        lucide!(r##"<polygon points="6 3 20 12 6 21 6 3"/>"##),
    ),
    (
        "rotate-ccw",
        lucide!(
            r##"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>"##
        ),
    ),
    (
        "home",
        lucide!(
            r##"<path d="m3 9 9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><polyline points="9 22 9 12 15 12 15 22"/>"##
        ),
    ),
    (
        "history",
        lucide!(
            r##"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M12 7v5l4 2"/>"##
        ),
    ),
    (
        "bug",
        lucide!(
            r##"<path d="m8 2 1.88 1.88"/><path d="M14.12 3.88 16 2"/><path d="M9 7.13v-1a3.003 3.003 0 1 1 6 0v1"/><path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v3c0 3.3-2.7 6-6 6"/><path d="M12 20v-9"/><path d="M6.53 9C4.6 8.8 3 7.1 3 5"/><path d="M6 13H2"/><path d="M3 21c0-2.1 1.7-3.9 3.8-4"/><path d="M20.97 5c0 2.1-1.6 3.8-3.5 4"/><path d="M22 13h-4"/><path d="M17.2 17c2.1.1 3.8 1.9 3.8 4"/>"##
        ),
    ),
    (
        "split",
        lucide!(
            r##"<path d="M16 3h5v5"/><path d="M8 3H3v5"/><path d="M12 22v-8.3a4 4 0 0 0-1.172-2.872L3 3"/><path d="m15 9 6-6"/>"##
        ),
    ),
    (
        "message-square-text",
        lucide!(
            r##"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/><path d="M13 8H7"/><path d="M17 12H7"/>"##
        ),
    ),
    ("check", lucide!(r##"<path d="M20 6 9 17l-5-5"/>"##)),
    (
        "panel-bottom",
        lucide!(r##"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 15h18"/>"##),
    ),
    (
        "user-round",
        lucide!(r##"<circle cx="12" cy="8" r="5"/><path d="M20 21a8 8 0 0 0-16 0"/>"##),
    ),
    (
        "settings-2",
        lucide!(
            r##"<path d="M20 7h-9"/><path d="M14 17H5"/><circle cx="17" cy="17" r="3"/><circle cx="7" cy="7" r="3"/>"##
        ),
    ),
    (
        "store",
        lucide!(
            r##"<path d="m2 7 4.41-4.41A2 2 0 0 1 7.83 2h8.34a2 2 0 0 1 1.42.59L22 7"/><path d="M4 12v8a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8"/><path d="M15 22v-4a2 2 0 0 0-2-2h-2a2 2 0 0 0-2 2v4"/><path d="M2 7h20"/>"##
        ),
    ),
    (
        "file-text",
        lucide!(
            r##"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/>"##
        ),
    ),
    (
        "list-checks",
        lucide!(
            r##"<path d="m3 17 2 2 4-4"/><path d="m3 7 2 2 4-4"/><path d="M13 6h8"/><path d="M13 12h8"/><path d="M13 18h8"/>"##
        ),
    ),
    (
        "network",
        lucide!(
            r##"<rect x="16" y="16" width="6" height="6" rx="1"/><rect x="2" y="16" width="6" height="6" rx="1"/><rect x="9" y="2" width="6" height="6" rx="1"/><path d="M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3"/><path d="M12 12V8"/>"##
        ),
    ),
    (
        "columns-2",
        lucide!(r##"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M12 3v18"/>"##),
    ),
    (
        "scroll-text",
        lucide!(
            r##"<path d="M15 12h-5"/><path d="M15 8h-5"/><path d="M19 17V5a2 2 0 0 0-2-2H4"/><path d="M8 21h12a2 2 0 0 0 2-2v-1a1 1 0 0 0-1-1H11a1 1 0 0 0-1 1v1a2 2 0 1 1-4 0V5a2 2 0 1 0-4 0v2a1 1 0 0 0 1 1h3"/>"##
        ),
    ),
    (
        "activity",
        lucide!(
            r##"<path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2"/>"##
        ),
    ),
    (
        "circle-alert",
        lucide!(
            r##"<circle cx="12" cy="12" r="10"/><line x1="12" x2="12" y1="8" y2="12"/><line x1="12" x2="12.01" y1="16" y2="16"/>"##
        ),
    ),
    (
        "logs",
        lucide!(
            r##"<path d="M13 12h8"/><path d="M13 18h8"/><path d="M13 6h8"/><path d="M3 12h1"/><path d="M3 18h1"/><path d="M3 6h1"/><path d="M8 12h1"/><path d="M8 18h1"/><path d="M8 6h1"/>"##
        ),
    ),
    (
        "trash-2",
        lucide!(
            r##"<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/><line x1="10" x2="10" y1="11" y2="17"/><line x1="14" x2="14" y1="11" y2="17"/>"##
        ),
    ),
    (
        "maximize-2",
        lucide!(
            r##"<polyline points="15 3 21 3 21 9"/><polyline points="9 21 3 21 3 15"/><line x1="21" x2="14" y1="3" y2="10"/><line x1="3" x2="10" y1="21" y2="14"/>"##
        ),
    ),
    (
        "plug",
        lucide!(
            r##"<path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z"/>"##
        ),
    ),
    (
        "pause",
        lucide!(
            r##"<rect x="14" y="4" width="4" height="16" rx="1"/><rect x="6" y="4" width="4" height="16" rx="1"/>"##
        ),
    ),
    (
        "rotate-cw",
        lucide!(
            r##"<path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>"##
        ),
    ),
    (
        "copy",
        lucide!(
            r##"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"##
        ),
    ),
    (
        "pin-off",
        lucide!(
            r##"<path d="M12 17v5"/><path d="M15 9.34V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H7.89"/><path d="m2 2 20 20"/><path d="M9 9v1.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h11"/>"##
        ),
    ),
    (
        "palette",
        lucide!(
            r##"<circle cx="13.5" cy="6.5" r=".5" fill="#000"/><circle cx="17.5" cy="10.5" r=".5" fill="#000"/><circle cx="8.5" cy="7.5" r=".5" fill="#000"/><circle cx="6.5" cy="12.5" r=".5" fill="#000"/><path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z"/>"##
        ),
    ),
    (
        "credit-card",
        lucide!(
            r##"<rect width="20" height="14" x="2" y="5" rx="2"/><line x1="2" x2="22" y1="10" y2="10"/>"##
        ),
    ),
    (
        "corner-down-right",
        lucide!(r##"<polyline points="15 10 20 15 15 20"/><path d="M4 4v7a4 4 0 0 0 4 4h12"/>"##),
    ),
    (
        "boxes",
        lucide!(
            r##"<path d="M2.97 12.92A2 2 0 0 0 2 14.63v3.24a2 2 0 0 0 .97 1.71l3 1.8a2 2 0 0 0 2.06 0L12 19v-5.5l-5-3-4.03 2.42Z"/><path d="m7 16.5-4.74-2.85"/><path d="m7 16.5 5-3"/><path d="M7 16.5v5.17"/><path d="M12 13.5V19l3.97 2.38a2 2 0 0 0 2.06 0l3-1.8a2 2 0 0 0 .97-1.71v-3.24a2 2 0 0 0-.97-1.71L17 10.5l-5 3Z"/><path d="m17 16.5-5-3"/><path d="m17 16.5 4.74-2.85"/><path d="M17 16.5v5.17"/><path d="M7.97 4.42A2 2 0 0 0 7 6.13v4.37l5 3 5-3V6.13a2 2 0 0 0-.97-1.71l-3-1.8a2 2 0 0 0-2.06 0l-3 1.8Z"/><path d="M12 8 7.26 5.15"/><path d="m12 8 4.74-2.85"/><path d="M12 13.5V8"/>"##
        ),
    ),
    (
        "puzzle",
        lucide!(
            r##"<path d="M19.439 7.85c-.049.322.059.648.289.878l1.568 1.568c.47.47.706 1.087.706 1.704s-.235 1.233-.706 1.704l-1.611 1.611a.98.98 0 0 1-.837.276c-.47-.07-.802-.48-.968-.925a2.501 2.501 0 1 0-3.214 3.214c.446.166.855.497.925.968a.979.979 0 0 1-.276.837l-1.61 1.61a2.404 2.404 0 0 1-1.705.707 2.402 2.402 0 0 1-1.704-.706l-1.568-1.568a1.026 1.026 0 0 0-.877-.29c-.493.074-.84.504-1.02.968a2.5 2.5 0 1 1-3.237-3.237c.464-.18.894-.527.967-1.02a1.026 1.026 0 0 0-.289-.877l-1.568-1.568A2.402 2.402 0 0 1 1.998 12c0-.617.236-1.234.706-1.704L4.23 8.77c.24-.24.581-.353.917-.303.515.077.877.528 1.073 1.01a2.5 2.5 0 1 0 3.259-3.259c-.482-.196-.933-.558-1.01-1.073-.05-.336.062-.676.303-.917l1.525-1.525A2.402 2.402 0 0 1 12 1.998c.617 0 1.234.236 1.704.706l1.568 1.568c.23.23.556.338.877.29.493-.074.84-.504 1.02-.968a2.5 2.5 0 1 1 3.237 3.237c-.464.18-.894.527-.967 1.02Z"/>"##
        ),
    ),
    (
        "arrow-left",
        lucide!(r##"<path d="m12 19-7-7 7-7"/><path d="M19 12H5"/>"##),
    ),
    (
        "chevrons-up-down",
        lucide!(r##"<path d="m7 15 5 5 5-5"/><path d="m7 9 5-5 5 5"/>"##),
    ),
    ("minus", lucide!(r##"<path d="M5 12h14"/>"##)),
    (
        "pencil",
        lucide!(
            r##"<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/>"##
        ),
    ),
    (
        "lock",
        lucide!(
            r##"<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"##
        ),
    ),
    (
        "alert-triangle",
        lucide!(
            r##"<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>"##
        ),
    ),
    (
        "info",
        lucide!(r##"<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>"##),
    ),
    (
        "refresh-cw",
        lucide!(
            r##"<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>"##
        ),
    ),
    (
        "download",
        lucide!(
            r##"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/>"##
        ),
    ),
];

/// Static asset source resolving `icons/<name>.svg` to the embedded Lucide set.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let Some(name) = path
            .strip_prefix("icons/")
            .and_then(|p| p.strip_suffix(".svg"))
        else {
            return Ok(None);
        };
        Ok(ICONS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, svg)| Cow::Borrowed(svg.as_bytes())))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        if path == "icons" || path == "icons/" {
            Ok(ICONS
                .iter()
                .map(|(n, _)| format!("icons/{n}.svg").into())
                .collect())
        } else {
            Ok(Vec::new())
        }
    }
}

/// A tinted Lucide icon sized like the legacy UI (typically 11-14px).
pub fn icon(name: &'static str, size: f32, color: Rgba) -> Svg {
    svg()
        .path(format!("icons/{name}.svg"))
        .w(gpui::px(size))
        .h(gpui::px(size))
        .text_color(color)
        .flex_none()
}
