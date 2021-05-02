use std::path::{Path, PathBuf};
use anyhow::{anyhow, bail, Result};
use git_commands::*;

mod conflicts;
use conflicts::*;

pub fn autorebase(repo_path: &Path, onto_branch: &str) -> Result<()> {
    let conflicts_path = repo_path.join(".git/autorebase/conflicts.toml");

    let mut conflicts = if conflicts_path.is_file() {
        Conflicts::read_from_file(&conflicts_path)?
    } else {
        Default::default()
    };

    let worktree_path = repo_path.join(".git/autorebase/autorebase_worktree");

    if !worktree_path.is_dir() {
        create_scratch_worktree(&repo_path, &worktree_path)?;
    }

    // For each branch, find the common ancestor with `master`. There must only be one.

    // TODO: Figure out the entire tree structure.
    // Hmm for now I'll just do it the dumb way.

    let all_branches = get_branches(&repo_path)?;

    // TODO: Run `git pull --ff-only master`, but only if it isn't checked out anywhere.
    let onto_branch_info = all_branches.iter().find(|b| b.branch == onto_branch);
    if let Some(onto_branch_info) = onto_branch_info {
        if onto_branch_info.worktree_path.is_none() {
            run_git_cmd(&["checkout", onto_branch], &worktree_path)?;
            run_git_cmd(&["pull", "--ff-only"], &worktree_path)?;
            run_git_cmd(&["checkout", "--detach"], &worktree_path)?;
        } else {
            eprintln!("Not pulling {} because it is checked out", onto_branch_info.branch);
        }
    } else {
        eprintln!("Warning: {} not found", onto_branch);
    }

    for branch in all_branches.iter() {

        if branch.branch == onto_branch {
            eprintln!("Skipping branch {} because it is the target", branch.branch);
            continue;
        }
        if branch.upstream.is_some() {
            eprintln!("Skipping branch {} because it tracks upstream", branch.branch);
            continue;
        }
        if branch.worktree_path.is_some() {
            eprintln!("Skipping branch {} because it is checked out", branch.branch);
            continue;
        }

        let branch = &branch.branch;

        let branch_commit = run_git_cmd_output(&["rev-parse", branch], repo_path)?;
        let branch_commit = String::from_utf8(branch_commit)?;

        // If the rebase for this branch got stopped by a conflict before and
        // it's still the same commit then skip it.
        if conflicts.branches.get(branch) == Some(&branch_commit) {
            eprintln!("Skipping branch {} because it had conflicts last time we tried; rebase manually", branch);
            continue;
        }

        conflicts.branches.remove(branch);
        conflicts.write_to_file(&conflicts_path)?;

        eprintln!("\nRebasing {}\n", branch);

        // Get the list of commits we will try to rebase onto (starting with `onto_branch`).
        let target_commit_list = get_target_commit_list(&repo_path, branch, onto_branch)?;

        // Check out the branch.
        checkout_branch(branch, &worktree_path)?;

        let mut stopped_by_conflicts = false;

        for target_commit in target_commit_list {
            eprintln!("\nRebasing onto {}\n", target_commit);

            let result = attempt_rebase(&repo_path, &worktree_path, &target_commit)?;
            match result {
                RebaseResult::Success => {
                    eprintln!("\nRebasing onto {}: success\n", target_commit);
                    break;
                }
                RebaseResult::Conflict => {
                    eprintln!("\nRebasing onto {}: conflict\n", target_commit);
                    stopped_by_conflicts = true;
                    continue;
                }
            }
        }

        // Detach HEAD so that the branch can be checked out again in the main worktree.
        run_git_cmd(&["checkout", "--detach"], &worktree_path)?;

        if stopped_by_conflicts {
            // Get the commit again because it will have changed (probably).
            let new_branch_commit = run_git_cmd_output(&["rev-parse", branch], repo_path)?;
            let new_branch_commit = String::from_utf8(new_branch_commit)?;

            conflicts.branches.insert(branch.clone(), new_branch_commit);
            conflicts.write_to_file(&conflicts_path)?;
        }
    }

    Ok(())
}

