//! JSON Schema type coercion and argument validation.
//!
//! Matches pi's `Value.Convert`, `coerceWithJsonSchema`, and
//! `validateToolArguments` utilities.

// ── Generic argument type coercion ─────────────────────────────

/// Coerce a single JSON value to match a JSON Schema type (modifies in place).
pub fn coerce_primitive_by_type(schema_type: &str, value: &mut serde_json::Value) {
    match schema_type {
        "string" => {
            if value.is_number() || value.is_boolean() {
                *value = serde_json::Value::String(match value {
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => unreachable!(),
                });
            } else if value.is_null() {
                *value = serde_json::Value::String(String::new());
            } else if value.is_array() || value.is_object() {
                *value =
                    serde_json::Value::String(serde_json::to_string(value).unwrap_or_default());
            }
        }
        "number" => {
            if let Some(s) = value.as_str() {
                if let Ok(n) = s.parse::<f64>() {
                    *value = serde_json::json!(n);
                }
            } else if value.is_boolean() {
                *value = serde_json::json!(if value.as_bool().unwrap() { 1.0 } else { 0.0 });
            } else if value.is_null() {
                *value = serde_json::json!(0.0);
            }
        }
        "integer" => {
            if let Some(s) = value.as_str() {
                if let Ok(n) = s.parse::<f64>() {
                    *value = serde_json::json!(n as i64);
                }
            } else if value.is_boolean() {
                *value = serde_json::json!(if value.as_bool().unwrap() { 1i64 } else { 0i64 });
            } else if value.is_null() {
                *value = serde_json::json!(0i64);
            } else if let Some(n) = value.as_f64() {
                *value = serde_json::json!(n as i64);
            }
        }
        "boolean" => {
            if let Some(s) = value.as_str() {
                match s.trim().to_lowercase().as_str() {
                    "true" | "1" | "yes" | "on" => *value = serde_json::Value::Bool(true),
                    "false" | "0" | "no" | "off" => *value = serde_json::Value::Bool(false),
                    _ => {}
                }
            } else if value.is_number() {
                *value = serde_json::Value::Bool(value.as_f64().unwrap_or(0.0) != 0.0);
            } else if value.is_null() {
                *value = serde_json::Value::Bool(false);
            }
        }
        "null" => {
            if value.as_str().is_some_and(|s| s.is_empty())
                || value.as_f64() == Some(0.0)
                || value.as_bool() == Some(false)
            {
                *value = serde_json::Value::Null;
            }
        }
        "array" => {
            if !value.is_array() && !value.is_null() {
                let v = std::mem::take(value);
                *value = serde_json::Value::Array(vec![v]);
            } else if value.is_null() {
                *value = serde_json::Value::Array(vec![]);
            }
        }
        _ => {}
    }
}

/// Recursively coerce tool arguments to match a JSON Schema (modifies in place).
pub fn coerce_with_json_schema(schema: &serde_json::Value, args: &mut serde_json::Value) {
    // Handle composed schemas
    if let Some(all_of) = schema.get("allOf").and_then(|v| v.as_array()) {
        for sub in all_of {
            coerce_with_json_schema(sub, args);
        }
    }

    if let Some(any_of) = schema.get("anyOf").and_then(|v| v.as_array())
        && !any_of.is_empty()
    {
        let original = args.clone();
        for sub in any_of {
            let mut candidate = original.clone();
            coerce_with_json_schema(sub, &mut candidate);
            if candidate != original {
                *args = candidate;
                break;
            }
        }
    }

    if let Some(one_of) = schema.get("oneOf").and_then(|v| v.as_array())
        && !one_of.is_empty()
    {
        let original = args.clone();
        for sub in one_of {
            let mut candidate = original.clone();
            coerce_with_json_schema(sub, &mut candidate);
            if candidate != original {
                *args = candidate;
                break;
            }
        }
    }

    if !args.is_object() {
        return;
    }
    let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
        return;
    };
    for (key, prop_schema) in properties {
        if args.get(key).is_none() {
            continue;
        }
        let arg_value = args.get_mut(key).unwrap();

        let schema_types = collect_schema_types(prop_schema);
        if !schema_types.is_empty() {
            let already_matches = schema_types.iter().any(|t| matches_json_type(arg_value, t));
            if !already_matches {
                for st in &schema_types {
                    let before = arg_value.clone();
                    coerce_primitive_by_type(st, arg_value);
                    if *arg_value != before {
                        break;
                    }
                }
            }

            if schema_types.iter().any(|t| t == "object") && arg_value.is_object() {
                coerce_with_json_schema(prop_schema, arg_value);
            }
            if schema_types.iter().any(|t| t == "array")
                && let Some(items_schema) = prop_schema.get("items")
                && let Some(arr) = arg_value.as_array_mut()
            {
                for item in arr.iter_mut() {
                    coerce_with_json_schema(items_schema, item);
                }
            }
        }
    }
}

