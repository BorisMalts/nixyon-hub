const MIB: f64 = 1024.0 * 1024.0;

/// Вес файла по-русски: «52,5 МБ», от 1024 МБ в гигабайтах, «2,6 ГБ». Мегабайт здесь
/// 1024 × 1024 байт, как в списке сборок.
pub fn size_label(bytes: u64) -> String {
    let mib = bytes as f64 / MIB;
    let (value, unit) = if mib >= 1024.0 {
        (mib / 1024.0, "ГБ")
    } else {
        (mib, "МБ")
    };
    let rounded = (value * 10.0).round() / 10.0;
    let number = if rounded.fract() == 0.0 {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.1}").replace('.', ",")
    };
    format!("{number} {unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn megabytes_and_gigabytes_with_a_comma() {
        assert_eq!(size_label(52_489_167), "50,1 МБ");
        assert_eq!(size_label(2_822_579_315), "2,6 ГБ");
        assert_eq!(size_label(3 * 1024 * 1024), "3 МБ");
        assert_eq!(size_label(1024 * 1024 * 1024), "1 ГБ");
        assert_eq!(size_label(0), "0 МБ");
    }
}
