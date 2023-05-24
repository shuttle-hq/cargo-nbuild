mod visitor;

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
};

use cargo_metadata::{camino::Utf8PathBuf, semver::Version, MetadataCommand, PackageId};
use visitor::{EnableFeaturesVisitor, NoDefaultsVisitor, SetDefaultVisitor, Visitor};

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
        let dep_names: Vec<_> = dependencies
            .into_iter()
            .map(|d| {
                let name = d.borrow().name.clone();
                Self::to_details(d, &mut build_details);
                name
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
            dep_names.join("\n      "),
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
            let dep_names: Vec<_> = this
                .dependencies
                .iter()
                .map(|d| d.borrow().name.clone())
                .collect();
            format!("\n    dependencies = [{}];", dep_names.join(" "))
        };

        let details = format!(
            r#"  {} = pkgs.buildRustCrate rec {{
    crateName = "{}";
    version = "{}";

    src = {};{}{}
  }};"#,
            this.name, this.name, this.version, this.src, deps, features
        );

        build_details.push(details);

        for dependency in this.dependencies.iter() {
            Self::to_details(Rc::clone(dependency), build_details);
        }

        this.printed = true;
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
                DependencyNode::get_dependency(id, package, &packages, &nodes, resolved_packages)
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
                    &packages,
                    &nodes,
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
                    }
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

        let itoa = RefCell::new(Package {
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
                    dependencies: vec![Rc::clone(&itoa)],
                    features: vec!["one".to_string()],
                    printed: false,
                })
                .into(),
                itoa,
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

        let itoa = RefCell::new(PackageNode {
            id: PackageId {
                repr: "itoa 1.0.6 (registry+https://github.com/rust-lang/crates.io-index)"
                    .to_string(),
            },
            name: "itoa".to_string(),
            version: "1.0.6".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(
                registry.join("src/github.com-1ecc6299db9ec823/itoa-1.0.6"),
            )
            .unwrap(),
            dependencies: Default::default(),
            features: HashMap::from([("no-panic".to_string(), vec!["dep:no-panic".to_string()])]),
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
                        dependencies: vec![DependencyNode {
                            package: Rc::clone(&itoa),
                            optional: false,
                            uses_default_features: true,
                            features: Default::default(),
                        }],
                        features: HashMap::from([
                            (
                                "default".to_string(),
                                vec!["one".to_string(), "two".to_string()],
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
                    package: itoa,
                    optional: false,
                    uses_default_features: true,
                    features: Default::default(),
                },
            ],
            features: Default::default(),
            enabled_features: Default::default(),
        };

        let actual = input.into_package();

        let itoa = RefCell::new(Package {
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
                    dependencies: vec![Rc::clone(&itoa)],
                    features: vec!["one".to_string()],
                    printed: false,
                })
                .into(),
                itoa,
            ],
            features: Default::default(),
            printed: false,
        };

        assert_eq!(actual, expected);

        // Make sure the itoas are linked
        actual.dependencies[1].borrow_mut().version = "1.0.0".parse().unwrap();

        assert_eq!(
            actual.dependencies[1],
            actual.dependencies[0].borrow().dependencies[0]
        );
    }

    #[test]
    fn resolve() {
        let workspace = PathBuf::from_str(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .join("tests")
            .join("workspace");
        let path = workspace.join("parent");
        let mut input = PackageNode {
            id: PackageId {
                repr: format!(
                    "parent 0.1.0 (path+file://{})",
                    workspace.join("parent").display()
                ),
            },
            name: "parent".to_string(),
            version: "0.1.0".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(path.clone()).unwrap(),
            dependencies: vec![DependencyNode {
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
                    dependencies: Default::default(),
                    features: HashMap::from([
                        (
                            "default".to_string(),
                            vec!["one".to_string(), "two".to_string()],
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
            }],
            features: Default::default(),
            enabled_features: Default::default(),
        };

        input.resolve();
        let expected = PackageNode {
            id: PackageId {
                repr: format!(
                    "parent 0.1.0 (path+file://{})",
                    workspace.join("parent").display()
                ),
            },
            name: "parent".to_string(),
            version: "0.1.0".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(path).unwrap(),
            dependencies: vec![DependencyNode {
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
                    dependencies: Default::default(),
                    features: HashMap::from([
                        (
                            "default".to_string(),
                            vec!["one".to_string(), "two".to_string()],
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
            }],
            features: Default::default(),
            enabled_features: Default::default(),
        };

        assert_eq!(input, expected);
    }
}
