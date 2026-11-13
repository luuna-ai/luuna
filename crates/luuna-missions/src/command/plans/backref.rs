//! Generated plan backrefs preserve surrounding manual text as Go does.
use crate::{config_tree::upgrade_legacy_markers, json::Value};
const START: &str = "<!-- missions:plan-backref:start -->";
const END: &str = "<!-- missions:plan-backref:end -->";
fn space(c: char) -> bool {
    matches!(c,'\t'..='\r'|' '|'\u{85}'|'\u{a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}')
}
pub(super) struct Backref<'a> {
    pub plan_id: &'a str,
    pub parent_id: &'a str,
    pub parent_title: &'a str,
    pub template: &'a str,
    pub approach: Option<&'a Value>,
}
impl Backref<'_> {
    fn block(&self, index: Option<i64>) -> String {
        let mut lines = vec![
            index
                .map(|i| format!("Step {} of plan {}.", i.wrapping_add(1), self.plan_id))
                .unwrap_or_else(|| format!("Adopted into plan {}.", self.plan_id)),
            String::new(),
            format!("Parent seed: {} — {}", self.parent_id, self.parent_title),
            format!("Plan template: {}", self.template),
        ];
        if let Some(Value::String(text)) = self.approach {
            let mut collapsed = text
                .split(space)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !collapsed.is_empty() {
                if collapsed.chars().count() > 240 {
                    let mut cut = collapsed.chars().take(240).collect::<String>();
                    if let Some(last) = cut.rfind(' ').filter(|i| *i > 120) {
                        cut.truncate(last);
                    }
                    collapsed = cut.trim_end_matches(space).to_owned() + "…";
                }
                lines.push(format!("Plan approach: {collapsed}"));
            }
        }
        lines.push(String::new());
        lines.push(format!("Run `luuna-missions plan show {}` for the full plan (context, alternatives, sibling steps, acceptance criteria).",self.plan_id));
        format!("{START}\n{}\n{END}", lines.join("\n"))
    }
    pub fn apply(&self, existing: &str, index: Option<i64>) -> String {
        let block = self.block(index);
        let prior = upgrade_legacy_markers(existing);
        let (start, end) = (prior.find(START), prior.find(END));
        if let (Some(start), Some(end)) = (start, end) {
            if end >= start {
                return prior[..start].to_owned() + &block + &prior[end + END.len()..];
            }
        }
        if prior.trim_matches(space).is_empty() {
            block
        } else {
            block + "\n\n" + &prior
        }
    }
}
pub(super) fn strip(existing: &str) -> String {
    let prior = upgrade_legacy_markers(existing);
    let (Some(start), Some(end)) = (prior.find(START), prior.find(END)) else {
        return prior;
    };
    if end < start {
        return prior;
    }
    let before = prior[..start].trim_end_matches(space);
    let after = prior[end + END.len()..].trim_start_matches(space);
    if before.is_empty() {
        after.into()
    } else if after.is_empty() {
        before.into()
    } else {
        format!("{before}\n\n{after}")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn context<'a>(approach: Option<&'a Value>) -> Backref<'a> {
        Backref {
            plan_id: "pl-a",
            parent_id: "seed",
            parent_title: "Example",
            template: "feature",
            approach,
        }
    }
    #[test]
    fn block_and_manual_text_follow_go() {
        let b = context(None);
        assert_eq!(b.apply("manual",Some(0)),"<!-- missions:plan-backref:start -->\nStep 1 of plan pl-a.\n\nParent seed: seed — Example\nPlan template: feature\n\nRun `luuna-missions plan show pl-a` for the full plan (context, alternatives, sibling steps, acceptance criteria).\n<!-- missions:plan-backref:end -->\n\nmanual");
    }
    #[test]
    fn updates_only_the_first_block_and_keeps_surrounding_bytes() {
        let b = context(None);
        let prior =
            "before<!-- seeds:plan-backref:start -->old<!-- seeds:plan-backref:end -->after";
        let next = b.apply(prior, None);
        assert!(
            next.starts_with("before<!-- missions:plan-backref:start -->\nAdopted into plan pl-a.")
        );
        assert!(next.ends_with("<!-- missions:plan-backref:end -->after"));
    }
    #[test]
    fn stripping_uses_unicode_whitespace_only_at_block_boundaries() {
        assert_eq!(strip("before\u{a0}<!-- missions:plan-backref:start -->old<!-- missions:plan-backref:end -->\u{a0}after"),"before\n\nafter");
        assert_eq!(strip("manual without markers"), "manual without markers");
    }
    #[test]
    fn approach_collapses_go_whitespace_and_truncates_at_rune_boundary() {
        let text = Value::String("x\u{85} y\n z".into());
        assert!(context(Some(&text))
            .block(None)
            .contains("Plan approach: x y z\n"));
        let text = Value::String("😀".repeat(241));
        let block = context(Some(&text)).block(None);
        assert!(block.contains(&("Plan approach: ".to_owned() + &"😀".repeat(240) + "…\n")));
    }
}
