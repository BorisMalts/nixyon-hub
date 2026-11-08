use nixyon_agent::protocol::Artifact;

// Лёгкую APK агент публикует под id полной с этим окончанием.
const LITE_SUFFIX: &str = "-lite";

/// Вид APK: полная с моделями внутри или лёгкая, которая качает модели при первом запуске.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApkKind {
    Full,
    Lite,
}

impl ApkKind {
    pub fn of(artifact: &Artifact) -> Self {
        if artifact.id.ends_with(LITE_SUFFIX) {
            Self::Lite
        } else {
            Self::Full
        }
    }

    /// Имя файла у скачавшего: одно и то же для любой сборки, без даты и коммита.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Full => "nixyon.apk",
            Self::Lite => "nixyon-lite.apk",
        }
    }
}

/// Самая новая APK этого вида. `items` идут от новых к старым, как их отдаёт
/// `artifacts::list`.
pub fn latest(items: &[Artifact], kind: ApkKind) -> Option<&Artifact> {
    items.iter().find(|a| ApkKind::of(a) == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(id: &str, created_at: i64) -> Artifact {
        Artifact {
            id: id.into(),
            file: format!("nixyon-{id}.apk"),
            commit: "abc1234".into(),
            subject: "Проверка".into(),
            created_at,
            size: 1,
            built_by: "andrey@example.org".into(),
        }
    }

    #[test]
    fn the_newest_of_each_kind_is_taken() {
        let items = [
            artifact("3-new-lite", 5),
            artifact("2-old", 4),
            artifact("2-old-lite", 3),
            artifact("1-older", 2),
        ];
        assert_eq!(latest(&items, ApkKind::Full).unwrap().id, "2-old");
        assert_eq!(latest(&items, ApkKind::Lite).unwrap().id, "3-new-lite");
    }

    #[test]
    fn a_missing_kind_gives_nothing() {
        let items = [artifact("1-only", 1)];
        assert!(latest(&items, ApkKind::Lite).is_none());
        assert!(latest(&[], ApkKind::Full).is_none());
    }

    #[test]
    fn file_names_carry_no_build_details() {
        assert_eq!(ApkKind::Full.file_name(), "nixyon.apk");
        assert_eq!(ApkKind::Lite.file_name(), "nixyon-lite.apk");
    }
}
