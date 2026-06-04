use std::path::{Path, PathBuf};
use git2::{Repository, Signature};
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
    /// Create a linked worktree at `<repo_root>/.gitzi/tasks/<task_id>/worktree`
    /// on `branch_name`, creating the branch off HEAD if it does not yet exist.
    pub fn create(repo: &Repository, task_id: &str, branch_name: &str) -> Result<Self> {
        let repo_root = repo
            .workdir()
            .ok_or_else(|| GitziError::Git(git2::Error::from_str("bare repo")))?
            .to_path_buf();

        let wt_path = crate::state::reader::task_worktree_path(&repo_root, task_id);
        std::fs::create_dir_all(&wt_path)?;

        // Ensure the branch exists before attaching a worktree to it.
        if repo.find_branch(branch_name, git2::BranchType::Local).is_err() {
            let head_commit = repo.head()?.peel_to_commit()?;
            repo.branch(branch_name, &head_commit, false)?;
        }

        let name = worktree_name(branch_name);
        let mut opts = git2::WorktreeAddOptions::new();
        let reference = repo.find_reference(&format!("refs/heads/{branch_name}"))?;
        opts.reference(Some(&reference));
        repo.worktree(&name, &wt_path, Some(&opts))?;

        Ok(Self { path: wt_path, name, repo_root })
    }

    /// Open an existing worktree by task ID (e.g. after a restart).
    pub fn open(repo: &Repository, task_id: &str, branch_name: &str) -> Result<Self> {
        let repo_root = repo
            .workdir()
            .ok_or_else(|| GitziError::Git(git2::Error::from_str("bare repo")))?
            .to_path_buf();
        let name = worktree_name(branch_name);
        let path = crate::state::reader::task_worktree_path(&repo_root, task_id);
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
