//! Three-way sync planning: compares the local tree, the remote tree, and the state both
//! had the last time they agreed, and decides what to do for each path. Pure, so it can be
//! tested without a server.
use std::collections::{BTreeMap, BTreeSet};

use crate::model::{Base, Node, Stamp};

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    MkdirRemote(String),
    MkdirLocal(String),
    /// Send the local file; the server must still have `base` (`None` = nothing).
    Upload { path: String, base: Option<Stamp>, local: Stamp },
    /// Fetch the remote file; the local path must still be `expect`.
    Download { path: String, expect: Option<Node> },
    /// Delete a remote file (`base` = its stamp) or empty folder (`base` = `None`).
    DeleteRemote { path: String, base: Option<Stamp> },
    /// Delete a local file or empty folder that must still be `expect`.
    DeleteLocal { path: String, expect: Node },
    /// Both sides changed the same file.
    Conflict { path: String, local: Stamp, remote: Stamp },
    /// Both sides already agree; remember it.
    Record { path: String, base: Base },
    /// Both sides are gone; drop the remembered state.
    Forget(String),
    Skip { path: String, reason: &'static str },
}

impl Action {
    /// True for actions that change the server.
    pub fn writes_remote(&self) -> bool {
        matches!(self, Action::MkdirRemote(_) | Action::Upload { .. } | Action::DeleteRemote { .. })
    }
}

/// Folder deletions run last and deepest-first, after their contents were handled.
pub fn plan(
    local: &BTreeMap<String, Node>,
    remote: &BTreeMap<String, Node>,
    state: &BTreeMap<String, Base>,
) -> Vec<Action> {
    let paths: BTreeSet<&String> = local.keys().chain(remote.keys()).chain(state.keys()).collect();
    let mut actions = Vec::new();
    let mut dir_deletes = Vec::new();
    for path in paths {
        let l = local.get(path).copied();
        let r = remote.get(path).copied();
        let b = state.get(path).copied();
        let path = path.clone();
        match (l, r) {
            (Some(l), Some(r)) if l.dir != r.dir => actions.push(Action::Skip {
                path,
                reason: "is a file on one side and a folder on the other",
            }),
            (Some(l), Some(_)) if l.dir => {
                if b != Some(Base::dir()) {
                    actions.push(Action::Record { path, base: Base::dir() });
                }
            }
            (Some(l), None) if l.dir => {
                if b.is_some_and(|b| b.dir) {
                    dir_deletes.push(Action::DeleteLocal { path, expect: l });
                } else {
                    actions.push(Action::MkdirRemote(path));
                }
            }
            (None, Some(r)) if r.dir => {
                if b.is_some_and(|b| b.dir) {
                    dir_deletes.push(Action::DeleteRemote { path, base: None });
                } else {
                    actions.push(Action::MkdirLocal(path));
                }
            }
            (None, None) => {
                if b.is_some() {
                    actions.push(Action::Forget(path));
                }
            }
            _ => {
                if let Some(action) = plan_file(path, l, r, b) {
                    actions.push(action);
                }
            }
        }
    }
    dir_deletes.reverse();
    actions.extend(dir_deletes);
    actions
}

