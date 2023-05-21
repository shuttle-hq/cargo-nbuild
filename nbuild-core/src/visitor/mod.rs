use crate::{DependencyNode, PackageNode};

pub trait Visitor {
    fn visit(&mut self, package: &mut PackageNode)
    where
        Self: Sized,
    {
        self.visit_package(package);

        for dependency in package.dependencies.iter() {
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
        let default = match (
            dependency.uses_default_features,
            dependency.package.borrow().features.get("default"),
        ) {
            (true, Some(features)) => features.clone(),
            _ => Default::default(),
        };

        dependency
            .package
            .borrow_mut()
            .enabled_features
            .extend(default);
    }
}

pub struct NoDefaultsVisitor;

impl Visitor for NoDefaultsVisitor {
    fn visit_dependency(&mut self, dependency: &DependencyNode) {
        if !dependency.uses_default_features {
            dependency.package.borrow_mut().enabled_features.clear();
        }
    }
}

pub struct EnableFeaturesVisitor;

impl Visitor for EnableFeaturesVisitor {
    fn visit_dependency(&mut self, dependency: &DependencyNode) {
        let features: Vec<String> = dependency
            .features
            .clone()
            .iter()
            .filter(|&f| dependency.package.borrow().features.contains_key(f))
            .cloned()
            .collect();

        dependency
            .package
            .borrow_mut()
            .enabled_features
            .extend(features);
    }
}
