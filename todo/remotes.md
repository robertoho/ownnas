# Remotes: rclone's servers and clouds

OwnNAS runs its own `rclone rcd` (src/remotes.rs), reachable only from this computer and
protected by a password of its own. Administrators see every remote and share each one with
other accounts. The remotes' secrets stay in rclone's config file.

Legend: `[x]` done · `[~]` partly · `[ ]` to do

## First round

- [x] rclone in Settings → Health, alongside the other helper tools.
- [x] Remotes view (cloud button): the remotes, with their kinds.
- [x] Browse folders on a remote.
- [x] Preview photos, videos (with seeking), audio, PDFs and text, streamed through rclone with
  byte ranges.
- [x] Download files.
- [x] Administrators choose the rclone program and its config file. These matter when OwnNAS
  runs as a service.
- [x] Share remotes with other accounts.
- [x] Backups to any remote: restic's `rclone:` backend, using the same program and config.

## Next

- [ ] Copy between the library and a remote, in both directions: a background job with
  progress and Cancel. Use rclone's `sync/copy` or `operations/copyfile` with `_async`, and
  `job/status` plus `core/stats` for progress. Add remotes as destinations in "Copy to…" and
  "Move to…".
- [ ] Change files on a remote: upload, new folder, rename, Trash or delete. Make this a
  separate "can change" right per shared account.
- [ ] Thumbnails for photos on remotes, made on demand and cached by path, size and date.
- [ ] Space used and free (`operations/about`) on each remote's card.
- [ ] MCP tools: `list_remotes`, `list_remote_folder`, and copying a file in or out, limited
  to the remotes an agent key is given.
- [ ] Searching inside a remote (`operations/list` with `recurse`, capped).