/// Utility function to get the repo dir for the current directory.
pub fn get_repo_path() -> Result<PathBuf> {
    let output = run_git_cmd_output_cwd(&["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(String::from_utf8(output)?))
}

fn create_scratch_worktree(repo_path: &Path, worktree_path: &Path) -> Result<()> {
    let worktree_path = worktree_path.to_str().ok_or(anyhow!("worktree path is not unicode"))?;
    run_git_cmd(&["worktree", "add", "--detach", worktree_path], repo_path)?;
    Ok(())
}

struct BranchInfo {
    branch: String,
    upstream: Option<String>,
    worktree_path: Option<String>,
}

fn get_branches(repo_path: &Path) -> Result<Vec<BranchInfo>> {
    use std::str;

    // TODO: Config system to allow specifying the branches? Maybe allow adding/removing them?
    // Store config in `.git/autorebase/autorebase.toml` or `autorebase.toml`?

    let output = run_git_cmd_output(&["for-each-ref", "--format=%(refname:short)%00%(upstream:short)%00%(worktreepath)", "refs/heads"], repo_path)?;
    let branches = output.split(|c| *c == '\n' as u8).filter(
        |line| !line.is_empty()
    ).map(
        |line| {
            let parts: Vec<&[u8]> = line.split(|c| *c == 0).collect();
            if parts.len() != 3 {
                bail!("for-each-ref parse error, got {} parts, expected 3", parts.len());
            }
            Ok(BranchInfo {
                branch: str::from_utf8(parts[0])?.to_owned(),
                upstream: if parts[1].is_empty() { None } else { Some(str::from_utf8(parts[1])?.to_owned()) },
                worktree_path: if parts[2].is_empty() { None } else { Some(str::from_utf8(parts[2])?.to_owned()) },
            })
        }
    ).collect::<Result<_, _>>()?;
    Ok(branches)
}

fn get_merge_base(repo_path: &Path, a: &str, b: &str) -> Result<String> {
    let output = run_git_cmd_output(&["merge-base", a, b], repo_path)?;
    let output = String::from_utf8(output)?;
    // TODO: Could be very slightly more efficient if we trim whitespace from the Vec<u8> instead.
    Ok(output.trim().to_owned())
}

fn checkout_branch(branch: &str, repo_path: &Path) -> Result<()> {
    run_git_cmd(&["switch", branch], repo_path)?;
    Ok(())
}

fn is_rebasing(repo_path: &Path, worktree: Option<&str>) -> bool {
    // Check `.git/rebase-merge` exists. See https://stackoverflow.com/questions/3921409/how-to-know-if-there-is-a-git-rebase-in-progress/67245016#67245016

    let worktree_git_dir = if let Some(worktree) = worktree {
        repo_path.join(".git/worktrees").join(worktree)
    } else {
        repo_path.join(".git")
    };

    let rebase_apply = worktree_git_dir.join("rebase-apply");
    let rebase_merge = worktree_git_dir.join("rebase-merge");

    rebase_apply.exists() || rebase_merge.exists()
}

enum RebaseResult {
    Success,
    Conflict,
}

fn attempt_rebase(repo_path: &Path, worktree_path: &Path, onto: &str) -> Result<RebaseResult> {
    let rebase_ok = run_git_cmd(&["rebase", onto], worktree_path);
    if rebase_ok.is_ok() {
        return Ok(RebaseResult::Success)
    }

    // We may need to abort if the rebase is still in progress. Git checks
    // the rebase status like this:
    // https://stackoverflow.com/questions/3921409/how-to-know-if-there-is-a-git-rebase-in-progress/67245016#67245016

    if is_rebasing(repo_path, Some("autorebase_worktree")) {
        // Abort the rebase.
        run_git_cmd(&["rebase", "--abort"], worktree_path)?;
    }

    Ok(RebaseResult::Conflict)
}

fn get_target_commit_list(repo_path: &Path, branch: &str, onto: &str) -> Result<Vec<String>> {
    let merge_base = get_merge_base(repo_path, branch, onto)?;

    let output = run_git_cmd_output(&["log", "--format=%H", &format!("{}..{}", merge_base, onto)], repo_path)?;
    let output = String::from_utf8(output)?;
    Ok(output.lines().map(ToOwned::to_owned).collect())
}