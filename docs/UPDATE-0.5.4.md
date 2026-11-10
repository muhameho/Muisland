# Updating the Ubuntu/Hyprland installation to 0.5.4

The active process was running from `dist/run/muisland-0.5.3-linux-any`, separately from the installed launcher. `git pull` alone does not update that running process.

Give the Ubuntu agent this task:

```text
Update Muisland to 0.5.4 from the current main branch in ~/CS/playground/muisland.
Do not log out, close/restart Claude, run Save/Switch, clear auth files, or print any credentials.
Preserve all accounts, settings, backups, and local work.

1. Use git pull --ff-only. If there are conflicting local edits, stop and report them; never reset or discard them.
2. Build the source runtime: bash build.sh --source.
3. Extract dist/muisland-0.5.4-linux-any.tar.gz into dist/run/.
4. Run bash install.sh from the newly extracted 0.5.4 folder.
5. Stop only the old Muisland instance once no Save/Switch operation is underway. Do not force-kill an active transaction.
6. Launch ~/.local/bin/muisland, not the old 0.5.3 build folder. Verify the GUI footer says 0.5.4.
7. Run muisland --version and muisland --diagnose-usage. Report only the safe diagnostic metadata.
8. Verify the circle drag, hover, outside-click behavior, pinch zoom prevention, and whether the current usage sample is displayed. Do not switch real accounts for this check.

The fixed reader supports Cookies at the root of ~/.config/Claude. Older saved profiles omitted that file and cannot recover cookies they never contained. Tell the user to save each signed-in account once with this version; do not use Claude's own Log out.
```
