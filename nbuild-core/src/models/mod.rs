//! Models to reason about the cargo inputs and the nix outputs

use std::{cell::RefCell, collections::BTreeMap, path::PathBuf, rc::Rc};

use cargo_lock::Version;
use tracing::{instrument, trace};

pub mod cargo;
pub mod nix;

/// Where does the crate's code come from
#[derive(Debug, PartialEq, Clone)]
pub enum Source {
    /// It is a local path
    ///
    /// ```toml
    /// [dependencies]
    /// dependency = { path = "/local/path" }
    /// ```
    Local(PathBuf),

    /// It is from crates.io
    ///
    /// ```toml
    /// [dependencies]
    /// dependency = "0.2.0"
    /// ```
    CratesIo(String),
}

/// Convert the cargo package to a nix package for output
impl From<cargo::Package> for nix::Package {
    fn from(package: cargo::Package) -> Self {
        let mut converted = Default::default();

        let result = cargo_to_nix(package, &mut converted);

        // Drop what was converted so that we can unwrap from the Rc
        drop(converted);

        Rc::try_unwrap(result).unwrap().into_inner()
    }
}

/// Recursively convert a cargo package to a nix package. Also ensure a crate is only converted once by using the
/// `converted` cache to lookup crates that have already been converted.
#[instrument(skip_all, fields(name = %cargo_package.name))]
fn cargo_to_nix(
    cargo_package: cargo::Package,
    converted: &mut BTreeMap<(String, Version), Rc<RefCell<nix::Package>>>,
) -> Rc<RefCell<nix::Package>> {
    let cargo::Package {
        name,
        lib_name,
        version,
        source,
        lib_path,
        build_path,
        proc_macro,
        features: _, // We only care about the features that were enabled at the end
        enabled_features,
        dependencies,
        build_dependencies,
        edition,
    } = cargo_package;

    match converted.get(&(name.clone(), version.clone())) {
        Some(package) => Rc::clone(package),
        None => {
            let dependencies = dependencies
                .iter()
                .filter(|d| !d.optional)
                .map(|dependency| convert_dependency(dependency, converted))
                .collect();
            let build_dependencies = build_dependencies
                .iter()
                .filter(|d| !d.optional)
                .map(|dependency| convert_dependency(dependency, converted))
                .collect();

            // Handle libs that rename themselves
            let lib_name = lib_name.and_then(|n| if n == name { None } else { Some(n) });

            // Handle libs with a custom `lib.rs` paths
            let lib_path = lib_path.and_then(|p| if p == "src/lib.rs" { None } else { Some(p) });

            // Handle custom `build.rs` paths
            let build_path = build_path.and_then(|p| if p == "build.rs" { None } else { Some(p) });

            // The features array needs to stay deterministic to prevent unneeded rebuilds, so we sort it
            let mut features = enabled_features.into_iter().collect::<Vec<_>>();
            features.sort();

            let package = RefCell::new(nix::Package {
                name: name.clone(),
                version: version.clone(),
                source,
                lib_name,
                lib_path,
                build_path,
                proc_macro,
                features,
                dependencies,
                build_dependencies,
                edition,
                printed: false,
            })
            .into();

            converted.insert((name, version), Rc::clone(&package));

            package
        }
    }
}

