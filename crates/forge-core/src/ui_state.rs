//! Window layout that is remembered between sessions: the sizes of the panels the user dragged.
//! Kept in `ui-state.json` beside `settings.json`, which holds options the user edits; this file
//! holds what the app itself writes. A missing or damaged file just means default sizes.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::settings::config_dir;

/// Panel sizes in pixels (the sidebar's at the default zoom), and the Debug panel's column shares.
#[derive(Clone, Debug, PartialEq)]
pub struct UiState {
    pub sidebar: f32,
    pub panel_height: f32,
    pub preview_width: Option<f32>,
    pub debug_columns: [f32; 3],
}

impl Default for UiState {
    fn default() -> Self {
        Self { sidebar: 300.0, panel_height: 250.0, preview_width: None, debug_columns: [0.36, 0.30, 0.34] }
    }
}

pub fn path() -> PathBuf {
    config_dir().join("ui-state.json")
}

/// A finite number inside `lo..=hi`, else `None`.
fn number(value: &Value, lo: f32, hi: f32) -> Option<f32> {
    value.as_f64().map(|v| v as f32).filter(|v| v.is_finite() && (lo..=hi).contains(v))
}

impl UiState {
    /// Read saved sizes from JSON text. Anything missing, out of range or nonsensical keeps its default.
    pub fn parse(text: &str) -> Self {
        let mut state = Self::default();
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else { return state };
        if let Some(v) = map.get("sidebar").and_then(|v| number(v, 140.0, 900.0)) {
            state.sidebar = v;
        }
        if let Some(v) = map.get("panelHeight").and_then(|v| number(v, 80.0, 2000.0)) {
            state.panel_height = v;
        }
        state.preview_width = map.get("previewWidth").and_then(|v| number(v, 160.0, 4000.0));
        if let Some([a, b, c]) = map.get("debugColumns").and_then(Value::as_array).map(|a| a.iter().map(|v| number(v, 0.05, 0.9)).collect::<Vec<_>>()).as_deref() {
            if let (Some(a), Some(b), Some(c)) = (a, b, c) {
                let total = a + b + c;
                state.debug_columns = [a / total, b / total, c / total];
            }
        }
        state
    }

    pub fn to_json(&self) -> String {
        // Four decimals are plenty for a pixel size or a share, and keep the file readable.
        let tidy = |v: f32| (f64::from(v) * 10_000.0).round() / 10_000.0;
        let mut value = json!({ "sidebar": tidy(self.sidebar), "panelHeight": tidy(self.panel_height), "debugColumns": self.debug_columns.map(tidy) });
        if let Some(width) = self.preview_width {
            value["previewWidth"] = json!(tidy(width));
        }
        serde_json::to_string_pretty(&value).unwrap_or_default() + "\n"
    }

    pub fn load() -> Self {
        std::fs::read_to_string(path()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        std::fs::create_dir_all(config_dir()).and_then(|()| std::fs::write(path(), self.to_json())).map_err(|e| format!("could not save the panel sizes: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_survive_a_round_trip() {
        let state = UiState { sidebar: 412.5, panel_height: 310.0, preview_width: Some(520.0), debug_columns: [0.5, 0.25, 0.25] };
        assert_eq!(UiState::parse(&state.to_json()), state);
        let without_preview = UiState { preview_width: None, ..state };
        assert_eq!(UiState::parse(&without_preview.to_json()).preview_width, None);
        let written = UiState::default().to_json();
        assert!(written.contains("0.36") && !written.contains("0.3600000"), "values are written tidily: {written}");
    }

    #[test]
    fn missing_damaged_or_absurd_values_fall_back_to_defaults() {
        assert_eq!(UiState::parse(""), UiState::default());
        assert_eq!(UiState::parse("not json"), UiState::default());
        assert_eq!(UiState::parse("[1, 2]"), UiState::default());
        let odd = UiState::parse(r#"{ "sidebar": 99999, "panelHeight": -5, "previewWidth": "wide", "debugColumns": [0.5, 0.5] }"#);
        assert_eq!(odd, UiState::default());
        let partly = UiState::parse(r#"{ "sidebar": 350, "panelHeight": "tall" }"#);
        assert_eq!((partly.sidebar, partly.panel_height), (350.0, 250.0));
    }

    #[test]
    fn debug_columns_are_renormalized_to_add_up_to_one() {
        let state = UiState::parse(r#"{ "debugColumns": [0.2, 0.2, 0.4] }"#);
        assert!((state.debug_columns.iter().sum::<f32>() - 1.0).abs() < 1e-5, "{:?}", state.debug_columns);
        assert!((state.debug_columns[2] - 0.5).abs() < 1e-5);
    }
}
