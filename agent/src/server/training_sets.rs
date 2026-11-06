use std::path::Path;

/// Откуда берётся набор для обучения классификатора «ребёнок или взрослый».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainingSource {
    /// Свободный набор: его качает скрипт fetch-child-datasets.py из репозитория Nixyon в
    /// папку наборов обучения, имя набора совпадает с именем в скрипте.
    Download,
    /// Набор с условиями доступа: владелец кладёт его в корень наборов на сервере сам.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrainingSet {
    pub name: &'static str,
    pub source: TrainingSource,
    /// Флаг программы child_adult_eval, которым набор передаётся.
    pub flag: &'static str,
    /// Папка набора от корня своего источника.
    pub dir: &'static str,
}

/// Без него обучать не на чем: в нём train и test с детьми и взрослыми одной записи.
pub const REQUIRED_TRAINING_SET: &str = "speechocean762";

pub const TRAINING_SETS: [TrainingSet; 5] = [
    TrainingSet {
        name: "speechocean762",
        source: TrainingSource::Download,
        flag: "--speechocean",
        dir: "speechocean762/speechocean762",
    },
    TrainingSet {
        name: "samromur_children",
        source: TrainingSource::Download,
        flag: "--samromur",
        dir: "samromur_children",
    },
    TrainingSet {
        name: "samromur",
        source: TrainingSource::Download,
        flag: "--samromur",
        dir: "samromur",
    },
    TrainingSet {
        name: "childmandarin",
        source: TrainingSource::Manual,
        flag: "--childmandarin",
        dir: "childmandarin",
    },
    TrainingSet {
        name: "sps_ru",
        source: TrainingSource::Manual,
        flag: "--sps",
        dir: "sps_ru",
    },
];

pub fn training_set(name: &str) -> Option<&'static TrainingSet> {
    TRAINING_SETS.iter().find(|s| s.name == name)
}

/// Наборы годятся: все известны, не повторяются, обязательный выбран, а наборы, которые
/// кладут вручную, лежат в корне наборов обычной папкой, без ссылок.
pub fn check_training(sets: &[String], datasets: &Path) -> Result<(), String> {
    if !sets.iter().any(|s| s == REQUIRED_TRAINING_SET) {
        return Err(format!(
            "без набора {REQUIRED_TRAINING_SET} обучать не на чем"
        ));
    }
    for (i, name) in sets.iter().enumerate() {
        let Some(set) = training_set(name) else {
            return Err("неизвестный набор".into());
        };
        if sets[..i].contains(name) {
            return Err(format!("набор {name} выбран дважды"));
        }
        if set.source == TrainingSource::Manual
            && !std::fs::symlink_metadata(datasets.join(set.dir)).is_ok_and(|m| m.is_dir())
        {
            return Err(format!("набора {name} нет на сервере"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "nixyon-agent-training-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn downloaded_sets_need_no_folder_and_the_required_set_must_be_there() {
        let dir = Dir::new("ok");
        assert_eq!(
            check_training(
                &names(&["speechocean762", "samromur_children", "samromur"]),
                &dir.0
            ),
            Ok(())
        );
        assert!(check_training(&names(&["samromur"]), &dir.0).is_err());
        assert!(check_training(&[], &dir.0).is_err());
    }

    #[test]
    fn unknown_or_repeated_sets_are_refused() {
        let dir = Dir::new("bad");
        assert!(check_training(&names(&["speechocean762", "../x"]), &dir.0).is_err());
        assert!(check_training(&names(&["speechocean762", "speechocean762"]), &dir.0).is_err());
    }

    #[test]
    fn a_manual_set_must_lie_on_the_server_as_a_real_folder() {
        let dir = Dir::new("manual");
        let plan = names(&["speechocean762", "childmandarin"]);
        assert!(check_training(&plan, &dir.0).is_err());
        std::os::unix::fs::symlink("/tmp", dir.0.join("childmandarin")).unwrap();
        assert!(check_training(&plan, &dir.0).is_err(), "ссылка не годится");
        std::fs::remove_file(dir.0.join("childmandarin")).unwrap();
        std::fs::create_dir(dir.0.join("childmandarin")).unwrap();
        assert_eq!(check_training(&plan, &dir.0), Ok(()));
    }
}
