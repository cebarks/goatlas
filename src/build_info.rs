use crate::report::*;

pub(crate) fn extract_build_info(
    bin: &unstrip::GoBinary,
) -> Result<(String, BuildIdentity), Issue> {
    let raw = unstrip::BuildInfo::parse(bin).map_err(|error| Issue {
        code: "build_info_unavailable".into(),
        message: error.to_string(),
        function: None,
        entry: None,
    })?;
    Ok((raw.go_version.clone(), identity_from(&raw)))
}

fn identity_from(raw: &unstrip::BuildInfo) -> BuildIdentity {
    let mut identity = BuildIdentity {
        path: raw.path.clone(),
        main: raw.main.as_ref().map(module_from),
        dependencies: raw.deps.iter().map(module_from).collect(),
        replacements: raw
            .replaces
            .iter()
            .map(|replacement| Replacement {
                from: module_from(&replacement.from),
                to: module_from(&replacement.to),
            })
            .collect(),
        settings: raw
            .settings
            .iter()
            .map(|setting| BuildSetting {
                key: setting.key.clone(),
                value: setting.value.clone(),
            })
            .collect(),
    };
    identity.dependencies.sort();
    identity.replacements.sort();
    identity.settings.sort();
    identity
}

fn module_from(raw: &unstrip::Module) -> ModuleRecord {
    ModuleRecord {
        path: raw.path.clone(),
        version: raw.version.clone(),
        sum: raw.sum.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unstrip::buildinfo::{Module, Replace, Setting};

    fn module(path: &str, version: &str) -> Module {
        Module {
            path: path.into(),
            version: version.into(),
            sum: Some("h1:recorded".into()),
        }
    }

    #[test]
    fn embedded_settings_and_development_versions_are_preserved() {
        for version in ["", "(devel)", "unknown"] {
            let raw = unstrip::BuildInfo {
                go_version: "go-recorded".into(),
                path: Some("embedded/path".into()),
                main: Some(module("main", version)),
                deps: vec![],
                replaces: vec![],
                settings: vec![Setting {
                    key: "vcs.time".into(),
                    value: "2026-01-02T03:04:05Z".into(),
                }],
            };
            let mapped = identity_from(&raw);
            assert_eq!(mapped.main.unwrap().version, version);
            assert_eq!(mapped.settings[0].value, "2026-01-02T03:04:05Z");
            assert_eq!(mapped.path.as_deref(), Some("embedded/path"));
        }
    }

    #[test]
    fn identity_sorts_without_deduplicating_or_merging_replacements() {
        let raw = unstrip::BuildInfo {
            go_version: String::new(),
            path: None,
            main: None,
            deps: vec![module("z", "unknown"), module("a", ""), module("a", "")],
            replaces: vec![Replace {
                from: module("z", "unknown"),
                to: module("../local", "(devel)"),
            }],
            settings: vec![
                Setting {
                    key: "z".into(),
                    value: "2".into(),
                },
                Setting {
                    key: "a".into(),
                    value: "1".into(),
                },
            ],
        };
        let mapped = identity_from(&raw);
        assert_eq!(mapped.dependencies.len(), 3);
        assert_eq!(mapped.dependencies[0].path, "a");
        assert_eq!(mapped.dependencies[1].sum.as_deref(), Some("h1:recorded"));
        assert_eq!(mapped.replacements[0].from.version, "unknown");
        assert_eq!(mapped.replacements[0].to.path, "../local");
        assert_eq!(mapped.settings[0].key, "a");
    }
}
