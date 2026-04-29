use std::{path::Path, process::Command};

pub fn run_script(script: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script_path = root.join(script);

    let output = Command::new(&script_path)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", script_path.display()));

    if !output.status.success() {
        panic!(
            "{} failed with status {}\n\nstdout:\n{}\n\nstderr:\n{}",
            script_path.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
