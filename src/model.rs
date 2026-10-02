//! Data model: lines, lists and settings, as stored on disk.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Task,
    Text,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListId {
    #[default]
    Next,
    Later,
}

impl ListId {
    pub fn label(self) -> &'static str {
        match self {
            ListId::Next => "Main",
            ListId::Later => "Long-term",
        }
    }
}

pub const MAX_INDENT: u8 = 3;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Item {
    pub id: u64,
    pub kind: Kind,
    pub text: String,
    pub done: bool,
    pub indent: u8,
    pub list: ListId,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// "#RRGGBB"; `None` means the list's default text color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Set only for archived items: the local date it was archived, "YYYY-MM-DD".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_on: Option<String>,
}

impl Item {
    pub fn is_task(&self) -> bool {
        self.kind == Kind::Task
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemFile {
    pub version: u32,
    pub items: Vec<Item>,
}

impl Default for ItemFile {
    fn default() -> Self {
        Self {
            version: 1,
            items: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Window rectangle in physical pixels. `None` until the window is first placed.
    pub rect: Option<[i32; 4]>,
    /// Expanded height in physical pixels, kept while collapsed.
    pub expanded_height: Option<i32>,
    pub collapsed: bool,
    pub on_top: bool,
    pub locked: bool,
    pub header_hidden: bool,
    pub font_family: String,
    pub font_size_pt: f32,
    /// Default text color, "#RRGGBB".
    pub ink: String,
    pub view: ListId,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            rect: None,
            expanded_height: None,
            collapsed: false,
            on_top: false,
            locked: false,
            header_hidden: false,
            font_family: "Segoe UI Variable Text".into(),
            font_size_pt: 10.5,
            ink: "#F3F4F6".into(),
            view: ListId::Next,
        }
    }
}

/// Parses "#RRGGBB" into 0xRRGGBB.
pub fn parse_hex(s: &str) -> Option<u32> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    u32::from_str_radix(h, 16).ok()
}

pub fn to_hex(rgb: u32) -> String {
    format!("#{:06X}", rgb & 0xFF_FFFF)
}
