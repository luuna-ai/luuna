//! Expected bytes recorded from the pinned Go toolchain and yaml.v3 v3.0.1.
use super::yaml_decode::decode;
#[test]
fn null() {
    let value = decode("null\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "null\n");
}
#[test]
fn bool() {
    let value = decode("true\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "true\n");
}
#[test]
fn legacy_bool() {
    let value = decode("yes\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "\"yes\"\n");
}
#[test]
fn signed() {
    let value = decode("-123\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "-123\n");
}
#[test]
fn uint() {
    let value = decode("18446744073709551615\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "18446744073709551615\n");
}
#[test]
fn float() {
    let value = decode("1.25\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "1.25\n");
}
#[test]
fn overflow() {
    let value = decode("1e400\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "1e400\n");
}
#[test]
fn underflow() {
    let value = decode("1e-400\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "0\n");
}
#[test]
fn nan() {
    let value = decode(".nan\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), ".nan\n");
}
#[test]
fn negative_zero() {
    let value = decode("-0.0\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "-0\n");
}
#[test]
fn tag_string() {
    let value = decode("!!str 12\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "\"12\"\n");
}
#[test]
fn binary() {
    let value = decode("!!binary /w==\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "!!binary /w==\n");
}
#[test]
fn time() {
    let value = decode("2001-12-14T21:59:43.1234567891Z\n", false)
        .unwrap()
        .value;
    assert_eq!(value.to_yaml(), "2001-12-14T21:59:43.123456789Z\n");
}
#[test]
fn mapping() {
    let value = decode("z: 2\na: 1\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "a: 1\nz: 2\n");
}
#[test]
fn natural() {
    let value = decode("a10: x\na2: y\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "a2: \"y\"\na10: x\n");
}
#[test]
fn sequence() {
    let value = decode("a: [1, 2]\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "a:\n    - 1\n    - 2\n");
}
#[test]
fn sequence_maps() {
    let value = decode("a: [{b: 1, c: 2}, {d: 3}]\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "a:\n    - b: 1\n      c: 2\n    - d: 3\n");
}
#[test]
fn literal() {
    let value = decode("a: |\n  one\n  two\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "a: |\n    one\n    two\n");
}
#[test]
fn merge() {
    let value = decode("a: &a {x: 1}\nb: {<<: *a, y: 2}\n", false)
        .unwrap()
        .value;
    assert_eq!(
        value.to_yaml(),
        "a:\n    x: 1\nb:\n    x: 1\n    \"y\": 2\n"
    );
}
#[test]
fn quoted() {
    let value = decode("a: 'true'\nb: 'a: b'\n", false).unwrap().value;
    assert_eq!(value.to_yaml(), "a: \"true\"\nb: 'a: b'\n");
}

#[test]
fn unicode_line_separator_survives_rewrite() {
    let value = decode("unicode: \"é 漢字 \\u2028\"\n", false)
        .unwrap()
        .value;
    let text = value.to_yaml();
    let after = decode(&text, false).unwrap().value;
    assert_eq!(
        value.to_go_json().unwrap(),
        after.to_go_json().unwrap(),
        "{text:?}"
    );
}

#[test]
fn edge_underscore() {
    let value = decode("x: _12\n", true).unwrap().value;
    assert_eq!(value.to_yaml(), "x: _12\n");
    assert_eq!(value.to_go_json().unwrap(), "{\"x\":\"_12\"}");
}

#[test]
fn edge_double_underscore() {
    let value = decode("x: __1\n", true).unwrap().value;
    assert_eq!(value.to_yaml(), "x: __1\n");
    assert_eq!(value.to_go_json().unwrap(), "{\"x\":\"__1\"}");
}

#[test]
fn edge_short_timestamp() {
    let value = decode("x: 2001-2-3T4:5:6Z\n", true).unwrap().value;
    assert_eq!(value.to_yaml(), "x: 2001-02-03T04:05:06Z\n");
    assert_eq!(
        value.to_go_json().unwrap(),
        "{\"x\":\"2001-02-03T04:05:06Z\"}"
    );
}

#[test]
fn edge_short_date_timestamp() {
    let value = decode("x: 2001-2-3T04:05:06Z\n", true).unwrap().value;
    assert_eq!(value.to_yaml(), "x: 2001-02-03T04:05:06Z\n");
    assert_eq!(
        value.to_go_json().unwrap(),
        "{\"x\":\"2001-02-03T04:05:06Z\"}"
    );
}

#[test]
fn edge_short_space_timestamp() {
    let value = decode("x: 2001-02-03 4:5:6\n", true).unwrap().value;
    assert_eq!(value.to_yaml(), "x: 2001-02-03T04:05:06Z\n");
    assert_eq!(
        value.to_go_json().unwrap(),
        "{\"x\":\"2001-02-03T04:05:06Z\"}"
    );
}

#[test]
fn edge_mixed_keys() {
    let value = decode("x: {a: one, 1: two}\n", true).unwrap().value;
    assert_eq!(value.to_yaml(), "x:\n    1: two\n    a: one\n");
    assert_eq!(
        value.to_go_json().unwrap(),
        "{\"x\":{\"1\":\"two\",\"a\":\"one\"}}"
    );
}

#[test]
fn edge_complex_keys() {
    assert_eq!(
        decode("x: {? [a, b]: v}\n", true)
            .err()
            .expect("Go refuses this key")
            .to_string(),
        "yaml: invalid map key: []interface {}{\"a\", \"b\"}"
    );
}

#[test]
fn edge_time_key() {
    let value = decode("x: {2001-12-14: v}\n", true).unwrap().value;
    assert_eq!(value.to_yaml(), "x:\n    2001-12-14T00:00:00Z: v\n");
    assert_eq!(
        value.to_go_json().unwrap(),
        "{\"x\":{\"2001-12-14T00:00:00Z\":\"v\"}}"
    );
}

#[test]
fn edge_large_integer_keys() {
    let value = decode(
        "x: {9223372036854775807: last, 9223372036854775806: first}\n",
        true,
    )
    .unwrap()
    .value;
    assert_eq!(
        value.to_yaml(),
        "x:\n    9223372036854775806: first\n    9223372036854775807: last\n"
    );
    assert_eq!(
        value.to_go_json().unwrap(),
        "{\"x\":{\"9223372036854775806\":\"first\",\"9223372036854775807\":\"last\"}}"
    );
}

#[test]
fn edge_invalid_binary_sequence() {
    let value = decode("x: !!binary 8JCA\n", true).unwrap().value;
    assert_eq!(value.to_yaml(), "x: !!binary 8JCA\n");
    assert_eq!(value.to_go_json().unwrap(), "{\"x\":\"���\"}");
}

#[test]
fn edge_complex_null() {
    assert_eq!(
        decode("x: {? [null]: v}\n", true)
            .err()
            .expect("Go refuses this key")
            .to_string(),
        "yaml: invalid map key: []interface {}{interface {}(nil)}"
    );
}

#[test]
fn edge_complex_bool() {
    assert_eq!(
        decode("x: {? [true, 1.25]: v}\n", true)
            .err()
            .expect("Go refuses this key")
            .to_string(),
        "yaml: invalid map key: []interface {}{true, 1.25}"
    );
}

#[test]
fn edge_complex_map() {
    assert_eq!(
        decode("x: {? {a: b}: v}\n", true)
            .err()
            .expect("Go refuses this key")
            .to_string(),
        "yaml: invalid map key: map[string]interface {}{\"a\":\"b\"}"
    );
}

#[test]
fn edge_complex_uint() {
    assert_eq!(
        decode("x: {? [18446744073709551615]: v}\n", true)
            .err()
            .expect("Go refuses this key")
            .to_string(),
        "yaml: invalid map key: []interface {}{0xffffffffffffffff}"
    );
}

#[test]
fn edge_complex_time() {
    assert_eq!(decode("x: {? [2001-12-14]: v}\n",true).err().expect("Go refuses this key").to_string(),"yaml: invalid map key: []interface {}{time.Date(2001, time.December, 14, 0, 0, 0, 0, time.UTC)}");
}

#[test]
fn edge_duplicate_before_invalid_tag() {
    assert_eq!(
        decode("x: !!int bad\nx: 1\n", true)
            .err()
            .expect("Go refuses this key")
            .to_string(),
        "yaml: unmarshal errors:\n  line 2: mapping key \"x\" already defined at line 1"
    );
}

#[test]
fn edge_binary_root_key() {
    let value = decode("!!binary aGVsbG8=: v\n", true).unwrap().value;
    assert_eq!(value.to_yaml(), "hello: v\n");
    assert_eq!(value.to_go_json().unwrap(), "{\"hello\":\"v\"}");
}

#[test]
fn edge_null_key_ignored_value() {
    let value = decode("null: !!int bad\nx: ok\n", true).unwrap().value;
    assert_eq!(value.to_yaml(), "x: ok\n");
    assert_eq!(value.to_go_json().unwrap(), "{\"x\":\"ok\"}");
}

#[test]
fn mixed_key_map_is_not_a_go_string_map() {
    let value = decode("x: {a: one, 1: two}\n", true).unwrap().value;
    let x = value.field(value.root, "x").unwrap();
    assert!(value.field(x, "a").is_none());
}
