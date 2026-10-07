//! JSON output that stays readable: indented, but with each list of numbers
//! (a matrix, a colour multiplier) kept on one line.

use anyhow::Result;
use serde::Serialize;

pub fn to_pretty<T: Serialize>(value: &T) -> Result<String> {
    let mut text = inline_number_lists(&serde_json::to_string_pretty(value)?);
    text.push('\n');
    Ok(text)
}

fn inline_number_lists(pretty: &str) -> String {
    let bytes = pretty.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if in_string {
            out.push(byte);
            if byte == b'\\' {
                // Copy the escaped character too, so an escaped quote does
                // not end the string.
                i += 1;
                if let Some(&escaped) = bytes.get(i) {
                    out.push(escaped);
                }
            } else if byte == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
        } else if byte == b'['
            && let Some(len) = number_list_len(&bytes[i + 1..])
        {
            out.push(b'[');
            let mut first = true;
            for number in bytes[i + 1..i + 1 + len]
                .split(|&b| b == b',')
                .map(|number| number.trim_ascii())
            {
                if !first {
                    out.extend_from_slice(b", ");
                }
                first = false;
                out.extend_from_slice(number);
            }
            out.push(b']');
            i += len + 2;
            continue;
        }
        out.push(byte);
        i += 1;
    }
    // Only ASCII bytes were added or removed, and never inside a string.
    String::from_utf8(out).expect("JSON stays valid UTF-8")
}

/// If `rest` starts with a non-empty list of numbers and then `]`, returns the
/// length of the part before the `]`.
fn number_list_len(rest: &[u8]) -> Option<usize> {
    let len = rest.iter().position(|&b| b == b']')?;
    let body = &rest[..len];
    let is_numbers = body.iter().any(u8::is_ascii_digit)
        && body
            .iter()
            .all(|&b| b.is_ascii_digit() || b.is_ascii_whitespace() || b"+-.eE,".contains(&b));
    is_numbers.then_some(len)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn number_lists_go_on_one_line() {
        let text = to_pretty(&json!({ "matrix": [1.5, -2, 0.25] })).unwrap();
        assert_eq!(text, "{\n  \"matrix\": [1.5, -2, 0.25]\n}\n");
    }

    #[test]
    fn other_lists_stay_indented() {
        let text = to_pretty(&json!({ "names": ["a", "b"] })).unwrap();
        assert_eq!(text, "{\n  \"names\": [\n    \"a\",\n    \"b\"\n  ]\n}\n");
    }

    #[test]
    fn brackets_inside_strings_are_left_alone() {
        let value = json!({ "name": "odd [1,\n 2] \"name\"" });
        let text = to_pretty(&value).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap(),
            value
        );
        assert!(text.contains(r#"odd [1,\n 2] \"name\""#));
    }

    #[test]
    fn the_output_parses_back_to_the_same_value() {
        let value = json!({
            "frames": [{ "ops": [{ "matrix": [1, 0, 0, 1, 12.5, -3] }] }],
            "empty": [],
            "nested": [[1, 2], [3, 4]],
        });
        let text = to_pretty(&value).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap(),
            value
        );
    }
}
