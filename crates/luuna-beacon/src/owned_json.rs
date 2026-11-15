//! Stack-safe ownership for arbitrary frontmatter within Go's nesting limit.
use serde_json::{Map, Value};
use std::{
    borrow::Borrow,
    fmt,
    ops::{Deref, DerefMut},
};

/// An owned JSON value with iterative destruction, cloning and comparison.
/// Borrow it as `serde_json::Value`; convert an existing value with `.into()`.
#[derive(Default)]
pub struct JsonValue(Value);

impl JsonValue {
    pub(crate) fn into_inner(mut self) -> Value {
        self.0.take()
    }
}

impl From<Value> for JsonValue {
    fn from(value: Value) -> Self {
        Self(value)
    }
}
impl Deref for JsonValue {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.0
    }
}
impl DerefMut for JsonValue {
    fn deref_mut(&mut self) -> &mut Value {
        &mut self.0
    }
}
impl Borrow<Value> for JsonValue {
    fn borrow(&self) -> &Value {
        &self.0
    }
}
impl Drop for JsonValue {
    fn drop(&mut self) {
        if !self.0.is_array() && !self.0.is_object() {
            return;
        }
        let mut pending = vec![self.0.take()];
        while let Some(mut value) = pending.pop() {
            match &mut value {
                Value::Array(items) => pending.append(items),
                Value::Object(map) => {
                    pending.extend(std::mem::take(map).into_iter().map(|(_, value)| value))
                }
                _ => {}
            }
        }
    }
}
impl Clone for JsonValue {
    fn clone(&self) -> Self {
        enum Step<'a> {
            Visit(&'a Value),
            Array(usize),
            Object(Vec<&'a String>),
        }
        let mut steps = vec![Step::Visit(&self.0)];
        let mut done: Vec<JsonValue> = Vec::new();
        while let Some(step) = steps.pop() {
            match step {
                Step::Visit(Value::Array(items)) => {
                    steps.push(Step::Array(items.len()));
                    steps.extend(items.iter().rev().map(Step::Visit));
                }
                Step::Visit(Value::Object(map)) => {
                    steps.push(Step::Object(map.keys().collect()));
                    steps.extend(map.values().rev().map(Step::Visit));
                }
                Step::Visit(leaf) => done.push(leaf.clone().into()),
                Step::Array(length) => {
                    let children = done.split_off(done.len() - length);
                    done.push(
                        Value::Array(children.into_iter().map(JsonValue::into_inner).collect())
                            .into(),
                    );
                }
                Step::Object(keys) => {
                    let children = done.split_off(done.len() - keys.len());
                    let map: Map<String, Value> = keys
                        .into_iter()
                        .zip(children)
                        .map(|(key, value)| (key.clone(), value.into_inner()))
                        .collect();
                    done.push(Value::Object(map).into());
                }
            }
        }
        done.pop().expect("one cloned JSON value")
    }
}
fn equal(left: &Value, right: &Value) -> bool {
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        match (left, right) {
            (Value::Array(left), Value::Array(right)) if left.len() == right.len() => {
                pending.extend(left.iter().zip(right))
            }
            (Value::Object(left), Value::Object(right)) if left.len() == right.len() => {
                for (key, value) in left {
                    let Some(other) = right.get(key) else {
                        return false;
                    };
                    pending.push((value, other));
                }
            }
            (Value::Array(_) | Value::Object(_), _) | (_, Value::Array(_) | Value::Object(_)) => {
                return false
            }
            (left, right) if left == right => {}
            _ => return false,
        }
    }
    true
}
impl PartialEq for JsonValue {
    fn eq(&self, other: &Self) -> bool {
        equal(&self.0, &other.0)
    }
}
impl PartialEq<Value> for JsonValue {
    fn eq(&self, other: &Value) -> bool {
        equal(&self.0, other)
    }
}
impl PartialEq<&str> for JsonValue {
    fn eq(&self, other: &&str) -> bool {
        self.0.as_str() == Some(*other)
    }
}
impl fmt::Debug for JsonValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = String::new();
        crate::gojson::write_any(&mut text, &self.0);
        formatter.debug_tuple("JsonValue").field(&text).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clone_and_comparison_preserve_shapes_numbers_and_object_semantics() {
        let value = serde_json::json!({"a":[1,1.0,-0.0,2.5,true,null,{"x":"y"}],"b":{}});
        let owned = JsonValue::from(value.clone());
        assert_eq!(*owned.clone(), value);
        assert_eq!(owned.clone(), owned);
        assert_ne!(owned, JsonValue::from(serde_json::json!({"a":[]})));
        assert_ne!(
            JsonValue::from(serde_json::json!([1])),
            JsonValue::from(serde_json::json!([2]))
        );
        assert_ne!(
            JsonValue::from(serde_json::json!({"a":1})),
            JsonValue::from(serde_json::json!({"b":1}))
        );
    }
}
