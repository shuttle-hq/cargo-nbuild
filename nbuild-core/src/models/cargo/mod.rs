use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
};

use cargo_lock::{Lockfile, Version};
use cargo_metadata::{camino::Utf8PathBuf, DependencyKind, MetadataCommand, PackageId};
use target_spec::{Platform, TargetSpec};
use tracing::{instrument, trace};

use super::Source;

mod visitor;

#[derive(Debug, PartialEq, Clone)]
pub struct Package {
    pub(super) name: String,
    pub(super) version: Version,
    pub(super) source: Source,
    pub(super) lib_path: Option<Utf8PathBuf>,
    pub(super) build_path: Option<Utf8PathBuf>,
    pub(super) proc_macro: bool,
    pub(super) features: HashMap<String, Vec<String>>,
    pub(super) enabled_features: HashSet<String>,
    pub(super) dependencies: Vec<Dependency>,
    pub(super) build_dependencies: Vec<Dependency>,
    pub(super) edition: String,
}

#[derive(Debug, PartialEq, Clone)]
pub struct Dependency {
    pub(super) name: String,
    pub(super) package: Rc<RefCell<Package>>,
    pub(super) optional: bool,
    pub(super) uses_default_features: bool,
    pub(super) features: Vec<String>,
}

impl Package {
    pub fn from_current_dir(path: impl Into<PathBuf>) -> Self {
        let platform = Platform::current().unwrap();

        let metadata = MetadataCommand::new()
            .current_dir(path)
            .other_options(vec![
                "--filter-platform".to_string(),
                platform.triple_str().to_string(),
            ])
            .exec()
            .unwrap();
        let lock_file = metadata.workspace_root.join("Cargo.lock");
        let lock_file = Lockfile::load(lock_file).unwrap();

        trace!(?platform, ?metadata, ?lock_file, "have metadata");

        let packages =
            BTreeMap::from_iter(metadata.packages.iter().map(|p| (p.id.clone(), p.clone())));
        let nodes = BTreeMap::from_iter(
            metadata
                .resolve
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .map(|n| (n.id.clone(), n.clone())),
        );
        let checksums = BTreeMap::from_iter(lock_file.packages.iter().filter_map(|p| {
            if let Some(checksum) = &p.checksum {
                Some((
                    (p.name.to_string(), p.version.to_string()),
                    checksum.to_string(),
                ))
            } else {
                None
            }
        }));

        let root_id = metadata
            .resolve
            .as_ref()
            .unwrap()
            .root
            .as_ref()
            .unwrap()
            .clone();

        let mut resolved_packages = Default::default();

        Self::get_package(
            root_id,
            &packages,
            &nodes,
            &checksums,
            &mut resolved_packages,
            &platform,
        )
    }

    fn get_package(
        id: PackageId,
        packages: &BTreeMap<PackageId, cargo_metadata::Package>,
        nodes: &BTreeMap<PackageId, cargo_metadata::Node>,
        checksums: &BTreeMap<(String, String), String>,
        resolved_packages: &mut BTreeMap<PackageId, Rc<RefCell<Package>>>,
        platform: &Platform,
    ) -> Self {
        let node = nodes.get(&id).unwrap().clone();
        let package = packages.get(&id).unwrap();

        trace!(
            package.name,
            ?package.features,
            ?node,
            "found package and node"
        );

        let features = package.features.clone();
        let package_dependencies = package
            .dependencies
            .iter()
            .filter(|d| d.kind == DependencyKind::Normal)
            .cloned()
            .collect();
        let package_build_dependencies = package
            .dependencies
            .iter()
            .filter(|d| d.kind == DependencyKind::Build)
            .cloned()
            .collect();

        let dependencies = node
            .dependencies
            .iter()
            .filter_map(|id| {
                Dependency::get_dependency(
                    id,
                    &package_dependencies,
                    packages,
                    nodes,
                    checksums,
                    resolved_packages,
                    platform,
                )
            })
            .collect();
        let build_dependencies = node
            .dependencies
            .iter()
            .filter_map(|id| {
                Dependency::get_dependency(
                    id,
                    &package_build_dependencies,
                    packages,
                    nodes,
                    checksums,
                    resolved_packages,
                    platform,
                )
            })
            .collect();

        let package_path: PathBuf = package.manifest_path.parent().unwrap().into();

        let lib_path = package
            .targets
            .iter()
            .find(|t| {
                t.kind.iter().any(|k| {
                    matches!(
                        k.as_str(),
                        "lib" | "cdylib" | "dylib" | "rlib" | "proc-macro"
                    )
                })
            })
            .map(|t| {
                t.src_path
                    .strip_prefix(&package_path)
                    .unwrap()
                    .to_path_buf()
            });
        let build_path = package
            .targets
            .iter()
            .find(|t| t.kind.iter().any(|k| k == "custom-build"))
            .map(|t| {
                t.src_path
                    .strip_prefix(&package_path)
                    .unwrap()
                    .to_path_buf()
            });
        let proc_macro = package
            .targets
            .iter()
            .any(|t| t.kind.iter().any(|k| k == "proc-macro"));

        let source = if package.source.is_some() {
            let checksum = checksums
                .get(&(package.name.to_string(), package.version.to_string()))
                .expect("to have a checksum");
            Source::CratesIo(checksum.to_string())
        } else {
            Source::Local(package_path)
        };

        Self {
            name: package.name.clone(),
            version: package.version.clone(),
            source,
            lib_path,
            build_path,
            proc_macro,
            dependencies,
            build_dependencies,
            features,
            enabled_features: Default::default(),
            edition: package.edition.to_string(),
        }
    }