/// Both `l` and `r` are files or absent, and not both absent.
fn plan_file(path: String, l: Option<Node>, r: Option<Node>, b: Option<Base>) -> Option<Action> {
    let file_base = b.filter(|b| !b.dir);
    let bl = file_base.and_then(|b| b.local);
    let br = file_base.and_then(|b| b.remote);
    let ls = l.map(|n| n.stamp);
    let rs = r.map(|n| n.stamp);
    let local_changed = ls != bl;
    let remote_changed = rs != br;
    Some(match (local_changed, remote_changed) {
        (false, false) => return None,
        (true, false) => match ls {
            Some(local) => Action::Upload { path, base: br, local },
            None => Action::DeleteRemote { path, base: br },
        },
        (false, true) => match rs {
            Some(_) => Action::Download { path, expect: l },
            None => Action::DeleteLocal { path, expect: l? },
        },
        (true, true) => match (ls, rs) {
            // A deletion loses to an edit on the other side.
            (None, Some(_)) => Action::Download { path, expect: None },
            (Some(local), None) => Action::Upload { path, base: None, local },
            (Some(local), Some(remote)) => Action::Conflict { path, local, remote },
            (None, None) => Action::Forget(path),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(items: &[(&str, Node)]) -> BTreeMap<String, Node> {
        items.iter().map(|(p, n)| (p.to_string(), *n)).collect()
    }

    fn st(size: u64, mtime: i64) -> Stamp {
        Stamp { size, mtime }
    }

    fn state(items: &[(&str, Base)]) -> BTreeMap<String, Base> {
        items.iter().map(|(p, b)| (p.to_string(), *b)).collect()
    }

    #[test]
    fn first_sync_creates_folders_and_copies_files_both_ways() {
        let local = tree(&[("a", Node::dir()), ("a/x.txt", Node::file(1, 10))]);
        let remote = tree(&[("b", Node::dir()), ("b/y.txt", Node::file(2, 20))]);
        let actions = plan(&local, &remote, &BTreeMap::new());
        assert_eq!(
            actions,
            vec![
                Action::MkdirRemote("a".into()),
                Action::Upload { path: "a/x.txt".into(), base: None, local: st(1, 10) },
                Action::MkdirLocal("b".into()),
                Action::Download { path: "b/y.txt".into(), expect: None },
            ]
        );
    }

    #[test]
    fn unchanged_paths_need_nothing() {
        let local = tree(&[("d", Node::dir()), ("d/f", Node::file(1, 10))]);
        let remote = tree(&[("d", Node::dir()), ("d/f", Node::file(1, 99))]);
        let base = state(&[("d", Base::dir()), ("d/f", Base::file(st(1, 10), st(1, 99)))]);
        assert!(plan(&local, &remote, &base).is_empty());
    }

    #[test]
    fn one_sided_edits_and_deletes_propagate_with_bases() {
        let base = state(&[
            ("edit-local", Base::file(st(1, 1), st(1, 2))),
            ("edit-remote", Base::file(st(1, 1), st(1, 2))),
            ("del-local", Base::file(st(1, 1), st(1, 2))),
            ("del-remote", Base::file(st(1, 1), st(1, 2))),
        ]);
        let local = tree(&[
            ("edit-local", Node::file(5, 5)),
            ("edit-remote", Node::file(1, 1)),
            ("del-remote", Node::file(1, 1)),
        ]);
        let remote = tree(&[
            ("edit-local", Node::file(1, 2)),
            ("edit-remote", Node::file(7, 7)),
            ("del-local", Node::file(1, 2)),
        ]);
        assert_eq!(
            plan(&local, &remote, &base),
            vec![
                Action::DeleteRemote { path: "del-local".into(), base: Some(st(1, 2)) },
                Action::DeleteLocal { path: "del-remote".into(), expect: Node::file(1, 1) },
                Action::Upload { path: "edit-local".into(), base: Some(st(1, 2)), local: st(5, 5) },
                Action::Download { path: "edit-remote".into(), expect: Some(Node::file(1, 1)) },
            ]
        );
    }

    #[test]
    fn edits_beat_deletes_and_double_edits_conflict() {
        let base = state(&[
            ("a", Base::file(st(1, 1), st(1, 2))),
            ("b", Base::file(st(1, 1), st(1, 2))),
            ("c", Base::file(st(1, 1), st(1, 2))),
            ("gone", Base::file(st(1, 1), st(1, 2))),
        ]);
        let local = tree(&[("b", Node::file(3, 3)), ("c", Node::file(4, 4))]);
        let remote = tree(&[("a", Node::file(9, 9)), ("c", Node::file(5, 5))]);
        assert_eq!(
            plan(&local, &remote, &base),
            vec![
                Action::Download { path: "a".into(), expect: None },
                Action::Upload { path: "b".into(), base: None, local: st(3, 3) },
                Action::Conflict { path: "c".into(), local: st(4, 4), remote: st(5, 5) },
                Action::Forget("gone".into()),
            ]
        );
    }

    #[test]
    fn new_file_on_both_sides_is_a_conflict_to_resolve_by_hash() {
        let local = tree(&[("n", Node::file(3, 1))]);
        let remote = tree(&[("n", Node::file(3, 2))]);
        assert_eq!(
            plan(&local, &remote, &BTreeMap::new()),
            vec![Action::Conflict { path: "n".into(), local: st(3, 1), remote: st(3, 2) }]
        );
    }

    #[test]
    fn deleted_folders_are_removed_last_and_deepest_first() {
        let base = state(&[
            ("p", Base::dir()),
            ("p/q", Base::dir()),
            ("p/q/f", Base::file(st(1, 1), st(1, 2))),
        ]);
        let remote = tree(&[("p", Node::dir()), ("p/q", Node::dir()), ("p/q/f", Node::file(1, 2))]);
        assert_eq!(
            plan(&BTreeMap::new(), &remote, &base),
            vec![
                Action::DeleteRemote { path: "p/q/f".into(), base: Some(st(1, 2)) },
                Action::DeleteRemote { path: "p/q".into(), base: None },
                Action::DeleteRemote { path: "p".into(), base: None },
            ]
        );
    }

    #[test]
    fn file_and_folder_with_same_name_is_skipped() {
        let local = tree(&[("x", Node::dir())]);
        let remote = tree(&[("x", Node::file(1, 1))]);
        assert!(matches!(plan(&local, &remote, &BTreeMap::new())[..], [Action::Skip { .. }]));
    }
}
