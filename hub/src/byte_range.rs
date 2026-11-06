/// Кусок файла из заголовка Range: первый и последний байт включительно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    /// Сколько байт в куске: он никогда не пуст.
    pub fn byte_count(&self) -> u64 {
        self.end - self.start + 1
    }
}

/// Что делать с заголовком Range у файла длиной `len`.
#[derive(Debug, PartialEq, Eq)]
pub enum RangeRequest {
    /// Заголовка нет или он не понят: отдаётся весь файл, это разрешает RFC 9110.
    Whole,
    Part(ByteRange),
    /// Кусок целиком за концом файла: ответ 416.
    Unsatisfiable,
}

/// Понимает один кусок `bytes=a-b`, `bytes=a-` и `bytes=-n`. Несколько кусков сразу
/// не поддерживаются: файл отдаётся целиком.
pub fn parse_range(header: Option<&str>, len: u64) -> RangeRequest {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return RangeRequest::Whole;
    };
    if spec.contains(',') {
        return RangeRequest::Whole;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return RangeRequest::Whole;
    };
    let number = |s: &str| -> Option<u64> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    match (first.is_empty(), number(first), number(last)) {
        // Последние n байт.
        (true, _, Some(tail)) => {
            if tail == 0 || len == 0 {
                return RangeRequest::Unsatisfiable;
            }
            RangeRequest::Part(ByteRange {
                start: len.saturating_sub(tail),
                end: len - 1,
            })
        }
        (false, Some(start), end) if last.is_empty() || end.is_some() => {
            if start >= len {
                return RangeRequest::Unsatisfiable;
            }
            let end = end.map_or(len - 1, |e| e.min(len - 1));
            if end < start {
                return RangeRequest::Whole;
            }
            RangeRequest::Part(ByteRange { start, end })
        }
        _ => RangeRequest::Whole,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(start: u64, end: u64) -> RangeRequest {
        RangeRequest::Part(ByteRange { start, end })
    }

    #[test]
    fn one_range_in_each_form_is_understood_and_clipped_to_the_file() {
        assert_eq!(parse_range(Some("bytes=0-9"), 100), part(0, 9));
        assert_eq!(parse_range(Some("bytes=90-"), 100), part(90, 99));
        assert_eq!(parse_range(Some("bytes=90-500"), 100), part(90, 99));
        assert_eq!(parse_range(Some("bytes=-10"), 100), part(90, 99));
        assert_eq!(parse_range(Some("bytes=-500"), 100), part(0, 99));
        assert_eq!(ByteRange { start: 10, end: 19 }.byte_count(), 10);
    }

    #[test]
    fn a_range_past_the_end_is_unsatisfiable() {
        assert_eq!(
            parse_range(Some("bytes=100-"), 100),
            RangeRequest::Unsatisfiable
        );
        assert_eq!(
            parse_range(Some("bytes=-0"), 100),
            RangeRequest::Unsatisfiable
        );
        assert_eq!(
            parse_range(Some("bytes=0-"), 0),
            RangeRequest::Unsatisfiable
        );
    }

    #[test]
    fn anything_else_gets_the_whole_file() {
        for header in [
            None,
            Some("items=0-9"),
            Some("bytes=0-9,20-29"),
            Some("bytes=9-0"),
            Some("bytes=a-9"),
            Some("bytes=+1-9"),
            Some("bytes=-"),
            Some("bytes=0-9x"),
            Some("bytes=99999999999999999999999-"),
        ] {
            assert_eq!(parse_range(header, 100), RangeRequest::Whole, "{header:?}");
        }
    }
}
