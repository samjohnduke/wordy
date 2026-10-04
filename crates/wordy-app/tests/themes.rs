//! The bundled theme files must parse as gpui-component theme sets with the
//! names `prefs::ThemeFamily` looks up, or Appearance silently falls back to
//! the stock pair. Note the `highlight` block: without gpui-component's
//! `tree-sitter` feature (gpui-kit builds without it) its keys are snake_case
//! (`editor_background`) and `syntax` is required, while with the feature they
//! are dotted (`editor.background`). The files carry both spellings.

use gpui_kit::component::{ThemeMode, ThemeSet};

const FILES: [(&str, &str, &str); 3] = [
    (include_str!("../themes/wordy.json"), "Wordy Light", "Wordy Dark"),
    (
        include_str!("../themes/catppuccin.json"),
        "Catppuccin Latte",
        "Catppuccin Mocha",
    ),
    (
        include_str!("../themes/high-contrast.json"),
        "High Contrast Light",
        "High Contrast Dark",
    ),
];

#[test]
fn bundled_themes_parse_with_expected_names() {
    for (json, light, dark) in FILES {
        let set: ThemeSet = serde_json::from_str(json).expect("theme set parses");
        let find = |name: &str| {
            set.themes
                .iter()
                .find(|t| t.name.as_ref() == name)
                .unwrap_or_else(|| panic!("{name} missing"))
        };
        assert_eq!(find(light).mode, ThemeMode::Light);
        assert_eq!(find(dark).mode, ThemeMode::Dark);
        for t in &set.themes {
            assert!(t.colors.background.is_some(), "{} has no background", t.name);
            let h = t
                .highlight
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no highlight block", t.name));
            assert!(h.editor_background.is_some(), "{} has no editor background", t.name);
        }
    }
}
