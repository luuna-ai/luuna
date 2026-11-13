//! Ticket read context uses the full plan and the shared frozen annotation index.
use super::{common::*, record::Plan};
use crate::{command::plan_index, error::Result, model::Ticket, store::Store};
use std::collections::HashMap;
pub(in crate::command) struct ReadPlans {
    plans: Vec<Plan>,
    index: plan_index::PlanIndex,
}
impl ReadPlans {
    pub(in crate::command) fn load(store: &Store) -> Result<Self> {
        Ok(Self {
            plans: load(store)?,
            index: plan_index::load(store)?,
        })
    }
    fn get(&self, ticket: &Ticket) -> Option<&Plan> {
        if ticket.plan_id.is_empty() {
            None
        } else {
            self.plans.iter().find(|p| p.id == ticket.plan_id)
        }
    }
    pub(in crate::command) fn suffix(&self, ticket: &Ticket) -> String {
        self.index.plan_suffix(ticket)
    }
    pub(in crate::command) fn annotated(&self, tickets: &[Ticket]) -> Result<Vec<u8>> {
        let records = tickets
            .iter()
            .map(|t| self.index.annotated_json(t))
            .collect::<Result<Vec<_>>>()?;
        Ok(array_json(records))
    }
    fn summary(p: &Plan) -> Vec<u8> {
        object_json([
            ("id", quoted(&p.id)),
            ("status", quoted(&p.status)),
            ("revision", p.revision.to_string().into_bytes()),
            ("template", quoted(&p.template)),
            (
                "children",
                super::record::string_array(p.children.as_deref()),
            ),
        ])
    }
    fn children(p: &Plan, issues: &HashMap<&str, &Ticket>) -> Vec<u8> {
        array_json(p.children().iter().map(|id| {
            let (title, status) = issues
                .get(id.as_str())
                .map(|t| (t.title.as_str(), t.status.as_str()))
                .unwrap_or(("(missing)", "missing"));
            object_json([
                ("adopted", p.adopted().contains(id).to_string().into_bytes()),
                ("id", quoted(id)),
                ("status", quoted(status)),
                ("title", quoted(title)),
            ])
        }))
    }
    pub(in crate::command) fn single(
        &self,
        ticket: &Ticket,
        issues: &HashMap<&str, &Ticket>,
    ) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        Ok(self
            .get(ticket)
            .map(|p| (Self::summary(p), Self::children(p, issues))))
    }
    pub(in crate::command) fn result(
        &self,
        ticket: &Ticket,
        issues: &HashMap<&str, &Ticket>,
    ) -> Result<Vec<u8>> {
        let mut fields = vec![("issue", ticket.marshal_json()?)];
        if let Some((plan, children)) = self.single(ticket, issues)? {
            fields.push(("plan", plan));
            fields.push(("plan_children", children));
        }
        Ok(object_json(fields))
    }
    pub(in crate::command) fn block(
        &self,
        ticket: &Ticket,
        issues: &HashMap<&str, &Ticket>,
    ) -> String {
        let Some(p) = self.get(ticket) else {
            return String::new();
        };
        let mut out = format!("\n\nPlan: {}  [{}]\n", p.id, p.status);
        if p.status == "draft" {
            out.push_str("plan in draft — run luuna-missions plan submit\n");
        } else if !p.children().is_empty() {
            out.push_str(&format!("Plan steps ({}):\n", p.children().len()));
            for id in p.children() {
                let (title, status) = issues
                    .get(id.as_str())
                    .map(|t| (t.title.as_str(), t.status.as_str()))
                    .unwrap_or(("(missing)", "missing"));
                out.push_str(&format!(
                    "  {id}  [{status}]  {title}{}\n",
                    if p.adopted().contains(id) {
                        " (adopted)"
                    } else {
                        ""
                    }
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::config_tree::tree::test_support::{invoke, project};
    use std::fs;
    #[test]
    fn ticket_summary_keeps_nil_children_while_annotation_uses_empty_array() {
        let t = project("project: demo\nversion: \"1\"\n");
        let d = t.path().join(".luuna/missions");
        fs::write(
            d.join("issues.jsonl"),
            b"{\"id\":\"a\",\"plan_id\":\"pl-a\",\"future\":9007199254740993}\n",
        )
        .unwrap();
        fs::write(
            d.join("plans.jsonl"),
            b"{\"id\":\"pl-a\",\"status\":\"approved\",\"future\":{\"n\":1e400}}\n",
        )
        .unwrap();
        let out = invoke(t.path(), &["show", "a", "--json"]);
        assert_eq!(out.exit_code, 0);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(v["plan"]["children"].is_null());
        assert_eq!(v["plan_children"], serde_json::json!([]));
        assert_eq!(v["issue"]["future"], 9007199254740993u64);
        let out = invoke(t.path(), &["list", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["issues"][0]["plan_children"], serde_json::json!([]));
    }
    #[test]
    fn child_summary_includes_missing_and_adopted_without_expanding_subplans() {
        let t = project("project: demo\nversion: \"1\"\n");
        let d = t.path().join(".luuna/missions");
        fs::write(
            d.join("issues.jsonl"),
            b"{\"id\":\"a\",\"plan_id\":\"pl-a\"}\n",
        )
        .unwrap();
        fs::write(d.join("plans.jsonl"),b"{\"id\":\"pl-a\",\"status\":\"active\",\"children\":[\"missing\"],\"adoptedChildren\":[\"missing\"]}\n").unwrap();
        let out = invoke(t.path(), &["show", "a", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            v["plan_children"],
            serde_json::json!([{"adopted":true,"id":"missing","status":"missing","title":"(missing)"}])
        );
        let out = invoke(t.path(), &["show", "a"]);
        assert!(String::from_utf8(out.stdout)
            .unwrap()
            .contains("missing  [missing]  (missing) (adopted)"));
    }
    #[test]
    fn known_nonfinite_sections_are_skipped_by_all_ticket_read_paths() {
        let t = project("project: demo\nversion: \"1\"\n");
        let d = t.path().join(".luuna/missions");
        fs::write(
            d.join("issues.jsonl"),
            b"{\"id\":\"a\",\"title\":\"A\",\"status\":\"open\",\"plan_id\":\"pl-a\"}\n",
        )
        .unwrap();
        fs::write(
            d.join("plans.jsonl"),
            b"{\"id\":\"pl-a\",\"status\":\"draft\",\"sections\":{\"n\":1e400}}\n",
        )
        .unwrap();
        for cmd in ["show", "list", "ready", "blocked"] {
            let args = if cmd == "show" {
                vec![cmd, "a", "--json"]
            } else {
                vec![cmd, "--json"]
            };
            let out = invoke(t.path(), &args);
            assert_eq!(out.exit_code, 0);
            let raw = String::from_utf8(out.stdout).unwrap();
            assert!(!raw.contains("plan_status"));
            assert!(!raw.contains("\"plan\":"));
        }
    }
}
