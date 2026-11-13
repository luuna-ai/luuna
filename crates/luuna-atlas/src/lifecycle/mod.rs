//! Native Atlas lifecycle and config operations, with explicit invocation state.
use crate::{options::Context, CommandOutput};
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub flags: Vec<(String, String)>,
    pub input: Vec<u8>,
}
pub fn run(_ctx: &Context, command: &str, _args: &[String], _options: &Options) -> CommandOutput {
    CommandOutput::failure(command, &format!("not implemented: {command}"))
}

#[cfg(test)]
mod raw_string_tests {
    use crate::{
        json::{self, Object, Value},
        yaml,
    };
    #[test]
    fn binary_values_keep_bytes_and_go_json_replacement_count() {
        let value = Value::from_go_bytes(vec![0xe2, 0x82]);
        assert_eq!(value.string_bytes(), Some(&[0xe2, 0x82][..]));
        assert_eq!(json::to_compact_string(&value).unwrap(), "\"��\"");
        assert_eq!(yaml::to_yaml_string(&value).unwrap(), "!!binary 4oI=\n");
        assert_eq!(
            yaml::to_yaml_string(&value.clone()).unwrap(),
            "!!binary 4oI=\n"
        );
    }
    #[test]
    fn binary_keys_do_not_collide_with_each_other_or_valid_unicode() {
        let mut o = Object::new();
        o.insert_go_key(&[0xff], Value::from("one"));
        o.insert_go_key(&[0x80], Value::from("two"));
        o.insert("�", "unicode");
        let value = Value::Object(o);
        assert_eq!(
            json::to_compact_string(&value).unwrap(),
            "{\"�\":\"one\",\"�\":\"two\",\"�\":\"unicode\"}"
        );
        assert_eq!(
            yaml::to_yaml_string(&value).unwrap(),
            "!!binary /w==: one\n!!binary gA==: two\n�: unicode\n"
        );
        assert_eq!(
            yaml::to_yaml_string(&value.clone()).unwrap(),
            yaml::to_yaml_string(&value).unwrap()
        );
    }
    #[test]
    fn binary_key_internal_identity_cannot_swallow_valid_key_data() {
        let mut o = Object::new();
        o.insert_go_key(&[0xff], Value::from("binary"));
        let identity = o.keys().next().unwrap().to_owned();
        o.insert(identity.clone(), "valid");
        assert_eq!(o.get_go_key(&[0xff]).unwrap().as_str(), Some("binary"));
        assert_eq!(
            o.get_go_key(identity.as_bytes()).unwrap().as_str(),
            Some("valid")
        );
        let key = o
            .keys()
            .find(|k| o.key_bytes(k) == [0xff])
            .unwrap()
            .to_owned();
        assert!(o.remove(&key).is_some());
        o.insert_go_key(&[0xff], Value::from("again"));
        assert_eq!(o.get_go_key(&[0xff]).unwrap().as_str(), Some("again"));
        assert_eq!(o.len(), 2);
    }
}
