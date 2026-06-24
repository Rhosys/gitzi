use std::path::Path;
use tokio::process::Command;
use crate::config::ResolvedRepoConfig;
use crate::error::Result;

#[derive(Debug)]
pub struct TestResult {
    pub success: bool,
    pub output: String,
}

pub async fn run_tests(repo_config: &ResolvedRepoConfig, repo_root: &Path) -> Result<TestResult> {
    let output = Command::new("sh")
        .args(["-c", &repo_config.test_command])
        .current_dir(repo_root)
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(TestResult {
        success: output.status.success(),
        output: format!("{stdout}{stderr}"),
    })
}
