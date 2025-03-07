use super::ParsedManifest;

/// All local dependencies are emptied out when running `prepare`.
/// We do not want the recipe file to change if the only difference with
/// the previous docker build attempt is the version of a local crate
/// encoded in `Cargo.lock` (while the remote dependency tree
/// is unchanged) or in the corresponding `Cargo.toml` manifest.
/// We replace versions of local crates in `Cargo.lock` and in all `Cargo.toml`s, including
/// when specified as dependency of another crate in the workspace.
pub(super) fn mask_local_crate_versions(
    manifests: &mut [ParsedManifest],
    lock_file: &mut Option<toml::Value>,
) {
    let local_package_names = parse_local_crate_names(manifests);
    println!("{:?}", local_package_names);
    mask_local_versions_in_manifests(manifests, &local_package_names);
    if let Some(l) = lock_file {
        mask_local_versions_in_lockfile(l, &local_package_names);
    }
}

/// Dummy version used for all local crates.
const CONST_VERSION: &str = "0.0.1";

fn mask_local_versions_in_lockfile(lock_file: &mut toml::Value, local_package_names: &[Package]) {
    if let Some(packages) = lock_file
        .get_mut("package")
        .and_then(|packages| packages.as_array_mut())
    {
        packages
            .iter_mut()
            // Find all local crates
            .filter(|package| {
                let Some(name) = package.get("name") else { return false };
                let Some(version) = package.get("version") else { return false };

                local_package_names.iter().any(|package| {
                    &toml::Value::String(package.name.clone()) == name
                        && covers(&package.version, version.as_str().unwrap())
                })
            })
            // Mask the version
            .for_each(|package| {
                if let Some(version) = package.get_mut("version") {
                    *version = toml::Value::String(CONST_VERSION.to_string())
                }
                if let Some(toml::Value::Array(dependencies)) = package.get_mut("dependencies") {
                    let dependency_strings: Vec<_> = local_package_names
                        .iter()
                        .map(|package| format!("{} {}", package.name, package.version))
                        .collect();
                    for dependency in dependencies {
                        if dependency_strings.contains(&dependency.as_str().unwrap().to_string()) {
                            *dependency = toml::Value::String(format!(
                                "{} {}",
                                dependency.as_str().unwrap().split_once(' ').unwrap().0,
                                CONST_VERSION
                            ));
                        }
                    }
                }
                println!("{}", package);
            });
    }
}

fn mask_local_versions_in_manifests(
    manifests: &mut [ParsedManifest],
    local_package_names: &[Package],
) {
    for manifest in manifests.iter_mut() {
        if let Some(package) = manifest.contents.get_mut("package") {
            if let Some(version) = package.get_mut("version") {
                if version.as_str().is_some() {
                    *version = toml::Value::String(CONST_VERSION.to_string());
                }
            }
        }
        mask_local_dependency_versions(local_package_names, manifest);
    }
}

fn mask_local_dependency_versions(local_package_names: &[Package], manifest: &mut ParsedManifest) {
    fn _mask(local_package_names: &[Package], toml_value: &mut toml::Value) {
        for dependency_key in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(dependencies) = toml_value.get_mut(dependency_key) {
                if let Some(dependencies) = dependencies.as_table_mut() {
                    for (key, dependency) in dependencies {
                        let package_name = dependency
                            .get("package")
                            .cloned()
                            .unwrap_or(toml::Value::String(key.to_string()));

                        if let Some(version) = dependency.get_mut("version") {
                            if local_package_names.iter().any(|local| {
                                package_name == toml::Value::String(local.name.clone())
                                    && covers(&local.version, version.as_str().unwrap())
                            }) {
                                *version = toml::Value::String(CONST_VERSION.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // There are three ways to specify dependencies:
    // - top-level
    // ```toml
    // [dependencies]
    // # [...]
    // ```
    // - target-specific (e.g. Windows-only)
    // ```toml
    // [target.'cfg(windows)'.dependencies]
    // winhttp = "0.4.0"
    // ```
    // The inner structure for target-specific dependencies mirrors the structure expected
    // for top-level dependencies.
    // Check out cargo's documentation (https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html)
    // for more details.
    _mask(local_package_names, &mut manifest.contents);
    if let Some(targets) = manifest.contents.get_mut("target") {
        if let Some(target_table) = targets.as_table_mut() {
            for (_, target_config) in target_table.iter_mut() {
                _mask(local_package_names, target_config)
            }
        }
    }

    // The third way to specify dependencies was introduced in rust 1.64: workspace inheritance.
    // ```toml
    // [workspace.dependencies]
    // anyhow = "1.0.66"
    // project_a = { path = "project_a", version = "0.2.0" }
    // ```
    // Check out cargo's documentation (https://doc.rust-lang.org/cargo/reference/workspaces.html#the-workspacedependencies-table)
    // for more details.
    if let Some(workspace) = manifest.contents.get_mut("workspace") {
        // Mask the workspace package version
        if let Some(package) = workspace.get_mut("package") {
            if let Some(version) = package.get_mut("version") {
                *version = toml::Value::String(CONST_VERSION.to_string());
            }
        }
        // Mask the local crates in the workspace dependencies
        _mask(local_package_names, workspace);
    }
}

fn parse_local_crate_names(manifests: &[ParsedManifest]) -> Vec<Package> {
    let mut local_package_names = vec![];
    for manifest in manifests.iter() {
        if let Some(package) = manifest.contents.get("package") {
            if let (Some(toml::Value::String(name)), Some(toml::Value::String(version))) =
                (package.get("name"), package.get("version"))
            {
                local_package_names.push(Package {
                    name: name.clone(),
                    version: version.clone(),
                });
            }
        }
    }
    local_package_names
}

#[derive(Debug, PartialEq)]
struct Package {
    pub name: String,
    pub version: String,
}

fn covers(first: &str, second: &str) -> bool {
    if second == "*" {
        return true;
    }
    println!("VERSIONS: `{}` `{}`", first, second);
    // fn covers(first: &toml::Value, second: &toml::Value) -> bool {
    // let first = first.as_str().unwrap();
    // let second = second.as_str().unwrap();
    let mut splits = first.split('.');
    let first_major: u32 = splits.next().unwrap().parse().unwrap();
    let first_minor: u32 = splits.next().unwrap().parse().unwrap();
    let mut splits = second.split('.');
    let second_major: u32 = splits.next().unwrap().parse().unwrap();
    let second_minor: u32 = splits.next().unwrap().parse().unwrap();

    if first_major != second_major {
        return false;
    }

    if first_major != 0 {
        return true;
    }

    first_minor == second_minor
}
