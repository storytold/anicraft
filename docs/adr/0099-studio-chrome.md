# ADR 0099: Studio chrome, and account files on 2kool.tv

* Status: accepted (2026-11-15)
* Relates to: ADR 0005 (type), ADR 0008 (visual language), ADR 0077 (palettes)

## Context

This fork is a remask of AniCraft, not a new app inside K-OS. The chrome should stay
dark grey and black, with white used only for the selected control, so the picture on
the stage is what the eye goes to. Labels should read like a terminal. The file list,
the exposure sheet and the tracker come later, on this same shell.

A project also has to be saveable in the signed-in user's own 2kool.tv account. The
account, the session and the library already live in K-OS (`users`, `user_assets`,
and the same pattern as `kooldraw_projects`). `files.2kool.tv` is the static site
mirror. Working bytes do not go there.

## Decisions

1. **A fourth palette, `Studio`, over the same roles.** Black field and panels
   (`#000000`), gutter and buttons `#141414`, rules and hover `#2a2a2a`, body text
   `#a0a0a0`. The active tool and a selected row are white with black type
   (`active` / `on_active`, `row_selected` / `on_selected`). Corners are square.
   Timeline blocks keep their meaning colours. The stage marquee stays dark, because
   it is drawn on the artwork and a white marquee would vanish on a white stage.
   Text selection inside a field stays `accent_dim` (dark), so typed text does not
   turn white-on-white. Preferences lists Studio with the other palettes.
   `--theme studio` selects it for one run and does not write the preference.
2. **`row_selected` and `on_selected`.** Selected list rows (timeline layer, library,
   movie explorer, find results, motion-editor properties, publish formats) fill with
   `row_selected` and draw their labels in `on_selected`. Graphite, Light and high
   contrast set those to the old pair (`accent_dim` and `text`), so their rows do
   not move.
3. **Studio prose uses the monospace face.** While Studio is the palette and
   Preferences has not chosen a system UI font, JetBrains Mono is also the
   proportional face. Data roles were already monospace. The role names stay, so a
   label is still a label.
4. **Account save uses the existing 2kool account.** A later change stores an
   `.anicraft` document the way KoolDraw stores a project: one row owned by
   `users.id` for the signed-in session, the bytes in that row, and a `user_assets`
   registration so the rest of the library can see it. Unsigned work stays on this
   machine. No second account, no second file host, and no upload to
   `files.2kool.tv`.

## Consequences

* Opening this build still follows the saved preference. Studio is chosen in
  Preferences › Appearance › Theme, or with `--theme studio`.
* The vertical exposure sheet is ADR 0100. Storyboard shots and the account save
  itself are not this change.
