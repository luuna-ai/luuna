//! Contract-driven Commander-compatible argument handling.
use crate::{
    json::{Object, Value},
    options::GlobalOptions,
    records,
};
#[derive(Clone, Default)]
pub(super) struct OptionSpec {
    pub flags: String,
    pub short: String,
    pub long: String,
    pub name: String,
    pub description: String,
    pub required: bool,
    pub optional: bool,
    pub variadic: bool,
    pub negate: bool,
    pub mandatory: bool,
    pub default: Option<Value>,
    pub choices: Vec<String>,
    pub global: bool,
}
#[derive(Clone, Default)]
pub(super) struct Argument {
    pub name: String,
    pub required: bool,
    pub variadic: bool,
}
#[derive(Clone, Default)]
pub(super) struct Spec {
    pub name: String,
    pub description: String,
    pub hidden: bool,
    pub options: Vec<OptionSpec>,
    pub arguments: Vec<Argument>,
    pub commands: Vec<Spec>,
    pub help: String,
}
fn s(o: &Object, k: &str) -> String {
    o.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
}
fn b(o: &Object, k: &str) -> bool {
    matches!(o.get(k), Some(Value::Bool(true)))
}
fn array<'a>(o: &'a Object, k: &str) -> &'a [Value] {
    o.get(k)
        .and_then(Value::as_array)
        .map_or(&[], |v| v.as_slice())
}
impl Spec {
    pub fn read(v: &Value) -> Self {
        let o = v.as_object().expect("embedded command object");
        Self {
            name: s(o, "name"),
            description: s(o, "description"),
            hidden: b(o, "hidden"),
            help: s(o, "help"),
            arguments: array(o, "arguments")
                .iter()
                .map(|v| {
                    let a = v.as_object().unwrap();
                    Argument {
                        name: s(a, "name"),
                        required: b(a, "required"),
                        variadic: b(a, "variadic"),
                    }
                })
                .collect(),
            options: array(o, "options")
                .iter()
                .map(|v| {
                    let a = v.as_object().unwrap();
                    OptionSpec {
                        flags: s(a, "flags"),
                        short: s(a, "short"),
                        long: s(a, "long"),
                        name: s(a, "name"),
                        description: s(a, "description"),
                        required: b(a, "required"),
                        optional: b(a, "optional"),
                        variadic: b(a, "variadic"),
                        negate: b(a, "negate"),
                        mandatory: b(a, "mandatory"),
                        default: a.get("default").cloned(),
                        choices: array(a, "choices")
                            .iter()
                            .map(|v| v.as_str().unwrap().to_owned())
                            .collect(),
                        global: false,
                    }
                })
                .collect(),
            commands: array(o, "commands").iter().map(Self::read).collect(),
        }
    }
}
#[derive(Default)]
pub(super) struct Invocation {
    pub positionals: Vec<String>,
    pub flags: Vec<(String, Vec<String>)>,
}
impl Invocation {
    pub fn flat(&self) -> Vec<(String, String)> {
        self.flags
            .iter()
            .flat_map(|(k, v)| v.iter().map(move |v| (k.clone(), v.clone())))
            .collect()
    }
    fn put(&mut self, key: &str, values: Vec<String>) {
        if let Some((_, v)) = self.flags.iter_mut().find(|(k, _)| k == key) {
            *v = values;
        } else {
            self.flags.push((key.into(), values));
        }
    }
    fn has(&self, key: &str) -> bool {
        self.flags.iter().any(|(k, _)| k == key)
    }
}
pub(super) enum Parsed {
    Invocation(Invocation),
    Help,
    Version,
}
pub(super) fn negative(s: &str) -> bool {
    let Some(s) = s.strip_prefix('-') else {
        return false;
    };
    let (base, exp) = s.split_once('e').map_or((s, None), |(b, e)| (b, Some(e)));
    if let Some(exp) = exp {
        let exp = exp.strip_prefix(['+', '-']).unwrap_or(exp);
        if exp.is_empty() || !exp.bytes().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    if let Some((a, b)) = base.split_once('.') {
        a.bytes().all(|c| c.is_ascii_digit())
            && !b.is_empty()
            && b.bytes().all(|c| c.is_ascii_digit())
    } else {
        !base.is_empty() && base.bytes().all(|c| c.is_ascii_digit())
    }
}
pub(super) fn find<'a>(
    options: &'a [OptionSpec],
    token: &str,
) -> Option<(&'a OptionSpec, Option<String>)> {
    let (name, inline) = token
        .split_once('=')
        .map_or((token, None), |(a, b)| (a, Some(b.to_owned())));
    options
        .iter()
        .find(|o| {
            (name == o.long || (!o.short.is_empty() && name == o.short))
                && (inline.is_none() || o.required || o.optional)
        })
        .map(|o| (o, inline))
}
pub(super) fn expand(options: &[OptionSpec], token: &str) -> Option<Vec<String>> {
    if token.len() <= 2
        || !token.starts_with('-')
        || token.starts_with("--")
        || !token.is_char_boundary(2)
    {
        return None;
    }
    let (o, _) = find(options, &token[..2])?;
    Some(vec![
        token[..2].to_owned(),
        if o.required || o.optional {
            token[2..].to_owned()
        } else {
            format!("-{}", &token[2..])
        },
    ])
}
pub(super) fn consume(
    o: &OptionSpec,
    args: &[String],
    index: usize,
    inline: Option<String>,
) -> Result<(Vec<String>, usize), String> {
    if !o.required && !o.optional {
        return Ok((
            vec![if o.negate { "false" } else { "true" }.into()],
            index + 1,
        ));
    }
    let mut vals = Vec::new();
    let mut next = index + 1;
    if let Some(v) = inline {
        vals.push(v)
    } else if next < args.len()
        && (o.required || !args[next].starts_with('-') || negative(&args[next]))
    {
        vals.push(args[next].clone());
        next += 1;
    } else if o.required {
        return Err(format!("option '{}' argument missing", o.flags));
    }
    if o.variadic {
        while next < args.len() && (!args[next].starts_with('-') || negative(&args[next])) {
            vals.push(args[next].clone());
            next += 1;
        }
    }
    for v in &vals {
        if !o.choices.is_empty() && !o.choices.contains(v) {
            return Err(format!(
                "option '{}' argument '{}' is invalid. Allowed choices are {}.",
                o.flags,
                v,
                o.choices.join(", ")
            ));
        }
    }
    Ok((vals, next))
}
pub(super) fn parse(
    spec: &Spec,
    globals: &[OptionSpec],
    raw: &[String],
    global: &mut GlobalOptions,
) -> Result<Parsed, String> {
    let mut i = Invocation::default();
    let mut options = spec.options.clone();
    for g in globals {
        let mut g = g.clone();
        g.global = true;
        if let Some(n) = options.iter().position(|o| o.long == g.long) {
            options[n] = g
        } else {
            options.push(g)
        }
    }
    for o in &spec.options {
        let key = o.name.strip_prefix("no-").unwrap_or(&o.name);
        if let Some(v) = o.default.as_ref().filter(|v| !matches!(v, Value::Null)) {
            let vals = if let Value::Array(a) = v {
                a.iter().map(|v| records::text_value(Some(v))).collect()
            } else {
                vec![records::text_value(Some(v))]
            };
            i.put(key, vals);
        } else if o.negate {
            i.put(key, vec!["true".into()]);
        }
    }
    let mut args = raw.to_vec();
    let mut pos = 0;
    let mut positional = false;
    let mut unknown = String::new();
    while pos < args.len() {
        let token = &args[pos];
        if token == "--" && !positional {
            positional = true;
            pos += 1;
            continue;
        }
        if !positional && (token == "--help" || token == "-h") {
            return Ok(Parsed::Help);
        }
        if !positional && (token == "--version" || token == "-v") {
            return Ok(Parsed::Version);
        }
        if positional || !token.starts_with('-') || token == "-" || negative(token) {
            i.positionals.push(token.clone());
            pos += 1;
            continue;
        }
        let Some((o, inline)) = find(&options, token) else {
            if let Some(exp) = expand(&options, token) {
                args.splice(pos..=pos, exp);
                continue;
            }
            if unknown.is_empty() {
                unknown = token.clone()
            }
            pos += 1;
            continue;
        };
        let (vals, next) = consume(o, &args, pos, inline)?;
        let key = o.name.strip_prefix("no-").unwrap_or(&o.name);
        if o.global {
            global.set(&o.name, &vals)
        } else if o.variadic || o.name == "dir-anchor" {
            let seen = args[..pos]
                .iter()
                .any(|a| a == &o.long || a.starts_with(&(o.long.clone() + "=")));
            if !seen {
                i.put(key, Vec::new())
            }
            if let Some((_, v)) = i.flags.iter_mut().find(|(k, _)| k == key) {
                v.extend(vals)
            }
        } else {
            i.put(key, vals)
        }
        pos = next;
    }
    for o in &spec.options {
        if o.mandatory && !i.has(o.name.strip_prefix("no-").unwrap_or(&o.name)) {
            return Err(format!("required option '{}' not specified", o.flags));
        }
    }
    if !unknown.is_empty() {
        return Err(format!("unknown option '{unknown}'"));
    }
    for (n, a) in spec.arguments.iter().enumerate() {
        if a.required && i.positionals.len() <= n {
            return Err(format!("missing required argument '{}'", a.name));
        }
    }
    if !spec.arguments.iter().any(|a| a.variadic) && i.positionals.len() > spec.arguments.len() {
        return Err(format!(
            "too many arguments for '{}'. Expected {} arguments but got {}.",
            spec.name,
            spec.arguments.len(),
            i.positionals.len()
        ));
    }
    Ok(Parsed::Invocation(i))
}
pub(super) fn suggest(input: &str, commands: &[Spec]) -> String {
    let a: Vec<_> = input.chars().collect();
    let mut best = String::new();
    let mut distance = 3;
    for c in commands {
        let b: Vec<_> = c.name.chars().collect();
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for (n, ca) in a.iter().enumerate() {
            let mut cur = vec![0; b.len() + 1];
            cur[0] = n + 1;
            for (j, cb) in b.iter().enumerate() {
                cur[j + 1] = (cur[j] + 1)
                    .min(prev[j + 1] + 1)
                    .min(prev[j] + usize::from(ca != cb));
            }
            prev = cur;
        }
        if prev[b.len()] < distance {
            distance = prev[b.len()];
            best = c.name.clone()
        }
    }
    best
}
