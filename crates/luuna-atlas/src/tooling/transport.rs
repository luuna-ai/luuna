//! Iterative numeric sidecars for the compatibility IPC facade.
use crate::{
    json::{Object, Value},
    records,
};
fn special(value: &str) -> Option<f64> {
    match value {
        "NaN" => Some(f64::NAN),
        "Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        "-0" => Some(-0.0),
        _ => None,
    }
}
pub fn restore(mut request: Value) -> Value {
    let entries = records::any_list(records::obj(Some(&request)).get("numbers")).to_vec();
    for entry in entries {
        let o = records::obj(Some(&entry));
        let Some(value) = special(&records::text_value(o.get("value"))) else {
            continue;
        };
        let path = records::any_list(o.get("path"));
        let mut node = &mut request;
        for (n, part) in path.iter().enumerate() {
            let last = n == path.len() - 1;
            match node {
                Value::Object(o) => {
                    let key = records::text_value(Some(part));
                    if last {
                        o.insert_unordered(key, value);
                        break;
                    }
                    let Some(next) = o.get_mut(&key) else { break };
                    node = next
                }
                Value::Array(a) => {
                    let number = records::number(Some(part));
                    let index = if let Some(s) = part.as_str() {
                        let Ok(index) = s.parse::<i64>() else { break };
                        index
                    } else {
                        number as i64
                    };
                    if index < 0 || index as usize >= a.len() || index as f64 != number {
                        break;
                    }
                    if last {
                        a[index as usize] = Value::Number(value);
                        break;
                    }
                    node = &mut a[index as usize]
                }
                _ => break,
            }
        }
    }
    request
}
pub fn collect(result: &Value) -> Value {
    enum Part<'a> {
        Visit(&'a Value),
        Key(Value),
        Pop,
    }
    let mut path = Vec::new();
    let mut stack = vec![Part::Visit(result)];
    let mut out = Vec::new();
    while let Some(part) = stack.pop() {
        match part {
            Part::Key(k) => path.push(k),
            Part::Pop => {
                path.pop();
            }
            Part::Visit(v) => match v {
                Value::Number(n) => {
                    let name = if n.is_nan() {
                        "NaN"
                    } else if *n == f64::INFINITY {
                        "Infinity"
                    } else if *n == f64::NEG_INFINITY {
                        "-Infinity"
                    } else if *n == 0.0 && n.is_sign_negative() {
                        "-0"
                    } else {
                        continue;
                    };
                    let mut o = Object::new();
                    o.insert("path", Value::Array(path.clone()));
                    o.insert("value", name);
                    o.remember_order(["path", "value"]);
                    out.push(Value::Object(o));
                }
                Value::Array(items) => {
                    for (n, v) in items.iter().enumerate().rev() {
                        stack.push(Part::Pop);
                        stack.push(Part::Visit(v));
                        stack.push(Part::Key(Value::Integer(n as i128)));
                    }
                }
                Value::Object(o) => {
                    if let Some(a) = o.record_array() {
                        for (n, v) in a.iter().enumerate().rev() {
                            stack.push(Part::Pop);
                            stack.push(Part::Visit(v));
                            stack.push(Part::Key(Value::Integer(n as i128)));
                        }
                    } else {
                        for (k, v) in o.iter().collect::<Vec<_>>().into_iter().rev() {
                            stack.push(Part::Pop);
                            stack.push(Part::Visit(v));
                            stack.push(Part::Key(Value::from_go_bytes(o.key_bytes(k).to_vec())));
                        }
                    }
                }
                _ => (),
            },
        }
    }
    Value::Array(out)
}
