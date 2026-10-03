# Two-host sync checklist

Manual. Run before each release, with one Mac and one Linux machine on the
same LAN, both running the release build from the same commit. About 15
minutes. Sync in Wordy is file-level: the whole Loro snapshot is exchanged
and merged on each side, so every step checks both content and that no
history was lost.

Setup
- [ ] Both machines: `cargo build --release`, launch, open the same project
      name (copy the project folder from host A to host B before starting,
      or create it on A and let the first sync create it on B).
- [ ] Home → Sync shows the other machine within ~10 s on both hosts
      (mDNS). If not, check that both are on the same network and that no
      firewall blocks the port shown on the Sync tab.

Plain exchange
- [ ] On A: add a scene "Sync A" with one paragraph. Wait for autosave
      ("saved HH:MM" in the status bar).
- [ ] On B: Sync → Pull from A. The scene appears in B's sidebar with the
      paragraph intact; word counts match.
- [ ] On B: edit the paragraph and add a comment. Sync → Push to A.
- [ ] On A: the edit and the comment are there; the comment's author is B's
      machine name.

Concurrent edits
- [ ] Disconnect nothing; just edit the *same* scene on both hosts without
      syncing: A appends a sentence at the end, B inserts a sentence at the
      start.
- [ ] Sync either way, then the other way. Both hosts show both sentences,
      in the right places, no duplicated text, no lost formatting.
- [ ] Undo on A removes only A's sentence (undo is per machine).

Structure
- [ ] On A: rename a chapter, move a scene to another chapter, delete a
      note. On B: add a World entity and link it from a scene with `[[`.
- [ ] Sync both ways. Tree matches on both hosts; the deleted note is gone
      on B; the entity link resolves on A and shows in Backlinks.

Compaction
- [ ] On A: Home → Dashboard → Compact history (with both hosts in sync).
      Sync A → B and B → A again. Both still exchange changes; B's
      Dashboard shows the project as shallow after its next relaunch.
- [ ] Make one more edit on each side and sync. Still merges cleanly.

Failure paths
- [ ] Quit Wordy on B mid-session. On A: Sync shows B gone within ~30 s and
      pulling fails with a clear message, not a hang.
- [ ] Relaunch B. Pull from A succeeds; nothing was lost on either side.
- [ ] On B: `snapshots/` holds a backup taken before the first import
      (project-<stamp>.loro); opening it with `cargo run --example dump`
      from `crates/wordy-doc` prints the pre-sync content.

Record the commit hash and both OS versions at the top of the release notes
when every box is ticked.
