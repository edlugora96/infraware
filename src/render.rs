use crate::{error::LibraryError, format::OutputFormat};
use chrono::{DateTime, FixedOffset};
use serde_json::Value;

fn humanize_field(value: &mut Value, key: &str, reference: DateTime<FixedOffset>) {
    if let Some(number) = value.get(key).and_then(Value::as_i64) {
        if let Some(object) = value.as_object_mut() {
            object.insert(
                key.into(),
                Value::String(crate::chrono_format::humanize(number, reference)),
            );
        }
    }
}

fn humanize_detailed(value: &mut Value, name: &str, reference: DateTime<FixedOffset>) {
    if matches!(name, "ingest" | "get") {
        humanize_field(value, "created_at", reference);
        for key in ["tasks", "rules"] {
            if let Some(items) = value.get_mut(key).and_then(Value::as_array_mut) {
                for item in items {
                    humanize_field(item, "created_at", reference);
                }
            }
        }
    }
    if name == "status" {
        for key in ["stale_tasks", "stale_rules"] {
            if let Some(items) = value.get_mut(key).and_then(Value::as_array_mut) {
                for item in items {
                    humanize_field(item, "last_revised_at", reference);
                }
            }
        }
    }
    if name == "list" {
        if let Some(rules) = value.get_mut("rules").and_then(Value::as_array_mut) {
            for rule in rules {
                humanize_field(rule, "created_at", reference);
                if let Some(tasks) = rule.get_mut("tasks").and_then(Value::as_array_mut) {
                    for task in tasks {
                        humanize_field(task, "created_at", reference);
                    }
                }
            }
        }
    }
}

