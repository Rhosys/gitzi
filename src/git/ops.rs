use std::path::{Path, PathBuf};
use git2::{Repository, Signature};
use crate::config::MergeStrategy;
use crate::error::{GitziError, Result};

pub fn open_repo(path: &Path) -> Result<Repository> {
    Ok(Repository::discover(path)?)
}

// ── Worktrees ─────────────────────────────────────────────────────────────────
//
// Each task branch is worked on inside a git worktree — a separate directory
// that shares the object store with the main repo but has its own checked-out
// tree and HEAD. The main workspace is never touched.

pub struct TaskWorktree {
    pub path: PathBuf,
    name: String,
    repo_root: PathBuf,
}

impl TaskWorktree {
    /// Create a linked worktree at `~/.gitzi/tmp/tasks/<task_id>/worktrees/<repo_slug>/`
    /// on `branch_name`, creating the branch off HEAD if it does not yet exist.
    /// Idempotent: if the worktree is already registered, returns it as-is.
    pub fn create(repo: &Repository, task_id: &str, branch_name: &str, repo_slug: &str) -> Result<Self> {
        let repo_root = repo
            .workdir()
            .ok_or_else(|| GitziError::Git(git2::Error::from_str("bare repo")))?
            .to_path_buf();

        let wt_path = crate::state::reader::task_worktree_path(task_id, repo_slug);
        let name = worktree_name(branch_name);

        // Idempotent: if already registered (e.g. after restart or partial failure), reuse it.
        if repo.find_worktree(&name).is_ok() {
            return Ok(Self { path: wt_path, name, repo_root });
        }

        // Create the parent dir only — git2 requires the worktree target dir to not yet exist.
        if let Some(parent) = wt_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Ensure the branch exists before attaching a worktree to it.
        if repo.find_branch(branch_name, git2::BranchType::Local).is_err() {
            let head_commit = repo.head()?.peel_to_commit()?;
            repo.branch(branch_name, &head_commit, false)?;
        }

        let mut opts = git2::WorktreeAddOptions::new();
        let reference = repo.find_reference(&format!("refs/heads/{branch_name}"))?;
        opts.reference(Some(&reference));
        repo.worktree(&name, &wt_path, Some(&opts))?;

        Ok(Self { path: wt_path, name, repo_root })
    }

    /// Open an existing worktree by task ID (e.g. after a restart).
    pub fn open(repo: &Repository, task_id: &str, branch_name: &str, repo_slug: &str) -> Result<Self> {
        let repo_root = repo
            .workdir()
            .ok_or_else(|| GitziError::Git(git2::Error::from_str("bare repo")))?
            .to_path_buf();
        let name = worktree_name(branch_name);
        let path = crate::state::reader::task_worktree_path(task_id, repo_slug);
        Ok(Self { path, name, repo_root })
    }

    /// Commit everything staged in the worktree using direct object writes —
    /// no index manipulation on the main repo.
    pub fn commit_all(&self, message: &str) -> Result<git2::Oid> {
        // Open the worktree as its own repository so we can manipulate its
        // index and object store without touching the main repo's index.
        let wt_repo = Repository::open(&self.path)?;
        let sig = signature(&wt_repo)?;

        let mut index = wt_repo.index()?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        index.write()?;

        let tree_oid = index.write_tree()?;
        let tree = wt_repo.find_tree(tree_oid)?;

        let parent = wt_repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();

        let oid = wt_repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;
        Ok(oid)
    }

    /// Remove the worktree directory and its registration in the main repo.
    pub fn remove(self) -> Result<()> {
        let main_repo = Repository::discover(&self.repo_root)?;
        if let Ok(wt) = main_repo.find_worktree(&self.name) {
            let mut prune_opts = git2::WorktreePruneOptions::new();
            prune_opts.valid(true);
            let _ = wt.prune(Some(&mut prune_opts));
        }
        if self.path.exists() {
            std::fs::remove_dir_all(&self.path)?;
        }
        Ok(())
    }
}

fn worktree_name(branch_name: &str) -> String {
    branch_name.replace('/', "-")
}

// ── Diff (pure object read — never touches working tree) ──────────────────────

pub fn get_diff(repo: &Repository, branch_name: &str) -> Result<String> {
    let branch_ref = format!("refs/heads/{branch_name}");
    let branch_commit = repo
        .find_reference(&branch_ref)?
        .peel_to_commit()?;

    let merge_base_oid = find_merge_base(repo, branch_name)?;
    let base_tree = repo.find_commit(merge_base_oid)?.tree()?;
    let branch_tree = branch_commit.tree()?;

    let diff = repo.diff_tree_to_tree(Some(&base_tree), Some(&branch_tree), None)?;

    let mut output = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        match line.origin_value() {
            git2::DiffLineType::Addition => output.push('+'),
            git2::DiffLineType::Deletion => output.push('-'),
            git2::DiffLineType::Context => output.push(' '),
            _ => {}
        }
        if let Ok(s) = std::str::from_utf8(line.content()) {
            output.push_str(s);
        }
        true
    })?;

    Ok(output)
}