fn convert_dependency(
    dependency: &cargo::Dependency,
    converted: &mut BTreeMap<(String, Version), Rc<RefCell<nix::Package>>>,
) -> nix::Dependency {
    let cargo_package = Rc::clone(&dependency.package).borrow().clone();
    let package = cargo_to_nix(cargo_package, converted);

    let rename = if dependency.name == package.borrow().name {
        None
    } else {
        trace!(dependency_name = dependency.name, "activating rename");

        Some(dependency.name.to_string())
    };

    nix::Dependency { package, rename }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        collections::{HashMap, HashSet},
        path::PathBuf,
        rc::Rc,
        str::FromStr,
    };

    use crate::models::{cargo, nix};

    use pretty_assertions::assert_eq;

    #[test]
    fn cargo_to_nix() {
        let workspace = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("workspace");
        let path = workspace.join("parent");

        let libc = RefCell::new(cargo::Package {
            name: "libc".to_string(),
            version: "0.2.144".parse().unwrap(),
            source: "libc_sha".into(),
            lib_name: Some("libc".to_string()),
            lib_path: Some("src/lib.rs".into()),
            build_path: Some("build.rs".into()),
            proc_macro: false,
            dependencies: Default::default(),
            build_dependencies: Default::default(),
            features: HashMap::from([
                ("std".to_string(), vec![]),
                ("default".to_string(), vec!["std".to_string()]),
                ("use_std".to_string(), vec!["std".to_string()]),
                ("extra_traits".to_string(), vec![]),
                ("align".to_string(), vec![]),
                (
                    "rustc-dep-of-std".to_string(),
                    vec!["align".to_string(), "rustc-std-workspace-core".to_string()],
                ),
                ("const-extern-fn".to_string(), vec![]),
                (
                    "rustc-std-workspace-core".to_string(),
                    vec!["dep:rustc-std-workspace-core".to_string()],
                ),
            ]),
            enabled_features: Default::default(),
            edition: "2015".to_string(),
        })
        .into();
        let optional = RefCell::new(cargo::Package {
            name: "optional".to_string(),
            version: "1.0.0".parse().unwrap(),
            source: "optional_sha".into(),
            lib_name: Some("optional".to_string()),
            lib_path: Some("src/lib.rs".into()),
            build_path: None,
            proc_macro: false,
            dependencies: Default::default(),
            build_dependencies: Default::default(),
            features: HashMap::from([
                ("std".to_string(), vec![]),
                ("default".to_string(), vec!["std".to_string()]),
            ]),
            enabled_features: Default::default(),
            edition: "2021".to_string(),
        })
        .into();

        let input = cargo::Package {
            name: "parent".to_string(),
            lib_name: None,
            version: "0.1.0".parse().unwrap(),
            source: path.clone().into(),
            lib_path: None,
            build_path: None,
            proc_macro: false,
            dependencies: vec![
                cargo::Dependency {
                    name: "child".to_string(),
                    package: RefCell::new(cargo::Package {
                        name: "child".to_string(),
                        version: "0.1.0".parse().unwrap(),
                        source: workspace.join("child").into(),
                        lib_name: Some("child".to_string()),
                        lib_path: Some("src/lib.rs".into()),
                        build_path: None,
                        proc_macro: false,
                        dependencies: vec![
                            cargo::Dependency {
                                name: "fnv".to_string(),
                                package: RefCell::new(cargo::Package {
                                    name: "fnv".to_string(),
                                    version: "1.0.7".parse().unwrap(),
                                    source: "fnv_sha".into(),
                                    lib_name: Some("fnv".to_string()),
                                    lib_path: Some("lib.rs".into()),
                                    build_path: None,
                                    proc_macro: false,
                                    dependencies: Default::default(),
                                    build_dependencies: Default::default(),
                                    features: HashMap::from([
                                        ("default".to_string(), vec!["std".to_string()]),
                                        ("std".to_string(), vec![]),
                                    ]),
                                    enabled_features: Default::default(),
                                    edition: "2015".to_string(),
                                })
                                .into(),
                                optional: false,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                            cargo::Dependency {
                                name: "itoa".to_string(),
                                package: RefCell::new(cargo::Package {
                                    name: "itoa".to_string(),
                                    version: "1.0.6".parse().unwrap(),
                                    source: "itoa_sha".into(),
                                    lib_name: Some("itoa".to_string()),
                                    lib_path: Some("src/lib.rs".into()),
                                    build_path: None,
                                    proc_macro: false,
                                    dependencies: Default::default(),
                                    build_dependencies: Default::default(),
                                    features: HashMap::from([(
                                        "no-panic".to_string(),
                                        vec!["dep:no-panic".to_string()],
                                    )]),
                                    enabled_features: Default::default(),
                                    edition: "2018".to_string(),
                                })
                                .into(),
                                optional: false,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                            cargo::Dependency {
                                name: "libc".to_string(),
                                package: Rc::clone(&libc),
                                optional: false,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                            cargo::Dependency {
                                name: "optional".to_string(),
                                package: Rc::clone(&optional),
                                optional: true,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                            cargo::Dependency {
                                name: "new_name".to_string(),
                                package: RefCell::new(cargo::Package {
                                    name: "rename".to_string(),
                                    version: "0.1.0".parse().unwrap(),
                                    source: workspace.join("rename").into(),
                                    lib_name: Some("lib_rename".to_string()),
                                    lib_path: Some("src/lib.rs".into()),
                                    build_path: None,
                                    proc_macro: false,
                                    dependencies: Default::default(),
                                    build_dependencies: Default::default(),
                                    features: Default::default(),
                                    enabled_features: Default::default(),
                                    edition: "2021".to_string(),
                                })
                                .into(),
                                optional: false,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                            cargo::Dependency {
                                name: "rustversion".to_string(),
                                package: RefCell::new(cargo::Package {
                                    name: "rustversion".to_string(),
                                    version: "1.0.12".parse().unwrap(),
                                    source: "rustversion_sha".into(),
                                    lib_name: Some("rustversion".to_string()),
                                    lib_path: Some("src/lib.rs".into()),
                                    build_path: Some("build/build.rs".into()),
                                    proc_macro: true,
                                    dependencies: Default::default(),
                                    build_dependencies: Default::default(),
                                    features: Default::default(),
                                    enabled_features: Default::default(),
                                    edition: "2018".to_string(),
                                })
                                .into(),
                                optional: false,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                        ],
                        build_dependencies: vec![cargo::Dependency {
                            name: "arbitrary".to_string(),
                            package: RefCell::new(cargo::Package {
                                name: "arbitrary".to_string(),
                                version: "1.3.0".parse().unwrap(),
                                source: "arbitrary_sha".into(),
                                lib_name: Some("arbitrary".to_string()),
                                lib_path: Some("src/lib.rs".into()),
                                build_path: None,
                                proc_macro: false,
                                dependencies: Default::default(),
                                build_dependencies: Default::default(),
                                features: HashMap::from([
                                    ("derive".to_string(), vec!["derive_arbitrary".to_string()]),
                                    (
                                        "derive_arbitrary".to_string(),
                                        vec!["dep:derive_arbitrary".to_string()],
                                    ),
                                ]),
                                enabled_features: Default::default(),
                                edition: "2018".to_string(),
                            })
                            .into(),
                            optional: false,
                            uses_default_features: true,
                            features: Default::default(),
                        }],
                        features: HashMap::from([
                            (
                                "default".to_string(),
                                vec!["one".to_string(), "two".to_string()],
                            ),
                            ("one".to_string(), vec!["new_name".to_string()]),
                            ("two".to_string(), vec![]),
                            ("new_name".to_string(), vec!["dep:new_name".to_string()]),
                        ]),
                        enabled_features: HashSet::from([
                            "one".to_string(),
                            "new_name".to_string(),
                        ]),
                        edition: "2021".to_string(),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: false,
                    features: vec!["one".to_string()],
                },
                cargo::Dependency {
                    name: "itoa".to_string(),
                    package: RefCell::new(cargo::Package {
                        name: "itoa".to_string(),
                        version: "0.4.8".parse().unwrap(),
                        source: "itoa_sha".into(),
                        lib_name: Some("itoa".to_string()),
                        lib_path: Some("src/lib.rs".into()),
                        build_path: None,
                        proc_macro: false,
                        dependencies: Default::default(),
                        build_dependencies: Default::default(),
                        features: HashMap::from([
                            ("default".to_string(), vec!["std".to_string()]),
                            ("no-panic".to_string(), vec!["dep:no-panic".to_string()]),
                            ("std".to_string(), vec![]),
                            ("i128".to_string(), vec![]),
                        ]),
                        enabled_features: Default::default(),
                        edition: "2018".to_string(),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: true,
                    features: Default::default(),
                },
                cargo::Dependency {
                    name: "libc".to_string(),
                    package: libc,
                    optional: false,
                    uses_default_features: true,
                    features: Default::default(),
                },
                cargo::Dependency {
                    name: "optional".to_string(),
                    package: optional,
                    optional: true,
                    uses_default_features: true,
                    features: Default::default(),
                },
                cargo::Dependency {
                    name: "targets".to_string(),
                    package: RefCell::new(cargo::Package {
                        name: "targets".to_string(),
                        version: "0.1.0".parse().unwrap(),
                        source: workspace.join("targets").into(),
                        lib_name: Some("targets".to_string()),
                        lib_path: Some("src/lib.rs".into()),
                        build_path: None,
                        proc_macro: false,
                        dependencies: Default::default(),
                        build_dependencies: Default::default(),
                        features: HashMap::from([
                            ("unix".to_string(), vec![]),
                            ("windows".to_string(), vec![]),
                        ]),
                        enabled_features: HashSet::from(["unix".to_string()]),
                        edition: "2021".to_string(),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: true,
                    features: vec!["unix".to_string()],
                },
            ],
            build_dependencies: Default::default(),
            features: Default::default(),
            enabled_features: Default::default(),
            edition: "2021".to_string(),
        };

        let actual: nix::Package = input.into();

        let libc = RefCell::new(nix::Package {
            name: "libc".to_string(),
            version: "0.2.144".parse().unwrap(),
            source: "libc_sha".into(),
            lib_name: None,
            lib_path: None,
            build_path: None,
            proc_macro: false,
            dependencies: Default::default(),
            build_dependencies: Default::default(),
            features: Default::default(),
            edition: "2015".to_string(),
            printed: false,
        })
        .into();
        let expected = nix::Package {
            name: "parent".to_string(),
            version: "0.1.0".parse().unwrap(),
            source: path.into(),
            lib_name: None,
            lib_path: None,
            build_path: None,
            proc_macro: false,
            dependencies: vec![
                nix::Package {
                    name: "child".to_string(),
                    version: "0.1.0".parse().unwrap(),
                    source: workspace.join("child").into(),
                    lib_name: None,
                    lib_path: None,
                    build_path: None,
                    proc_macro: false,
                    dependencies: vec![
                        nix::Package {
                            name: "fnv".to_string(),
                            version: "1.0.7".parse().unwrap(),
                            source: "fnv_sha".into(),
                            lib_name: None,
                            lib_path: Some("lib.rs".into()),
                            build_path: None,
                            proc_macro: false,
                            dependencies: Default::default(),
                            build_dependencies: Default::default(),
                            features: Default::default(),
                            edition: "2015".to_string(),
                            printed: false,
                        }
                        .into(),
                        nix::Package {
                            name: "itoa".to_string(),
                            version: "1.0.6".parse().unwrap(),
                            source: "itoa_sha".into(),
                            lib_name: None,
                            lib_path: None,
                            build_path: None,
                            proc_macro: false,
                            dependencies: Default::default(),
                            build_dependencies: Default::default(),
                            features: Default::default(),
                            edition: "2018".to_string(),
                            printed: false,
                        }
                        .into(),
                        nix::Dependency {
                            package: Rc::clone(&libc),
                            rename: None,
                        },
                        nix::Dependency {
                            package: RefCell::new(nix::Package {
                                name: "rename".to_string(),
                                version: "0.1.0".parse().unwrap(),
                                source: workspace.join("rename").into(),
                                lib_name: Some("lib_rename".to_string()),
                                lib_path: None,
                                build_path: None,
                                proc_macro: false,
                                dependencies: Default::default(),
                                build_dependencies: Default::default(),
                                features: Default::default(),
                                edition: "2021".to_string(),
                                printed: false,
                            })
                            .into(),
                            rename: Some("new_name".to_string()),
                        },
                        nix::Package {
                            name: "rustversion".to_string(),
                            version: "1.0.12".parse().unwrap(),
                            source: "rustversion_sha".into(),
                            lib_name: None,
                            lib_path: None,
                            build_path: Some("build/build.rs".into()),
                            proc_macro: true,
                            dependencies: Default::default(),
                            build_dependencies: Default::default(),
                            features: Default::default(),
                            edition: "2018".to_string(),
                            printed: false,
                        }
                        .into(),
                    ],
                    build_dependencies: vec![nix::Package {
                        name: "arbitrary".to_string(),
                        version: "1.3.0".parse().unwrap(),
                        source: "arbitrary_sha".into(),
                        lib_name: None,
                        lib_path: None,
                        build_path: None,
                        proc_macro: false,
                        dependencies: Default::default(),
                        build_dependencies: Default::default(),
                        features: Default::default(),
                        edition: "2018".to_string(),
                        printed: false,
                    }
                    .into()],
                    features: vec!["new_name".to_string(), "one".to_string()],
                    edition: "2021".to_string(),
                    printed: false,
                }
                .into(),
                nix::Package {
                    name: "itoa".to_string(),
                    version: "0.4.8".parse().unwrap(),
                    source: "itoa_sha".into(),
                    lib_name: None,
                    lib_path: None,
                    build_path: None,
                    proc_macro: false,
                    dependencies: Default::default(),
                    build_dependencies: Default::default(),
                    features: Default::default(),
                    edition: "2018".to_string(),
                    printed: false,
                }
                .into(),
                nix::Dependency {
                    package: libc,
                    rename: None,
                },
                nix::Package {
                    name: "targets".to_string(),
                    version: "0.1.0".parse().unwrap(),
                    source: workspace.join("targets").into(),
                    lib_name: None,
                    lib_path: None,
                    build_path: None,
                    proc_macro: false,
                    dependencies: Default::default(),
                    build_dependencies: Default::default(),
                    features: vec!["unix".to_string()],
                    edition: "2021".to_string(),
                    printed: false,
                }
                .into(),
            ],
            build_dependencies: Default::default(),
            features: Default::default(),
            edition: "2021".to_string(),
            printed: false,
        };

        assert_eq!(actual, expected);

        // Make sure the libcs are linked - ie, the version for both libcs should change with this one assignment
        actual.dependencies[2].package.borrow_mut().version = "0.2.0".parse().unwrap();

        assert_eq!(
            actual.dependencies[2],
            actual.dependencies[0].package.borrow().dependencies[2]
        );
    }
}
