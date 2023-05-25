mod visitor;

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
};

use cargo_metadata::{camino::Utf8PathBuf, semver::Version, MetadataCommand, PackageId};
use visitor::{
    EnableFeaturesVisitor, NoDefaultsVisitor, SetDefaultVisitor, UnpackChainVisitor,
    UnpackDefaultVisitor, Visitor,
};

#[derive(Debug, PartialEq)]
pub struct Package {
    name: String,
    version: Version,
    src: Utf8PathBuf,
    features: Vec<String>,
    dependencies: Vec<Rc<RefCell<Package>>>,
    printed: bool,
}

impl Package {
    pub fn to_derivative(self) -> String {
        let Self {
            name,
            version,
            src,
            features: _,
            dependencies,
            printed: _,
        } = self;

        let mut build_details = Default::default();
        let dep_idents: Vec<_> = dependencies
            .into_iter()
            .map(|d| {
                let identifier = d.borrow().identifier();
                Self::to_details(d, &mut build_details);
                identifier
            })
            .collect();

        format!(
            r#"{{ pkgs ? import <nixpkgs> {{}} }}:

let
  # Core
  {} = pkgs.buildRustCrate rec {{
    crateName = "{}";
    version = "{}";

    src = {};

    dependencies = [
      {}
    ];
  }} ;

  # Dependencies
{}
in
{}
"#,
            name,
            name,
            version,
            src,
            dep_idents.join("\n      "),
            build_details.join("\n"),
            name
        )
    }

    fn to_details(dependency: Rc<RefCell<Self>>, build_details: &mut Vec<String>) {
        let mut this = dependency.borrow_mut();

        // Only print once
        if this.printed {
            return;
        }

        let features = if this.features.is_empty() {
            Default::default()
        } else {
            format!(
                "\n    features = [{}];",
                this.features
                    .iter()
                    .map(|f| format!("\"{f}\""))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };

        let deps = if this.dependencies.is_empty() {
            Default::default()
        } else {
            let dep_idents: Vec<_> = this
                .dependencies
                .iter()
                .map(|d| d.borrow().identifier())
                .collect();
            format!("\n    dependencies = [{}];", dep_idents.join(" "))
        };

        let details = format!(
            r#"  {} = pkgs.buildRustCrate rec {{
    crateName = "{}";
    version = "{}";

    src = {};{}{}
  }};"#,
            this.identifier(),
            this.name,
            this.version,
            this.src,
            deps,
            features
        );

        build_details.push(details);

        for dependency in this.dependencies.iter() {
            Self::to_details(Rc::clone(dependency), build_details);
        }

