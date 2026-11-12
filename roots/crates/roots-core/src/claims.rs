//! Read-side claim resolution, including agent text a human explicitly endorsed.

use crate::errors::RootsResult;
use crate::graph::find_node;
use crate::model::{Graph, NodeRecord};
use crate::paths::RootsPaths;
use crate::prose::{NodeProse, content_hash, read_node_prose};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndorsedClaim {
    pub sprout: String,
    pub statement: String,
    pub body: String,
    pub changed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Claim {
    pub prose: NodeProse,
    pub endorsed: Option<EndorsedClaim>,
    pub statement: String,
    pub text: String,
}

pub fn read_claim(paths: &RootsPaths, graph: &Graph, node: &NodeRecord) -> RootsResult<Claim> {
    let prose = read_node_prose(paths, node, None)?;
    let endorsed = node
        .endorses
        .as_ref()
        .map(|pin| {
            let Some(sprout) = find_node(graph, &pin.sprout) else {
                return Ok(EndorsedClaim {
                    sprout: pin.sprout.clone(),
                    statement: String::new(),
                    body: String::new(),
                    changed: true,
                });
            };
            let agent_prose = read_node_prose(paths, sprout, None)?;
            Ok(EndorsedClaim {
                sprout: sprout.id.clone(),
                statement: agent_prose.statement,
                body: agent_prose.body,
                changed: !agent_prose.exists || content_hash(&agent_prose.text) != pin.hash,
            })
        })
        .transpose()?;
    let statement = endorsed
        .as_ref()
        .filter(|value| !value.statement.is_empty())
        .map(|value| value.statement.clone())
        .unwrap_or_else(|| prose.statement.clone());
    let text = if let Some(value) = endorsed
        .as_ref()
        .filter(|value| !value.statement.is_empty())
    {
        [
            value.statement.as_str(),
            value.body.as_str(),
            prose.text.as_str(),
        ]
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
    } else {
        prose.text.clone()
    };
    Ok(Claim {
        prose,
        endorsed,
        statement,
        text,
    })
}
