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
    dependencies: Vec<Package>,
}

impl Package {
    pub fn to_derivative(self) -> String {
        let Self {
            name,
            version,
            src,
            features: _,
            dependencies,
        } = self;
        let (dep_names, dependencies): (Vec<_>, Vec<_>) = dependencies
            .into_iter()
            .map(|d| {
                let features = if d.features.is_empty() {
                    Default::default()
                } else {
                    format!(
                        "\n    features = [{}];",
                        d.features
                            .iter()
                            .map(|f| format!("\"{f}\""))
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                };

                (
                    d.name.clone(),
                    format!(
                        r#"
  {} = pkgs.buildRustCrate rec {{
    crateName = "{}";
    version = "{}";

    src = {};{}
  }};
"#,
                        d.name, d.name, d.version, d.src, features
                    ),
                )
            })
            .unzip();

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

  # Dependencies{}in
{}
"#,
            name,
            name,
            version,
            src,
            dep_names.join("\n"),
            dependencies.join("\n"),
            name
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

        let root_id = metadata
            .resolve
            .as_ref()
            .unwrap()
            .root
            .as_ref()
            .unwrap()
            .clone();

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
        let root = nodes.get(&root_id).unwrap().clone();

        let root_package = packages.get(&root_id).unwrap();

        let dependencies = root
            .dependencies
            .iter()
            .map(|id| {
                let package = packages.get(id).unwrap();
                let features = package.features.clone();
                let details = root_package
                    .dependencies
                    .iter()
                    .find(|d| d.name == package.name)
                    .unwrap();

                DependencyNode {
                    package: RefCell::new(PackageNode {
                        id: id.clone(),
                        name: package.name.clone(),
                        version: package.version.clone(),
                        src: package.manifest_path.parent().unwrap().into(),
                        dependencies: Default::default(),
                        features,
                        enabled_features: Default::default(),
                    })
                    .into(),
                    optional: details.optional,
                    uses_default_features: details.uses_default_features,
                    features: details.features.clone(),
                }
            })
            .collect();

        Self {
            id: root_id,
            name: root_package.name.clone(),
            version: root_package.version.clone(),
            src: root_package.manifest_path.parent().unwrap().into(),
            dependencies,
            features: Default::default(),
            enabled_features: Default::default(),
        }
    }

    pub fn into_package(self) -> Package {
        let Self {
            id: _,
            name,
            version,
            src,
            features: _,
            enabled_features,
            dependencies,
        } = self;

        let dependencies = dependencies
            .iter()
            .map(|d| Rc::clone(&d.package).borrow().clone().into_package())
            .collect();

        Package {
            name,
            version,
            src,
            features: enabled_features.into_iter().collect(),
            dependencies,
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
            dependencies: vec![Package {
                name: "itoa".to_string(),
                version: "1.0.6".parse().unwrap(),
                src: "/home/user/.cargo/registry/src/github.com-1ecc6299db9ec823/itoa-1.0.6"
                    .parse()
                    .unwrap(),
                dependencies: Default::default(),
                features: Default::default(),
            }],
            features: Default::default(),
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
                }],
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

        let package = Package {
            name: "parent".to_string(),
            version: "0.1.0".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(path.clone()).unwrap(),
            dependencies: vec![Package {
                name: "child".to_string(),
                version: "0.1.0".parse().unwrap(),
                src: Utf8PathBuf::from_path_buf(workspace.join("child")).unwrap(),
                dependencies: Default::default(),
                features: vec!["one".to_string()],
            }],
            features: Default::default(),
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

        let actual = input.into_package();
        let expected = Package {
            name: "parent".to_string(),
            version: "0.1.0".parse().unwrap(),
            src: Utf8PathBuf::from_path_buf(path).unwrap(),
            dependencies: vec![Package {
                name: "child".to_string(),
                version: "0.1.0".parse().unwrap(),
                src: Utf8PathBuf::from_path_buf(workspace.join("child")).unwrap(),
                dependencies: Default::default(),
                features: vec!["one".to_string()],
            }],
            features: Default::default(),
        };

        assert_eq!(actual, expected);
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
