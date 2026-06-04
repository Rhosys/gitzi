use std::path::Path;
use git2::{Repository, Signature};
use crate::error::Result;

pub fn open_repo(path: &Path) -> Result<Repository> {
    Ok(Repository::discover(path)?)
}

pub fn create_branch(repo: &Repository, branch_name: &str) -> Result<()> {
    let head = repo.head()?;
    let commit = head.peel_to_commit()?;
    repo.branch(branch_name, &commit, false)?;
    Ok(())
}

pub fn checkout_branch(repo: &Repository, branch_name: &str) -> Result<()> {
    let (object, reference) = repo.revparse_ext(branch_name)?;
    repo.checkout_tree(&object, None)?;
    if let Some(r) = reference {
        repo.set_head(r.name().unwrap_or(branch_name))?;
    }
    Ok(())
}

pub fn get_diff(repo: &Repository, branch_name: &str) -> Result<String> {
    let branch_ref = format!("refs/heads/{branch_name}");
    let branch_obj = repo.revparse_single(&branch_ref)?;
    let branch_commit = branch_obj.peel_to_commit()?;

    let merge_base = find_merge_base(repo, branch_name)?;
    let base_commit = repo.find_commit(merge_base)?;

    let base_tree = base_commit.tree()?;
    let branch_tree = branch_commit.tree()?;

    let diff = repo.diff_tree_to_tree(Some(&base_tree), Some(&branch_tree), None)?;

    let mut output = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        use git2::DiffLineType::*;
        match line.origin_value() {
            Addition => output.push('+'),
            Deletion => output.push('-'),
            Context => output.push(' '),
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

pub fn stage_and_commit(repo: &Repository, paths: &[&Path], message: &str) -> Result<()> {
    let mut index = repo.index()?;
    for path in paths {
        let relative = path
            .strip_prefix(repo.workdir().unwrap_or(path))
            .unwrap_or(path);
        index.add_path(relative)?;
    }
    index.write()?;

    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    let sig = signature(repo)?;
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();

    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;
    Ok(())
}

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