/// Collect all type names from a schema property.
fn collect_schema_types(schema: &serde_json::Value) -> Vec<String> {
    let type_val = match schema.get("type") {
        Some(t) => t,
        None => return vec![],
    };
    if let Some(s) = type_val.as_str() {
        return vec![s.to_string()];
    }
    if let Some(arr) = type_val.as_array() {
        return arr
            .iter()
            .filter_map(|t| t.as_str().map(|s| s.to_string()))
            .collect();
    }
    vec![]
}

// ── Schema validation ──────────────────────────────────────────

fn resolve_schema_type(schema: &serde_json::Value) -> Option<&str> {
    let type_val = schema.get("type")?;
    if type_val.is_string() {
        return type_val.as_str();
    }
    if type_val.is_array() {
        return type_val
            .as_array()
            .and_then(|arr| arr.iter().find_map(|t| t.as_str().filter(|&s| s != "null")));
    }
    None
}

fn matches_json_type(value: &serde_json::Value, schema_type: &str) -> bool {
    match schema_type {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    }
}

fn value_matches_schema_types(schema: &serde_json::Value, value: &serde_json::Value) -> bool {
    let type_val = match schema.get("type") {
        Some(t) => t,
        None => return true,
    };
    if type_val.is_string() {
        return matches_json_type(value, type_val.as_str().unwrap());
    }
    if let Some(types) = type_val.as_array() {
        return types
            .iter()
            .filter_map(|t| t.as_str())
            .any(|t| matches_json_type(value, t));
    }
    true
}

/// A single validation error, matching pi's TypeBox error structure.
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

/// Validate tool arguments against its JSON Schema.
///
/// Returns `Ok(())` on success, or `Err` with pi-compatible format:
/// ```text
/// Validation failed for tool "edit":
///   - path: Required
///   - edits[0].oldText: Required
///
/// Received arguments:
/// { "path": "/foo.txt" }
/// ```
pub fn validate_tool_arguments(
    tool_name: &str,
    schema: &serde_json::Value,
    args: &serde_json::Value,
) -> Result<(), String> {
    let mut errors: Vec<ValidationError> = Vec::new();
    collect_validation_errors(schema, args, "root", &mut errors);

    if errors.is_empty() {
        return Ok(());
    }

    let error_lines: Vec<String> = errors
        .iter()
        .map(|e| format!("  - {}: {}", e.path, e.message))
        .collect();

    let pretty_args =
        serde_json::to_string_pretty(args).unwrap_or_else(|_| "<unprintable>".to_string());

    Err(format!(
        "Validation failed for tool \"{tool_name}\":\n{}\n\nReceived arguments:\n{pretty_args}",
        error_lines.join("\n"),
    ))
}

fn collect_validation_errors(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
    errors: &mut Vec<ValidationError>,
) {
    if (path.is_empty() || path == "root")
        && let Some(schema_type) = resolve_schema_type(schema)
        && schema_type == "object"
        && !value.is_object()
    {
        errors.push(ValidationError {
            path: path.to_string(),
            message: "Expected object".to_string(),
        });
        return;
    }

    if !value.is_object()
        && let Some(schema_type) = resolve_schema_type(schema)
        && !matches_json_type(value, schema_type)
    {
        let expected = if schema_type == "integer" {
            "integer"
        } else {
            schema_type
        };
        errors.push(ValidationError {
            path: path.to_string(),
            message: format!("Expected {}", expected),
        });
        return;
    }

    if !value.is_object() {
        return;
    }

    let obj = value.as_object().unwrap();
    let properties = schema.get("properties").and_then(|p| p.as_object());
    let known_keys: std::collections::HashSet<&str> = properties
        .map(|p| p.keys().map(|k| k.as_str()).collect())
        .unwrap_or_default();

    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for required_val in required {
            if let Some(required_key) = required_val.as_str()
                && !obj.contains_key(required_key)
            {
                let err_path = if path.is_empty() || path == "root" {
                    required_key.to_string()
                } else {
                    format!("{}.{}", path, required_key)
                };
                errors.push(ValidationError {
                    path: err_path,
                    message: "Required".to_string(),
                });
            }
        }
    }

    if schema.get("additionalProperties") == Some(&serde_json::Value::Bool(false)) {
        for key in obj.keys() {
            if !known_keys.contains(key.as_str()) {
                let err_path = if path.is_empty() || path == "root" {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };
                errors.push(ValidationError {
                    path: err_path,
                    message: "must NOT have additional properties".to_string(),
                });
            }
        }
    }

    if let Some(props) = properties {
        for (key, prop_schema) in props {
            if let Some(val) = value.get(key) {
                let child_path = if path.is_empty() || path == "root" {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };
                validate_property(prop_schema, val, &child_path, errors);
            }
        }
    }
}

