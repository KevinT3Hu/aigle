//! Shared action glyphs and accessible button styles across the application.
use gpui::{ElementId, SharedString};
use gpui_component::{Icon, button::Button};

#[derive(Clone, Copy)]
pub enum Glyph {
    Add,
    Close,
    Delete,
    Clear,
    Reset,
    Folder,
    File,
    Edit,
    View,
    Grid,
    Develop,
    Save,
    Import,
    Export,
    Copy,
    Check,
    Select,
    Deselect,
    Swap,
    Picker,
    Auto,
    Camera,
    Lens,
    Disable,
    Invert,
    Brush,
    Erase,
    Heal,
    Clone,
    Keyboard,
    Filter,
    Sort,
    Star,
    Tag,
    Calendar,
    RotateLeft,
    RotateRight,
    Mirror,
    Gradient,
}
impl Glyph {
    pub fn icon(self) -> Icon {
        let path = match self {
            Self::Add => "M12 5v14 M5 12h14",
            Self::Close => "M6 6l12 12 M18 6L6 18",
            Self::Delete => "M3 6h18 M9 6V3h6v3 M5 6l1 15h12l1-15 M10 10v7 M14 10v7",
            Self::Clear => "M4 4l16 16 M4 20l5-5 M15 9l5-5 M8 3l13 13 M3 8l13 13",
            Self::Reset => "M3 10a9 9 0 1 1 2 8 M3 4v6h6",
            Self::Folder => "M3 6h7l2 3h9v11H3Z",
            Self::File => "M5 3h9l5 5v13H5Z M14 3v6h5",
            Self::Edit => "M4 20l1-5L17 3l4 4L9 19Z M14 6l4 4",
            Self::View => {
                "M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12Z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6"
            }
            Self::Grid => "M3 3h7v7H3Z M14 3h7v7h-7Z M3 14h7v7H3Z M14 14h7v7h-7Z",
            Self::Develop => "M3 6h18 M3 12h18 M3 18h18 M8 3v6 M16 9v6 M10 15v6",
            Self::Save => "M4 3h13l4 4v14H3V3Z M7 3v6h9V3 M7 21v-8h10v8",
            Self::Import => "M3 15v6h18v-6 M12 3v12 M7 10l5 5 5-5",
            Self::Export => "M3 15v6h18v-6 M12 15V3 M7 8l5-5 5 5",
            Self::Copy => "M8 8h13v13H8Z M16 8V3H3v13h5",
            Self::Check => "M4 12l5 5L20 5",
            Self::Select => "M4 4h16v16H4Z M7 12l3 3 7-7",
            Self::Deselect => "M4 4h16v16H4Z M8 12h8",
            Self::Swap => "M3 7h17 M16 3l4 4-4 4 M21 17H4 M8 13l-4 4 4 4",
            Self::Picker => "M14 3l7 7 M12 5l7 7 M5 14l8-8 5 5-8 8H5Z M5 19l-2 2",
            Self::Auto => "M4 20L17 7 M14 4l6 6 M5 3v4 M3 5h4 M18 15v6 M15 18h6",
            Self::Camera => "M3 7h4l2-3h6l2 3h4v14H3Z M12 10a4 4 0 1 0 0 8 4 4 0 0 0 0-8",
            Self::Lens => "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18 M12 7a5 5 0 1 0 0 10 5 5 0 0 0 0-10",
            Self::Disable => "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18 M6 6l12 12",
            Self::Invert => {
                "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18 M12 3v18 M12 7h6 M12 11h8 M12 15h7"
            }
            Self::Brush => "M9 14L18 3l3 3-11 9 M9 14c-5-2-2 7-7 7 8 2 12-4 7-7Z",
            Self::Erase => "M3 15L14 4l7 7-10 10H8Z M8 10l7 7 M11 21h10",
            Self::Heal => "M4 14L14 4a4 4 0 0 1 6 6L10 20a4 4 0 0 1-6-6Z M8 10l6 6 M11 8l5 5",
            Self::Clone => "M9 3h6v6l3 5H6l3-5Z M5 18h14v3H5Z",
            Self::Keyboard => {
                "M2 5h20v14H2Z M5 9h1 M9 9h1 M13 9h1 M17 9h1 M5 13h1 M9 13h1 M13 13h5 M6 16h12"
            }
            Self::Filter => "M3 4h18l-7 8v7l-4 2v-9Z",
            Self::Sort => "M4 4v16 M1 17l3 3 3-3 M10 5h11 M10 11h8 M10 17h5",
            Self::Star => "M12 3l3 6 6 1-5 5 1 6-5-3-5 3 1-6-5-5 6-1Z",
            Self::Tag => "M3 3h8l10 10-8 8L3 11Z M7 7h.01",
            Self::RotateLeft => "M3 10a9 9 0 1 1 2 8 M3 4v6h6",
            Self::RotateRight => "M21 10a9 9 0 1 0-2 8 M21 4v6h-6",
            Self::Mirror => "M12 3v18 M3 6l5 6-5 6Z M21 6l-5 6 5 6Z",
            Self::Gradient => "M3 4h18v16H3Z M7 4v16 M12 4v16 M17 4v16",
            Self::Calendar => "M3 5h18v16H3Z M3 10h18 M7 3v4 M17 3v4",
        };
        Icon::default().data(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="{path}"/></svg>"#).as_bytes())
    }
}

/// Icon-only actions always retain a translated accessible name and tooltip.
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: Glyph,
    label: impl Into<SharedString>,
) -> Button {
    let label = label.into();
    Button::new(id)
        .icon(icon.icon())
        .accessibility_label(label.clone())
        .tooltip(label)
}

/// Dialog confirmations and choices retain their visible text.
pub fn labeled_button(
    id: impl Into<ElementId>,
    icon: Glyph,
    label: impl Into<SharedString>,
) -> Button {
    let label = label.into();
    icon_button(id, icon, label.clone()).label(label)
}
