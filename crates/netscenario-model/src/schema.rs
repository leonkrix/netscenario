//! The JSON Schema of the scenario format.

use schemars::schema_for;
use serde_json::{Value, json};

use crate::scenario::Scenario;

/// A string with an expression in it: `"{rtt / 2}"`, `"link-{n}"` (D-093).
const EXPRESSION: &str = r"\{[^{}]+\}";

/// The JSON Schema of a scenario file, generated from the types, as pretty-printed JSON with a final newline.
///
/// It describes the file as it is written: where the scenario has a value, a string with an expression in it is also accepted
/// (`"{rtt / 2}"`), and where it has a mapping or a list, a place of use (`use:` or `include:`, with `with:`) is, so that an editor
/// does not mark a file that uses parameters, templates and includes (D-093). The scenario that results is checked by the
/// reader, not by the schema.
///
/// The committed copy is `docs/schema/scenario-v0.json`; a test fails when it differs from this.
#[must_use]
pub fn json_schema() -> String {
    let mut schema = typed_schema();
    extend(&mut schema);
    // `{:#}` is the pretty-printed form of a JSON value
    format!("{schema:#}\n")
}

/// The schema of the scenario as the types write it, without the expressions and places of use of the file.
pub(crate) fn typed_schema() -> Value {
    schema_for!(Scenario).as_value().clone()
}

/// Whether the schema is for a value that is written as a scalar.
fn is_scalar(schema: &serde_json::Map<String, Value>) -> bool {
    let scalar = |name: &str| matches!(name, "string" | "integer" | "number" | "boolean");
    schema.contains_key("enum")
        || schema.contains_key("const")
        || match schema.get("type") {
            Some(Value::String(name)) => scalar(name),
            Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).any(scalar),
            _ => false,
        }
}

/// Whether the schema is for a mapping or a list (as the types of the scenario write it).
fn is_collection(schema: &serde_json::Map<String, Value>) -> bool {
    let collection = |name: &str| matches!(name, "object" | "array");
    match schema.get("type") {
        Some(Value::String(name)) => collection(name),
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).any(collection),
        _ => false,
    }
}

/// The keys that describe a schema and are kept on the outside when it is wrapped.
const ANNOTATIONS: [&str; 4] = ["description", "title", "examples", "default"];

fn wrapped(schema: Value, alternative: &Value) -> Value {
    let Value::Object(mut inner) = schema else {
        return schema;
    };
    let mut outer = serde_json::Map::new();
    for key in ANNOTATIONS {
        if let Some(value) = inner.remove(key) {
            outer.insert(key.to_owned(), value);
        }
    }
    outer.insert(
        "anyOf".to_owned(),
        json!([Value::Object(inner), alternative]),
    );
    Value::Object(outer)
}

/// Lets a schema accept expressions and places of use, from the leaves up. Only the places where a schema holds schemas are
/// visited; the examples and defaults are values and stay as they are.
fn loosen(node: &mut Value, use_place: &Value, expression: &Value) {
    let Value::Object(map) = node else {
        return;
    };
    for key in ["properties", "$defs"] {
        if let Some(Value::Object(schemas)) = map.get_mut(key) {
            for schema in schemas.values_mut() {
                loosen(schema, use_place, expression);
            }
        }
    }
    for key in ["additionalProperties", "items"] {
        if let Some(schema) = map.get_mut(key) {
            loosen(schema, use_place, expression);
        }
    }
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(Value::Array(schemas)) = map.get_mut(key) {
            for schema in schemas {
                loosen(schema, use_place, expression);
            }
        }
    }
    if is_scalar(map) {
        *node = wrapped(node.take(), expression);
    } else if is_collection(map) {
        *node = wrapped(node.take(), use_place);
    }
}

/// The schema of the file from the schema of the scenario.
fn extend(schema: &mut Value) {
    let use_place = json!({ "$ref": "#/$defs/Use" });
    let expression = json!({ "type": "string", "pattern": EXPRESSION });
    // The keys of the schema that are not schemas themselves must not be taken for leaves
    let Some(root) = schema.as_object_mut() else {
        return;
    };
    let mut defs = root.remove("$defs").unwrap_or_else(|| json!({}));
    let mut properties = root.remove("properties").unwrap_or_else(|| json!({}));
    // The two maps hold schemas; they are not schemas themselves
    for map in [&mut defs, &mut properties] {
        if let Value::Object(schemas) = map {
            for schema in schemas.values_mut() {
                loosen(schema, &use_place, &expression);
            }
        }
    }
    // `params` and `templates` are read by the pass that expands the file, so the file form is not the form of the record
    let values = json!({
        "type": "object",
        "additionalProperties": { "type": ["string", "number", "boolean", "null"] }
    });
    if let Some(map) = properties.as_object_mut() {
        let description = "The parameters of the scenario: a name and its default, or no value for a parameter that must be given (`--param name=value`). `{name}` in a value is replaced (D-093).";
        map.insert(
            "params".to_owned(),
            json!({ "description": description, "type": "object", "additionalProperties": values["additionalProperties"] }),
        );
        map.insert(
            "templates".to_owned(),
            json!({
                "description": "Named templates: a `content` that a place of use (`use: name`) is replaced by, with the `params` it takes (D-093).",
                "type": "object",
                "additionalProperties": { "$ref": "#/$defs/Template" }
            }),
        );
    }
    if let Some(map) = defs.as_object_mut() {
        map.insert(
            "Use".to_owned(),
            json!({
                "description": "A place of use (D-093): `use` names a template of the file, `include` a file; `with` gives the values of the parameters. It is replaced by the content of the template.",
                "type": "object",
                "properties": {
                    "use": { "type": "string" },
                    "include": { "type": "string" },
                    "with": {
                        "type": "object",
                        "additionalProperties": { "type": ["string", "number", "boolean"] }
                    }
                },
                "oneOf": [{ "required": ["use"] }, { "required": ["include"] }],
                "additionalProperties": false
            }),
        );
        map.insert(
            "Template".to_owned(),
            json!({
                "description": "A template (D-093): the parameters it takes and the content that a place of use is replaced by.",
                "type": "object",
                "properties": {
                    "params": {
                        "type": "object",
                        "additionalProperties": { "type": ["string", "number", "boolean", "null"] }
                    },
                    "content": true
                },
                "required": ["content"],
                "additionalProperties": false
            }),
        );
    }
    root.insert("$defs".to_owned(), defs);
    root.insert("properties".to_owned(), properties);
}
