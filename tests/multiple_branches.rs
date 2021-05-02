mod utils;
use utils::*;
use autorebase::autorebase;

// Basic test but there is more than one branch that needs to be rebased.
#[test]
fn multiple_branches() {
    git_fixed_dates();

    let root =
        commit("First")
        .write("a.txt", "hello")
        .child(
            commit("Second")
            .write("a.txt", "world")
            .branch("master")
        )
        .child(
            commit("WIP 1")
            .write("b.txt", "foo1")
            .branch("wip1")
        )
        .child(
            commit("WIP 2")
            .write("b.txt", "foo2")
            .branch("wip2")
        );

    let repo = build_repo(&root, Some("master"));

    let repo_dir = repo.path();

    print_git_log_graph(&repo_dir);

    autorebase(repo_dir, "master").expect("error autorebasing");

    print_git_log_graph(&repo_dir);

    let graph = get_repo_graph(&repo_dir).expect("error getting repo graph");

    let expected_graph = commit_graph!(
        "540f822d14ae077991e2a722996825e4e7f9d667": CommitGraphNode {
            parents: [
                "a6de41485a5af44adc18b599a63840c367043e39",
            ],
            refs: {
                "wip1",
            },
        },
        "634e0a6b255cfff4cb9ba33a052da2f7e85a7bb9": CommitGraphNode {
            parents: [
                "a6de41485a5af44adc18b599a63840c367043e39",
            ],
            refs: {
                "wip2",
            },
        },
        "a6de41485a5af44adc18b599a63840c367043e39": CommitGraphNode {
            parents: [
                "d3591307bd5590f14ae24d03ab41121ab94e2a90",
            ],
            refs: {
                "master",
            },
        },
        "d3591307bd5590f14ae24d03ab41121ab94e2a90": CommitGraphNode {
            parents: [],
            refs: {
                "",
            },
        },
    );
    assert_eq!(graph, expected_graph);
}