        this.printed = true;
    }

    fn identifier(&self) -> String {
        format!(
            "{}_{}",
            self.name,
            self.version.to_string().replace('.', "_").replace('+', "_")
        )
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct PackageNode {
    id: PackageId,
    name: String,
    version: Version,
    src: Utf8PathBuf,
    features: HashMap<String, Vec<String>>,
    enabled_features: HashSet<String>,
    dependencies: Vec<DependencyNode>,
}

#[derive(Debug, PartialEq, Clone)]
pub struct DependencyNode {
    package: Rc<RefCell<PackageNode>>,
    optional: bool,
    uses_default_features: bool,
    features: Vec<String>,
}

impl PackageNode {
    pub fn from_current_dir(path: impl Into<PathBuf>) -> Self {
        let metadata = MetadataCommand::new().current_dir(path).exec().unwrap();

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

        let root_id = metadata
            .resolve
            .as_ref()
            .unwrap()
            .root
            .as_ref()
            .unwrap()
            .clone();

        let mut resolved_packages = Default::default();

        Self::get_package(root_id, &packages, &nodes, &mut resolved_packages)
    }

    fn get_package(
        id: PackageId,
        packages: &BTreeMap<PackageId, cargo_metadata::Package>,
        nodes: &BTreeMap<PackageId, cargo_metadata::Node>,
        resolved_packages: &mut BTreeMap<PackageId, Rc<RefCell<PackageNode>>>,
    ) -> Self {
        let node = nodes.get(&id).unwrap().clone();
        let package = packages.get(&id).unwrap();

        let features = package.features.clone();
        let dependencies = node
            .dependencies
            .iter()
            .map(|id| {
                DependencyNode::get_dependency(id, package, packages, nodes, resolved_packages)
            })
            .collect();

        Self {
            id: id.clone(),
            name: package.name.clone(),
            version: package.version.clone(),
            src: package.manifest_path.parent().unwrap().into(),
            dependencies,
            features,
            enabled_features: Default::default(),
        }
    }

    pub fn into_package(self) -> Package {
        let mut converted = Default::default();

        let result = Self::convert_to_package(self, &mut converted);

        // Drop what was converted so that we can unwrap from the Rc
        drop(converted);

        Rc::try_unwrap(result).unwrap().into_inner()
    }

    fn convert_to_package(
        self,
        converted: &mut BTreeMap<PackageId, Rc<RefCell<Package>>>,
    ) -> Rc<RefCell<Package>> {
        let Self {
            id,
            name,
            version,
            src,
            features: _,
            enabled_features,
            dependencies,
        } = self;

        match converted.get(&id) {
            Some(package) => Rc::clone(package),
            None => {
                let dependencies = dependencies
                    .iter()
                    .map(|d| {
                        Rc::clone(&d.package)
                            .borrow()
                            .clone()
                            .convert_to_package(converted)
                    })
                    .collect();

                let package = RefCell::new(Package {
                    name,
                    version,
                    src,
                    features: enabled_features.into_iter().collect(),
                    dependencies,
                    printed: false,
                })
                .into();

                converted.insert(id, Rc::clone(&package));

                package
            }
        }
    }

    pub fn resolve(&mut self) {
        self.visit(&mut SetDefaultVisitor);
        self.visit(&mut NoDefaultsVisitor);
        self.visit(&mut EnableFeaturesVisitor);
        self.visit(&mut UnpackDefaultVisitor);
        self.visit(&mut UnpackChainVisitor);
    }

    fn visit(&mut self, visitor: &mut impl Visitor) {
        visitor.visit(self);
    }
}

impl DependencyNode {
    fn get_dependency(
        id: &PackageId,
        parent_package: &cargo_metadata::Package,
        packages: &BTreeMap<PackageId, cargo_metadata::Package>,
        nodes: &BTreeMap<PackageId, cargo_metadata::Node>,
        resolved_packages: &mut BTreeMap<PackageId, Rc<RefCell<PackageNode>>>,
    ) -> Self {
        // We eventually want only one representation of a package when we do feature resolution
        let package = match resolved_packages.get(id) {
            Some(package) => Rc::clone(package),
            None => {
                let package = RefCell::new(PackageNode::get_package(
                    id.clone(),
                    packages,
                    nodes,
                    resolved_packages,
                ))
                .into();

                resolved_packages.insert(id.clone(), Rc::clone(&package));

                package
            }
        };

        let name = package.borrow().name.clone();

        let details = parent_package
            .dependencies
            .iter()
            .find(|d| d.name == name)
            .unwrap();

        Self {
            package,
            optional: details.optional,
            uses_default_features: details.uses_default_features,
            features: details.features.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, str::FromStr};

    use super::*;

    use pretty_assertions::assert_eq;

    #[test]
    fn simple_package_input() {
        let path = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("simple");
        let registry = PathBuf::from_str(env!("HOME"))
            .unwrap()
            .join(".cargo")
            .join("registry");

        let package = PackageNode::from_current_dir(path.clone());

        assert_eq!(
            package,
            PackageNode {
                id: PackageId {
                    repr: format!("simple 0.1.0 (path+file://{})", path.display()),
                },
                name: "simple".to_string(),
                src: Utf8PathBuf::from_path_buf(path).unwrap(),
                version: "0.1.0".parse().unwrap(),
                dependencies: vec![DependencyNode {
                    package: RefCell::new(PackageNode {
                        id: PackageId {
                            repr:
                                "itoa 1.0.6 (registry+https://github.com/rust-lang/crates.io-index)"
                                    .to_string()
                        },
                        name: "itoa".to_string(),
                        version: "1.0.6".parse().unwrap(),
                        src: Utf8PathBuf::from_path_buf(
                            registry.join("src/github.com-1ecc6299db9ec823/itoa-1.0.6")
                        )
                        .unwrap(),
                        dependencies: Default::default(),
                        features: HashMap::from([(
                            "no-panic".to_string(),
                            vec!["dep:no-panic".to_string()]
                        )]),
                        enabled_features: Default::default(),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: true,
                    features: Default::default(),
                }],
                features: Default::default(),
                enabled_features: Default::default(),
            }
        );
    }

    #[test]
    fn simple_package_output() {
        let path = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("simple");

        let package = Package {
            name: "simple".to_string(),
            version: "0.1.0".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(path.clone()).unwrap(),
            dependencies: vec![RefCell::new(Package {
                name: "itoa".to_string(),
                version: "1.0.6".parse().unwrap(),
                src: "/home/user/.cargo/registry/src/github.com-1ecc6299db9ec823/itoa-1.0.6"
                    .parse()
                    .unwrap(),
                dependencies: Default::default(),
                features: Default::default(),
                printed: false,
            })
            .into()],
            features: Default::default(),
            printed: false,
        };

        let actual = package.to_derivative();

        let expected = fs::read_to_string(path.join("expected.nix")).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn workspace_input() {
        let workspace = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("workspace");
        let path = workspace.join("parent");

        let package = PackageNode::from_current_dir(path.clone());

        let registry = PathBuf::from_str(env!("HOME"))
            .unwrap()
            .join(".cargo")
            .join("registry");

        assert_eq!(
            package,
            PackageNode {
                id: PackageId {
                    repr: format!(
                        "parent 0.1.0 (path+file://{})",
                        workspace.join("parent").display()
                    ),
                },
                name: "parent".to_string(),
                version: "0.1.0".parse().unwrap(),
                src: Utf8PathBuf::from_path_buf(path).unwrap(),
                dependencies: vec![
                    DependencyNode {
                        package: RefCell::new(PackageNode {
                            id: PackageId {
                                repr: format!(
                                    "child 0.1.0 (path+file://{})",
                                    workspace.join("child").display()
                                ),
                            },
                            name: "child".to_string(),
                            version: "0.1.0".parse().unwrap(),
                            src: Utf8PathBuf::from_path_buf(workspace.join("child")).unwrap(),
                            dependencies: vec![
                                DependencyNode {
                                    package: RefCell::new(PackageNode {
                                        id: PackageId {
                                            repr:
                                                "itoa 1.0.6 (registry+https://github.com/rust-lang/crates.io-index)"
                                                    .to_string()
                                        },
                                        name: "itoa".to_string(),
                                        version: "1.0.6".parse().unwrap(),
                                        src: Utf8PathBuf::from_path_buf(
                                            registry.join("src/github.com-1ecc6299db9ec823/itoa-1.0.6")
                                        )
                                        .unwrap(),
                                        dependencies: Default::default(),
                                        features: HashMap::from([(
                                            "no-panic".to_string(),
                                            vec!["dep:no-panic".to_string()]
                                        )]),
                                        enabled_features: Default::default(),
                                    })
                                    .into(),
                                    optional: false,
                                    uses_default_features: true,
                                    features: Default::default(),
                                },
                                DependencyNode {
                                    package: RefCell::new(PackageNode {
                                        id: PackageId {
                                            repr:
                                                "libc 0.2.144 (registry+https://github.com/rust-lang/crates.io-index)"
                                                    .to_string()
                                        },
                                        name: "libc".to_string(),
                                        version: "0.2.144".parse().unwrap(),
                                        src: Utf8PathBuf::from_path_buf(
                                            registry.join("src/github.com-1ecc6299db9ec823/libc-0.2.144")
                                        )
                                        .unwrap(),
                                        dependencies: Default::default(),
                                        features: HashMap::from([
                                            ("std".to_string(), vec![]),
                                            ("default".to_string(), vec!["std".to_string()]),
                                            ("use_std".to_string(), vec!["std".to_string()]),
                                            ("extra_traits".to_string(), vec![]),
                                            ("align".to_string(), vec![]),
                                            ("rustc-dep-of-std".to_string(), vec!["align".to_string(), "rustc-std-workspace-core".to_string()]),
                                            ("const-extern-fn".to_string(), vec![]),
                                            ("rustc-std-workspace-core".to_string(), vec!["dep:rustc-std-workspace-core".to_string()]),
                                        ]),
                                        enabled_features: Default::default(),
                                    })
                                    .into(),
                                    optional: false,
                                    uses_default_features: true,
                                    features: Default::default(),
                                }
                            ],
                            features: HashMap::from([
                                (
                                    "default".to_string(),
                                    vec!["one".to_string(), "two".to_string()]
                                ),
                                ("one".to_string(), vec![]),
                                ("two".to_string(), vec![]),
                            ]),
                            enabled_features: Default::default(),
                        })
                        .into(),
                        optional: false,
                        uses_default_features: false,
                        features: vec!["one".to_string()],
                    },
                    DependencyNode {
                        package: RefCell::new(PackageNode {
                            id: PackageId {
                                repr:
                                    "itoa 0.4.8 (registry+https://github.com/rust-lang/crates.io-index)"
                                        .to_string()
                            },
                            name: "itoa".to_string(),
                            version: "0.4.8".parse().unwrap(),
                            src: Utf8PathBuf::from_path_buf(
                                registry.join("src/github.com-1ecc6299db9ec823/itoa-0.4.8")
                            )
                            .unwrap(),
                            dependencies: Default::default(),
                            features: HashMap::from([
                                ("default".to_string(), vec!["std".to_string()]),
                                ("no-panic".to_string(), vec!["dep:no-panic".to_string()]),
                                ("std".to_string(), vec![]),
                                ("i128".to_string(), vec![]),
                            ]),
                            enabled_features: Default::default(),
                        })
                        .into(),
                        optional: false,
                        uses_default_features: true,
                        features: Default::default(),
                    },
                    DependencyNode {
                        package: RefCell::new(PackageNode {
                            id: PackageId {
                                repr:
                                    "libc 0.2.144 (registry+https://github.com/rust-lang/crates.io-index)"
                                        .to_string()
                            },
                            name: "libc".to_string(),
                            version: "0.2.144".parse().unwrap(),
                            src: Utf8PathBuf::from_path_buf(
                                registry.join("src/github.com-1ecc6299db9ec823/libc-0.2.144")
                            )
                            .unwrap(),
                            dependencies: Default::default(),
                            features: HashMap::from([
                                ("std".to_string(), vec![]),
                                ("default".to_string(), vec!["std".to_string()]),
                                ("use_std".to_string(), vec!["std".to_string()]),
                                ("extra_traits".to_string(), vec![]),
                                ("align".to_string(), vec![]),
                                ("rustc-dep-of-std".to_string(), vec!["align".to_string(), "rustc-std-workspace-core".to_string()]),
                                ("const-extern-fn".to_string(), vec![]),
                                ("rustc-std-workspace-core".to_string(), vec!["dep:rustc-std-workspace-core".to_string()]),
                            ]),
                            enabled_features: Default::default(),
                        })
                        .into(),
                        optional: false,
                        uses_default_features: true,
                        features: Default::default(),
                    },
                ],
                features: Default::default(),
                enabled_features: Default::default(),
            }
        );
    }

    #[test]
    fn workspace_output() {
        let workspace = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("workspace");
        let path = workspace.join("parent");

        let registry = PathBuf::from_str(env!("HOME"))
            .unwrap()
            .join(".cargo")
            .join("registry");

        let libc = RefCell::new(Package {
            name: "libc".to_string(),
            version: "0.2.144".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(
                registry.join("src/github.com-1ecc6299db9ec823/libc-0.2.144"),
            )
            .unwrap(),
            dependencies: Default::default(),
            features: Default::default(),
            printed: false,
        })
        .into();

        let package = Package {
            name: "parent".to_string(),
            version: "0.1.0".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(path.clone()).unwrap(),
            dependencies: vec![
                RefCell::new(Package {
                    name: "child".to_string(),
                    version: "0.1.0".parse().unwrap(),
                    src: Utf8PathBuf::from_path_buf(workspace.join("child")).unwrap(),
                    dependencies: vec![
                        RefCell::new(Package {
                            name: "itoa".to_string(),
                            version: "1.0.6".parse().unwrap(),
                            src: Utf8PathBuf::from_path_buf(
                                registry.join("src/github.com-1ecc6299db9ec823/itoa-1.0.6"),
                            )
                            .unwrap(),
                            dependencies: Default::default(),
                            features: Default::default(),
                            printed: false,
                        })
                        .into(),
                        Rc::clone(&libc),
                    ],
                    features: vec!["one".to_string()],
                    printed: false,
                })
                .into(),
                RefCell::new(Package {
                    name: "itoa".to_string(),
                    version: "0.4.8".parse().unwrap(),
                    src: Utf8PathBuf::from_path_buf(
                        registry.join("src/github.com-1ecc6299db9ec823/itoa-0.4.8"),
                    )
                    .unwrap(),
                    dependencies: Default::default(),
                    features: Default::default(),
                    printed: false,
                })
                .into(),
                libc,
            ],
            features: Default::default(),
            printed: false,
        };

        let actual = package.to_derivative();

        let expected = fs::read_to_string(path.join("expected.nix")).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn into_package() {
        let workspace = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("workspace");
        let path = workspace.join("parent");

        let registry = PathBuf::from_str(env!("HOME"))
            .unwrap()
            .join(".cargo")
            .join("registry");

        let libc = RefCell::new(PackageNode {
            id: PackageId {
                repr: "libc 0.2.144 (registry+https://github.com/rust-lang/crates.io-index)"
                    .to_string(),
            },
            name: "libc".to_string(),
            version: "0.2.144".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(
                registry.join("src/github.com-1ecc6299db9ec823/libc-0.2.144"),
            )
            .unwrap(),
            dependencies: Default::default(),
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
        })
        .into();

        let input = PackageNode {
            id: PackageId {
                repr: format!(
                    "parent 0.1.0 (path+file://{})",
                    workspace.join("parent").display()
                ),
            },
            name: "parent".to_string(),
            version: "0.1.0".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(path.clone()).unwrap(),
            dependencies: vec![
                DependencyNode {
                    package: RefCell::new(PackageNode {
                        id: PackageId {
                            repr: format!(
                                "child 0.1.0 (path+file://{})",
                                workspace.join("child").display()
                            ),
                        },
                        name: "child".to_string(),
                        version: "0.1.0".parse().unwrap(),
                        src: Utf8PathBuf::from_path_buf(workspace.join("child")).unwrap(),
                        dependencies: vec![
                            DependencyNode {
                                package: RefCell::new(PackageNode {
                                    id: PackageId {
                                        repr:
                                            "itoa 1.0.6 (registry+https://github.com/rust-lang/crates.io-index)"
                                                .to_string()
                                    },
                                    name: "itoa".to_string(),
                                    version: "1.0.6".parse().unwrap(),
                                    src: Utf8PathBuf::from_path_buf(
                                        registry.join("src/github.com-1ecc6299db9ec823/itoa-1.0.6")
                                    )
                                    .unwrap(),
                                    dependencies: Default::default(),
                                    features: HashMap::from([(
                                        "no-panic".to_string(),
                                        vec!["dep:no-panic".to_string()]
                                    )]),
                                    enabled_features: Default::default(),
                                })
                                .into(),
                                optional: false,
                                uses_default_features: true,
                                features: Default::default(),
                            },
                            DependencyNode {
                                package: Rc::clone(&libc),
                                optional: false,
                                uses_default_features: true,
                                features: Default::default(),
                            }
                        ],
                        features: HashMap::from([
                            (
                                "default".to_string(),
                                vec!["one".to_string(), "two".to_string()]
                            ),
                            ("one".to_string(), vec![]),
                            ("two".to_string(), vec![]),
                        ]),
                        enabled_features: HashSet::from(["one".to_string()]),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: false,
                    features: vec!["one".to_string()],
                },
                DependencyNode {
                    package: RefCell::new(PackageNode {
                        id: PackageId {
                            repr:
                                "itoa 0.4.8 (registry+https://github.com/rust-lang/crates.io-index)"
                                    .to_string()
                        },
                        name: "itoa".to_string(),
                        version: "0.4.8".parse().unwrap(),
                        src: Utf8PathBuf::from_path_buf(
                            registry.join("src/github.com-1ecc6299db9ec823/itoa-0.4.8")
                        )
                        .unwrap(),
                        dependencies: Default::default(),
                        features: HashMap::from([
                            ("default".to_string(), vec!["std".to_string()]),
                            ("no-panic".to_string(), vec!["dep:no-panic".to_string()]),
                            ("std".to_string(), vec![]),
                            ("i128".to_string(), vec![]),
                        ]),
                        enabled_features: Default::default(),
                    })
                    .into(),
                    optional: false,
                    uses_default_features: true,
                    features: Default::default(),
                },
                DependencyNode {
                    package: libc,
                    optional: false,
                    uses_default_features: true,
                    features: Default::default(),
                },
            ],
            features: Default::default(),
            enabled_features: Default::default(),
        };

        let actual = input.into_package();

        let libc = RefCell::new(Package {
            name: "libc".to_string(),
            version: "0.2.144".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(
                registry.join("src/github.com-1ecc6299db9ec823/libc-0.2.144"),
            )
            .unwrap(),
            dependencies: Default::default(),
            features: Default::default(),
            printed: false,
        })
        .into();
        let expected = Package {
            name: "parent".to_string(),
            version: "0.1.0".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(path).unwrap(),
            dependencies: vec![
                RefCell::new(Package {
                    name: "child".to_string(),
                    version: "0.1.0".parse().unwrap(),
                    src: Utf8PathBuf::from_path_buf(workspace.join("child")).unwrap(),
                    dependencies: vec![
                        RefCell::new(Package {
                            name: "itoa".to_string(),
                            version: "1.0.6".parse().unwrap(),
                            src: Utf8PathBuf::from_path_buf(
                                registry.join("src/github.com-1ecc6299db9ec823/itoa-1.0.6"),
                            )
                            .unwrap(),
                            dependencies: Default::default(),
                            features: Default::default(),
                            printed: false,
                        })
                        .into(),
                        Rc::clone(&libc),
                    ],
                    features: vec!["one".to_string()],
                    printed: false,
                })
                .into(),
                RefCell::new(Package {
                    name: "itoa".to_string(),
                    version: "0.4.8".parse().unwrap(),
                    src: Utf8PathBuf::from_path_buf(
                        registry.join("src/github.com-1ecc6299db9ec823/itoa-0.4.8"),
                    )
                    .unwrap(),
                    dependencies: Default::default(),
                    features: Default::default(),
                    printed: false,
                })
                .into(),
                libc,
            ],
            features: Default::default(),
            printed: false,
        };

        assert_eq!(actual, expected);

        // Make sure the libcs are linked - ie, the version for both libcs should change with this one assignment
        actual.dependencies[2].borrow_mut().version = "0.2.0".parse().unwrap();

        assert_eq!(
            actual.dependencies[2],
            actual.dependencies[0].borrow().dependencies[1]
        );
    }

    fn make_package_node(
        name: &str,
        features: Vec<(&str, Vec<&str>)>,
        dependency: Option<DependencyNode>,
    ) -> PackageNode {
        let dependencies = if let Some(dependency) = dependency {
            vec![dependency]
        } else {
            Default::default()
        };

        PackageNode {
            id: PackageId {
                repr: format!("{} 0.1.0 (path+file://{})", name, name),
            },
            name: name.to_string(),
            version: "0.1.0".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(PathBuf::from_str(name).unwrap()).unwrap(),
            dependencies,
            features: HashMap::from_iter(features.into_iter().map(|(b, d)| {
                (
                    b.to_string(),
                    d.into_iter().map(ToString::to_string).collect(),
                )
            })),
            enabled_features: Default::default(),
        }
    }

    // Defaults should not be enabled when no-defaults is used
    #[test]
    fn resolve_no_defaults() {
        let mut child = make_package_node(
            "child",
            vec![
                ("default", vec!["one", "two"]),
                ("one", vec![]),
                ("two", vec![]),
            ],
            None,
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: false,
                features: vec!["one".to_string()],
            }),
        );

        input.resolve();

        child.enabled_features.insert("one".to_string());
        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: false,
                features: vec!["one".to_string()],
            }),
        );

        assert_eq!(input, expected);
    }

    // Enable defaults correctly
    #[test]
    fn resolve_defaults() {
        let mut child = make_package_node(
            "child",
            vec![
                ("default", vec!["one", "two"]),
                ("one", vec![]),
                ("two", vec![]),
            ],
            None,
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );

        input.resolve();

        child
            .enabled_features
            .extend(["one".to_string(), "two".to_string()]);
        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );

        assert_eq!(input, expected);
    }

    // Enable everything on a chain of defaults
    #[test]
    fn resolve_defaults_chain() {
        let mut child = make_package_node(
            "child",
            vec![
                ("default", vec!["one"]),
                ("one", vec!["two"]),
                ("two", vec![]),
            ],
            None,
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );

        input.resolve();

        child
            .enabled_features
            .extend(["one".to_string(), "two".to_string()]);
        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec![],
            }),
        );

        assert_eq!(input, expected);
    }

    // Optionals should not enable default features since they will not be used
    #[test]
    fn resolve_optional_no_defaults() {
        let child = make_package_node(
            "child",
            vec![
                ("default", vec!["one", "two"]),
                ("one", vec![]),
                ("two", vec![]),
            ],
            None,
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );

        input.resolve();

        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );

        assert_eq!(input, expected);
    }

    // Optionals should not enable any features since they will not be used
    #[test]
    fn resolve_optional_features() {
        let child = make_package_node("child", vec![("one", vec![]), ("two", vec![])], None);

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        input.resolve();

        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        assert_eq!(input, expected);
    }

    // Enable everything on a chain
    #[test]
    fn resolve_chain() {
        let mut child = make_package_node(
            "child",
            vec![
                ("one", vec!["two"]),
                ("two", vec!["three"]),
                ("three", vec![]),
            ],
            None,
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        input.resolve();

        child
            .enabled_features
            .extend(["one".to_string(), "two".to_string(), "three".to_string()]);
        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        assert_eq!(input, expected);
    }

    // Dependencies behind a feature should be enabled
    #[test]
    fn resolve_feature_dependency() {
        let optional = make_package_node("optional", vec![], None);
        let mut child = make_package_node(
            "child",
            vec![
                ("one", vec!["optional"]),
                ("optional", vec!["dep:optional"]),
            ],
            Some(DependencyNode {
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        input.resolve();

        child.dependencies[0].optional = false;
        child
            .enabled_features
            .extend(["one".to_string(), "optional".to_string()]);
        child.dependencies[0].package = RefCell::new(optional).into();

        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        assert_eq!(input, expected);
    }

    // Features on dependencies behind a feature should be enabled
    #[test]
    fn resolve_feature_dependency_features() {
        let optional = make_package_node("optional", vec![("feature", vec![])], None);
        let mut child = make_package_node(
            "child",
            vec![
                ("one", vec!["optional/feature"]),
                ("optional", vec!["dep:optional"]),
            ],
            Some(DependencyNode {
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        input.resolve();

        child.dependencies[0].optional = false;
        child.dependencies[0].features.push("feature".to_string());
        child.dependencies[0].package = RefCell::new(optional).into();
        child.dependencies[0]
            .package
            .borrow_mut()
            .enabled_features
            .extend(["feature".to_string()]);
        child
            .enabled_features
            .extend(["one".to_string(), "optional".to_string()]);

        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        assert_eq!(input, expected);
    }

    // Dependencies behind a feature should be enabled
    #[test]
    fn resolve_feature_dependency_defaults() {
        let optional = make_package_node(
            "optional",
            vec![("default", vec!["std"]), ("std", vec![])],
            None,
        );
        let mut child = make_package_node(
            "child",
            vec![
                ("one", vec!["optional"]),
                ("optional", vec!["dep:optional"]),
            ],
            Some(DependencyNode {
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: true,
                features: vec![],
            }),
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        input.resolve();

        child.dependencies[0].optional = false;
        child.dependencies[0].package = RefCell::new(optional).into();
        child.dependencies[0]
            .package
            .borrow_mut()
            .enabled_features
            .extend(["std".to_string()]);
        child
            .enabled_features
            .extend(["one".to_string(), "optional".to_string()]);

        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        assert_eq!(input, expected);
    }

    // Default features on a dependency (with no-defaults) behind a feature should not be enabled
    #[test]
    fn resolve_feature_dependency_no_defaults() {
        let optional = make_package_node(
            "optional",
            vec![("default", vec!["std"]), ("std", vec![])],
            None,
        );
        let mut child = make_package_node(
            "child",
            vec![
                ("one", vec!["optional"]),
                ("optional", vec!["dep:optional"]),
            ],
            Some(DependencyNode {
                package: RefCell::new(optional.clone()).into(),
                optional: true,
                uses_default_features: false,
                features: vec![],
            }),
        );

        let mut input = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        input.resolve();

        child.dependencies[0].optional = false;
        child.dependencies[0].package = RefCell::new(optional).into();
        child
            .enabled_features
            .extend(["one".to_string(), "optional".to_string()]);

        let expected = make_package_node(
            "parent",
            vec![],
            Some(DependencyNode {
                package: RefCell::new(child.clone()).into(),
                optional: false,
                uses_default_features: true,
                features: vec!["one".to_string()],
            }),
        );

        assert_eq!(input, expected);
    }
}
