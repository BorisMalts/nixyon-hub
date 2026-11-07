use serde::Serialize;
use serde::de::DeserializeOwned;

pub const MAX_LINE_BYTES: usize = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("строка протокола длиннее {MAX_LINE_BYTES} байт")]
    TooLong,
    #[error("строка протокола не читается: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, WireError> {
    let mut line = serde_json::to_vec(value)?;
    if line.len() >= MAX_LINE_BYTES {
        return Err(WireError::TooLong);
    }
    line.push(b'\n');
    Ok(line)
}

pub fn decode<T: DeserializeOwned>(line: &[u8]) -> Result<T, WireError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(WireError::TooLong);
    }
    Ok(serde_json::from_slice(line)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ClientFrame, ErrorCode, Op, Request, ServerFrame};

    #[test]
    fn frames_survive_round_trip_as_single_lines() {
        let request = Request {
            token: "t".into(),
            op: Op::Terminal,
            user: "a@x.org".into(),
            cols: 80,
            rows: 24,
            plan: Vec::new(),
            records: Vec::new(),
            training: vec!["speechocean762".into()],
            ci_run: String::new(),
        };
        let line = encode(&request).unwrap();
        assert_eq!(line.iter().filter(|b| **b == b'\n').count(), 1);
        assert_eq!(decode::<Request>(line.trim_ascii_end()).unwrap(), request);

        let frames = [
            ServerFrame::Log {
                line: "строка\nс переводом".into(),
            },
            ServerFrame::Output {
                data: vec![0, 27, 255],
            },
            ServerFrame::Error {
                code: ErrorCode::Busy,
                message: "занято".into(),
            },
        ];
        for frame in frames {
            let line = encode(&frame).unwrap();
            assert_eq!(line.iter().filter(|b| **b == b'\n').count(), 1);
            assert_eq!(decode::<ServerFrame>(line.trim_ascii_end()).unwrap(), frame);
        }
        let input = ClientFrame::Input { data: vec![3] };
        assert_eq!(
            decode::<ClientFrame>(encode(&input).unwrap().trim_ascii_end()).unwrap(),
            input
        );
    }

    #[test]
    fn oversized_and_broken_lines_are_errors() {
        let huge = ServerFrame::Log {
            line: "ж".repeat(MAX_LINE_BYTES),
        };
        assert!(matches!(encode(&huge), Err(WireError::TooLong)));
        assert!(matches!(decode::<Request>(b"{"), Err(WireError::Json(_))));
        assert!(matches!(
            decode::<Request>(&vec![b' '; MAX_LINE_BYTES + 1]),
            Err(WireError::TooLong)
        ));
    }
}
