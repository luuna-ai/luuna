//! Flat plan-tree arena: hostile graph depth never grows the Rust call stack.
use super::{common::*, record::Plan};
use crate::{
    command::output::Envelope,
    error::{Error, Result},
    model::Ticket,
};
use std::collections::HashMap;
const MAX_NODES: usize = 100_000;
const MAX_OUTPUT: usize = 128 * 1024 * 1024;
fn bounded() -> Error {
    Error::unsupported(
        "plan show expansion beyond 100000 nodes or 128 MiB of buffered output",
        "none (bounded native plan-tree expansion)",
    )
}
fn add(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_OUTPUT.saturating_sub(out.len()) {
        return Err(bounded());
    }
    out.extend_from_slice(bytes);
    Ok(())
}
struct Child<'a> {
    id: &'a str,
    title: &'a str,
    status: &'a str,
    adopted: bool,
}
pub(super) enum Nested {
    Node(usize),
    Truncated(String),
}
pub(super) struct Node<'a> {
    pub plan: &'a Plan,
    wire: Vec<u8>,
    children: Vec<Child<'a>>,
    nested: Vec<Nested>,
}
pub(super) struct Tree<'a> {
    pub nodes: Vec<Node<'a>>,
}
impl<'a> Tree<'a> {
    pub fn build(
        root: &'a Plan,
        plans: &'a [Plan],
        issues: &'a [Ticket],
        max_depth: i64,
    ) -> Result<Self> {
        let mut latest: HashMap<&str, &Plan> = HashMap::new();
        for p in plans {
            if latest
                .get(p.seed.as_str())
                .is_none_or(|old| old.updated_at < p.updated_at)
            {
                latest.insert(&p.seed, p);
            }
        }
        let mut budget = 0usize;
        let mut nodes = vec![Self::node(root, issues, &mut budget)?];
        let mut pending = vec![(0usize, 1i64)];
        while let Some((index, depth)) = pending.pop() {
            for id in nodes[index].plan.children() {
                let Some(sub) = latest.get(id.as_str()).copied() else {
                    continue;
                };
                if depth + 1 > max_depth {
                    let hint = format!(
                        "depth limit reached — use `luuna-missions plan show {}` to drill in",
                        sub.id
                    );
                    budget = budget.saturating_add(hint.len() + 64);
                    if budget > MAX_OUTPUT {
                        return Err(bounded());
                    }
                    nodes[index].nested.push(Nested::Truncated(sub.id.clone()));
                } else {
                    if nodes.len() >= MAX_NODES {
                        return Err(bounded());
                    }
                    let child = nodes.len();
                    nodes.push(Self::node(sub, issues, &mut budget)?);
                    nodes[index].nested.push(Nested::Node(child));
                    pending.push((child, depth + 1));
                }
            }
        }
        Ok(Self { nodes })
    }
    fn node(plan: &'a Plan, issues: &'a [Ticket], budget: &mut usize) -> Result<Node<'a>> {
        let wire = plan.marshal()?;
        *budget = budget.saturating_add(wire.len() + 64);
        if *budget > MAX_OUTPUT {
            return Err(bounded());
        }
        let mut children = vec![];
        for id in plan.children() {
            let child = issues.iter().find(|t| t.id == *id);
            let (title, status) = child
                .map(|t| (t.title.as_str(), t.status.as_str()))
                .unwrap_or(("(missing)", "missing"));
            *budget = budget.saturating_add(id.len() + title.len() + status.len() + 64);
            if *budget > MAX_OUTPUT {
                return Err(bounded());
            }
            children.push(Child {
                id,
                title,
                status,
                adopted: plan.adopted().contains(id),
            });
        }
        Ok(Node {
            plan,
            wire,
            children,
            nested: vec![],
        })
    }
    pub fn plan_json(&self) -> &[u8] {
        &self.nodes[0].wire
    }
    pub fn children_json(&self, index: usize) -> Vec<u8> {
        array_json(self.nodes[index].children.iter().map(|c| {
            compact(
                Envelope::object()
                    .string("id", c.id)
                    .string("title", c.title)
                    .string("status", c.status)
                    .boolean("adopted", c.adopted),
            )
        }))
    }
    pub fn nested_json(&self) -> Result<Vec<u8>> {
        enum Op<'a> {
            Bytes(Vec<u8>),
            Slice(&'a [u8]),
            Node(usize),
            Hint(&'a str),
        }
        let mut out = vec![b'['];
        let mut pending = vec![Op::Bytes(vec![b']'])];
        for (i, n) in self.nodes[0].nested.iter().enumerate().rev() {
            pending.push(match n {
                Nested::Node(n) => Op::Node(*n),
                Nested::Truncated(id) => Op::Hint(id),
            });
            if i > 0 {
                pending.push(Op::Bytes(vec![b',']));
            }
        }
        while let Some(op) = pending.pop() {
            match op{Op::Slice(bytes)=>add(&mut out,bytes)?,Op::Bytes(bytes)=>add(&mut out,&bytes)?,Op::Hint(id)=>add(&mut out,&compact(Envelope::object().string("plan_id",id).boolean("truncated",true).string("hint",&format!("depth limit reached — use `luuna-missions plan show {id}` to drill in"))))?,Op::Node(index)=>{add(&mut out,b"{\"plan\":")?;add(&mut out,&self.nodes[index].wire)?;add(&mut out,b",\"children\":")?;add(&mut out,&self.children_json(index))?;add(&mut out,b",\"children_plans\":[")?;pending.push(Op::Slice(b"]}"));for(i,n)in self.nodes[index].nested.iter().enumerate().rev(){pending.push(match n{Nested::Node(n)=>Op::Node(*n),Nested::Truncated(id)=>Op::Hint(id)});if i>0{pending.push(Op::Bytes(vec![b',']));}}}}
        }
        Ok(out)
    }
    pub fn children_text(&self, index: usize, indent: &str, out: &mut Vec<u8>) -> Result<()> {
        for c in &self.nodes[index].children {
            add(
                out,
                format!(
                    "{indent}{}  [{}]  {}{}\n",
                    c.id,
                    c.status,
                    c.title,
                    if c.adopted { " (adopted)" } else { "" }
                )
                .as_bytes(),
            )?;
        }
        Ok(())
    }
    pub fn child_count(&self, index: usize) -> usize {
        self.nodes[index].children.len()
    }
    pub fn nested_text(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut pending = vec![];
        for n in self.nodes[0].nested.iter().rev() {
            pending.push((n, 1usize));
        }
        while let Some((nested, level)) = pending.pop() {
            add(out, b"\n")?;
            let indent = "  ".repeat(level);
            match nested{Nested::Truncated(id)=>add(out,format!("{indent}depth limit reached — use `luuna-missions plan show {id}` to drill in\n").as_bytes())?,Nested::Node(index)=>{let p=self.nodes[*index].plan;let name=if p.name.is_empty(){String::new()}else{format!("  {}",p.name)};add(out,format!("{indent}Sub-plan: {}{name}  [{}]  rev {}  seed={}\n{indent}Children ({}):\n",p.id,p.status,p.revision,p.seed,self.child_count(*index)).as_bytes())?;let child_indent=format!("{indent}  ");if self.child_count(*index)==0{add(out,format!("{child_indent}(none)\n").as_bytes())?;}else{self.children_text(*index,&child_indent,out)?;}for n in self.nodes[*index].nested.iter().rev(){pending.push((n,level+1));}}}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Record;
    fn plans() -> Vec<Plan> {
        vec![Plan::from_line(br#"{"id":"pl-a","seed":"root","children":["child"],"sections":{}}"#).unwrap(),Plan::from_line(br#"{"id":"pl-b","seed":"child","name":"Nested","status":"approved","revision":1,"children":[],"sections":{}}"#).unwrap()]
    }
    #[test]
    fn children_and_depth_limit_match_go_struct_order() {
        let ps = plans();
        let tree = Tree::build(&ps[0], &ps, &[], 1).unwrap();
        assert_eq!(
            tree.children_json(0),
            br#"[{"id":"child","title":"(missing)","status":"missing","adopted":false}]"#
        );
        assert_eq!(tree.nested_json().unwrap(),"[{\"plan_id\":\"pl-b\",\"truncated\":true,\"hint\":\"depth limit reached — use `luuna-missions plan show pl-b` to drill in\"}]".as_bytes());
    }
    #[test]
    fn nested_text_keeps_go_spacing_and_blank_lines() {
        let ps = plans();
        let tree = Tree::build(&ps[0], &ps, &[], 2).unwrap();
        let mut out = vec![];
        tree.nested_text(&mut out).unwrap();
        assert_eq!(out,b"\n  Sub-plan: pl-b  Nested  [approved]  rev 1  seed=child\n  Children (0):\n    (none)\n");
    }
    #[test]
    fn cyclic_plan_graph_is_bounded_without_call_stack_recursion() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let p = Plan::from_line(
                    br#"{"id":"pl-a","seed":"root","children":["root"],"sections":{}}"#,
                )
                .unwrap();
                let ps = vec![p];
                let tree = Tree::build(&ps[0], &ps, &[], 2000).unwrap();
                assert_eq!(tree.nodes.len(), 2000);
                assert!(crate::json::is_valid(&tree.nested_json().unwrap()));
                drop(tree);
                assert!(matches!(
                    Tree::build(&ps[0], &ps, &[], i64::MAX),
                    Err(Error::Unsupported { .. })
                ));
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
