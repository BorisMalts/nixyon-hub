use crate::latest_apk::ApkKind;
use crate::size_label::size_label;

const TEMPLATE: &str = include_str!("../ui/apk.html");
const MARKER: &str = "<!-- варианты -->";

/// Адрес скачивания последней APK этого вида.
pub fn download_path(kind: ApkKind) -> &'static str {
    match kind {
        ApkKind::Full => "/apk/full",
        ApkKind::Lite => "/apk/lite",
    }
}

fn title(kind: ApkKind) -> &'static str {
    match kind {
        ApkKind::Full => "Полная",
        ApkKind::Lite => "Лёгкая",
    }
}

/// Страница скачивания: у каждого собранного вида название, вес и кнопка «Скачать».
/// В разметку попадают только свои строки и число, поэтому экранировать нечего.
pub fn render(variants: &[(ApkKind, u64)]) -> String {
    let body = if variants.is_empty() {
        "<p>Сборок пока нет</p>".to_string()
    } else {
        variants
            .iter()
            .map(|&(kind, size)| {
                format!(
                    "<div class=\"apk-variant\"><div><h2>{}</h2><p>{}</p></div>\
                     <a class=\"button button-primary\" href=\"{}\">Скачать</a></div>",
                    title(kind),
                    size_label(size),
                    download_path(kind)
                )
            })
            .collect()
    };
    TEMPLATE.replacen(MARKER, &body, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_kinds_get_a_title_a_size_and_a_link() {
        let page = render(&[(ApkKind::Full, 2_822_579_315), (ApkKind::Lite, 52_489_167)]);
        assert!(page.contains("<h2>Полная</h2><p>2,6 ГБ</p>"));
        assert!(page.contains("href=\"/apk/full\">Скачать"));
        assert!(page.contains("<h2>Лёгкая</h2><p>50,1 МБ</p>"));
        assert!(page.contains("href=\"/apk/lite\">Скачать"));
        assert!(!page.contains(MARKER));
    }

    #[test]
    fn no_builds_are_said_so() {
        let page = render(&[]);
        assert!(page.contains("Сборок пока нет"));
        assert!(!page.contains("Скачать"));
    }

    #[test]
    fn the_template_has_the_marker_once() {
        assert_eq!(TEMPLATE.matches(MARKER).count(), 1);
    }
}