fn project_list(value: &mut Value, detailed: bool) {
    if let Some(rules) = value.get_mut("rules").and_then(Value::as_array_mut) {
        for rule in rules {
            if let Some(object) = rule.as_object_mut() {
                if detailed {
                    if object.get("updated_at") == Some(&Value::Null) {
                        object.remove("updated_at");
                    }
                } else {
                    object.remove("created_at");
                    object.remove("updated_at");
                }
                if let Some(tasks) = object.get_mut("tasks").and_then(Value::as_array_mut) {
                    for task in tasks {
                        if let Some(task_object) = task.as_object_mut() {
                            if detailed {
                                if task_object.get("updated_at") == Some(&Value::Null) {
                                    task_object.remove("updated_at");
                                }
                            } else {
                                task_object.remove("created_at");
                                task_object.remove("updated_at");
                            }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn render(
    name: &str,
    body: &str,
    format: OutputFormat,
    detailed: bool,
) -> Result<String, LibraryError> {
    render_at(
        name,
        body,
        format,
        detailed,
        crate::chrono_format::production_reference(),
    )
}

fn render_at(
    name: &str,
    body: &str,
    format: OutputFormat,
    detailed: bool,
    reference: chrono::DateTime<chrono::FixedOffset>,
) -> Result<String, LibraryError> {
    if format == OutputFormat::Json {
        if name != "list" && name != "patch" && name != "get" {
            return Ok(body.to_string());
        }
        let mut value: Value = serde_json::from_str(body).map_err(|_| LibraryError::Io {
            message: "render failed".into(),
        })?;
        if name == "list" {
            project_list(&mut value, detailed);
        } else if name == "get" {
            if !detailed {
                if let Some(object) = value.as_object_mut() {
                    object.remove("tasks");
                    object.remove("rules");
                }
            }
        } else if let Some(object) = value.as_object_mut() {
            object.remove("duplicate");
        }
        return serde_json::to_string(&value).map_err(|_| LibraryError::Io {
            message: "render failed".into(),
        });
    }
    let mut v: Value = serde_json::from_str(body).map_err(|_| LibraryError::Io {
        message: "render failed".into(),
    })?;
    if !detailed {
        match name {
            "ingest" => {
                if let Some(object) = v.as_object_mut() {
                    object.remove("created_at");
                }
            }
            "query" => {
                if let Some(o) = v.as_object_mut() {
                    let rules = o
                        .remove("hits")
                        .and_then(|h| {
                            h.as_array().map(|a| {
                                a.iter()
                                    .map(|x| {
                                        let mut r = serde_json::Map::new();
                                        r.insert(
                                            "text".into(),
                                            x.get("text").cloned().unwrap_or(Value::Null),
                                        );
                                        Value::Object(r)
                                    })
                                    .collect::<Vec<_>>()
                            })
                        })
                        .unwrap_or_default();
                    o.remove("task");
                    o.remove("max_distance");
                    o.insert("rules".into(), Value::Array(rules));
                }
            }
            "status" => {
                if let Some(o) = v.as_object_mut() {
                    o.remove("stale_tasks");
                    let sr = o
                        .remove("stale_rules")
                        .and_then(|x| x.as_array().map(|a| a.len()))
                        .unwrap_or(0);
                    if sr > 0 {
                        o.insert("stale_rules_count".into(), sr.into());
                    }
                }
            }
            "list" => project_list(&mut v, false),
            "get" => {
                if let Some(o) = v.as_object_mut() {
                    o.remove("tasks");
                    o.remove("rules");
                }
            }
            _ => {}
        }
    } else {
        if name == "list" {
            project_list(&mut v, detailed);
        }
        if detailed {
            humanize_detailed(&mut v, name, reference);
        }
    }
    serde_yaml::to_string(&v).map_err(|_| LibraryError::Io {
        message: "render failed".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    fn keys(text: &str) -> Vec<String> {
        let mut keys: Vec<String> = serde_yaml::from_str::<serde_yaml::Value>(text)
            .unwrap()
            .as_mapping()
            .unwrap()
            .keys()
            .map(|k| k.as_str().unwrap().into())
            .collect();
        keys.sort();
        keys
    }
    #[test]
    fn projections_have_required_shapes() {
        assert_eq!(
            keys(
                &render(
                    "ingest",
                    r#"{"task_id":1,"rule_id":2,"created_at":3,"duplicate":false}"#,
                    OutputFormat::Yaml,
                    false
                )
                .unwrap()
            ),
            vec!["duplicate", "rule_id", "task_id"]
        );
        assert_eq!(
            keys(
                &render(
                    "ingest",
                    r#"{"task_id":1,"rule_id":2,"created_at":3,"duplicate":false}"#,
                    OutputFormat::Yaml,
                    true
                )
                .unwrap()
            ),
            vec!["created_at", "duplicate", "rule_id", "task_id"]
        );
        assert_eq!(keys(&render("query", r#"{"task":"t","max_distance":0.15,"hits":[{"id":2,"score":1,"distance":0,"text":"r"}]}"#, OutputFormat::Yaml, false).unwrap()), vec!["rules"]);
        assert_eq!(
            keys(
                &render(
                    "query",
                    r#"{"task":"t","max_distance":0.15,"hits":[]}"#,
                    OutputFormat::Yaml,
                    true
                )
                .unwrap()
            ),
            vec!["hits", "max_distance", "task"]
        );
        assert_eq!(
            keys(
                &render(
                    "patch",
                    r#"{"id":1,"patched":true,"duplicate":true}"#,
                    OutputFormat::Yaml,
                    false
                )
                .unwrap()
            ),
            vec!["duplicate", "id", "patched"]
        );
        assert!(
            !render(
                "patch",
                r#"{"id":1,"patched":true,"duplicate":true}"#,
                OutputFormat::Json,
                false
            )
            .unwrap()
            .contains("duplicate")
        );
    }
    #[test]
    fn get_list_remove_revise_projections_preserve_contracts() {
        let get_body = r#"{"id":7,"text":"value","created_at":42,"related_count":0,"tasks":[]}"#;
        for detailed in [false, true] {
            let yaml = render("get", get_body, OutputFormat::Yaml, detailed).unwrap();
            assert!(yaml.ends_with('\n'));
            assert_eq!(
                keys(&yaml),
                if detailed {
                    vec!["created_at", "id", "related_count", "tasks", "text"]
                } else {
                    vec!["created_at", "id", "related_count", "text"]
                }
            );
            let value: Value = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(value["id"], 7);
            assert_eq!(value["text"], "value");
            if detailed {
                assert!(value["created_at"].as_str().is_some());
            } else {
                assert_eq!(value["created_at"], 42);
            }
        }
        let json: Value =
            serde_json::from_str(&render("get", get_body, OutputFormat::Json, false).unwrap())
                .unwrap();
        assert_eq!(json["related_count"], 0);
        assert!(json.get("tasks").is_none());

        let list_body = r#"{"total":2,"limit":50,"offset":0,"rules":[{"id":1,"text":"rule","created_at":10,"updated_at":null,"tasks":[]},{"id":2,"text":"second","created_at":11,"updated_at":12,"tasks":[{"id":7,"text":"task","created_at":9,"updated_at":null}]}]}"#;
        let yaml_keys = |value: &serde_yaml::Value| {
            let mut keys: Vec<String> = value
                .as_mapping()
                .unwrap()
                .keys()
                .map(|k| k.as_str().unwrap().to_owned())
                .collect();
            keys.sort();
            keys
        };
        for detailed in [false, true] {
            let yaml = render("list", list_body, OutputFormat::Yaml, detailed).unwrap();
            assert!(yaml.ends_with('\n'));
            let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(value["total"], 2);
            assert_eq!(
                value["rules"][0]["tasks"],
                serde_yaml::Value::Sequence(vec![])
            );
            assert_eq!(
                yaml_keys(&value["rules"][0]),
                if detailed {
                    vec!["created_at", "id", "tasks", "text"]
                } else {
                    vec!["id", "tasks", "text"]
                }
            );
            assert_eq!(
                yaml_keys(&value["rules"][1]),
                if detailed {
                    vec!["created_at", "id", "tasks", "text", "updated_at"]
                } else {
                    vec!["id", "tasks", "text"]
                }
            );
            assert_eq!(
                yaml_keys(&value["rules"][1]["tasks"][0]),
                if detailed {
                    vec!["created_at", "id", "text"]
                } else {
                    vec!["id", "text"]
                }
            );
            if detailed {
                assert_eq!(value["rules"][1]["updated_at"], 12);
            } else {
                assert!(value["rules"][1].get("updated_at").is_none());
            }
            assert!(value["rules"][0].get("updated_at").is_none());
            assert!(value["rules"][1]["tasks"][0].get("updated_at").is_none());
        }
        for detailed in [false, true] {
            let json: Value = serde_json::from_str(
                &render("list", list_body, OutputFormat::Json, detailed).unwrap(),
            )
            .unwrap();
            assert_eq!(json["rules"][0]["tasks"], serde_json::json!([]));
            assert!(json["rules"][0].get("updated_at").is_none());
            assert!(json["rules"][1]["tasks"][0].get("updated_at").is_none());
            if !detailed {
                assert!(json["rules"][0].get("created_at").is_none());
                assert!(json["rules"][1]["tasks"][0].get("created_at").is_none());
            }
            let encoded = json.to_string();
            assert!(!encoded.contains("records"));
            assert!(!encoded.contains("entity"));
            assert!(!encoded.contains("type"));
        }

        for (name, body, expected) in [
            (
                "remove",
                r#"{"id":3,"removed":true}"#,
                vec!["id", "removed"],
            ),
            (
                "revise",
                r#"{"id":4,"revised":true}"#,
                vec!["id", "revised"],
            ),
        ] {
            for detailed in [false, true] {
                let yaml = render(name, body, OutputFormat::Yaml, detailed).unwrap();
                assert!(yaml.ends_with('\n'));
                assert_eq!(keys(&yaml), expected);
                let value: Value = serde_yaml::from_str(&yaml).unwrap();
                assert_eq!(value["id"], if name == "remove" { 3 } else { 4 });
            }
            assert_eq!(render(name, body, OutputFormat::Json, false).unwrap(), body);
            assert_eq!(render(name, body, OutputFormat::Json, true).unwrap(), body);
        }
    }

    #[test]
    fn detailed_timestamp_projection_is_humanized_without_changing_json() {
        let zone = chrono::FixedOffset::east_opt(0).unwrap();
        let reference = zone.with_ymd_and_hms(2024, 6, 12, 12, 0, 0).unwrap();
        let body = r#"{"task_id":1,"rule_id":2,"created_at":1718193600000,"duplicate":false}"#;
        let yaml = render_at("ingest", body, OutputFormat::Yaml, true, reference).unwrap();
        let value: Value = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(value["created_at"], "today");
        assert_eq!(
            render_at("ingest", body, OutputFormat::Yaml, false, reference).unwrap(),
            "duplicate: false\nrule_id: 2\ntask_id: 1\n"
        );
        assert_eq!(
            render_at("ingest", body, OutputFormat::Json, true, reference).unwrap(),
            body
        );

        let status = r#"{"all_up_to_date":false,"window_days":14,"stale_rules":[{"id":1,"text":"r","last_revised_at":1717934400000}]}"#;
        let status_yaml = render_at("status", status, OutputFormat::Yaml, true, reference).unwrap();
        let status_value: Value = serde_yaml::from_str(&status_yaml).unwrap();
        assert_eq!(
            status_value["stale_rules"][0]["last_revised_at"],
            "last week"
        );
        assert!(status_value.get("stale_tasks").is_none());
    }

    #[test]
    fn status_projection_omits_empty_counts_and_lists() {
        let s = render(
            "status",
            r#"{"all_up_to_date":true,"window_days":14,"stale_rules":[]}"#,
            OutputFormat::Yaml,
            false,
        )
        .unwrap();
        assert_eq!(keys(&s), vec!["all_up_to_date", "window_days"]);
    }
}