fn validate_property(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
    errors: &mut Vec<ValidationError>,
) {
    if !value_matches_schema_types(schema, value) {
        let schema_type = resolve_schema_type(schema).unwrap_or("unknown");
        let expected = if schema_type == "integer" {
            "integer"
        } else {
            schema_type
        };
        errors.push(ValidationError {
            path: path.to_string(),
            message: format!("Expected {}", expected),
        });
        return;
    }

    if value.is_object() {
        let schema_type = resolve_schema_type(schema);
        if schema_type == Some("object") {
            collect_validation_errors(schema, value, path, errors);
        }
        return;
    }

    if let Some(arr) = value.as_array()
        && resolve_schema_type(schema) == Some("array")
        && let Some(items_schema) = schema.get("items")
    {
        for (i, item) in arr.iter().enumerate() {
            let item_path = format!("{}.{}", path, i);
            validate_property(items_schema, item, &item_path, errors);
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── coerce_primitive_by_type ────────────────────────────────────

    #[test]
    fn test_coerce_string_from_number() {
        let mut v = serde_json::json!(42);
        coerce_primitive_by_type("string", &mut v);
        assert_eq!(v, serde_json::json!("42"));
    }

    #[test]
    fn test_coerce_string_from_boolean() {
        let mut v = serde_json::json!(true);
        coerce_primitive_by_type("string", &mut v);
        assert_eq!(v, serde_json::json!("true"));
    }

    #[test]
    fn test_coerce_string_from_null() {
        let mut v = serde_json::json!(null);
        coerce_primitive_by_type("string", &mut v);
        assert_eq!(v, serde_json::json!(""));
    }

    #[test]
    fn test_coerce_string_unchanged() {
        let mut v = serde_json::json!("hello");
        coerce_primitive_by_type("string", &mut v);
        assert_eq!(v, serde_json::json!("hello"));
    }

    #[test]
    fn test_coerce_number_from_string() {
        let mut v = serde_json::json!("42.5");
        coerce_primitive_by_type("number", &mut v);
        assert_eq!(v, serde_json::json!(42.5));
    }

    #[test]
    fn test_coerce_number_from_boolean() {
        let mut v = serde_json::json!(true);
        coerce_primitive_by_type("number", &mut v);
        assert_eq!(v, serde_json::json!(1.0));
    }

    #[test]
    fn test_coerce_number_from_null() {
        let mut v = serde_json::json!(null);
        coerce_primitive_by_type("number", &mut v);
        assert_eq!(v, serde_json::json!(0.0));
    }

    #[test]
    fn test_coerce_integer_from_string() {
        let mut v = serde_json::json!("7");
        coerce_primitive_by_type("integer", &mut v);
        assert_eq!(v, serde_json::json!(7i64));
    }

    #[test]
    fn test_coerce_integer_from_float() {
        let mut v = serde_json::json!(3.9);
        coerce_primitive_by_type("integer", &mut v);
        assert_eq!(v, serde_json::json!(3i64));
    }

    #[test]
    fn test_coerce_integer_from_boolean() {
        let mut v = serde_json::json!(false);
        coerce_primitive_by_type("integer", &mut v);
        assert_eq!(v, serde_json::json!(0i64));
    }

    #[test]
    fn test_coerce_boolean_from_string_true() {
        let mut v = serde_json::json!("true");
        coerce_primitive_by_type("boolean", &mut v);
        assert_eq!(v, serde_json::json!(true));
    }

    #[test]
    fn test_coerce_boolean_from_string_yes() {
        let mut v = serde_json::json!("yes");
        coerce_primitive_by_type("boolean", &mut v);
        assert_eq!(v, serde_json::json!(true));
    }

    #[test]
    fn test_coerce_boolean_from_number() {
        let mut v = serde_json::json!(1);
        coerce_primitive_by_type("boolean", &mut v);
        assert_eq!(v, serde_json::json!(true));
    }

    #[test]
    fn test_coerce_boolean_from_null() {
        let mut v = serde_json::json!(null);
        coerce_primitive_by_type("boolean", &mut v);
        assert_eq!(v, serde_json::json!(false));
    }

    #[test]
    fn test_coerce_array_from_scalar() {
        let mut v = serde_json::json!("single");
        coerce_primitive_by_type("array", &mut v);
        assert_eq!(v, serde_json::json!(["single"]));
    }

    #[test]
    fn test_coerce_array_from_null() {
        let mut v = serde_json::json!(null);
        coerce_primitive_by_type("array", &mut v);
        assert_eq!(v, serde_json::json!([]));
    }

    #[test]
    fn test_coerce_array_unchanged() {
        let mut v = serde_json::json!([1, 2, 3]);
        coerce_primitive_by_type("array", &mut v);
        assert_eq!(v, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn test_coerce_unknown_type_does_nothing() {
        let mut v = serde_json::json!(42);
        coerce_primitive_by_type("widget", &mut v);
        assert_eq!(v, serde_json::json!(42));
    }

    // ── coerce_with_json_schema ─────────────────────────────────────

    #[test]
    fn test_coerce_schema_string_from_number() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            }
        });
        let mut args = serde_json::json!({"name": 42});
        coerce_with_json_schema(&schema, &mut args);
        assert_eq!(args, serde_json::json!({"name": "42"}));
    }

    #[test]
    fn test_coerce_schema_nested_object() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "metadata": {
                    "type": "object",
                    "properties": {
                        "count": {"type": "integer"}
                    }
                }
            }
        });
        let mut args = serde_json::json!({"metadata": {"count": "5"}});
        coerce_with_json_schema(&schema, &mut args);
        assert_eq!(args, serde_json::json!({"metadata": {"count": 5i64}}));
    }

    #[test]
    fn test_coerce_schema_array_items() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "integer"}
                        }
                    }
                }
            }
        });
        let mut args = serde_json::json!({"items": [{"id": "3"}, {"id": "7"}]});
        coerce_with_json_schema(&schema, &mut args);
        assert_eq!(
            args,
            serde_json::json!({"items": [{"id": 3i64}, {"id": 7i64}]})
        );
    }

    #[test]
    fn test_coerce_schema_non_object_skipped() {
        let schema = serde_json::json!({"type": "string"});
        let mut args = serde_json::json!("hello");
        coerce_with_json_schema(&schema, &mut args);
        assert_eq!(args, serde_json::json!("hello"));
    }

    // ── validate_tool_arguments ─────────────────────────────────────

    #[test]
    fn test_validate_valid_args() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            },
            "required": ["path"]
        });
        let args = serde_json::json!({"path": "/tmp/foo.txt"});
        assert!(validate_tool_arguments("test", &schema, &args).is_ok());
    }

    #[test]
    fn test_validate_missing_required() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            },
            "required": ["path"]
        });
        let args = serde_json::json!({});
        let err = validate_tool_arguments("test", &schema, &args).unwrap_err();
        assert!(err.contains("Required"));
        assert!(err.contains("test"));
    }

    #[test]
    fn test_validate_wrong_type() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "count": {"type": "integer"}
            }
        });
        let args = serde_json::json!({"count": "not-a-number"});
        let err = validate_tool_arguments("test", &schema, &args).unwrap_err();
        assert!(err.contains("Expected integer"));
    }

    #[test]
    fn test_validate_additional_properties() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "additionalProperties": false
        });
        let args = serde_json::json!({"name": "alice", "extra": "bad"});
        let err = validate_tool_arguments("test", &schema, &args).unwrap_err();
        assert!(err.contains("must NOT have additional properties"));
    }

    #[test]
    fn test_validate_not_an_object() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {}
        });
        let args = serde_json::json!("a string, not an object");
        let err = validate_tool_arguments("test", &schema, &args).unwrap_err();
        assert!(err.contains("Expected object"));
    }

    #[test]
    fn test_validate_array_item_types() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "tags": {
                    "type": "array",
                    "items": {"type": "string"}
                }
            }
        });
        let args = serde_json::json!({"tags": [1, 2, 3]});
        let err = validate_tool_arguments("test", &schema, &args).unwrap_err();
        assert!(err.contains("Expected string"));
    }
}