fn find_merge_base(repo: &Repository, branch_name: &str) -> Result<git2::Oid> {
    let head = repo.head()?.target().unwrap_or(git2::Oid::ZERO_SHA1);
    let branch_ref = format!("refs/heads/{branch_name}");
    let branch_oid = repo.revparse_single(&branch_ref)?.id();
    Ok(repo.merge_base(head, branch_oid)?)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn signature(repo: &Repository) -> Result<Signature<'static>> {
    let config = repo.config()?;
    let name = config
        .get_string("user.name")
        .unwrap_or_else(|_| "gitzi".to_string());
    let email = config
        .get_string("user.email")
        .unwrap_or_else(|_| "gitzi@localhost".to_string());
    Ok(Signature::now(&name, &email)?)
}

// ── Merge ─────────────────────────────────────────────────────────────────────

/// Outcome of a merge attempt.
#[derive(Debug)]
pub enum MergeOutcome {
    /// Branch was successfully merged (or pushed).
    Merged,
    /// Merge was intentionally skipped (strategy does not target main).
    Skipped(String),
    /// Fast-forward not possible — needs human intervention.
    FfFailed(String),
}

/// Merge a task branch into the main branch according to the configured strategy.
pub fn merge_task_branch(
    repo_path: &Path,
    task_branch: &str,
    main_branch: &str,
    strategy: &MergeStrategy,
) -> Result<MergeOutcome> {
    match strategy {
        MergeStrategy::FfOnly => merge_ff_only(repo_path, task_branch, main_branch),
        MergeStrategy::GitziBranch => Ok(MergeOutcome::Skipped(
            "gitzi-branch: no merge to main".to_string(),
        )),
        MergeStrategy::MergeCommit => {
            merge_with_commit(repo_path, task_branch, main_branch)
        }
        MergeStrategy::PullRequest => Ok(MergeOutcome::Skipped(
            "pull-request: branch pushed, create PR manually".to_string(),
        )),
        MergeStrategy::PushToRemote => push_to_remote(repo_path, task_branch),
    }
}

fn merge_ff_only(
    repo_path: &Path,
    task_branch: &str,
    main_branch: &str,
) -> Result<MergeOutcome> {
    let repo = Repository::discover(repo_path)?;

    let task_ref = format!("refs/heads/{task_branch}");
    let task_commit = repo
        .find_reference(&task_ref)
        .map_err(GitziError::Git)?
        .peel_to_commit()
        .map_err(GitziError::Git)?;

    let main_ref = format!("refs/heads/{main_branch}");
    let main_commit = repo
        .find_reference(&main_ref)
        .map_err(GitziError::Git)?
        .peel_to_commit()
        .map_err(GitziError::Git)?;

    // FF is possible when task_branch is a descendant of main (main's tip is
    // an ancestor of task_branch's tip).
    let can_ff = repo
        .graph_descendant_of(task_commit.id(), main_commit.id())
        .unwrap_or(false);

    if !can_ff {
        return Ok(MergeOutcome::FfFailed(format!(
            "branch '{task_branch}' cannot be fast-forwarded onto \
             '{main_branch}'"
        )));
    }

    // Move main's ref to task_commit (safe — it's a strict fast-forward).
    repo.reference(
        &main_ref,
        task_commit.id(),
        true,
        &format!("gitzi: ff-merge {task_branch} into {main_branch}"),
    )
    .map_err(GitziError::Git)?;

    Ok(MergeOutcome::Merged)
}

fn merge_with_commit(
    repo_path: &Path,
    task_branch: &str,
    main_branch: &str,
) -> Result<MergeOutcome> {
    let status = std::process::Command::new("git")
        .args([
            "merge",
            "--no-ff",
            task_branch,
            "-m",
            &format!("Merge {task_branch} into {main_branch}"),
        ])
        .current_dir(repo_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => Ok(MergeOutcome::Merged),
        Ok(_) => Ok(MergeOutcome::FfFailed(format!(
            "merge of '{task_branch}' into '{main_branch}' failed \
             — conflicts likely"
        ))),
        Err(e) => Err(GitziError::AgentFailed(format!(
            "git merge failed: {e}"
        ))),
    }
}

fn push_to_remote(
    repo_path: &Path,
    task_branch: &str,
) -> Result<MergeOutcome> {
    let status = std::process::Command::new("git")
        .args(["push", "-u", "origin", task_branch])
        .current_dir(repo_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => Ok(MergeOutcome::Merged),
        Ok(_) => Ok(MergeOutcome::FfFailed(format!(
            "push of '{task_branch}' to remote failed"
        ))),
        Err(e) => Err(GitziError::AgentFailed(format!(
            "git push failed: {e}"
        ))),
    }
}
