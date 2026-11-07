# Gallery workspaces

## Show a whole folder in workspaces, from the folder

A workspace's folders already include everything inside them, including photos added later
(`workspace_folders`, `workspaces::filter`). Today they can only be set from the workspace's
settings in the Gallery. The idea is to set it from the folder too, so a folder can feed
several workspaces at once.

- [x] Files: right-click one or more folders → **Show in workspace…**. A dialog with one
  checkbox per workspace: ticked means everything in the folder shows there, now and later.
  Admins only, as editing a workspace's folders is.
- [x] A folder already included through a parent folder shows ticked and disabled, with the
  note "Through “Photos”".
- [x] Partly ticked (indeterminate) when only some of the chosen folders are in a workspace, or
  when sub-folders of it are.
- [x] Also from the folder you're in (the background menu): "Show this folder in workspace…".
- [x] Server: `POST /api/workspaces/folders` `{workspaceIds, folders, action}` and
  `POST /api/workspaces/folder-membership` `{folders}`. Adding goes through
  `normalize_folders`, so a sub-folder already listed is absorbed by its parent. Removing a
  folder that is only included through a parent explains which folder to take out instead.
- [x] Picks inside a folder that is added stay stored, so taking the folder out later doesn't
  lose them.
- [ ] Show a folder's workspaces in its details panel (and maybe as a small badge on the card).

Done, with a headless-Chrome test in `scripts/test-workspace-folders.mjs` and a unit test in
`workspaces.rs`. Still to do: the folder's workspaces in its details panel and on its card.
