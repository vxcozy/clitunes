use tokio_util::codec::LinesCodec;

const MAX_LINE_LENGTH: usize = 65_536;

pub fn control_codec() -> LinesCodec {
    LinesCodec::new_with_max_length(MAX_LINE_LENGTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use tokio_util::codec::Decoder;

    #[test]
    fn decodes_a_normal_line() {
        let mut codec = control_codec();
        let mut buf = BytesMut::from("{\"verb\":\"play\"}\n");
        let line = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(line, "{\"verb\":\"play\"}");
    }

    #[test]
    fn rejects_oversized_line() {
        let mut codec = control_codec();
        let oversized = "x".repeat(MAX_LINE_LENGTH + 1) + "\n";
        let mut buf = BytesMut::from(oversized.as_str());
        let result = codec.decode(&mut buf);
        assert!(result.is_err(), "should reject lines > 65536 bytes");
    }

    #[test]
    fn incomplete_line_returns_none() {
        let mut codec = control_codec();
        let mut buf = BytesMut::from("{\"verb\":\"play\"}");
        let result = codec.decode(&mut buf).unwrap();
        assert!(result.is_none(), "no newline means incomplete frame");
    }
}
