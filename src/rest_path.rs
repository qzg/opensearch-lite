pub(crate) fn decode_path_param(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_digit(bytes[index + 1]), hex_digit(bytes[index + 2]))
            {
                output.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::decode_path_param;

    #[test]
    fn path_param_decoder_decodes_percent_escapes_only() {
        assert_eq!(
            decode_path_param("index-pattern%3Aorders"),
            "index-pattern:orders"
        );
        assert_eq!(decode_path_param("a%2fb"), "a/b");
        assert_eq!(decode_path_param("a+b"), "a+b");
        assert_eq!(decode_path_param("bad%zz"), "bad%zz");
    }
}
