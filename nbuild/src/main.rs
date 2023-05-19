use std::env::current_dir;

use nbuild_core::Package;
use runix::{
    arguments::{eval::EvaluationArgs, source::SourceArgs, NixArgs},
    command::Build,
    command_line::NixCommandLine,
    RunJson,
};

#[tokio::main]
async fn main() {
    let expr = Package::from_current_dir(current_dir().unwrap()).to_derivative();
    let cli = NixCommandLine::default();

    let value = Build {
        eval: EvaluationArgs {
            impure: true.into(),
        },
        source: SourceArgs {
            expr: Some(expr.into()),
        },
        ..Default::default()
    }
    .run_json(&cli, &NixArgs::default())
    .await
    .unwrap();

    println!("{value}");
}