    pub fn resolve(&mut self) {
        self.visit(&mut visitor::SetDefaultVisitor);
        self.visit(&mut visitor::EnableFeaturesVisitor);
        self.visit(&mut visitor::UnpackDefaultVisitor);
        self.visit(&mut visitor::UnpackChainVisitor);
        self.visit(&mut visitor::OptionalDependencyFeaturesVisitor);
    }

    fn visit(&mut self, visitor: &mut impl visitor::Visitor) {
        visitor.visit(self);
    }

    pub fn dependencies_iter(&self) -> impl Iterator<Item = &Dependency> {
        self.dependencies
            .iter()
            .chain(self.build_dependencies.iter())
    }

    pub fn dependencies_iter_mut(&mut self) -> impl Iterator<Item = &mut Dependency> {
        self.dependencies
            .iter_mut()
            .chain(self.build_dependencies.iter_mut())
    }
}

impl Dependency {
    #[instrument(skip_all, fields(%id))]
    fn get_dependency(
        id: &PackageId,
        parent_dependencies: &Vec<cargo_metadata::Dependency>,
        packages: &BTreeMap<PackageId, cargo_metadata::Package>,
        nodes: &BTreeMap<PackageId, cargo_metadata::Node>,
        checksums: &BTreeMap<(String, String), String>,
        resolved_packages: &mut BTreeMap<PackageId, Rc<RefCell<Package>>>,
        platform: &Platform,
    ) -> Option<Self> {
        // We eventually want only one representation of a package when we do feature resolution
        let package = match resolved_packages.get(id) {
            Some(package) => Rc::clone(package),
            None => {
                let package = RefCell::new(Package::get_package(
                    id.clone(),
                    packages,
                    nodes,
                    checksums,
                    resolved_packages,
                    platform,
                ))
                .into();

                resolved_packages.insert(id.clone(), Rc::clone(&package));

                package
            }
        };

        let name = package.borrow().name.clone();

        let dependencies: Vec<_> = parent_dependencies
            .iter()
            .filter(|d| d.name == name)
            .filter(|d| match &d.target {
                Some(target_spec) => {
                    let target_spec = TargetSpec::new(target_spec.to_string()).unwrap();

                    target_spec.eval(platform).unwrap_or(false)
                }
                None => true,
            })
            .collect();

        // It could happen that this kind of dependency is not part of the kind passed into this function,
        // in which case this dependency should not we considered as a real dependency.
        if dependencies.is_empty() {
            return None;
        }

        let mut optional = true;
        let mut uses_default_features = false;
        let mut features: Vec<String> = Default::default();
        let mut dependency_name: String = Default::default();
        let mut dependency_rename = None;

        for dependency in dependencies {
            if !dependency.optional {
                optional = false;
            }

            if dependency.uses_default_features {
                uses_default_features = true;
            }

            features.extend(dependency.features.iter().cloned());

            if dependency_rename.is_none() && dependency.rename.is_some() {
                dependency_rename = dependency.rename.clone();
            }

            if dependency_name.is_empty() {
                dependency_name = dependency.name.clone();
            }
        }

        if let Some(dependency_rename) = dependency_rename {
            dependency_name = dependency_rename;
        };

        trace!(
            name,
            dependency_name,
            optional,
            uses_default_features,
            ?features,
            "done with dependency"
        );

        Some(Self {
            name: dependency_name,
            package,
            optional,
            uses_default_features,
            features,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::HashMap, path::PathBuf, str::FromStr};

    use crate::models::cargo::{Dependency, Package};

    use pretty_assertions::assert_eq;

    #[test]
    fn simple_package() {
        let path = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("simple");

        let package = Package::from_current_dir(path.clone());

        assert_eq!(
            package,
            Package {
                name: "simple".to_string(),
                source: path.into(),
                lib_path: None,
                build_path: None,
                proc_macro: false,
                version: "0.1.0".parse().unwrap(),
                dependencies: vec![Dependency {
                    name: "itoa".to_string(),
                    package: RefCell::new(Package {
                        name: "itoa".to_string(),
                        version: "1.0.6".parse().unwrap(),
                        source: "453ad9f582a441959e5f0d088b02ce04cfe8d51a8eaf077f12ac6d3e94164ca6"
                            .into(),
                        lib_path: Some("src/lib.rs".into()),
                        build_path: None,
                        proc_macro: false,
                        dependencies: Default::default(),
                        build_dependencies: Default::default(),
                        features: HashMap::from([(
                            "no-panic".to_string(),
                            vec!["dep:no-panic".to_string()]
                        )]),
                        enabled_features: Default::default(),
                        edition: "2018".to_string(),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: true,
                    features: Default::default(),
                },],
                build_dependencies: vec![Dependency {
                    name: "arbitrary".to_string(),
                    package: RefCell::new(Package {
                        name: "arbitrary".to_string(),
                        version: "1.3.0".parse().unwrap(),
                        source: "e2d098ff73c1ca148721f37baad5ea6a465a13f9573aba8641fbbbae8164a54e"
                            .into(),
                        lib_path: Some("src/lib.rs".into()),
                        build_path: None,
                        proc_macro: false,
                        dependencies: Default::default(),
                        build_dependencies: Default::default(),
                        features: HashMap::from([
                            ("derive".to_string(), vec!["derive_arbitrary".to_string()]),
                            (
                                "derive_arbitrary".to_string(),
                                vec!["dep:derive_arbitrary".to_string()]
                            ),
                        ]),
                        enabled_features: Default::default(),
                        edition: "2018".to_string(),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: true,
                    features: Default::default(),
                },],
                features: Default::default(),
                enabled_features: Default::default(),
                edition: "2021".to_string(),
            }
        );
    }

    #[test]
    fn workspace() {
        let workspace = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("workspace");
        let path = workspace.join("parent");

        let package = Package::from_current_dir(path.clone());

        assert_eq!(
            package,
            Package {
                name: "parent".to_string(),
                version: "0.1.0".parse().unwrap(),
                source: path.into(),
                lib_path: None,
                build_path: None,
                proc_macro: false,
                dependencies: vec![
                    Dependency {
                        name: "child".to_string(),
                        package: RefCell::new(Package {
                            name: "child".to_string(),
                            version: "0.1.0".parse().unwrap(),
                            source: workspace.join("child").into(),
                            lib_path: Some("src/lib.rs".into()),
                            build_path: None,
                            proc_macro: false,
                            dependencies: vec![
                                Dependency {
                                    name: "fnv".to_string(),
                                    package: RefCell::new(Package {
                                        name: "fnv".to_string(),
                                        version: "1.0.7".parse().unwrap(),
                                        source: "3f9eec918d3f24069decb9af1554cad7c880e2da24a9afd88aca000531ab82c1".into(),
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
                                Dependency {
                                    name: "itoa".to_string(),
                                    package: RefCell::new(Package {
                                        name: "itoa".to_string(),
                                        version: "1.0.6".parse().unwrap(),
                                        source: "453ad9f582a441959e5f0d088b02ce04cfe8d51a8eaf077f12ac6d3e94164ca6".into(),
                                        lib_path: Some("src/lib.rs".into()),
                                        build_path: None,
                                        proc_macro: false,
                                        dependencies: Default::default(),
                                        build_dependencies: Default::default(),
                                        features: HashMap::from([(
                                            "no-panic".to_string(),
                                            vec!["dep:no-panic".to_string()]
                                        )]),
                                        enabled_features: Default::default(),
                                        edition: "2018".to_string(),
                                    })
                                    .into(),
                                    optional: false,
                                    uses_default_features: true,
                                    features: Default::default(),
                                },
                                Dependency {
                                    name: "libc".to_string(),
                                    package: RefCell::new(Package {
                                        name: "libc".to_string(),
                                        version: "0.2.144".parse().unwrap(),
                                        source: "2b00cc1c228a6782d0f076e7b232802e0c5689d41bb5df366f2a6b6621cfdfe1".into(),
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
                                                vec![
                                                    "align".to_string(),
                                                    "rustc-std-workspace-core".to_string()
                                                ]
                                            ),
                                            ("const-extern-fn".to_string(), vec![]),
                                            (
                                                "rustc-std-workspace-core".to_string(),
                                                vec!["dep:rustc-std-workspace-core".to_string()]
                                            ),
                                        ]),
                                        enabled_features: Default::default(),
                                        edition: "2015".to_string(),
                                    })
                                    .into(),
                                    optional: false,
                                    uses_default_features: true,
                                    features: Default::default(),
                                },
                                Dependency {
                                    name: "new_name".to_string(),
                                    package: RefCell::new(Package {
                                        name: "rename".to_string(),
                                        version: "0.1.0".parse().unwrap(),
                                        source: workspace.join("rename").into(),
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
                                    optional: true,
                                    uses_default_features: true,
                                    features: Default::default(),
                                },
                                Dependency {
                                    name: "rustversion".to_string(),
                                    package: RefCell::new(Package {
                                        name: "rustversion".to_string(),
                                        version: "1.0.12".parse().unwrap(),
                                        source: "4f3208ce4d8448b3f3e7d168a73f5e0c43a61e32930de3bceeccedb388b6bf06".into(),
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
                            build_dependencies: Default::default(),
                            features: HashMap::from([
                                (
                                    "default".to_string(),
                                    vec!["one".to_string(), "two".to_string()]
                                ),
                                ("one".to_string(), vec!["new_name".to_string()]),
                                ("two".to_string(), vec![]),
                                ("new_name".to_string(), vec!["dep:new_name".to_string()]),
                            ]),
                            enabled_features: Default::default(),
                            edition: "2021".to_string(),
                        })
                        .into(),
                        optional: false,
                        uses_default_features: false,
                        features: vec!["one".to_string()],
                    },
                    Dependency {
                        name: "itoa".to_string(),
                        package: RefCell::new(Package {
                            name: "itoa".to_string(),
                            version: "0.4.8".parse().unwrap(),
                            source: "b71991ff56294aa922b450139ee08b3bfc70982c6b2c7562771375cf73542dd4".into(),
                            lib_path: Some("src/lib.rs".into()),
                            build_path: None,
                            proc_macro: false,
                            dependencies: Default::default(),
                            build_dependencies: Default::default(),
                            features: HashMap::from([
                                ("default".to_string(), vec!["std".to_string()]),
                                ("std".to_string(), vec![]),
                                ("i128".to_string(), vec![]),
                            ]),
                            enabled_features: Default::default(),
                            edition: "2015".to_string(),
                        })
                        .into(),
                        optional: false,
                        uses_default_features: true,
                        features: Default::default(),
                    },
                    Dependency {
                        name: "libc".to_string(),
                        package: RefCell::new(Package {
                            name: "libc".to_string(),
                            version: "0.2.144".parse().unwrap(),
                            source: "2b00cc1c228a6782d0f076e7b232802e0c5689d41bb5df366f2a6b6621cfdfe1".into(),
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
                                    vec![
                                        "align".to_string(),
                                        "rustc-std-workspace-core".to_string()
                                    ]
                                ),
                                ("const-extern-fn".to_string(), vec![]),
                                (
                                    "rustc-std-workspace-core".to_string(),
                                    vec!["dep:rustc-std-workspace-core".to_string()]
                                ),
                            ]),
                            enabled_features: Default::default(),
                            edition: "2015".to_string(),
                        })
                        .into(),
                        optional: false,
                        uses_default_features: true,
                        features: Default::default(),
                    },
                    Dependency {
                        name: "targets".to_string(),
                        package: RefCell::new(Package {
                            name: "targets".to_string(),
                            version: "0.1.0".parse().unwrap(),
                            source: workspace.join("targets").into(),
                            lib_path: Some("src/lib.rs".into()),
                            build_path: None,
                            proc_macro: false,
                            dependencies: Default::default(),
                            build_dependencies: Default::default(),
                            features: HashMap::from([
                                ("unix".to_string(), vec![]),
                                ("windows".to_string(), vec![]),
                            ]),
                            enabled_features: Default::default(),
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
            }
        );
    }
}
