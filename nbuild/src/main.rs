use std::{env::current_dir, process::Stdio};

use nbuild_core::PackageNode;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() {
    let fmt_layer = tracing_subscriber::fmt::layer().pretty().with_ansi(false);
    let filter_layer = tracing_subscriber::EnvFilter::from_default_env();

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .init();

    let mut package = PackageNode::from_current_dir(current_dir().unwrap());
    package.resolve();

    package.into_package().into_file();

    let mut cmd = Command::new("nix");
    cmd.args([
        "build",
        "--file",
        ".nbuild.nix",
        "--max-jobs",
        "auto",
        "--cores",
        "0",
    ])
    .stdout(Stdio::piped());

    let mut child = cmd.spawn().expect("to spawn build command");
    let stdout = child.stdout.take().expect("to get handle on stdout");

    let mut reader = BufReader::new(stdout).lines();

    // Drive process forward
    tokio::spawn(async move {
        let status = child.wait().await.expect("build to finish");

        if status.success() {
            println!("Build done");
        } else {
            println!("Build failed");
        }
    });

    while let Some(line) = reader.next_line().await.expect("to get line") {
        println!("{line}");
    }
}
