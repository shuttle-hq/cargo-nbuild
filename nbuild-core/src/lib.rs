use std::{collections::BTreeMap, path::PathBuf};

use cargo_metadata::{camino::Utf8PathBuf, semver::Version, MetadataCommand};

#[derive(Debug, PartialEq)]
pub struct Package {
    name: String,
    src: Utf8PathBuf,
    dependencies: Vec<Dependency>,
}

#[derive(Debug, PartialEq)]
struct Dependency {
    name: String,
    version: Version,
    src: Utf8PathBuf,
}

impl Package {
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

        let root = metadata
            .resolve
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.id == root_id)
            .unwrap()
            .clone();

        let root_package = packages.get(&root_id).unwrap();

        Self {
            name: root_package.name.clone(),
            src: root_package.manifest_path.parent().unwrap().into(),
            dependencies: root
                .dependencies
                .iter()
                .map(|id| {
                    let package = packages.get(id).unwrap();
                    Dependency {
                        name: package.name.clone(),
                        version: package.version.clone(),
                        src: package.manifest_path.parent().unwrap().into(),
                    }
                })
                .collect(),
        }
    }

    pub fn to_derivative(self) -> String {
        let Self {
            name,
            src,
            dependencies,
        } = self;
        let (dep_names, dependencies): (Vec<_>, Vec<_>) = dependencies
            .into_iter()
            .map(|d| {
                (
                    d.name.clone(),
                    format!(
                        r#"
  itoa = pkgs.buildRustCrate rec {{
    crateName = "{}";
    version = "{}";

    src = {};
  }};
"#,
                        d.name, d.version, d.src
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
    version = "0.1.0";

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
            src,
            dep_names.join("\n"),
            dependencies.join("\n"),
            name
        )
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

        let package = Package::from_current_dir(path.clone());

        assert_eq!(
            package,
            Package {
                name: "simple".to_string(),
                src: Utf8PathBuf::from_path_buf(path).unwrap(),
                dependencies: vec![Dependency {
                    name: "itoa".to_string(),
                    version: "1.0.6".parse().unwrap(),
                    src: Utf8PathBuf::from_path_buf(
                        registry
                            .clone()
                            .join("src/github.com-1ecc6299db9ec823/itoa-1.0.6")
                    )
                    .unwrap(),
                }]
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
            src: Utf8PathBuf::from_path_buf(path.clone()).unwrap(),
            dependencies: vec![Dependency {
                name: "itoa".to_string(),
                version: "1.0.6".parse().unwrap(),
                src: "/home/user/.cargo/registry/src/github.com-1ecc6299db9ec823/itoa-1.0.6"
                    .parse()
                    .unwrap(),
            }],
        };

        let actual = package.to_derivative();

        let expected = fs::read_to_string(path.join("expected.nix")).unwrap();

        assert_eq!(actual, expected);
    }
}
