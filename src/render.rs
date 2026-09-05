use crate::{error::LibraryError, format::OutputFormat};
use serde_json::Value;

pub(crate) fn render(
    name: &str,
    body: &str,
    format: OutputFormat,
    detailed: bool,
) -> Result<String, LibraryError> {
    if format == OutputFormat::Json {
        if name != "list" && name != "patch" {
            return Ok(body.to_string());
        }
        let mut value: Value = serde_json::from_str(body).map_err(|_| LibraryError::Io {
            message: "render failed".into(),
        })?;
        if name == "list" {
            if let Some(records) = value.get_mut("records").and_then(Value::as_array_mut) {
                for record in records {
                    if let Some(object) = record.as_object_mut() {
                        object.remove("entity");
                    }
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
                    let st = o
                        .remove("stale_tasks")
                        .and_then(|x| x.as_array().map(|a| a.len()))
                        .unwrap_or(0);
                    let sr = o
                        .remove("stale_rules")
                        .and_then(|x| x.as_array().map(|a| a.len()))
                        .unwrap_or(0);
                    o.remove("window_days");
                    if st > 0 {
                        o.insert("stale_tasks_count".into(), st.into());
                    }
                    if sr > 0 {
                        o.insert("stale_rules_count".into(), sr.into());
                    }
                }
            }
            "list" => {
                if let Some(a) = v.get_mut("records").and_then(Value::as_array_mut) {
                    for r in a {
                        if let Some(o) = r.as_object_mut() {
                            let t = o.remove("entity").unwrap_or(Value::String("rule".into()));
                            o.insert("type".into(), t);
                            o.remove("created_at");
                        }
                    }
                }
            }
            _ => {}
        }
    } else if name == "list" {
        if let Some(a) = v.get_mut("records").and_then(Value::as_array_mut) {
            for r in a {
                if let Some(o) = r.as_object_mut() {
                    let t = o.remove("entity").unwrap_or(Value::String("rule".into()));
                    o.insert("type".into(), t);
                }
            }
        }
    }
    serde_yaml::to_string(&v).map_err(|_| LibraryError::Io {
        message: "render failed".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let get_body = r#"{"id":7,"text":"value","created_at":42}"#;
        for detailed in [false, true] {
            let yaml = render("get", get_body, OutputFormat::Yaml, detailed).unwrap();
            assert!(yaml.ends_with('\n'));
            assert_eq!(keys(&yaml), vec!["created_at", "id", "text"]);
            let value: Value = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(value["id"], 7);
            assert_eq!(value["text"], "value");
            assert_eq!(value["created_at"], 42);
        }
        let json = render("get", get_body, OutputFormat::Json, false).unwrap();
        assert_eq!(json, get_body);

        let list_body = r#"{"total":2,"limit":50,"offset":0,"records":[{"id":1,"text":"rule","created_at":10,"entity":"rule"},{"id":2,"text":"task","created_at":11,"entity":"task"}]}"#;
        for detailed in [false, true] {
            let yaml = render("list", list_body, OutputFormat::Yaml, detailed).unwrap();
            assert!(yaml.ends_with('\n'));
            let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(value["total"], 2);
            assert_eq!(value["records"][0]["type"], "rule");
            assert_eq!(value["records"][1]["type"], "task");
            let record_keys = |record: &serde_yaml::Value| {
                let mut keys: Vec<String> = record
                    .as_mapping()
                    .unwrap()
                    .keys()
                    .map(|k| k.as_str().unwrap().to_owned())
                    .collect();
                keys.sort();
                keys
            };
            assert_eq!(
                record_keys(&value["records"][0]),
                if detailed {
                    vec!["created_at", "id", "text", "type"]
                        .into_iter()
                        .map(String::from)
                        .collect::<Vec<String>>()
                } else {
                    vec!["id", "text", "type"]
                        .into_iter()
                        .map(String::from)
                        .collect::<Vec<String>>()
                }
            );
        }
        assert!(
            !render("list", list_body, OutputFormat::Json, false)
                .unwrap()
                .contains("entity")
        );

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
        }
    }

    #[test]
    fn status_projection_omits_empty_counts_and_lists() {
        let s = render(
            "status",
            r#"{"all_up_to_date":true,"window_days":14,"stale_tasks":[],"stale_rules":[]}"#,
            OutputFormat::Yaml,
            false,
        )
        .unwrap();
        assert_eq!(keys(&s), vec!["all_up_to_date"]);
    }
}
