use tracing::{info_span, trace};

use crate::{DependencyNode, PackageNode};

pub trait Visitor {
    fn visit(&mut self, package: &mut PackageNode)
    where
        Self: Sized,
    {
        self.visit_package(package);

        for dependency in package.dependencies.iter() {
            let dependency_span = info_span!(
                "processing dependency",
                name = dependency.package.borrow().name
            );
            let _dependency_span_guard = dependency_span.enter();

            self.visit_dependency(dependency);

            dependency.package.borrow_mut().visit(self);
        }
    }

    fn visit_package(&mut self, _package: &mut PackageNode) {}

    fn visit_dependency(&mut self, _dependency: &DependencyNode) {}
}

pub struct SetDefaultVisitor;

impl Visitor for SetDefaultVisitor {
    fn visit_dependency(&mut self, dependency: &DependencyNode) {
        if !dependency.optional
            && dependency.uses_default_features
            && dependency.package.borrow().features.contains_key("default")
        {
            trace!("enabling default feature");

            dependency
                .package
                .borrow_mut()
                .enabled_features
                .insert("default".to_string());
        }
    }
}

pub struct EnableFeaturesVisitor;

impl Visitor for EnableFeaturesVisitor {
    fn visit_dependency(&mut self, dependency: &DependencyNode) {
        if !dependency.optional {
            let features: Vec<String> = dependency
                .features
                .clone()
                .iter()
                .filter(|&f| dependency.package.borrow().features.contains_key(f))
                .cloned()
                .collect();

            trace!(?features, "enabling features");

            dependency
                .package
                .borrow_mut()
                .enabled_features
                .extend(features);
        }
    }
}

pub struct UnpackDefaultVisitor;

impl Visitor for UnpackDefaultVisitor {
    fn visit_package(&mut self, package: &mut PackageNode) {
        let has_default = package.enabled_features.remove("default");

        if has_default {
            if let Some(default_features) = package.features.get("default") {
                trace!(?default_features, "enabling default features");

                package.enabled_features.extend(default_features.clone());
            }
        }
    }
}

pub struct UnpackChainVisitor;

impl Visitor for UnpackChainVisitor {
    fn visit_package(&mut self, package: &mut PackageNode) {
        loop {
            let new_features: Vec<_> = package
                .enabled_features
                .iter()
                .filter_map(|f| package.features.get(f))
                .flatten()
                .cloned()
                .filter(|f| !package.enabled_features.contains(f))
                .filter_map(|f| {
                    if let Some(dependency_name) = f.strip_prefix("dep:") {
                        if let Some(dependency) = package
                            .dependencies
                            .iter_mut()
                            .find(|d| d.package.borrow().name == dependency_name)
                        {
                            trace!(name = dependency_name, "activating optional dependency");
                            dependency.optional = false;

                            if dependency.uses_default_features {
                                let mut dependency_package = dependency.package.borrow_mut();

                                if let Some(default_features) =
                                    dependency_package.features.get("default").cloned()
                                {
                                    trace!(?default_features, "add default features");

                                    dependency_package.enabled_features.extend(default_features);
                                }
                            }

                            if !dependency.features.is_empty() {
                                trace!(features = ?dependency.features, "add dependency features");

                                dependency
                                    .package
                                    .borrow_mut()
                                    .enabled_features
                                    .extend(dependency.features.iter().cloned());
                            }
                        }

                        return None;
                    } else {
                        if let Some((dependency_name, feature)) = f.split_once("/") {
                            if let Some(dependency) = package
                                .dependencies
                                .iter_mut()
                                .find(|d| d.package.borrow().name == dependency_name)
                            {
                                let feature = feature.to_string();

                                if !dependency.features.contains(&feature) {
                                    dependency.features.push(feature.clone());
                                }

                                dependency
                                    .package
                                    .borrow_mut()
                                    .enabled_features
                                    .insert(feature);

                                return Some(dependency_name.to_string());
                            }
                        }
                    }

                    Some(f)
                })
                .filter(|f| !package.enabled_features.contains(f))
                .collect();

            if !new_features.is_empty() {
                trace!(?new_features, "adding new features");

                package.enabled_features.extend(new_features);
            } else {
                break;
            }
        }
    }
}

pub struct OptionalDependencyFeaturesVisitor;

impl Visitor for OptionalDependencyFeaturesVisitor {
    fn visit_package(&mut self, package: &mut PackageNode) {
        let new_dependencies_features: Vec<_> = package
            .enabled_features
            .iter()
            .filter_map(|f| f.split_once("?/"))
            .map(|(d, f)| (d.to_string(), f.to_string()))
            .collect();

        for (dependency_name, feature) in new_dependencies_features {
            if let Some(dependency) = package
                .dependencies
                .iter_mut()
                .find(|d| d.package.borrow().name == dependency_name && !d.optional)
            {
                if !dependency.features.contains(&feature) {
                    dependency.features.push(feature.clone());
                }

                trace!(
                    dependency = dependency_name,
                    feature,
                    "adding feature on optional dependency"
                );

                dependency
                    .package
                    .borrow_mut()
                    .enabled_features
                    .insert(feature.clone());
            }

            package
                .enabled_features
                .remove(&format!("{dependency_name}?/{feature}"));
        }
    }
}
