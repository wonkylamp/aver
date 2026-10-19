use aver_core::canonical::{to_canonical, CanonicalError};
use serde_json::{json, Value};

fn canon(input: &str) -> Result<String, CanonicalError> {
    let value: Value = serde_json::from_str(input).unwrap();
    to_canonical(&value).map(|bytes| String::from_utf8(bytes).unwrap())
}

#[test]
fn canonical_output_table() {
    let cases = [
        (
            r#" { "b" : 1 , "a" : [ 3 , 1 , 2 ] } "#,
            r#"{"a":[3,1,2],"b":1}"#,
        ),
        (
            r#"{"a":1,"B":2,"_":3,"aa":4,"a_":5}"#,
            r#"{"B":2,"_":3,"a":1,"a_":5,"aa":4}"#,
        ),
        (
            r#"{"z":{"y":{"x":null}},"a":true}"#,
            r#"{"a":true,"z":{"y":{"x":null}}}"#,
        ),
        (
            r#"[9007199254740991,-9007199254740991,0,-1]"#,
            r#"[9007199254740991,-9007199254740991,0,-1]"#,
        ),
        (
            r#""\u00e9\u2028/\ud83d\ude00""#,
            "\"\u{e9}\u{2028}/\u{1f600}\"",
        ),
        (
            r#""quote\" backslash\\ tab\t nl\n cr\r bs\b ff\f""#,
            r#""quote\" backslash\\ tab\t nl\n cr\r bs\b ff\f""#,
        ),
        (
            r#""\u0000\u0001\u001f\u007f""#,
            "\"\\u0000\\u0001\\u001f\u{7f}\"",
        ),
        (r#"{}"#, r#"{}"#),
        (r#"[]"#, r#"[]"#),
        (r#""""#, r#""""#),
    ];
    for (input, expected) in cases {
        assert_eq!(canon(input).unwrap(), expected, "input {input}");
    }
}

#[test]
fn floats_and_unsafe_integers_are_rejected() {
    for input in ["1.5", "1.0", "1e2", "-0.0", "[0.1]", r#"{"a":2.5}"#] {
        assert!(
            matches!(canon(input), Err(CanonicalError::Float(_))),
            "{input} must be rejected as a float"
        );
    }
    for input in [
        "9007199254740992",
        "-9007199254740992",
        "18446744073709551615",
    ] {
        assert!(
            matches!(canon(input), Err(CanonicalError::IntegerOutOfRange(_))),
            "{input} must be rejected as out of range"
        );
    }
}

#[test]
fn non_ascii_keys_are_rejected() {
    assert_eq!(
        to_canonical(&json!({"é": 1})),
        Err(CanonicalError::NonAsciiKey("é".into()))
    );
    assert!(to_canonical(&json!({"a": {"ü": 1}})).is_err());
}
