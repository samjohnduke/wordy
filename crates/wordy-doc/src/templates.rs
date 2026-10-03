//! Entity templates: which sheet fields an entity of a given template shows.
//! Fields are stored in the node's `fields` map keyed by `FieldSpec::key`;
//! unknown keys are kept, so switching templates never loses data.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub multiline: bool,
}

const fn f(key: &'static str, label: &'static str, multiline: bool) -> FieldSpec {
    FieldSpec { key, label, multiline }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Template {
    pub id: &'static str,
    pub label: &'static str,
    pub fields: &'static [FieldSpec],
}

pub const CHARACTER: Template = Template {
    id: "character",
    label: "Character",
    fields: &[
        f("role", "Role", false),
        f("pronunciation", "Pronunciation", false),
        f("age", "Age", false),
        f("physical", "Physical", true),
        f("personality", "Personality", true),
        f("motivation", "Motivation", true),
        f("backstory", "Backstory", true),
    ],
};

pub const LOCATION: Template = Template {
    id: "location",
    label: "Location",
    fields: &[
        f("type", "Type", false),
        f("region", "Region", false),
        f("demographics", "Demographics", true),
        f("description", "Description", true),
        f("history", "History", true),
    ],
};

pub const CULTURE: Template = Template {
    id: "culture",
    label: "Culture",
    fields: &[
        f("values", "Values", true),
        f("customs", "Customs", true),
        f("language", "Language", true),
        f("history", "History", true),
    ],
};

pub const SYSTEM: Template = Template {
    id: "system",
    label: "System",
    fields: &[
        f("rules", "Rules", true),
        f("costs", "Costs & limits", true),
        f("origins", "Origins", true),
    ],
};

pub const OBJECT: Template = Template {
    id: "object",
    label: "Object",
    fields: &[
        f("appearance", "Appearance", true),
        f("significance", "Significance", true),
        f("owner", "Owner", false),
    ],
};

pub const CUSTOM: Template = Template { id: "custom", label: "Custom", fields: &[f("notes", "Notes", true)] };

pub const ALL: [Template; 6] = [CHARACTER, LOCATION, CULTURE, SYSTEM, OBJECT, CUSTOM];

pub fn by_id(id: &str) -> Template {
    ALL.iter().copied().find(|t| t.id == id).unwrap_or(CUSTOM)
}

pub const DEFAULT: Template = CHARACTER;
